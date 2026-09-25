//! The threads: one `starwire-db` and a pool of `starwire-net`.
//!
//! Everything that touches SQLite or a socket happens on one of these, and
//! nothing else does. The database thread owns the one `rusqlite::Connection`
//! -- the single-writer rule `AGENTS.md` describes, kept by there being one
//! `Db` rather than by a mutex -- and the net threads fetch feeds, pull
//! pages, resolve channels and hand links to the browser or the player.
//!
//! Both kinds fold their results into [`State`] through
//! [`super::state::apply`], taking the write lock only for the fold itself
//! and never while the query or the request is in progress. That is what
//! keeps a slow server from stalling the list being drawn beside it.
//!
//! **Two lanes out to the network.** A net thread takes an urgent job ahead
//! of a background one: the feed on screen, an article somebody is waiting
//! to read, a channel just typed into the add box. A refresh of forty-one
//! feeds fills the background lane, and the reader never waits behind it.
//!
//! **A net thread is never asleep for long.** Politeness is a gap between
//! two requests to one host, and some hosts want a minute of it. A thread
//! that slept through one of those was a quarter of the pool gone, so a
//! wait over `net::MAX_PARK` comes back as [`Done::Deferred`] instead: the
//! job goes into `State::deferred` and the clock dispatches it when its time
//! comes. Nothing is asked any sooner than it would have been.
//!
//! **Every fetch and every extraction carries a generation**, stamped from
//! `State::refresh_generation` when the job was made and checked again when
//! it is picked up. `CancelRefresh` bumps the generation, and what is left
//! in the queues is dropped before a connection is opened rather than after
//! the answer comes back.
//!
//! [`perform_db`], [`perform_net`] and [`finish`] are public so that the
//! window's fixture can run the very same code inline, on the test thread,
//! with no channel and no thread between a job and its result.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use jiff::Timestamp;

use super::db::schema::ChannelSource;
use super::db::{articles, entries, feeds, youtube as db_youtube, Db};
use super::extract::{ArticleResult, Limits};
use super::feed::{
    ArticleStatus, ArticleView, EntryId, FeedId, FeedKind, FeedRow, Folder, FolderId, ParsedFeed,
    Selection,
};
use super::fetch::FetchOutcome;
use super::handle::{EventSink, Note, RefreshScope};
use super::import::{self, ImportReport};
use super::net::Http;
use super::open::Target;
use super::state::{self, Change, State};
use super::youtube::ChannelId;
use super::{db, open, WireConfig};

/// How often the database thread wakes with nothing to do: to ask
/// `PRAGMA data_version` whether another process has written, and to hand
/// `apply` a clock for the periodic refresh and the daily sweep.
pub const TICK: Duration = Duration::from_secs(1);

/// Which queue a net job joins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Somebody is waiting for this one.
    Urgent,
    /// The refresh, and the extraction queue behind it.
    Background,
}

/// Work for the database thread. Every SQLite write in the running program
/// is one of these.
#[derive(Debug)]
pub enum DbJob {
    LoadFeeds,
    LoadEntries {
        sel: Selection,
        unread_only: bool,
        offset: usize,
        limit: usize,
    },
    LoadArticle(EntryId),
    /// Which feeds a refresh should fetch. The conditional headers and the
    /// backoff live in the row, so this is the only place that can answer.
    Due {
        scope: RefreshScope,
        generation: u64,
    },
    /// Offer every failed or unextracted entry in `scope` again, at once.
    /// What a refresh somebody asked for adds to re-fetching the feed; the
    /// clock's own refresh never sends one.
    ReofferExtracts {
        scope: RefreshScope,
        generation: u64,
    },
    /// Stamp `meta.last_refresh` and sweep the fetch log.
    RecordRefresh,
    Store {
        feed: FeedId,
        kind: FeedKind,
        /// Boxed: a parsed feed is the largest thing in this enum by an
        /// order of magnitude, and every other job would otherwise be that
        /// size in the channel.
        parsed: Box<ParsedFeed>,
        conditional: feeds::Conditional,
        bytes: usize,
        millis: u128,
    },
    RecordUnchanged {
        feed: FeedId,
        millis: u128,
    },
    RecordFailure {
        feed: FeedId,
        error: String,
        status: Option<u16>,
        retry_after: Option<i64>,
        refresh_minutes: u32,
        millis: u128,
    },
    StoreArticle {
        entry: EntryId,
        result: Box<ArticleResult>,
    },
    SetRead {
        entries: Vec<EntryId>,
        read: bool,
    },
    MarkAllRead(Selection),
    SetFlag {
        entry: EntryId,
        starred: bool,
    },
    AddFeed {
        url: String,
        title: Option<String>,
        folder: Option<String>,
    },
    RemoveFeed(FeedId),
    RenameFeed {
        feed: FeedId,
        title: Option<String>,
    },
    SetFeedFolder {
        feed: FeedId,
        folder: Option<String>,
    },
    RenameFolder {
        folder: FolderId,
        name: String,
    },
    RemoveFolder(FolderId),
    AddChannels {
        channels: Vec<(ChannelId, Option<String>)>,
        source: ChannelSource,
    },
    ImportTakeout(PathBuf),
    ImportNewsboat {
        urls: PathBuf,
        cache: Option<PathBuf>,
    },
    ImportOpml(PathBuf),
    ExportOpml(PathBuf),
    DismissImportOffer,
    /// Top the extraction queue up from what is still pending.
    PendingExtracts {
        limit: usize,
    },
    Retain {
        keep_days: u32,
        max_per_feed: usize,
    },
}

