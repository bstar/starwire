//! The truth, and the one function that is allowed to change it.
//!
//! Everything the window draws and everything a worker reads comes out of
//! [`State`], held behind the one `RwLock` [`super::handle::Handle`] owns.
//! [`apply`] is the *only* writer, and it is pure: given the state as it was
//! and a [`Change`], it returns the state as it now is and the
//! [`super::worker::Job`]s that change implies -- no query, no request,
//! nothing that can block the caller holding the write lock. That is what
//! lets a net thread take the lock just long enough to fold a fetch in and
//! let go again, and it is why the grep test at the bottom of this file
//! refuses `std::fs::`, `rusqlite::` and `ureq::` in these sources. // NO-IO-HERE
//!
//! The shape is STAR/FOLD's `fold::state`: one `match` per [`Command`] and
//! one per [`Done`], each arm a small named function, and `Effects` carrying
//! the jobs and the events back out rather than sending them from in here.
//!
//! Two rules worth stating because everything else follows from them:
//!
//! - **The core holds one open selection and one open article.** Where the
//!   reader is looking is the window's business (`ui::stack`); what is
//!   loaded is this module's. The window sends `OpenFeed` whenever its
//!   active entries frame changes and `OpenEntry` whenever its active
//!   article does.
//! - **A read that the window can see is applied here before the database
//!   has it.** `SetRead` and `SetStarred` move the rows and the unread
//!   counts immediately and queue the write; the row the list draws is never
//!   a round trip behind the key that changed it.

use std::collections::{BTreeSet, VecDeque};

use jiff::Timestamp;

use super::db::entries;
use super::db::feeds::DueFeed;
use super::extract::Limits;
use super::feed::{
    ArticleStatus, ArticleView, EntryId, EntryRow, FeedId, FeedRow, Folder, Selection,
};
use super::handle::{Command, Event, ImportOffer, Note, OpenKind, RefreshScope, Setting};
use super::import::ImportReport;
use super::open::Target;
use super::search;
use super::worker::{DbJob, Done, Job, Lane, NetJob};
use super::WireConfig;

/// The settings a running core will change its mind about.
///
/// The rest of [`WireConfig`] -- the player's argv, `yt-dlp`'s name, the
/// request timeout -- is held by the worker threads instead and takes effect
/// on restart, which is what the settings overlay says about those rows.
/// These are the ones a keystroke moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub refresh_minutes: u32,
    pub extract: bool,
    pub images: bool,
    pub keep_days: u32,
    pub max_entries_per_feed: usize,
    pub page_size: usize,
    /// How many net threads there are, and so -- doubled -- how many
    /// extractions may be in flight before the queue holds the rest.
    pub parallel: usize,
    pub max_feed_bytes: u64,
    pub max_article_bytes: u64,
    pub max_markdown_bytes: usize,
}

impl From<&WireConfig> for RuntimeSettings {
    fn from(cfg: &WireConfig) -> Self {
        Self {
            refresh_minutes: cfg.fetch.refresh_minutes,
            extract: cfg.articles.extract,
            images: cfg.articles.images,
            keep_days: cfg.articles.keep_days,
            max_entries_per_feed: cfg.articles.max_entries_per_feed,
            page_size: cfg.articles.page_size.max(1),
            parallel: cfg.fetch.parallel.max(1),
            max_feed_bytes: cfg.fetch.max_feed_bytes,
            max_article_bytes: cfg.articles.max_article_bytes,
            max_markdown_bytes: cfg.articles.max_markdown_bytes,
        }
    }
}

impl RuntimeSettings {
    /// What an extraction job is given. Built from the settings rather than
    /// from the thread's own config so that turning images off applies to
    /// the next page fetched rather than to the next run of the program.
    pub fn limits(&self) -> Limits {
        Limits {
            max_article_bytes: self.max_article_bytes,
            max_markdown_bytes: self.max_markdown_bytes,
            images: self.images,
            retry_base_secs: self.refresh_minutes as i64 * 60,
        }
    }
}

/// One page of the ENTRIES list, and what the `/` filter made of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryPage {
    pub rows: Vec<EntryRow>,
    /// Indices into `rows`, in the order the list draws them. Equal to
    /// `0..rows.len()` when no filter is set -- which is what makes clearing
    /// the filter put the list back exactly as it was.
    pub visible: Vec<usize>,
    pub filter: String,
    /// How many rows the selection has in the database, which is what
    /// [`Command::MoreEntries`] stops at.
    pub total: i64,
    pub loading: bool,
}

impl EntryPage {
    fn refilter(&mut self) {
        self.visible = search::filter(&self.filter, &self.rows);
    }

    /// The rows the list draws, in order.
    pub fn visible_rows(&self) -> impl Iterator<Item = &EntryRow> {
        self.visible.iter().filter_map(|&i| self.rows.get(i))
    }

    pub fn row(&self, id: EntryId) -> Option<&EntryRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Whether every row the selection has is loaded.
    pub fn is_complete(&self) -> bool {
        self.rows.len() as i64 >= self.total
    }
}

/// How far a refresh has got. Mirrored into [`Event::Progress`] so the window
/// can redraw the bar without polling.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshProgress {
    pub done: usize,
    pub total: usize,
    pub running: bool,
    /// The feed being fetched, for the status line. A name rather than an
    /// id: the bar is text, and the row it names may not be loaded.
    pub current: Option<String>,
}

/// What can change the truth: a command the window asked for, or a worker
/// reporting that a job finished.
#[derive(Debug)]
pub enum Change {
    Command(Command),
    Done(Done),
}

/// What one [`apply`] implied. Both are returned rather than sent from
/// inside, so the write lock is released before anything touches a channel.
#[derive(Debug, Default)]
pub struct Effects {
    pub jobs: Vec<Job>,
    pub events: Vec<Event>,
}

impl Effects {
    fn none() -> Self {
        Self::default()
    }

    fn event(event: Event) -> Self {
        Self {
            jobs: Vec::new(),
            events: vec![event],
        }
    }

    fn job(job: Job) -> Self {
        Self {
            jobs: vec![job],
            events: Vec::new(),
        }
    }

    fn note(note: Note) -> Self {
        Self::event(Event::Note(note))
    }

    fn push_job(&mut self, job: Job) {
        self.jobs.push(job);
    }

    fn push_event(&mut self, event: Event) {
        self.events.push(event);
    }

    fn absorb(&mut self, other: Effects) {
        self.jobs.extend(other.jobs);
        self.events.extend(other.events);
    }
}

/// The whole of what the program knows, right now.
pub struct State {
    /// Every feed, in the order the source list draws them, with the unread
    /// count that is always a query rather than a column.
    pub feeds: Vec<FeedRow>,
    pub folders: Vec<Folder>,
    /// What the ENTRIES list is showing.
    pub selection: Selection,
    pub unread_only: bool,
    pub page: EntryPage,
    /// The open article's text, once it has been read back out of the
    /// database. `None` while one is loading, and after `CloseEntry`.
    pub article: Option<ArticleView>,
    /// The entry the reader has open, set the moment `OpenEntry` is sent and
    /// before its text has arrived.
    ///
    /// Not in the original contract, and added for one reason: an article
    /// loaded for an entry the reader has already stepped away from must not
    /// replace the one now open. It is also what tells an extraction landing
    /// behind an open entry to reload it and everything else to stay quiet.
    pub open_entry: Option<EntryId>,
    pub refresh: RefreshProgress,
    /// Feeds with a fetch in flight. A feed in here is never fetched again
    /// until its answer comes back, which is what stops a held-down `R` from
    /// opening four connections to one server.
    pub fetching: BTreeSet<FeedId>,
    /// Every feed counted into [`RefreshProgress::total`] for the refresh now
    /// running, cleared the moment one stops.
    ///
    /// The database answers `due` with the whole list every time it is asked
    /// -- the interval is the tick's business, not the query's -- so a second
    /// `Refresh` arriving while the first is still running used to add every
    /// feed that had already come back to the total a second time, and the
    /// bar read `12 of 82` for forty-one feeds. A feed is counted once per
    /// refresh, and this is the set that says which.
    pub refresh_counted: BTreeSet<FeedId>,
    /// Entries waiting for their page to be pulled, newest first.
    pub extract_queue: VecDeque<(EntryId, String)>,
    pub extract_inflight: usize,
    /// Set when there is a newsboat list to offer to import, and the window
    /// has not been told about it yet.
    pub import_offer: Option<ImportOffer>,
    /// What the last import came to. The import overlay's second stage.
    pub last_import: Option<ImportReport>,
    pub settings: RuntimeSettings,
    /// When a refresh of everything last started, from `meta.last_refresh`.
    pub last_refresh: Option<Timestamp>,
    /// When retention last swept, from `meta.last_retention`.
    pub last_retention: Option<Timestamp>,
    /// Bumped by `CancelRefresh`. A fetch or an extraction carries the
    /// generation it was issued under and is dropped, unopened, when it is
    /// behind this.
    pub refresh_generation: u64,
    /// Bumped by [`apply`] whenever it emitted an event, so the render loop
    /// can tell "nothing happened" from "copy a fresh view out" without
    /// comparing the whole structure.
    pub version: u64,
}