/// Work for a net thread.
#[derive(Debug)]
pub enum NetJob {
    /// An unfinished job and the HTTP hops it already completed.
    Resume {
        job: Box<NetJob>,
        history: super::net::RequestHistory,
    },
    Fetch {
        feed: FeedId,
        url: String,
        kind: FeedKind,
        conditional: feeds::Conditional,
        max_bytes: u64,
        generation: u64,
    },
    Extract {
        entry: EntryId,
        url: String,
        limits: Limits,
        generation: u64,
    },
    ResolveChannel {
        input: String,
    },
    YtSubs {
        cookies_from_browser: Option<String>,
    },
    /// One picture from inside an article: the cached file if there is one,
    /// a request if there is not, and a decode either way. On the urgent
    /// lane -- a picture is only asked for when it is on the screen -- and
    /// behind a generation of its own so that closing the article drops what
    /// has not started.
    Picture {
        url: String,
        max_w: u32,
        max_h: u32,
        generation: u64,
    },
    /// Take the picture cache back under its ceiling. Chained off the daily
    /// retention sweep, and on a net thread rather than the database one
    /// because it is a directory walk and the database thread is the only
    /// thread that writes.
    SweepPictures,
    /// Hand a link to the browser or the player. Net work only in the sense
    /// that it leaves the program; it is here because it must not block the
    /// one thread that writes.
    Open(Target),
}

/// Work, routed by its kind.
#[derive(Debug)]
pub enum Job {
    Db(DbJob),
    Net(NetJob, Lane),
    Shutdown,
}

/// What a worker reports back, to be folded into [`State`] through
/// [`super::state::apply`].
///
/// A result carries its data rather than naming what changed, because
/// `apply` is the only thing allowed to write to `State`: a worker that
/// inserted its own page and then sent a notification would be a second
/// writer.
#[derive(Debug)]
pub enum Done {
    Feeds {
        feeds: Vec<FeedRow>,
        folders: Vec<Folder>,
    },
    Entries {
        sel: Selection,
        unread_only: bool,
        offset: usize,
        page: entries::Page,
    },
    Article {
        entry: EntryId,
        view: Option<Box<ArticleView>>,
    },
    Due {
        scope: RefreshScope,
        feeds: Vec<feeds::DueFeed>,
        generation: u64,
    },
    /// How many entries a re-offer marked, so the refresh can start the
    /// queue and say what it is about to try.
    Reoffered {
        scope: RefreshScope,
        count: i64,
        generation: u64,
    },
    RefreshStamped(Timestamp),
    Fetched {
        feed: FeedId,
        kind: FeedKind,
        outcome: FetchOutcome,
        millis: u128,
        generation: u64,
    },
    Stored {
        feed: FeedId,
        new: usize,
        extractable: Vec<(EntryId, String)>,
    },
    Extracted {
        entry: EntryId,
        result: Box<ArticleResult>,
        generation: u64,
    },
    ArticleStored {
        entry: EntryId,
        status: ArticleStatus,
    },
    FeedAdded {
        feed: FeedId,
        url: String,
        kind: FeedKind,
        is_new: bool,
    },
    MarkedRead(usize),
    Imported(ImportReport),
    Resolved {
        channels: Vec<(ChannelId, Option<String>)>,
        source: ChannelSource,
    },
    Pending(Vec<(EntryId, String)>),
    Retained {
        retained: entries::Retained,
        at: Timestamp,
    },
    Picture {
        url: String,
        /// The pixels, or a sentence saying why there are none. Everything
        /// that can fail here ends up in the same place -- the line drawn
        /// where the picture would have been -- so there is nothing for a
        /// type to tell apart.
        result: Result<Arc<super::pictures::Picture>, String>,
        generation: u64,
    },
    /// A job that did not run because a host it needed asked for longer
    /// than a net thread should be asleep for. Not a failure and not an
    /// answer: the work itself, handed back for `State` to hold until
    /// `until` and then dispatch again. See `net::MAX_PARK`.
    Deferred {
        job: NetJob,
        lane: Lane,
        until: Instant,
    },
    /// Another process committed to the file -- a `starwire fetch` from a
    /// timer while the window was open.
    External,
    Tick(Timestamp),
    /// Something to say, from a worker that has no other result. A failure
    /// `apply` could not have known about.
    Note(Note),
}

/// Where every job goes.
///
/// Three senders rather than one: the database's queue, and the two lanes
/// the net pool reads. A thread is given clones of all three and routes a
/// job by its kind, the same way [`super::handle::Handle::send`] does --
/// folding a result in can imply work for either side, and a `Done::Stored`
/// handled on the database thread produces extraction jobs for the net pool.
#[derive(Clone)]
pub struct Senders {
    pub db: crossbeam_channel::Sender<Job>,
    pub urgent: crossbeam_channel::Sender<Job>,
    pub background: crossbeam_channel::Sender<Job>,
}

impl Senders {
    /// Send `job` to whichever queue owns its kind of work.
    ///
    /// `try_send`, and a warning rather than a wait, when a queue is full: a
    /// worker folding a result in holds nothing, but the window's `send`
    /// would otherwise block the key that produced it. A dropped job is
    /// picked up by the next tick -- which is why the tick asks for pending
    /// extractions and why a refresh is re-issued rather than remembered.
    pub fn dispatch(&self, job: Job) {
        let sender = match &job {
            Job::Db(_) => &self.db,
            Job::Net(_, Lane::Urgent) => &self.urgent,
            Job::Net(_, Lane::Background) => &self.background,
            Job::Shutdown => &self.db,
        };
        if let Err(crossbeam_channel::TrySendError::Full(job)) = sender.try_send(job) {
            tracing::warn!(?job, "a job did not fit its queue and was dropped");
        }
    }
}

/// Fold one [`Done`] into `state` and dispatch whatever it implies, taking
/// the write lock only for the fold itself.
///
/// A free function rather than a closure each thread builds for itself, so
/// that a fixture folding a result on the test thread with no worker behind
/// it goes through exactly the same code the threads do.
pub fn finish(done: Done, state: &Arc<RwLock<State>>, events: &EventSink, senders: &Senders) {
    let effects = {
        let mut s = state.write().unwrap_or_else(|e| e.into_inner());
        state::apply(&mut s, Change::Done(done))
    };
    for job in effects.jobs {
        senders.dispatch(job);
    }
    for event in effects.events {
        events.send(event);
    }
}

/// Whether a job was made before the last `CancelRefresh`.
///
/// A named function rather than a comparison inline in two loops: "behind",
/// not "different from", is the whole of the rule, and it is worth a test
/// that does not also need a server.
pub fn is_stale(state: &Arc<RwLock<State>>, generation: u64) -> bool {
    let s = state.read().unwrap_or_else(|e| e.into_inner());
    generation < s.refresh_generation
}

/// The same question for a picture, which counts on its own generation.
///
/// Separate because the two are cancelled by different things: a refresh is
/// cancelled by `CancelRefresh`, and a picture by the reader opening
/// something else -- which happens with every `n`, and must not drop a
/// refresh of forty-one feeds with it.
pub fn is_stale_picture(state: &Arc<RwLock<State>>, generation: u64) -> bool {
    let s = state.read().unwrap_or_else(|e| e.into_inner());
    generation < s.picture_generation
}

fn failed(what: &'static str, e: impl std::fmt::Display) -> Vec<Done> {
    tracing::warn!("{what}: {e}");
    vec![Done::Note(Note::error(what, format!("{what}: {e}")))]
}

/// Run one database job.
///
/// Returns what to fold in rather than folding it: the caller takes the
/// write lock, and a function that both queried and wrote to `State` would
/// hold two locks at once for no reason. A failure comes back as a
/// [`Done::Note`] -- a feed list that cannot be read is a line in the status
/// bar, not a reason to stop the program.
pub fn perform_db(job: DbJob, db: &mut Db) -> Vec<Done> {
    match job {
        DbJob::LoadFeeds => match load_feeds(db) {
            Ok(done) => vec![done],
            Err(e) => failed("feeds", e),
        },
        DbJob::LoadEntries {
            sel,
            unread_only,
            offset,
            limit,
        } => match entries::page(db, &sel, unread_only, offset, limit) {
            Ok(page) => vec![Done::Entries {
                sel,
                unread_only,
                offset,
                page,
            }],
            Err(e) => failed("entries", e),
        },
        DbJob::LoadArticle(entry) => match articles::get(db, entry) {
            Ok(view) => vec![Done::Article {
                entry,
                view: view.map(Box::new),
            }],
            Err(e) => failed("article", e),
        },
        DbJob::Due { scope, generation } => {
            // A refresh somebody asked for by name ignores the backoff: the
            // answer to "this feed is failing" is often to try it again, and
            // a person pressing `r` has said so.
            let ignore_backoff = !matches!(scope, RefreshScope::All);
            match feeds::due(db, ignore_backoff) {
                Ok(feeds) => vec![Done::Due {
                    scope,
                    feeds,
                    generation,
                }],
                Err(e) => failed("refresh", e),
            }
        }
        DbJob::ReofferExtracts { scope, generation } => {
            // The window's scope, translated here rather than carried into
            // the database: nothing under `db/` knows what a `RefreshScope`
            // is, the same way nothing there knows what a key press is.
            let which = match scope {
                RefreshScope::All => articles::Scope::Everything,
                RefreshScope::Feed(id) => articles::Scope::Feed(id),
                RefreshScope::Folder(id) => articles::Scope::Folder(id),
            };
            match articles::reoffer(db, which) {
                Ok(count) => vec![Done::Reoffered {
                    scope,
                    count,
                    generation,
                }],
                Err(e) => failed("extract", e),
            }
        }
        DbJob::RecordRefresh => {
            let at = Timestamp::now();
            let result = db
                .set_meta(db::schema::meta::LAST_REFRESH, &at.as_second().to_string())
                .and_then(|()| feeds::sweep_fetch_log(db).map(|_| ()));
            match result {
                Ok(()) => vec![Done::RefreshStamped(at)],
                Err(e) => failed("refresh", e),
            }
        }
        DbJob::Store {
            feed,
            kind,
            parsed,
            conditional,
            bytes,
            millis,
        } => match store(db, feed, kind, &parsed, &conditional, bytes, millis) {
            Ok(done) => done,
            Err(e) => failed("store", e),
        },
        DbJob::RecordUnchanged { feed, millis } => {
            let result = feeds::record_ok(db, feed, &feeds::Conditional::default(), None, None)
                .and_then(|()| {
                    feeds::log_fetch(db, feed, Some(304), Some(0), Some(0), millis, None)
                });
            match result {
                Ok(()) => Vec::new(),
                Err(e) => failed("store", e),
            }
        }
        DbJob::RecordFailure {
            feed,
            error,
            status,
            retry_after,
            refresh_minutes,
            millis,
        } => {
            let result = feeds::record_failure(db, feed, &error, refresh_minutes, retry_after)
                .and_then(|_| feeds::log_fetch(db, feed, status, None, None, millis, Some(&error)));
            match result {
                Ok(()) => vec![
                    Done::Note(Note::warning("fetch", error)),
                    // The row's error column and its backoff both changed.
                    load_feeds(db).unwrap_or(Done::Feeds {
                        feeds: Vec::new(),
                        folders: Vec::new(),
                    }),
                ],
                Err(e) => failed("store", e),
            }
        }
        DbJob::StoreArticle { entry, result } => match articles::put(db, entry, &result) {
            Ok(()) => vec![Done::ArticleStored {
                entry,
                status: result.status,
            }],
            Err(e) => failed("store", e),
        },
        DbJob::SetRead { entries: ids, read } => match entries::set_read(db, &ids, read) {
            Ok(_) => Vec::new(),
            Err(e) => failed("read", e),
        },
        DbJob::MarkAllRead(sel) => match entries::mark_all_read(db, &sel) {
            Ok(n) => vec![Done::MarkedRead(n)],
            Err(e) => failed("read", e),
        },
        DbJob::SetFlag { entry, starred } => match entries::set_starred(db, entry, starred) {
            Ok(()) => Vec::new(),
            Err(e) => failed("star", e),
        },
        DbJob::AddFeed { url, title, folder } => match add_feed(db, &url, title, folder) {
            Ok(done) => done,
            Err(e) => failed("add", e),
        },
        DbJob::RemoveFeed(feed) => match feeds::remove(db, feed) {
            Ok(_) => with_feeds(db, Note::info("feed removed")),
            Err(e) => failed("remove", e),
        },
        DbJob::RenameFeed { feed, title } => match feeds::rename(db, feed, title.as_deref()) {
            Ok(()) => reloaded_feeds(db),
            Err(e) => failed("rename", e),
        },
        DbJob::SetFeedFolder { feed, folder } => match set_feed_folder(db, feed, folder) {
            Ok(()) => reloaded_feeds(db),
            Err(e) => failed("folder", e),
        },
        DbJob::RenameFolder { folder, name } => match feeds::rename_folder(db, folder, &name) {
            Ok(()) => reloaded_feeds(db),
            Err(e) => failed("folder", e),
        },
        DbJob::RemoveFolder(folder) => match feeds::remove_folder(db, folder) {
            Ok(()) => reloaded_feeds(db),
            Err(e) => failed("folder", e),
        },
        DbJob::AddChannels { channels, source } => match add_channels(db, &channels, source) {
            Ok(done) => done,
            Err(e) => failed("youtube", e),
        },
        DbJob::ImportTakeout(path) => match import_takeout(db, &path) {
            Ok(done) => done,
            Err(e) => failed("import", e),
        },
        DbJob::ImportNewsboat { urls, cache } => {
            match import_newsboat(db, &urls, cache.as_deref()) {
                Ok(done) => done,
                Err(e) => failed("import", e),
            }
        }
        DbJob::ImportOpml(path) => match import_opml(db, &path) {
            Ok(done) => done,
            Err(e) => failed("import", e),
        },
        DbJob::ExportOpml(path) => match export_opml(db, &path) {
            Ok(note) => vec![Done::Note(note)],
            Err(e) => failed("export", e),
        },
        DbJob::DismissImportOffer => {
            match db.set_meta(db::schema::meta::NEWSBOAT_OFFER_DECLINED, "1") {
                Ok(()) => Vec::new(),
                Err(e) => failed("import", e),
            }
        }
        DbJob::PendingExtracts { limit } => match articles::pending(db, limit) {
            Ok(rows) => vec![Done::Pending(rows)],
            Err(e) => failed("extract", e),
        },
        DbJob::Retain {
            keep_days,
            max_per_feed,
        } => {
            let at = Timestamp::now();
            match entries::retain(db, keep_days, max_per_feed) {
                Ok(retained) => {
                    let _ = db.set_meta(
                        db::schema::meta::LAST_RETENTION,
                        &at.as_second().to_string(),
                    );
                    vec![Done::Retained { retained, at }]
                }
                Err(e) => failed("retention", e),
            }
        }
    }
}