impl State {
    pub fn new(cfg: &WireConfig) -> Self {
        Self {
            feeds: Vec::new(),
            folders: Vec::new(),
            selection: Selection::All,
            unread_only: false,
            page: EntryPage::default(),
            article: None,
            open_entry: None,
            refresh: RefreshProgress::default(),
            fetching: BTreeSet::new(),
            refresh_counted: BTreeSet::new(),
            extract_queue: VecDeque::new(),
            extract_inflight: 0,
            import_offer: None,
            last_import: None,
            settings: RuntimeSettings::from(cfg),
            last_refresh: None,
            last_retention: None,
            refresh_generation: 0,
            version: 0,
        }
    }

    /// Whether a fetch for this feed is in flight -- the spinner beside a
    /// source row.
    pub fn is_fetching(&self, feed: FeedId) -> bool {
        self.fetching.contains(&feed)
    }

    pub fn feed(&self, id: FeedId) -> Option<&FeedRow> {
        self.feeds.iter().find(|f| f.id == id)
    }

    /// The unread count for a selection, from the rows already loaded where
    /// that is the whole truth and from the feed list otherwise.
    pub fn unread_total(&self) -> i64 {
        self.feeds.iter().map(|f| f.unread).sum()
    }

    /// The URL an entry points at, and whether it is a video -- looked up in
    /// what is loaded rather than asked of the database, because the only
    /// entries the window can ask to open are ones it is drawing.
    fn entry_target(&self, id: EntryId) -> Option<Target> {
        let row = self.page.row(id)?;
        let url = row.url.clone()?;
        Some(match row.kind {
            super::feed::EntryKind::Video => Target::Video(url),
            _ => super::open::kind_of(&url),
        })
    }
}

/// Fold one [`Change`] into `state`, and say what it implies.
///
/// `state.version` moves exactly when `events` comes back non-empty, which
/// is STAR/FOLD's rule and STAR/CORD's before it: an event is never emitted
/// for a change that left the drawn view alone, so "something worth telling
/// the window about happened" and "the version moved" are one question.
pub fn apply(state: &mut State, change: Change) -> Effects {
    let effects = match change {
        Change::Command(command) => apply_command(state, command),
        Change::Done(done) => apply_done(state, done),
    };
    if !effects.events.is_empty() {
        state.version += 1;
    }
    effects
}

fn apply_command(state: &mut State, command: Command) -> Effects {
    match command {
        Command::OpenFeed(sel) => cmd_open_feed(state, sel),
        Command::MoreEntries => cmd_more_entries(state),
        Command::ReloadEntries => cmd_reload_entries(state),
        Command::SetUnreadOnly(on) => cmd_set_unread_only(state, on),
        Command::Filter(query) => cmd_filter(state, query),
        Command::ClearFilter => cmd_filter(state, String::new()),
        Command::OpenEntry(id) => cmd_open_entry(state, id),
        Command::CloseEntry => cmd_close_entry(state),
        Command::SetRead { entries, read } => cmd_set_read(state, entries, read),
        Command::MarkAllRead(sel) => Effects::job(Job::Db(DbJob::MarkAllRead(sel))),
        Command::SetStarred { entry, on } => cmd_set_starred(state, entry, on),
        Command::Refresh(scope) => cmd_refresh(state, scope),
        Command::CancelRefresh => cmd_cancel_refresh(state),
        Command::Extract(id) => cmd_extract(state, id),
        Command::AddFeed { url, title, folder } => {
            Effects::job(Job::Db(DbJob::AddFeed { url, title, folder }))
        }
        Command::RemoveFeed(id) => Effects::job(Job::Db(DbJob::RemoveFeed(id))),
        Command::RenameFeed { feed, title } => {
            Effects::job(Job::Db(DbJob::RenameFeed { feed, title }))
        }
        Command::SetFeedFolder { feed, folder } => {
            Effects::job(Job::Db(DbJob::SetFeedFolder { feed, folder }))
        }
        Command::RenameFolder { folder, name } => {
            Effects::job(Job::Db(DbJob::RenameFolder { folder, name }))
        }
        Command::RemoveFolder(id) => Effects::job(Job::Db(DbJob::RemoveFolder(id))),
        Command::YoutubeAdd(input) => {
            Effects::job(Job::Net(NetJob::ResolveChannel { input }, Lane::Urgent))
        }
        Command::YoutubeImportTakeout(path) => Effects::job(Job::Db(DbJob::ImportTakeout(path))),
        Command::YoutubeSync {
            cookies_from_browser,
        } => Effects::job(Job::Net(
            NetJob::YtSubs {
                cookies_from_browser,
            },
            Lane::Urgent,
        )),
        Command::ImportNewsboat { urls, cache } => {
            state.import_offer = None;
            Effects::job(Job::Db(DbJob::ImportNewsboat { urls, cache }))
        }
        Command::ImportOpml(path) => Effects::job(Job::Db(DbJob::ImportOpml(path))),
        Command::ExportOpml(path) => Effects::job(Job::Db(DbJob::ExportOpml(path))),
        Command::DismissImportOffer => cmd_dismiss_import_offer(state),
        Command::Search(query) => cmd_open_feed(state, Selection::Search(query)),
        Command::OpenExternal(id) => cmd_open_external(state, id),
        Command::OpenUrl { url, kind } => Effects::job(Job::Net(
            NetJob::Open(match kind {
                OpenKind::Browser => Target::Browser(url),
                OpenKind::Video => Target::Video(url),
            }),
            Lane::Urgent,
        )),
        Command::SetSetting(setting) => cmd_set_setting(state, setting),
        // The threads are stopped by `Handle::drop`, which is the only thing
        // that can join them. Nothing to fold in.
        Command::Shutdown => Effects::none(),
    }
}

fn apply_done(state: &mut State, done: Done) -> Effects {
    match done {
        Done::Feeds { feeds, folders } => done_feeds(state, feeds, folders),
        Done::Entries {
            sel,
            unread_only,
            offset,
            page,
        } => done_entries(state, sel, unread_only, offset, page),
        Done::Article { entry, view } => done_article(state, entry, view),
        Done::Due {
            scope,
            feeds,
            generation,
        } => done_due(state, scope, feeds, generation),
        Done::Fetched {
            feed,
            kind,
            outcome,
            millis,
            generation,
        } => done_fetched(state, feed, kind, outcome, millis, generation),
        Done::Stored {
            feed,
            new,
            extractable,
        } => done_stored(state, feed, new, extractable),
        Done::Extracted {
            entry,
            result,
            generation,
        } => done_extracted(state, entry, result, generation),
        Done::ArticleStored { entry, status } => done_article_stored(state, entry, status),
        Done::FeedAdded {
            feed,
            url,
            kind,
            is_new,
        } => done_feed_added(state, feed, url, kind, is_new),
        Done::MarkedRead(n) => done_marked_read(state, n),
        Done::Imported(report) => done_imported(state, report),
        Done::Resolved { channels, source } => {
            if channels.is_empty() {
                return Effects::note(Note::warning("youtube", "no channels came back"));
            }
            Effects::job(Job::Db(DbJob::AddChannels { channels, source }))
        }
        Done::Pending(entries) => done_pending(state, entries),
        Done::Retained { retained, at } => done_retained(state, retained, at),
        Done::RefreshStamped(at) => {
            state.last_refresh = Some(at);
            Effects::none()
        }
        Done::External => Effects {
            jobs: vec![Job::Db(DbJob::LoadFeeds), reload_page(state)],
            events: Vec::new(),
        },
        Done::Tick(now) => done_tick(state, now),
        Done::Note(note) => Effects::note(note),
    }
}