fn load_feeds(db: &Db) -> anyhow::Result<Done> {
    Ok(Done::Feeds {
        feeds: feeds::list_feeds_with_unread(db)?,
        folders: feeds::folders(db)?,
    })
}

fn reloaded_feeds(db: &Db) -> Vec<Done> {
    match load_feeds(db) {
        Ok(done) => vec![done],
        Err(e) => failed("feeds", e),
    }
}

fn with_feeds(db: &Db, note: Note) -> Vec<Done> {
    let mut out = reloaded_feeds(db);
    out.push(Done::Note(note));
    out
}

fn store(
    db: &mut Db,
    feed: FeedId,
    kind: FeedKind,
    parsed: &ParsedFeed,
    conditional: &feeds::Conditional,
    bytes: usize,
    millis: u128,
) -> anyhow::Result<Vec<Done>> {
    feeds::record_ok(
        db,
        feed,
        conditional,
        parsed.title.as_deref(),
        parsed.site_url.as_deref(),
    )?;
    let stored = entries::upsert_parsed(db, feed, kind, parsed)?;
    feeds::log_fetch(
        db,
        feed,
        Some(200),
        Some(bytes),
        Some(stored.new.len()),
        millis,
        None,
    )?;
    Ok(vec![Done::Stored {
        feed,
        new: stored.new.len(),
        extractable: stored.extractable,
    }])
}

fn add_feed(
    db: &mut Db,
    url: &str,
    title: Option<String>,
    folder: Option<String>,
) -> anyhow::Result<Vec<Done>> {
    // Every URL entering the program goes through here: a bare host gets a
    // scheme, `http` becomes `https`, and a YouTube channel becomes its
    // `videos.xml` feed. See `youtube::canonicalise`.
    let canonical = super::youtube::canonicalise(url)?;
    let folder_id = match folder.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        Some(name) => Some(feeds::folder_named(db, name)?),
        None => None,
    };
    let title = title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| canonical.title.clone());
    let added = feeds::add(
        db,
        &canonical.url,
        (canonical.url != url).then_some(url),
        canonical.kind,
        title.as_deref(),
        folder_id,
    )?;
    if let Some(channel) = &canonical.channel {
        db_youtube::upsert(
            &db.conn,
            channel,
            title.as_deref(),
            None,
            Some(added.id()),
            ChannelSource::Added,
        )?;
    }
    Ok(vec![Done::FeedAdded {
        feed: added.id(),
        url: canonical.url,
        kind: canonical.kind,
        is_new: added.is_new(),
    }])
}

fn set_feed_folder(db: &mut Db, feed: FeedId, folder: Option<String>) -> anyhow::Result<()> {
    let id = match folder.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        Some(name) => Some(feeds::folder_named(db, name)?),
        None => None,
    };
    feeds::set_folder(db, feed, id)
}

fn add_channels(
    db: &mut Db,
    channels: &[(ChannelId, Option<String>)],
    source: ChannelSource,
) -> anyhow::Result<Vec<Done>> {
    let mut report = ImportReport::default();
    for (channel, title) in channels {
        let canonical = super::youtube::canonicalise(&channel.feed_url())?;
        let added = feeds::add(
            db,
            &canonical.url,
            None,
            FeedKind::Youtube,
            title.as_deref(),
            None,
        )?;
        if added.is_new() {
            report.added += 1;
            report.videos += 1;
        } else {
            report.already_there += 1;
        }
        db_youtube::upsert(
            &db.conn,
            channel,
            title.as_deref(),
            None,
            Some(added.id()),
            source,
        )?;
    }
    Ok(vec![Done::Imported(report)])
}

fn import_takeout(db: &mut Db, path: &std::path::Path) -> anyhow::Result<Vec<Done>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let channels = super::youtube::parse_takeout(&text)?;
    add_channels(db, &channels, ChannelSource::Takeout)
}

fn import_newsboat(
    db: &mut Db,
    urls: &std::path::Path,
    cache: Option<&std::path::Path>,
) -> anyhow::Result<Vec<Done>> {
    let text = std::fs::read_to_string(urls)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", urls.display()))?;
    // A cache that cannot be read costs the titles and the read marks, not
    // the import: the urls file is the subscription list, and the cache is
    // what newsboat learned about it.
    let rows = match cache {
        Some(path) => match super::import::newsboat::read_cache(path) {
            Ok(rows) => Some(rows),
            Err(e) => {
                tracing::warn!("reading {}: {e}", path.display());
                None
            }
        },
        None => None,
    };
    let plan = import::plan_newsboat(&text, rows.as_ref());
    let report = import::apply_plan(db, &plan)?;
    db.set_meta(
        db::schema::meta::NEWSBOAT_IMPORTED_AT,
        &db::now().to_string(),
    )?;
    Ok(vec![Done::Imported(report)])
}

fn import_opml(db: &mut Db, path: &std::path::Path) -> anyhow::Result<Vec<Done>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let plan = import::opml::parse(&text)?;
    let report = import::apply_plan(db, &plan)?;
    Ok(vec![Done::Imported(report)])
}

fn export_opml(db: &Db, path: &std::path::Path) -> anyhow::Result<Note> {
    let rows = feeds::list_feeds_with_unread(db)?;
    let folders = feeds::folders(db)?;
    let text = import::opml::export(&rows, &folders)?;
    starkit::fs::write_atomic(path, text.as_bytes())
        .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
    Ok(Note::info(format!(
        "{} feed{} written to {}",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" },
        path.display()
    )))
}

/// Run one net job, and defer it instead where a host said to come back.
///
/// The deferral is read off the thread rather than out of the result --
/// `net::take_deferral`, which explains why -- and it wins over whatever
/// [`run_net`] made of the requests it did get to make. So a feed whose host
/// is not askable yet is not recorded as a failure, an entry does not spend
/// an attempt, and a picture stays `Loading` rather than turning into a line
/// of text: `lane` is where the job goes back, and the clock sends it.
pub fn perform_net(
    job: NetJob,
    lane: Lane,
    http: &dyn Http,
    cfg: &WireConfig,
    state: &Arc<RwLock<State>>,
) -> Vec<Done> {
    let (job, history) = match job {
        NetJob::Resume { job, history } => (*job, history),
        job => (job, super::net::RequestHistory::default()),
    };
    let requests = super::net::RequestScope::new(history);
    super::net::clear_deferral();
    let done = run_net(&job, http, cfg, state);
    let history = requests.finish();
    match super::net::take_deferral() {
        Some(until) => {
            tracing::debug!(?job, ?lane, "deferred: the host is not askable yet");
            vec![Done::Deferred {
                job: NetJob::Resume {
                    job: Box::new(job),
                    history,
                },
                lane,
                until,
            }]
        }
        None => done,
    }
}