// ------------------------------------------------------------ the list ----

fn load_entries(state: &State, offset: usize, limit: usize) -> Job {
    Job::Db(DbJob::LoadEntries {
        sel: state.selection.clone(),
        unread_only: state.unread_only,
        offset,
        limit,
    })
}

/// Re-page the current selection from the top, keeping as many rows loaded
/// as were loaded before -- a reload after a refresh must not shrink a list
/// somebody has paged four screens into.
fn reload_page(state: &State) -> Job {
    let limit = state.page.rows.len().max(state.settings.page_size);
    load_entries(state, 0, limit)
}

fn cmd_open_feed(state: &mut State, sel: Selection) -> Effects {
    state.selection = sel;
    state.page = EntryPage {
        loading: true,
        ..EntryPage::default()
    };
    Effects {
        jobs: vec![load_entries(state, 0, state.settings.page_size)],
        events: vec![Event::Entries],
    }
}

fn cmd_more_entries(state: &mut State) -> Effects {
    if state.page.loading || state.page.is_complete() {
        return Effects::none();
    }
    state.page.loading = true;
    Effects::job(load_entries(
        state,
        state.page.rows.len(),
        state.settings.page_size,
    ))
}

fn cmd_reload_entries(state: &mut State) -> Effects {
    state.page.loading = true;
    Effects::job(reload_page(state))
}

fn cmd_set_unread_only(state: &mut State, on: bool) -> Effects {
    if state.unread_only == on {
        return Effects::none();
    }
    state.unread_only = on;
    state.page.loading = true;
    Effects {
        jobs: vec![load_entries(state, 0, state.settings.page_size)],
        events: vec![Event::Entries],
    }
}

fn cmd_filter(state: &mut State, query: String) -> Effects {
    if state.page.filter == query {
        return Effects::none();
    }
    state.page.filter = query;
    state.page.refilter();
    Effects::event(Event::Entries)
}

fn done_entries(
    state: &mut State,
    sel: Selection,
    unread_only: bool,
    offset: usize,
    page: entries::Page,
) -> Effects {
    // A page for a selection the window has already left. Dropped rather
    // than shown: the command that changed the selection has its own page
    // on the way.
    if sel != state.selection || unread_only != state.unread_only {
        return Effects::none();
    }
    state.page.total = page.total;
    state.page.loading = false;
    if offset == 0 {
        state.page.rows = page.rows;
    } else if offset == state.page.rows.len() {
        state.page.rows.extend(page.rows);
    } else {
        // Neither the first page nor the next one: two `MoreEntries` raced,
        // or rows were swept underneath. The rows on screen stay; the next
        // reload settles it.
        return Effects::none();
    }
    state.page.refilter();
    Effects::event(Event::Entries)
}

// --------------------------------------------------------- the article ----

fn cmd_open_entry(state: &mut State, id: EntryId) -> Effects {
    state.open_entry = Some(id);
    state.article = None;
    let mut effects = Effects {
        jobs: vec![Job::Db(DbJob::LoadArticle(id))],
        events: vec![Event::Article(id)],
    };
    // Opening an entry whose page has never been pulled asks for it now and
    // jumps the queue: this is the one extraction somebody is waiting on.
    if state.settings.extract {
        if let Some(row) = state.page.row(id) {
            if row.article_status.is_pending() {
                if let Some(url) = row.url.clone() {
                    state.extract_inflight += 1;
                    effects.push_job(Job::Net(
                        NetJob::Extract {
                            entry: id,
                            url,
                            limits: state.settings.limits(),
                            generation: state.refresh_generation,
                        },
                        Lane::Urgent,
                    ));
                }
            }
        }
    }
    effects
}

fn cmd_close_entry(state: &mut State) -> Effects {
    let Some(entry) = state.open_entry.take() else {
        return Effects::none();
    };
    state.article = None;
    Effects::event(Event::Article(entry))
}

/// Force an extraction, whatever the article's status -- the reader's `e`.
fn cmd_extract(state: &mut State, id: EntryId) -> Effects {
    let url = state.page.row(id).and_then(|r| r.url.clone()).or_else(|| {
        state
            .article
            .as_ref()
            .filter(|a| a.entry == id)?
            .url
            .clone()
    });
    let Some(url) = url else {
        return Effects::note(Note::warning("extract", "that entry has no link to pull"));
    };
    state.extract_inflight += 1;
    Effects::job(Job::Net(
        NetJob::Extract {
            entry: id,
            url,
            limits: state.settings.limits(),
            generation: state.refresh_generation,
        },
        Lane::Urgent,
    ))
}

fn done_article(state: &mut State, entry: EntryId, view: Option<Box<ArticleView>>) -> Effects {
    if state.open_entry != Some(entry) {
        return Effects::none();
    }
    state.article = view.map(|v| *v);
    Effects::event(Event::Article(entry))
}

fn done_article_stored(state: &mut State, entry: EntryId, status: ArticleStatus) -> Effects {
    let mut effects = Effects::none();
    if let Some(row) = state.page.rows.iter_mut().find(|r| r.id == entry) {
        if row.article_status != status {
            row.article_status = status;
            effects.push_event(Event::Entries);
        }
    }
    // The text landed behind an entry somebody already has open: read it
    // back so the reader swaps `extracting…` for the article.
    if state.open_entry == Some(entry) {
        effects.push_job(Job::Db(DbJob::LoadArticle(entry)));
    }
    effects
}

// ------------------------------------------------------------ the flags ----

fn cmd_set_read(state: &mut State, ids: Vec<EntryId>, read: bool) -> Effects {
    if ids.is_empty() {
        return Effects::none();
    }
    let mut moved = false;
    for id in &ids {
        let Some(row) = state.page.rows.iter_mut().find(|r| r.id == *id) else {
            continue;
        };
        if row.read == read {
            continue;
        }
        row.read = read;
        moved = true;
        let feed = row.feed_id;
        if let Some(f) = state.feeds.iter_mut().find(|f| f.id == feed) {
            f.unread = if read {
                (f.unread - 1).max(0)
            } else {
                f.unread + 1
            };
        }
    }
    let mut effects = Effects::job(Job::Db(DbJob::SetRead { entries: ids, read }));
    if moved {
        effects.push_event(Event::Entries);
        effects.push_event(Event::Feeds);
    }
    effects
}

fn cmd_set_starred(state: &mut State, entry: EntryId, on: bool) -> Effects {
    let mut effects = Effects::job(Job::Db(DbJob::SetFlag { entry, starred: on }));
    if let Some(row) = state.page.rows.iter_mut().find(|r| r.id == entry) {
        if row.starred != on {
            row.starred = on;
            effects.push_event(Event::Entries);
        }
    }
    effects
}

fn done_marked_read(state: &mut State, n: usize) -> Effects {
    Effects {
        jobs: vec![Job::Db(DbJob::LoadFeeds), reload_page(state)],
        events: vec![Event::Note(Note::info(match n {
            0 => "nothing here was unread".to_string(),
            1 => "1 entry marked read".to_string(),
            n => format!("{n} entries marked read"),
        }))],
    }
}

// --------------------------------------------------------- the refresh ----