/// The work itself.
///
/// `state` is read for one thing only: whether the job is stale. Checking it
/// here rather than in the loop is what makes "dropped before a connection
/// is opened" true of an extraction as well as of a fetch.
///
/// Borrows the job rather than taking it, because [`perform_net`] may have
/// to hand it back whole.
fn run_net(
    job: &NetJob,
    http: &dyn Http,
    cfg: &WireConfig,
    state: &Arc<RwLock<State>>,
) -> Vec<Done> {
    match job {
        NetJob::Resume { job, .. } => run_net(job, http, cfg, state),
        NetJob::Fetch {
            feed,
            url,
            kind,
            conditional,
            max_bytes,
            generation,
        } => {
            if is_stale(state, *generation) {
                return Vec::new();
            }
            let started = Instant::now();
            let outcome = match super::fetch::fetch(http, url, conditional, *max_bytes) {
                Ok(outcome) => outcome,
                // The one `Err` fetching has is a URL that will not parse,
                // which is a permanent fact about the row rather than
                // something the network did.
                Err(e) => FetchOutcome::Failed {
                    error: e.to_string(),
                    status: None,
                    retry_after: None,
                },
            };
            vec![Done::Fetched {
                feed: *feed,
                kind: *kind,
                outcome,
                millis: started.elapsed().as_millis(),
                generation: *generation,
            }]
        }
        NetJob::Extract {
            entry,
            url,
            limits,
            generation,
        } => {
            if is_stale(state, *generation) {
                return Vec::new();
            }
            match super::extract::run(http, url, *limits) {
                Ok(result) => vec![Done::Extracted {
                    entry: *entry,
                    result: Box::new(result),
                    generation: *generation,
                }],
                Err(e) => failed("extract", e),
            }
        }
        NetJob::ResolveChannel { input } => match resolve_channel(http, input, &cfg.youtube) {
            Ok(done) => done,
            Err(e) => failed("youtube", e),
        },
        NetJob::YtSubs {
            cookies_from_browser,
        } => {
            let mut youtube = cfg.youtube.clone();
            if let Some(browser) = cookies_from_browser {
                youtube.cookies_from_browser = browser.clone();
            }
            match super::youtube::yt_subscriptions(&youtube, 200) {
                Ok(channels) => vec![Done::Resolved {
                    channels,
                    source: ChannelSource::YtSubs,
                }],
                Err(e) => failed("youtube", e),
            }
        }
        NetJob::Picture {
            url,
            max_w,
            max_h,
            generation,
        } => {
            if is_stale_picture(state, *generation) {
                // The answer is thrown away and the *slot* is not. The
                // ceiling in `pump_pictures` counts what is in flight, so a
                // job that ends without a `Done` is one picture's worth of
                // room nothing ever gives back -- and two of those, at
                // `pictures::PARALLEL`, is a reader whose pictures stop
                // arriving for the rest of the session, which is what
                // holding `n` down through an article with pictures in it
                // used to do. `done_picture` frees the slot and keeps
                // nothing.
                return vec![Done::Picture {
                    url: url.clone(),
                    result: Err("the reader moved on".to_string()),
                    generation: *generation,
                }];
            }
            let result =
                super::pictures::fetch(http, cfg.pictures_dir.as_deref(), url, *max_w, *max_h)
                    .map(Arc::new);
            if let Err(e) = &result {
                tracing::debug!("picture {url}: {e}");
            }
            vec![Done::Picture {
                url: url.clone(),
                result,
                generation: *generation,
            }]
        }
        NetJob::SweepPictures => {
            if let Some(dir) = cfg.pictures_dir.as_deref() {
                super::pictures::sweep(
                    dir,
                    cfg.articles.pictures_mib.saturating_mul(1024 * 1024),
                    cfg.articles.keep_days,
                    std::time::SystemTime::now(),
                );
            }
            Vec::new()
        }
        NetJob::Open(target) => match open::open(target, &cfg.player) {
            Ok(()) => Vec::new(),
            Err(e) => failed("open", e),
        },
    }
}

fn resolve_channel(
    http: &dyn Http,
    input: &str,
    cfg: &super::YoutubeConfig,
) -> anyhow::Result<Vec<Done>> {
    let detected = super::youtube::detect(input)
        .ok_or_else(|| anyhow::anyhow!("{input} is not a channel, a handle or a video"))?;
    let resolved = super::youtube::resolve(http, &detected, cfg)?;
    Ok(vec![Done::Resolved {
        channels: vec![(resolved.channel, resolved.title)],
        source: ChannelSource::Added,
    }])
}

/// Start the database thread: every query and every write, and the clock.
///
/// The connection is opened by `Handle::spawn` and moved in here, so that a
/// file that cannot be opened is an error the window can print rather than a
/// thread that dies quietly.
pub fn spawn_db(
    jobs: crossbeam_channel::Receiver<Job>,
    events: EventSink,
    state: Arc<RwLock<State>>,
    senders: Senders,
    mut db: Db,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("starwire-db".into())
        .spawn(move || {
            let mut data_version = db.data_version().unwrap_or(0);
            loop {
                let job = match jobs.recv_timeout(TICK) {
                    Ok(job) => job,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        // Another process committed -- a `starwire fetch`
                        // from a timer. `PRAGMA data_version` is the one
                        // cheap way to notice through WAL, where the file's
                        // mtime does not move.
                        if let Ok(version) = db.data_version() {
                            if version != data_version {
                                data_version = version;
                                finish(Done::External, &state, &events, &senders);
                            }
                        }
                        finish(Done::Tick(Timestamp::now()), &state, &events, &senders);
                        continue;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                };

                match job {
                    Job::Shutdown => break,
                    Job::Db(job) => {
                        for done in perform_db(job, &mut db) {
                            finish(done, &state, &events, &senders);
                        }
                        data_version = db.data_version().unwrap_or(data_version);
                    }
                    // Routed here by mistake rather than dropped: `dispatch`
                    // sends net work to a lane, and a job arriving on the
                    // wrong queue is a bug worth surviving.
                    other => senders.dispatch(other),
                }
            }
        })
        .expect("spawning the database thread")
}

/// What every net thread shares: one client and its connection pool, one
/// state, one sink, and the flag that stops them all.
#[derive(Clone)]
pub struct NetPool {
    pub events: EventSink,
    pub state: Arc<RwLock<State>>,
    pub cfg: WireConfig,
    pub senders: Senders,
    /// One `Http` for the pool, not one per thread: the agent underneath
    /// holds the connections, and the per-host lease that keeps requests to
    /// one server serial is inside it.
    pub http: Arc<dyn Http>,
    /// Checked between jobs, so a shutdown does not wait for a queue to
    /// drain.
    pub cancel: Arc<AtomicBool>,
}

/// Start one net thread. `index` is only its name.
pub fn spawn_net(
    index: usize,
    urgent: crossbeam_channel::Receiver<Job>,
    background: crossbeam_channel::Receiver<Job>,
    pool: NetPool,
) -> std::thread::JoinHandle<()> {
    let NetPool {
        events,
        state,
        cfg,
        senders,
        http,
        cancel,
    } = pool;
    std::thread::Builder::new()
        .name(format!("starwire-net-{index}"))
        .spawn(move || loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Some(job) = next_job(&urgent, &background) else {
                break;
            };
            match job {
                Job::Shutdown => break,
                Job::Net(job, lane) => {
                    for done in perform_net(job, lane, http.as_ref(), &cfg, &state) {
                        finish(done, &state, &events, &senders);
                    }
                }
                other => senders.dispatch(other),
            }
        })
        .expect("spawning a net thread")
}