fn cmd_refresh(state: &mut State, scope: RefreshScope) -> Effects {
    // Which feeds are due is the database's answer, not this module's: the
    // conditional headers and the backoff live in the row and never in
    // `State`.
    Effects::job(Job::Db(DbJob::Due {
        scope,
        generation: state.refresh_generation,
    }))
}

fn done_due(
    state: &mut State,
    scope: RefreshScope,
    feeds: Vec<DueFeed>,
    generation: u64,
) -> Effects {
    if generation != state.refresh_generation {
        return Effects::none();
    }
    // A feed already in flight, and a feed this refresh has already counted
    // and finished with, are both left out: the bar counts each feed once
    // for as long as one refresh is running.
    let wanted: Vec<DueFeed> = feeds
        .into_iter()
        .filter(|f| {
            in_scope(state, &scope, f.id)
                && !state.fetching.contains(&f.id)
                && !state.refresh_counted.contains(&f.id)
        })
        .collect();

    let mut effects = Effects::none();
    if matches!(scope, RefreshScope::All) {
        // Stamped when a refresh of everything *starts*, which is the
        // question the tick asks -- "how long since we last tried" -- and
        // the answer stays right when half the list is backing off.
        effects.push_job(Job::Db(DbJob::RecordRefresh));
    }

    if wanted.is_empty() {
        if !state.fetching.is_empty() {
            return effects;
        }
        let (done, total) = (state.refresh.done, state.refresh.total);
        end_refresh(state);
        effects.push_event(Event::Progress { done, total });
        return effects;
    }

    if state.refresh.running {
        state.refresh.total += wanted.len();
    } else {
        state.refresh.done = 0;
        state.refresh.total = wanted.len();
        state.refresh.running = true;
    }

    let open_feed = match &state.selection {
        Selection::Feed(id) => Some(*id),
        _ => None,
    };
    for due in wanted {
        state.fetching.insert(due.id);
        state.refresh_counted.insert(due.id);
        // The feed on screen first, and everything the window explicitly
        // asked for; the rest of the list goes behind it so that a refresh
        // of forty-one feeds does not make the one being read wait.
        let lane = if Some(due.id) == open_feed || !matches!(scope, RefreshScope::All) {
            Lane::Urgent
        } else {
            Lane::Background
        };
        effects.push_job(Job::Net(
            NetJob::Fetch {
                feed: due.id,
                url: due.url,
                kind: due.kind,
                conditional: due.conditional,
                max_bytes: state.settings.max_feed_bytes,
                generation,
            },
            lane,
        ));
    }
    state.refresh.current = current_feed_name(state);
    effects.push_event(Event::Progress {
        done: state.refresh.done,
        total: state.refresh.total,
    });
    effects
}

/// The refresh is over: the bar goes away, and the counters go back to
/// nothing so the next one starts from zero rather than from what the last
/// one left behind.
///
/// Called *after* the `n of n` event has been pushed, which is the event a
/// window or a test watches for: the numbers are the news, and the state is
/// what is true once the news has been read.
fn end_refresh(state: &mut State) {
    state.refresh.running = false;
    state.refresh.current = None;
    state.refresh.done = 0;
    state.refresh.total = 0;
    state.refresh_counted.clear();
}

fn in_scope(state: &State, scope: &RefreshScope, feed: FeedId) -> bool {
    match scope {
        RefreshScope::All => true,
        RefreshScope::Feed(id) => *id == feed,
        RefreshScope::Folder(folder) => state.feed(feed).is_some_and(|f| f.folder == Some(*folder)),
    }
}

fn current_feed_name(state: &State) -> Option<String> {
    let id = *state.fetching.iter().next()?;
    Some(match state.feed(id) {
        Some(row) => row.display_title().to_string(),
        None => id.to_string(),
    })
}

fn cmd_cancel_refresh(state: &mut State) -> Effects {
    state.refresh_generation += 1;
    state.fetching.clear();
    state.extract_queue.clear();
    state.extract_inflight = 0;
    end_refresh(state);
    Effects {
        jobs: Vec::new(),
        events: vec![
            // Nothing of nothing: a cancelled refresh has no last number to
            // leave on screen, and the bar is gone by the time this is read.
            Event::Progress { done: 0, total: 0 },
            Event::Note(Note::info("refresh cancelled")),
        ],
    }
}

fn done_fetched(
    state: &mut State,
    feed: FeedId,
    kind: super::feed::FeedKind,
    outcome: super::fetch::FetchOutcome,
    millis: u128,
    generation: u64,
) -> Effects {
    // Whether this feed was one the bar is counting. A second answer for a
    // feed already folded in -- a re-issued job the first answer beat home --
    // is stored and not counted twice.
    let counted = state.fetching.remove(&feed);
    if generation != state.refresh_generation {
        // Cancelled while it was in flight. What came back is not written:
        // the whole point of cancelling is that the list stops moving.
        return Effects::none();
    }

    let mut effects = Effects::none();
    effects.push_job(Job::Db(match outcome {
        super::fetch::FetchOutcome::Fetched {
            feed: parsed,
            conditional,
            bytes,
        } => DbJob::Store {
            feed,
            kind,
            parsed: Box::new(parsed),
            conditional,
            bytes,
            millis,
        },
        super::fetch::FetchOutcome::NotModified => DbJob::RecordUnchanged { feed, millis },
        super::fetch::FetchOutcome::Failed {
            error,
            status,
            retry_after,
        } => DbJob::RecordFailure {
            feed,
            error,
            status,
            retry_after,
            refresh_minutes: state.settings.refresh_minutes,
            millis,
        },
    }));

    if counted {
        state.refresh.done += 1;
        let (done, total) = (state.refresh.done, state.refresh.total);
        if done >= total && state.fetching.is_empty() {
            end_refresh(state);
            effects.push_job(Job::Db(DbJob::LoadFeeds));
        } else {
            state.refresh.current = current_feed_name(state);
        }
        effects.push_event(Event::Progress { done, total });
    }
    effects
}

fn done_stored(
    state: &mut State,
    feed: FeedId,
    new: usize,
    extractable: Vec<(EntryId, String)>,
) -> Effects {
    let mut effects = Effects::none();
    if new == 0 {
        return effects;
    }
    effects.push_job(Job::Db(DbJob::LoadFeeds));
    // Only when the list on screen would have changed. A background feed
    // arriving behind a reader who is three pages into another one must not
    // move the rows under the cursor.
    if selection_covers(state, feed) {
        effects.push_job(reload_page(state));
    }
    if state.settings.extract {
        state.extract_queue.extend(extractable);
        effects.absorb(pump_extractions(state));
    }
    effects
}

fn selection_covers(state: &State, feed: FeedId) -> bool {
    match &state.selection {
        Selection::All | Selection::Videos => true,
        Selection::Feed(id) => *id == feed,
        Selection::Folder(folder) => state.feed(feed).is_some_and(|f| f.folder == Some(*folder)),
        // A star is set by hand and a search is a query over text already
        // stored: neither changes because a feed arrived.
        Selection::Starred | Selection::Search(_) => false,
    }
}

// ------------------------------------------------------ the extractions ----

/// Start as many queued extractions as the in-flight ceiling allows.
///
/// Twice the thread count, so every thread has one to pick up and one
/// waiting: more than that is a queue held in a channel instead of in
/// `State`, where the generation stamp cannot reach it.
fn pump_extractions(state: &mut State) -> Effects {
    let ceiling = state.settings.parallel.saturating_mul(2).max(1);
    let mut effects = Effects::none();
    while state.extract_inflight < ceiling {
        let Some((entry, url)) = state.extract_queue.pop_front() else {
            break;
        };
        state.extract_inflight += 1;
        effects.push_job(Job::Net(
            NetJob::Extract {
                entry,
                url,
                limits: state.settings.limits(),
                generation: state.refresh_generation,
            },
            Lane::Background,
        ));
    }
    effects
}

fn done_extracted(
    state: &mut State,
    entry: EntryId,
    result: Box<super::extract::ArticleResult>,
    generation: u64,
) -> Effects {
    state.extract_inflight = state.extract_inflight.saturating_sub(1);
    if generation != state.refresh_generation {
        return pump_extractions(state);
    }
    let mut effects = Effects::job(Job::Db(DbJob::StoreArticle { entry, result }));
    effects.absorb(pump_extractions(state));
    effects
}

fn done_pending(state: &mut State, entries: Vec<(EntryId, String)>) -> Effects {
    if entries.is_empty() || !state.settings.extract {
        return Effects::none();
    }
    let queued: std::collections::HashSet<EntryId> =
        state.extract_queue.iter().map(|(id, _)| *id).collect();
    for (id, url) in entries {
        if !queued.contains(&id) {
            state.extract_queue.push_back((id, url));
        }
    }
    pump_extractions(state)
}

// ------------------------------------------------- feeds, folders, import ----

fn done_feeds(state: &mut State, feeds: Vec<FeedRow>, folders: Vec<Folder>) -> Effects {
    if state.feeds == feeds && state.folders == folders {
        return Effects::none();
    }
    state.feeds = feeds;
    state.folders = folders;
    Effects::event(Event::Feeds)
}

fn done_feed_added(
    state: &mut State,
    feed: FeedId,
    url: String,
    kind: super::feed::FeedKind,
    is_new: bool,
) -> Effects {
    let mut effects = Effects::job(Job::Db(DbJob::LoadFeeds));
    if is_new {
        state.fetching.insert(feed);
        state.refresh_counted.insert(feed);
        effects.push_job(Job::Net(
            NetJob::Fetch {
                feed,
                url: url.clone(),
                kind,
                conditional: super::db::feeds::Conditional::default(),
                max_bytes: state.settings.max_feed_bytes,
                generation: state.refresh_generation,
            },
            Lane::Urgent,
        ));
        if !state.refresh.running {
            state.refresh.done = 0;
            state.refresh.total = 1;
            state.refresh.running = true;
        } else {
            state.refresh.total += 1;
        }
        state.refresh.current = current_feed_name(state);
        effects.push_event(Event::Progress {
            done: state.refresh.done,
            total: state.refresh.total,
        });
    }
    effects.push_event(Event::Note(Note::info(if is_new {
        format!("added {url}")
    } else {
        format!("{url} was already on the list")
    })));
    effects
}

fn done_imported(state: &mut State, report: ImportReport) -> Effects {
    let text = format!(
        "imported {} feed{}{}{}",
        report.added,
        if report.added == 1 { "" } else { "s" },
        if report.already_there > 0 {
            format!(" · {} already there", report.already_there)
        } else {
            String::new()
        },
        if report.skipped.is_empty() {
            String::new()
        } else {
            format!(" · {} skipped", report.skipped.len())
        }
    );
    state.last_import = Some(report);
    state.import_offer = None;
    Effects {
        // The list is worth nothing until it has been fetched once, so an
        // import is followed by a refresh with the ordinary progress bar.
        jobs: vec![
            Job::Db(DbJob::LoadFeeds),
            Job::Db(DbJob::Due {
                scope: RefreshScope::All,
                generation: state.refresh_generation,
            }),
        ],
        events: vec![Event::Feeds, Event::Note(Note::info(text))],
    }
}

fn cmd_dismiss_import_offer(state: &mut State) -> Effects {
    state.import_offer = None;
    Effects {
        // Written down, not merely forgotten: an offer that comes back after
        // being refused is a bug rather than a reminder.
        jobs: vec![Job::Db(DbJob::DismissImportOffer)],
        events: vec![Event::ImportOffer],
    }
}

fn done_retained(state: &mut State, retained: entries::Retained, at: Timestamp) -> Effects {
    state.last_retention = Some(at);
    if retained.by_age + retained.by_count == 0 {
        return Effects::none();
    }
    Effects {
        jobs: vec![Job::Db(DbJob::LoadFeeds), reload_page(state)],
        events: Vec::new(),
    }
}

// -------------------------------------------------------------- opening ----

fn cmd_open_external(state: &mut State, id: EntryId) -> Effects {
    match state.entry_target(id) {
        Some(target) => Effects::job(Job::Net(NetJob::Open(target), Lane::Urgent)),
        None => Effects::note(Note::warning("open", "that entry has no link")),
    }
}

// ------------------------------------------------------------ the clock ----

const DAY: i64 = 24 * 60 * 60;

fn done_tick(state: &mut State, now: Timestamp) -> Effects {
    let mut effects = Effects::none();

    let refresh_secs = i64::from(state.settings.refresh_minutes) * 60;
    if refresh_secs > 0 && !state.refresh.running {
        let due = state
            .last_refresh
            .is_none_or(|last| now.as_second() - last.as_second() >= refresh_secs);
        if due {
            effects.push_job(Job::Db(DbJob::Due {
                scope: RefreshScope::All,
                generation: state.refresh_generation,
            }));
        }
    }

    if state
        .last_retention
        .is_none_or(|last| now.as_second() - last.as_second() >= DAY)
    {
        effects.push_job(Job::Db(DbJob::Retain {
            keep_days: state.settings.keep_days,
            max_per_feed: state.settings.max_entries_per_feed,
        }));
    }

    // Whatever is still pending -- an entry whose page was down an hour ago,
    // or one whose job did not fit its queue and was dropped. This is what
    // makes a dropped job cost a delay rather than an article.
    if state.settings.extract && state.extract_queue.is_empty() && state.extract_inflight == 0 {
        effects.push_job(Job::Db(DbJob::PendingExtracts {
            limit: state.settings.parallel.saturating_mul(2).max(1),
        }));
    }

    effects
}

// ------------------------------------------------------------ settings ----