/// One job, urgent lane first.
///
/// The `try_recv` before the `select!` is the whole of the priority: `select!`
/// picks at random between two ready channels, so asking the urgent lane
/// first is what makes an article somebody is waiting for jump a refresh of
/// forty-one feeds. `None` means both lanes are gone and the thread is done.
fn next_job(
    urgent: &crossbeam_channel::Receiver<Job>,
    background: &crossbeam_channel::Receiver<Job>,
) -> Option<Job> {
    match urgent.try_recv() {
        Ok(job) => return Some(job),
        Err(crossbeam_channel::TryRecvError::Empty) => {}
        Err(crossbeam_channel::TryRecvError::Disconnected) => {
            return background.recv().ok();
        }
    }
    crossbeam_channel::select! {
        recv(urgent) -> job => job.ok(),
        recv(background) -> job => job.ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::feed::Selection;
    use crate::wire::handle::Event;
    use crate::wire::testing::{self, Fixture};
    use std::sync::atomic::AtomicU64;

    fn channels() -> (
        Senders,
        crossbeam_channel::Receiver<Job>,
        crossbeam_channel::Receiver<Job>,
        crossbeam_channel::Receiver<Job>,
    ) {
        let (db, db_rx) = crossbeam_channel::unbounded();
        let (urgent, urgent_rx) = crossbeam_channel::unbounded();
        let (background, background_rx) = crossbeam_channel::unbounded();
        (
            Senders {
                db,
                urgent,
                background,
            },
            db_rx,
            urgent_rx,
            background_rx,
        )
    }

    fn sink() -> (EventSink, crossbeam_channel::Receiver<Event>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (EventSink::new(tx, Arc::new(AtomicU64::new(0))), rx)
    }

    fn state_of(cfg: &WireConfig) -> Arc<RwLock<State>> {
        Arc::new(RwLock::new(State::new(cfg)))
    }

    #[test]
    fn the_tick_is_one_second() {
        assert_eq!(TICK, Duration::from_secs(1));
    }

    #[test]
    fn a_job_routes_to_the_queue_that_owns_its_kind() {
        let (senders, db, urgent, background) = channels();
        senders.dispatch(Job::Db(DbJob::LoadFeeds));
        senders.dispatch(Job::Net(
            NetJob::ResolveChannel { input: "x".into() },
            Lane::Urgent,
        ));
        senders.dispatch(Job::Net(
            NetJob::Open(Target::Browser("https://example.org".into())),
            Lane::Background,
        ));
        assert!(matches!(db.try_recv(), Ok(Job::Db(DbJob::LoadFeeds))));
        assert!(matches!(urgent.try_recv(), Ok(Job::Net(..))));
        assert!(matches!(background.try_recv(), Ok(Job::Net(..))));
    }

    #[test]
    fn a_job_that_does_not_fit_is_dropped_rather_than_waited_on() {
        let (db, db_rx) = crossbeam_channel::bounded(1);
        let (urgent, _urgent_rx) = crossbeam_channel::bounded(1);
        let (background, _background_rx) = crossbeam_channel::bounded(1);
        let senders = Senders {
            db,
            urgent,
            background,
        };
        senders.dispatch(Job::Db(DbJob::LoadFeeds));
        // The second one has nowhere to go. It must not block: the window's
        // own `send` runs through here, and a full queue would stop the key
        // that produced it.
        senders.dispatch(Job::Db(DbJob::LoadFeeds));
        assert_eq!(db_rx.len(), 1);
    }

    /// The whole of the priority: `select!` picks at random between two
    /// ready channels, so the urgent lane is asked first. A background
    /// refresh of forty-one feeds queued ahead of an article somebody is
    /// waiting for must not make them wait for it.
    #[test]
    fn the_urgent_lane_goes_first() {
        let (senders, _db, urgent, background) = channels();
        senders.dispatch(Job::Net(
            NetJob::Open(Target::Browser("background".into())),
            Lane::Background,
        ));
        senders.dispatch(Job::Net(
            NetJob::Open(Target::Browser("urgent".into())),
            Lane::Urgent,
        ));

        match next_job(&urgent, &background).expect("a job") {
            Job::Net(NetJob::Open(Target::Browser(url)), _) => assert_eq!(url, "urgent"),
            other => panic!("{other:?} came out before the urgent job"),
        }
        match next_job(&urgent, &background).expect("a job") {
            Job::Net(NetJob::Open(Target::Browser(url)), _) => assert_eq!(url, "background"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_fetch_from_before_a_cancel_is_dropped_before_a_connection_is_opened() {
        let fixture = Fixture::seeded();
        let cfg = WireConfig::default();
        let state = state_of(&cfg);
        state
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .refresh_generation = 1;

        let done = perform_net(
            NetJob::Fetch {
                feed: FeedId(1),
                url: "https://example.org/feed.xml".into(),
                kind: FeedKind::Web,
                conditional: feeds::Conditional::default(),
                max_bytes: 1024 * 1024,
                generation: 0,
            },
            Lane::Background,
            &fixture.http,
            &cfg,
            &state,
        );
        assert!(done.is_empty(), "a stale fetch reports nothing at all");

        // And the same job under the current generation does the work.
        let done = perform_net(
            NetJob::Fetch {
                feed: FeedId(1),
                url: "https://example.org/feed.xml".into(),
                kind: FeedKind::Web,
                conditional: feeds::Conditional::default(),
                max_bytes: 1024 * 1024,
                generation: 1,
            },
            Lane::Background,
            &fixture.http,
            &cfg,
            &state,
        );
        assert!(matches!(done.as_slice(), [Done::Fetched { .. }]));
    }

    #[test]
    fn a_stale_extraction_is_dropped_too() {
        let fixture = Fixture::seeded();
        let cfg = WireConfig::default();
        let state = state_of(&cfg);
        state
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .refresh_generation = 2;
        let done = perform_net(
            NetJob::Extract {
                entry: EntryId(1),
                url: "https://example.org/posts/borrow-checker".into(),
                limits: Limits::default(),
                generation: 1,
            },
            Lane::Background,
            &fixture.http,
            &cfg,
            &state,
        );
        assert!(done.is_empty());
    }

    /// A picture the reader moved on from is not fetched, and the room it
    /// was holding comes back. Nothing is written: `done_picture` throws a
    /// stale answer away.
    #[test]
    fn a_stale_picture_gives_its_slot_back() {
        let fixture = Fixture::seeded();
        let cfg = WireConfig::default();
        let state = state_of(&cfg);
        {
            let mut s = state.write().unwrap_or_else(|e| e.into_inner());
            s.picture_generation = 1;
            s.picture_inflight = 1;
        }

        let done = perform_net(
            NetJob::Picture {
                url: "https://example.org/hero.png".into(),
                max_w: 640,
                max_h: 320,
                generation: 0,
            },
            Lane::Urgent,
            &fixture.http,
            &cfg,
            &state,
        );
        assert!(
            matches!(
                done.as_slice(),
                [Done::Picture {
                    result: Err(_),
                    generation: 0,
                    ..
                }]
            ),
            "{done:?}"
        );

        for done in done {
            finish(done, &state, &sink().0, &channels().0);
        }
        let s = state.read().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            s.picture_inflight, 0,
            "two of these and `pump_pictures` never starts another picture"
        );
    }

    #[test]
    fn a_dropped_extraction_is_re_issued_by_the_next_tick() {
        // The queue is empty and nothing is in flight, which is exactly the
        // state a dropped job leaves behind. The tick asks the database what
        // is still pending rather than trying to remember what was lost.
        let cfg = WireConfig::default();
        let mut s = State::new(&cfg);
        s.last_refresh = Some(testing::now());
        s.last_retention = Some(testing::now());
        let effects = state::apply(&mut s, Change::Done(Done::Tick(testing::now())));
        assert!(effects
            .jobs
            .iter()
            .any(|j| matches!(j, Job::Db(DbJob::PendingExtracts { .. }))));

        // And what it answers turns straight back into extraction jobs.
        let mut fixture = Fixture::seeded();
        let pending = perform_db(DbJob::PendingExtracts { limit: 4 }, &mut fixture.db);
        let Some(Done::Pending(rows)) = pending.into_iter().next() else {
            panic!("the database answered something else");
        };
        assert!(!rows.is_empty(), "the fixture has pages left to pull");
        let effects = state::apply(&mut s, Change::Done(Done::Pending(rows)));
        assert!(effects
            .jobs
            .iter()
            .any(|j| matches!(j, Job::Net(NetJob::Extract { .. }, Lane::Background))));
    }

    #[test]
    fn storing_a_fetched_feed_reports_what_was_new_and_logs_the_round_trip() {
        let mut fixture = Fixture::seeded();
        let feed = fixture.feed("https://example.org/");
        let parsed = crate::wire::fetch::parse(&testing::feed_bytes("atom-basic.xml"), None)
            .expect("the fixture parses");

        let done = perform_db(
            DbJob::Store {
                feed,
                kind: FeedKind::Web,
                parsed: Box::new(parsed),
                conditional: feeds::Conditional::default(),
                bytes: 512,
                millis: 3,
            },
            &mut fixture.db,
        );
        // The fixture already stored this feed, so a second store of the
        // same bytes is new entries: none. That is the whole point of the
        // upsert being keyed on `(feed_id, guid)`.
        match done.as_slice() {
            [Done::Stored { new: 0, .. }] => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn adding_a_feed_canonicalises_it_on_the_way_in() {
        let mut fixture = Fixture::seeded();
        let done = perform_db(
            DbJob::AddFeed {
                url: "http://old.reddit.com/r/rust/.rss".into(),
                title: None,
                folder: Some("Reddit".into()),
            },
            &mut fixture.db,
        );
        match done.as_slice() {
            [Done::FeedAdded {
                url, is_new: true, ..
            }] => {
                assert_eq!(url, "https://www.reddit.com/r/rust/.rss")
            }
            other => panic!("{other:?}"),
        }
        assert!(feeds::folders(&fixture.db)
            .unwrap()
            .iter()
            .any(|f| f.name == "Reddit"));
    }

    #[test]
    fn a_database_failure_is_a_line_in_the_status_bar_and_not_the_end() {
        let mut fixture = Fixture::seeded();
        // A folder that is not there. The job fails; the program does not.
        let done = perform_db(
            DbJob::LoadEntries {
                sel: Selection::Search("\"".into()),
                unread_only: false,
                offset: 0,
                limit: 10,
            },
            &mut fixture.db,
        );
        assert!(matches!(done.as_slice(), [Done::Entries { .. }]));
    }

    #[test]
    fn both_kinds_of_thread_stop_on_shutdown() {
        let cfg = WireConfig::default();
        let fixture = Fixture::seeded();
        let (senders, db_rx, urgent_rx, background_rx) = channels();
        let (events, _rx) = sink();
        let state = state_of(&cfg);
        let cancel = Arc::new(AtomicBool::new(false));

        let db_thread = spawn_db(
            db_rx,
            events.clone(),
            Arc::clone(&state),
            senders.clone(),
            fixture.db,
        );
        let net_thread = spawn_net(
            0,
            urgent_rx,
            background_rx,
            NetPool {
                events,
                state: Arc::clone(&state),
                cfg,
                senders: senders.clone(),
                http: Arc::new(
                    crate::wire::net::Replay::open(&testing::testdata().join("replay")).unwrap(),
                ),
                cancel,
            },
        );

        senders.db.send(Job::Shutdown).unwrap();
        senders.urgent.send(Job::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while (!db_thread.is_finished() || !net_thread.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            db_thread.is_finished(),
            "the database thread is still running"
        );
        assert!(net_thread.is_finished(), "a net thread is still running");
        db_thread.join().unwrap();
        net_thread.join().unwrap();
    }
}