fn cmd_set_setting(state: &mut State, setting: Setting) -> Effects {
    match setting {
        Setting::RefreshMinutes(n) => {
            state.settings.refresh_minutes = n;
            Effects::none()
        }
        Setting::Extract(on) => {
            state.settings.extract = on;
            if !on {
                state.extract_queue.clear();
            }
            Effects::none()
        }
        Setting::Images(on) => {
            state.settings.images = on;
            Effects::none()
        }
        Setting::KeepDays(days) => {
            state.settings.keep_days = days;
            Effects::none()
        }
        Setting::UnreadOnly(on) => cmd_set_unread_only(state, on),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::db::entries::Page;
    use crate::wire::feed::{ArticleStatus, EntryKind, FeedKind, FolderId};
    use crate::wire::worker::Done;
    use proptest::prelude::*;

    fn state() -> State {
        let mut s = State::new(&WireConfig::default());
        s.feeds = vec![
            feed(1, "Example", Some(FolderId(1))),
            feed(2, "Hacker News", None),
        ];
        s.folders = vec![Folder {
            id: FolderId(1),
            name: "Tech".into(),
            position: 0,
        }];
        s
    }

    fn feed(id: i64, title: &str, folder: Option<FolderId>) -> FeedRow {
        FeedRow {
            id: FeedId(id),
            url: format!("https://example.org/{id}"),
            source_url: None,
            kind: FeedKind::Web,
            title: Some(title.into()),
            custom_title: None,
            site_url: None,
            folder,
            position: id,
            unread: 3,
            last_ok: None,
            error: None,
            failures: 0,
            backoff_until: None,
        }
    }

    fn row(id: i64, feed: i64, title: &str) -> EntryRow {
        EntryRow {
            id: EntryId(id),
            feed_id: FeedId(feed),
            feed_title: "Example".into(),
            title: title.into(),
            author: None,
            url: Some(format!("https://example.org/posts/{id}")),
            published: None,
            kind: EntryKind::Article,
            read: false,
            starred: false,
            article_status: ArticleStatus::Pending,
            thumbnail_url: None,
        }
    }

    /// Hand the state a page for whatever it currently has open.
    fn deliver(s: &mut State, rows: Vec<EntryRow>, total: i64, offset: usize) -> Effects {
        let sel = s.selection.clone();
        let unread_only = s.unread_only;
        apply(
            s,
            Change::Done(Done::Entries {
                sel,
                unread_only,
                offset,
                page: Page { rows, total },
            }),
        )
    }

    fn db_jobs(effects: &Effects) -> Vec<&DbJob> {
        effects
            .jobs
            .iter()
            .filter_map(|j| match j {
                Job::Db(job) => Some(job),
                _ => None,
            })
            .collect()
    }

    fn net_jobs(effects: &Effects) -> Vec<(&NetJob, Lane)> {
        effects
            .jobs
            .iter()
            .filter_map(|j| match j {
                Job::Net(job, lane) => Some((job, *lane)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn opening_a_selection_clears_the_page_and_asks_for_the_first_one() {
        let mut s = state();
        deliver(&mut s, vec![row(1, 1, "one")], 1, 0);
        let effects = apply(
            &mut s,
            Change::Command(Command::OpenFeed(Selection::Feed(FeedId(2)))),
        );
        assert_eq!(s.selection, Selection::Feed(FeedId(2)));
        assert!(
            s.page.rows.is_empty(),
            "the old feed's rows must not linger"
        );
        assert!(s.page.loading);
        assert!(matches!(
            db_jobs(&effects).as_slice(),
            [DbJob::LoadEntries { offset: 0, .. }]
        ));
    }

    #[test]
    fn a_page_for_a_selection_the_window_has_left_is_dropped() {
        let mut s = state();
        let effects = apply(
            &mut s,
            Change::Done(Done::Entries {
                sel: Selection::Starred,
                unread_only: false,
                offset: 0,
                page: Page {
                    rows: vec![row(1, 1, "one")],
                    total: 1,
                },
            }),
        );
        assert!(s.page.rows.is_empty());
        assert!(effects.events.is_empty());
    }

    #[test]
    fn more_entries_appends_and_then_stops_at_the_total() {
        let mut s = state();
        s.settings.page_size = 2;
        deliver(&mut s, vec![row(1, 1, "one"), row(2, 1, "two")], 3, 0);

        let effects = apply(&mut s, Change::Command(Command::MoreEntries));
        assert!(matches!(
            db_jobs(&effects).as_slice(),
            [DbJob::LoadEntries { offset: 2, .. }]
        ));
        deliver(&mut s, vec![row(3, 1, "three")], 3, 2);
        assert_eq!(s.page.rows.len(), 3);
        assert_eq!(s.page.visible.len(), 3);

        // Everything is loaded: asking again is a no-op rather than a query
        // that comes back empty.
        let effects = apply(&mut s, Change::Command(Command::MoreEntries));
        assert!(effects.jobs.is_empty());
    }

    #[test]
    fn filtering_moves_the_visible_list_and_nothing_else() {
        let mut s = state();
        deliver(
            &mut s,
            vec![row(1, 1, "lifetimes"), row(2, 1, "something else")],
            2,
            0,
        );
        let effects = apply(&mut s, Change::Command(Command::Filter("lifetimes".into())));
        assert!(
            effects.jobs.is_empty(),
            "the filter never asks the database"
        );
        assert_eq!(s.page.rows.len(), 2);
        assert_eq!(s.page.visible, vec![0]);

        apply(&mut s, Change::Command(Command::ClearFilter));
        assert_eq!(s.page.visible, vec![0, 1]);
    }

    #[test]
    fn opening_an_entry_loads_its_text_pulls_its_page_and_does_not_mark_it_read() {
        let mut s = state();
        deliver(&mut s, vec![row(1, 1, "one")], 1, 0);
        let effects = apply(&mut s, Change::Command(Command::OpenEntry(EntryId(1))));

        assert_eq!(s.open_entry, Some(EntryId(1)));
        assert!(matches!(
            db_jobs(&effects).as_slice(),
            [DbJob::LoadArticle(_)]
        ));
        assert!(matches!(
            net_jobs(&effects).as_slice(),
            [(NetJob::Extract { .. }, Lane::Urgent)]
        ));
        assert!(
            !s.page.rows[0].read,
            "opening is not reading -- the window sends SetRead when the config says so"
        );
    }

    #[test]
    fn an_article_for_an_entry_the_reader_has_left_is_dropped() {
        let mut s = state();
        deliver(&mut s, vec![row(1, 1, "one"), row(2, 1, "two")], 2, 0);
        apply(&mut s, Change::Command(Command::OpenEntry(EntryId(2))));
        let effects = apply(
            &mut s,
            Change::Done(Done::Article {
                entry: EntryId(1),
                view: Some(Box::new(view(EntryId(1)))),
            }),
        );
        assert!(s.article.is_none());
        assert!(effects.events.is_empty());

        apply(
            &mut s,
            Change::Done(Done::Article {
                entry: EntryId(2),
                view: Some(Box::new(view(EntryId(2)))),
            }),
        );
        assert_eq!(s.article.as_ref().map(|a| a.entry), Some(EntryId(2)));
    }

    fn view(entry: EntryId) -> ArticleView {
        ArticleView {
            entry,
            title: "A title".into(),
            byline: None,
            site_name: None,
            url: None,
            markdown: std::sync::Arc::from("text"),
            status: ArticleStatus::Extracted,
            image_url: None,
            error: None,
            extracted_at: None,
        }
    }

    #[test]
    fn an_extraction_landing_behind_the_open_entry_reloads_it() {
        let mut s = state();
        deliver(&mut s, vec![row(1, 1, "one")], 1, 0);
        apply(&mut s, Change::Command(Command::OpenEntry(EntryId(1))));
        let effects = apply(
            &mut s,
            Change::Done(Done::ArticleStored {
                entry: EntryId(1),
                status: ArticleStatus::Extracted,
            }),
        );
        assert_eq!(s.page.rows[0].article_status, ArticleStatus::Extracted);
        assert!(matches!(
            db_jobs(&effects).as_slice(),
            [DbJob::LoadArticle(EntryId(1))]
        ));
    }

    #[test]
    fn marking_read_moves_the_row_and_the_count_before_the_write_lands() {
        let mut s = state();
        deliver(&mut s, vec![row(1, 1, "one")], 1, 0);
        let before = s.unread_total();
        let effects = apply(
            &mut s,
            Change::Command(Command::SetRead {
                entries: vec![EntryId(1)],
                read: true,
            }),
        );
        assert!(s.page.rows[0].read);
        assert_eq!(s.unread_total(), before - 1);
        assert!(matches!(
            db_jobs(&effects).as_slice(),
            [DbJob::SetRead { read: true, .. }]
        ));

        // And back again, so `m` on a read entry is not a one-way door.
        apply(
            &mut s,
            Change::Command(Command::SetRead {
                entries: vec![EntryId(1)],
                read: false,
            }),
        );
        assert!(!s.page.rows[0].read);
        assert_eq!(s.unread_total(), before);
    }

    #[test]
    fn starring_is_optimistic_too() {
        let mut s = state();
        deliver(&mut s, vec![row(1, 1, "one")], 1, 0);
        apply(
            &mut s,
            Change::Command(Command::SetStarred {
                entry: EntryId(1),
                on: true,
            }),
        );
        assert!(s.page.rows[0].starred);
    }

    fn due(id: i64) -> DueFeed {
        DueFeed {
            id: FeedId(id),
            url: format!("https://example.org/{id}"),
            kind: super::super::feed::FeedKind::Web,
            conditional: super::super::db::feeds::Conditional::default(),
            backoff_until: None,
        }
    }

    #[test]
    fn a_refresh_asks_the_database_which_feeds_are_due() {
        let mut s = state();
        let effects = apply(&mut s, Change::Command(Command::Refresh(RefreshScope::All)));
        assert!(matches!(
            db_jobs(&effects).as_slice(),
            [DbJob::Due {
                scope: RefreshScope::All,
                ..
            }]
        ));
    }

    #[test]
    fn the_feed_on_screen_is_fetched_on_the_urgent_lane_and_the_rest_behind_it() {
        let mut s = state();
        s.selection = Selection::Feed(FeedId(2));
        let effects = apply(
            &mut s,
            Change::Done(Done::Due {
                scope: RefreshScope::All,
                feeds: vec![due(1), due(2)],
                generation: 0,
            }),
        );
        let lanes: Vec<Lane> = net_jobs(&effects)
            .into_iter()
            .map(|(_, lane)| lane)
            .collect();
        assert_eq!(lanes, vec![Lane::Background, Lane::Urgent]);
        assert_eq!(s.refresh.total, 2);
        assert!(s.refresh.running);
        assert_eq!(s.fetching.len(), 2);
    }

    #[test]
    fn a_feed_already_in_flight_is_not_fetched_twice() {
        let mut s = state();
        apply(
            &mut s,
            Change::Done(Done::Due {
                scope: RefreshScope::All,
                feeds: vec![due(1)],
                generation: 0,
            }),
        );
        let effects = apply(
            &mut s,
            Change::Done(Done::Due {
                scope: RefreshScope::All,
                feeds: vec![due(1)],
                generation: 0,
            }),
        );
        assert!(net_jobs(&effects).is_empty());
        assert_eq!(s.refresh.total, 1);
    }

    /// The bug this guards: `due` answers with the whole list every time it
    /// is asked, so a second `Refresh(All)` landing while the first is still
    /// running once added every feed to the total again -- `12 of 82` for
    /// forty-one feeds. Run over the fixture, with the real `due`, the real
    /// fetches and the real `apply`, because that is where the two answers
    /// actually overlap.
    #[test]
    fn two_refreshes_over_one_another_still_count_each_feed_once() {
        let cfg = WireConfig::default();
        let (handle, mut driver) = crate::wire::testing::driver(&cfg);
        driver.pump();
        let feeds = handle.state().feeds.len();
        assert!(feeds > 1, "the fixture has a list worth refreshing");

        handle.send(Command::Refresh(RefreshScope::All));
        handle.send(Command::Refresh(RefreshScope::All));
        driver.pump();

        let progress: Vec<(usize, usize)> = handle
            .drain()
            .filter_map(|e| match e {
                Event::Progress { done, total } => Some((done, total)),
                _ => None,
            })
            .collect();
        assert!(
            progress.iter().all(|(_, total)| *total == feeds),
            "the total is the feed count throughout, not twice it: {progress:?}"
        );
        assert!(
            progress.contains(&(feeds, feeds)),
            "the bar reaches {feeds} of {feeds}: {progress:?}"
        );
        let s = handle.state();
        assert!(!s.refresh.running);
        assert!(s.fetching.is_empty());
        assert!(s.refresh_counted.is_empty(), "and the set is put away");
    }

    #[test]
    fn a_folder_refresh_only_covers_that_folders_feeds() {
        let mut s = state();
        let effects = apply(
            &mut s,
            Change::Done(Done::Due {
                scope: RefreshScope::Folder(FolderId(1)),
                feeds: vec![due(1), due(2)],
                generation: 0,
            }),
        );
        assert_eq!(net_jobs(&effects).len(), 1);
        assert!(s.fetching.contains(&FeedId(1)));
    }

    #[test]
    fn cancelling_a_refresh_drops_what_comes_back_afterwards() {
        let mut s = state();
        apply(
            &mut s,
            Change::Done(Done::Due {
                scope: RefreshScope::All,
                feeds: vec![due(1)],
                generation: 0,
            }),
        );
        apply(&mut s, Change::Command(Command::CancelRefresh));
        assert!(s.fetching.is_empty());
        assert!(!s.refresh.running);
        assert_eq!(s.refresh_generation, 1);

        let effects = apply(
            &mut s,
            Change::Done(Done::Fetched {
                feed: FeedId(1),
                kind: FeedKind::Web,
                outcome: super::super::fetch::FetchOutcome::NotModified,
                millis: 1,
                generation: 0,
            }),
        );
        assert!(
            effects.jobs.is_empty(),
            "a cancelled fetch must not be written: the point of cancelling \
             is that the list stops moving"
        );
    }

    #[test]
    fn a_fetch_that_lands_moves_the_bar_and_queues_the_store() {
        let mut s = state();
        apply(
            &mut s,
            Change::Done(Done::Due {
                scope: RefreshScope::All,
                feeds: vec![due(1)],
                generation: 0,
            }),
        );
        let effects = apply(
            &mut s,
            Change::Done(Done::Fetched {
                feed: FeedId(1),
                kind: FeedKind::Web,
                outcome: super::super::fetch::FetchOutcome::NotModified,
                millis: 4,
                generation: 0,
            }),
        );
        assert!(!s.refresh.running, "the last feed in ends the refresh");
        assert_eq!(
            (s.refresh.done, s.refresh.total),
            (0, 0),
            "and the counters go back to nothing behind the `1 of 1` event"
        );
        assert!(matches!(
            db_jobs(&effects).as_slice(),
            [DbJob::RecordUnchanged { .. }, DbJob::LoadFeeds]
        ));
        assert!(effects
            .events
            .iter()
            .any(|e| matches!(e, Event::Progress { done: 1, total: 1 })));
    }

    #[test]
    fn no_more_extractions_are_started_than_twice_the_thread_count() {
        let mut s = state();
        s.settings.parallel = 2;
        let effects = apply(
            &mut s,
            Change::Done(Done::Stored {
                feed: FeedId(1),
                new: 9,
                extractable: (1..=9)
                    .map(|i| (EntryId(i), format!("https://example.org/{i}")))
                    .collect(),
            }),
        );
        assert_eq!(net_jobs(&effects).len(), 4);
        assert_eq!(s.extract_inflight, 4);
        assert_eq!(s.extract_queue.len(), 5);

        // One comes back, one goes out. The queue is what holds the rest,
        // where the generation stamp can still reach it.
        let effects = apply(
            &mut s,
            Change::Done(Done::Extracted {
                entry: EntryId(1),
                result: Box::new(super::super::extract::ArticleResult::default()),
                generation: 0,
            }),
        );
        assert_eq!(net_jobs(&effects).len(), 1);
        assert_eq!(s.extract_inflight, 4);
        assert_eq!(s.extract_queue.len(), 4);
    }

    #[test]
    fn nothing_is_extracted_when_the_setting_is_off() {
        let mut s = state();
        s.settings.extract = false;
        let effects = apply(
            &mut s,
            Change::Done(Done::Stored {
                feed: FeedId(1),
                new: 1,
                extractable: vec![(EntryId(1), "https://example.org/1".into())],
            }),
        );
        assert!(net_jobs(&effects).is_empty());
        assert!(s.extract_queue.is_empty());
    }

    #[test]
    fn a_feed_arriving_behind_another_selection_does_not_move_the_rows_on_screen() {
        let mut s = state();
        s.selection = Selection::Feed(FeedId(2));
        let effects = apply(
            &mut s,
            Change::Done(Done::Stored {
                feed: FeedId(1),
                new: 4,
                extractable: Vec::new(),
            }),
        );
        assert!(
            !db_jobs(&effects)
                .iter()
                .any(|j| matches!(j, DbJob::LoadEntries { .. })),
            "the list being read must not shuffle because a background feed arrived"
        );
    }

    #[test]
    fn a_tick_asks_for_a_refresh_only_once_the_interval_has_passed() {
        let mut s = state();
        s.settings.refresh_minutes = 15;
        s.last_retention = Some(crate::wire::testing::now());
        s.last_refresh = Some(crate::wire::testing::now());

        let soon = crate::wire::testing::now() + jiff::SignedDuration::from_secs(60);
        let effects = apply(&mut s, Change::Done(Done::Tick(soon)));
        assert!(
            !db_jobs(&effects)
                .iter()
                .any(|j| matches!(j, DbJob::Due { .. })),
            "a minute is not fifteen"
        );

        let later = crate::wire::testing::now() + jiff::SignedDuration::from_secs(16 * 60);
        let effects = apply(&mut s, Change::Done(Done::Tick(later)));
        assert!(db_jobs(&effects)
            .iter()
            .any(|j| matches!(j, DbJob::Due { .. })));
    }

    #[test]
    fn a_tick_sweeps_once_a_day_and_tops_the_extraction_queue_up() {
        let mut s = state();
        s.last_refresh = Some(crate::wire::testing::now());
        let effects = apply(
            &mut s,
            Change::Done(Done::Tick(crate::wire::testing::now())),
        );
        assert!(db_jobs(&effects)
            .iter()
            .any(|j| matches!(j, DbJob::Retain { .. })));
        assert!(
            db_jobs(&effects)
                .iter()
                .any(|j| matches!(j, DbJob::PendingExtracts { .. })),
            "this is what makes a job that did not fit its queue cost a delay \
             rather than an article"
        );
    }

    #[test]
    fn an_import_sets_the_report_and_refreshes_the_new_list() {
        let mut s = state();
        s.import_offer = Some(ImportOffer {
            urls: "/tmp/urls".into(),
            cache: None,
        });
        let effects = apply(
            &mut s,
            Change::Done(Done::Imported(ImportReport {
                added: 39,
                skipped: vec![("x".into(), "why".into())],
                ..ImportReport::default()
            })),
        );
        assert_eq!(s.last_import.as_ref().map(|r| r.added), Some(39));
        assert!(s.import_offer.is_none());
        assert!(db_jobs(&effects)
            .iter()
            .any(|j| matches!(j, DbJob::Due { .. })));
        assert!(effects.events.iter().any(|e| matches!(e, Event::Feeds)));
    }

    #[test]
    fn refusing_the_offer_writes_it_down() {
        let mut s = state();
        s.import_offer = Some(ImportOffer {
            urls: "/tmp/urls".into(),
            cache: None,
        });
        let effects = apply(&mut s, Change::Command(Command::DismissImportOffer));
        assert!(s.import_offer.is_none());
        assert!(
            matches!(db_jobs(&effects).as_slice(), [DbJob::DismissImportOffer]),
            "an offer that comes back after being refused is a bug, not a reminder"
        );
    }

    #[test]
    fn a_search_is_a_selection_like_any_other() {
        let mut s = state();
        apply(&mut s, Change::Command(Command::Search("lifetimes".into())));
        assert_eq!(s.selection, Selection::Search("lifetimes".into()));
    }

    #[test]
    fn opening_an_entry_externally_sends_a_video_to_the_player() {
        let mut s = state();
        let mut video = row(1, 1, "a video");
        video.kind = EntryKind::Video;
        video.url = Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ".into());
        deliver(&mut s, vec![video, row(2, 1, "a page")], 2, 0);

        let effects = apply(&mut s, Change::Command(Command::OpenExternal(EntryId(1))));
        assert!(matches!(
            net_jobs(&effects).as_slice(),
            [(NetJob::Open(Target::Video(_)), Lane::Urgent)]
        ));

        let effects = apply(&mut s, Change::Command(Command::OpenExternal(EntryId(2))));
        assert!(matches!(
            net_jobs(&effects).as_slice(),
            [(NetJob::Open(Target::Browser(_)), Lane::Urgent)]
        ));
    }

    #[test]
    fn a_setting_changes_the_running_core_and_nothing_on_disk() {
        let mut s = state();
        apply(
            &mut s,
            Change::Command(Command::SetSetting(Setting::RefreshMinutes(60))),
        );
        assert_eq!(s.settings.refresh_minutes, 60);
        let effects = apply(
            &mut s,
            Change::Command(Command::SetSetting(Setting::UnreadOnly(true))),
        );
        assert!(s.unread_only);
        assert!(matches!(
            db_jobs(&effects).as_slice(),
            [DbJob::LoadEntries {
                unread_only: true,
                ..
            }]
        ));
    }

    #[test]
    fn the_version_moves_exactly_when_something_was_said() {
        let mut s = state();
        let before = s.version;
        // A command with nothing to draw differently.
        apply(&mut s, Change::Command(Command::Shutdown));
        assert_eq!(s.version, before);
        apply(
            &mut s,
            Change::Command(Command::OpenFeed(Selection::Starred)),
        );
        assert_eq!(s.version, before + 1);
    }

    /// The rule the module doc states, kept the way STAR/FOLD keeps it: by
    /// grepping this file's own source. `apply` runs under the one write
    /// lock, and a query or a request in here would hold it for the length
    /// of a round trip.
    #[test]
    fn apply_does_no_io() {
        // NO-IO-HERE
        const FORBIDDEN: &[&str] = &[
            "std::fs::",  // NO-IO-HERE
            "rusqlite::", // NO-IO-HERE
            "ureq::",     // NO-IO-HERE
        ];
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")) // NO-IO-HERE
            .join("src/wire/state.rs"); // NO-IO-HERE
        let text = std::fs::read_to_string(&path).expect("state.rs is beside its test"); // NO-IO-HERE
        for (n, line) in text.lines().enumerate() {
            if line.contains("NO-IO-HERE") {
                continue;
            }
            for needle in FORBIDDEN {
                assert!(
                    !line.contains(needle),
                    "state.rs:{}: {} -- apply must never wait on anything",
                    n + 1,
                    line.trim()
                );
            }
        }
    }

    proptest! {
        /// Random commands against a loaded page. Nothing here may panic,
        /// the visible list may only ever hold indices into the rows it
        /// filters, and the progress bar may not overshoot.
        #[test]
        fn applying_random_commands_keeps_the_page_and_the_bar_honest(
            ops in proptest::collection::vec(0u8..18, 0..120)
        ) {
            let mut s = state();
            deliver(
                &mut s,
                vec![row(1, 1, "lifetimes"), row(2, 1, "borrow"), row(3, 2, "news")],
                3,
                0,
            );

            for op in ops {
                match op {
                    0 => { apply(&mut s, Change::Command(Command::OpenFeed(Selection::All))); }
                    1 => { apply(&mut s, Change::Command(Command::OpenFeed(Selection::Starred))); }
                    2 => { apply(&mut s, Change::Command(Command::OpenFeed(Selection::Feed(FeedId(1))))); }
                    3 => { apply(&mut s, Change::Command(Command::MoreEntries)); }
                    4 => { apply(&mut s, Change::Command(Command::ReloadEntries)); }
                    5 => { apply(&mut s, Change::Command(Command::SetUnreadOnly(true))); }
                    6 => { apply(&mut s, Change::Command(Command::SetUnreadOnly(false))); }
                    7 => { apply(&mut s, Change::Command(Command::Filter("l".into()))); }
                    8 => { apply(&mut s, Change::Command(Command::ClearFilter)); }
                    9 => { apply(&mut s, Change::Command(Command::OpenEntry(EntryId(2)))); }
                    10 => { apply(&mut s, Change::Command(Command::CloseEntry)); }
                    11 => { apply(&mut s, Change::Command(Command::SetRead { entries: vec![EntryId(1)], read: true })); }
                    12 => { apply(&mut s, Change::Command(Command::SetRead { entries: vec![EntryId(1)], read: false })); }
                    13 => { apply(&mut s, Change::Command(Command::Refresh(RefreshScope::All))); }
                    14 => { apply(&mut s, Change::Command(Command::CancelRefresh)); }
                    15 => {
                        let generation = s.refresh_generation;
                        apply(&mut s, Change::Done(Done::Due {
                            scope: RefreshScope::All,
                            feeds: vec![due(1), due(2)],
                            generation,
                        }));
                    }
                    16 => {
                        let generation = s.refresh_generation;
                        apply(&mut s, Change::Done(Done::Fetched {
                            feed: FeedId(1),
                            kind: FeedKind::Web,
                            outcome: super::super::fetch::FetchOutcome::NotModified,
                            millis: 0,
                            generation,
                        }));
                    }
                    _ => {
                        let offset = s.page.rows.len();
                        deliver(&mut s, vec![row(4, 1, "later")], 4, offset);
                    }
                }

                for &index in &s.page.visible {
                    prop_assert!(index < s.page.rows.len());
                }
                prop_assert!(s.page.visible.len() <= s.page.rows.len());
                prop_assert!(s.refresh.done <= s.refresh.total);
                // An article on screen is always the one the reader has
                // open: a text loaded for an entry since left is dropped.
                if let Some(article) = &s.article {
                    prop_assert_eq!(Some(article.entry), s.open_entry);
                }
            }
        }
    }
}
