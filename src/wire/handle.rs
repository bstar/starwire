//! The contract between the core and the window.
//!
//! The core runs on `1 + parallel` OS threads -- `starwire-db` and the
//! `starwire-net-N` pool, spawned by [`Handle::spawn`] -- with no async
//! runtime. The window's loop is synchronous: it sends [`Command`]s through
//! [`Handle::send`], drains [`Event`]s once a frame through
//! [`Handle::drain`], and reads the truth out of [`State`] behind an
//! `RwLock`.
//!
//! **Events carry no state.** `Event::Feeds` means "the feed list changed",
//! not what it changed to. So an event may be coalesced, delayed or dropped
//! outright and the next frame still draws the truth out of `State`. That is
//! what makes the bounded event channel safe: when it fills, [`EventSink`]
//! counts the drop and the next send that fits is preceded by
//! [`Event::Refresh`], which tells the window to stop trusting its
//! incremental picture and re-read everything. The shape is STAR/FOLD's
//! `fold::handle`, which took it from STAR/CORD.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use anyhow::Result;

use super::db::{feeds, schema, Db};
use super::feed::{EntryId, FeedId, FolderId, Selection};
use super::state::{self, Change, State};
use super::worker::{self, DbJob, Job, Senders};
use super::WireConfig;

/// How many jobs may be queued for a worker before they start being dropped.
///
/// A refresh of a long feed list is the case this is sized for: forty-one
/// fetches, each of which implies a store, and an extraction queue behind
/// them. A job that does not fit is warned about and picked up by the next
/// tick.
pub const JOB_CAPACITY: usize = 1024;
/// How many events may be queued before they start being dropped.
pub const EVENT_CAPACITY: usize = 4096;
/// How long `Drop` waits for the threads to finish.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Which feeds a refresh covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshScope {
    All,
    Feed(FeedId),
    Folder(FolderId),
}

/// A setting the running core will change its mind about.
///
/// Runtime only. The window writes `config.toml` itself through
/// `starkit::config::edit` and sends one of these so the change takes effect
/// without a restart; the player's argv and the request timeout have no
/// variant here because they are read where a restart is the honest answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    RefreshMinutes(u32),
    Extract(bool),
    Images(bool),
    KeepDays(u32),
    UnreadOnly(bool),
}

/// What a link inside an article turned out to be.
///
/// Decided by the window with the pure table in
/// [`super::open::kind_of`] -- a YouTube watch link, a `youtu.be` link and a
/// bare video file are videos, and everything else is a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenKind {
    Browser,
    Video,
}

/// A newsboat installation worth offering to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOffer {
    pub urls: PathBuf,
    /// Where the read marks and the feed titles are, when there is a cache.
    pub cache: Option<PathBuf>,
}

/// Everything the window can ask the core to do.
///
/// Named for what it means rather than for the key that reaches it, so the
/// window's dispatcher is a `match` on meaning and not on input.
#[derive(Debug)]
pub enum Command {
    /// Show a selection. Sent whenever the window's active ENTRIES frame
    /// changes -- a push, a pop, an `alt+up` jump.
    OpenFeed(Selection),
    /// The next page of the current selection.
    MoreEntries,
    /// Re-page the current selection from the top.
    ReloadEntries,
    SetUnreadOnly(bool),
    /// The `/` filter over the rows already loaded. Pure; no query.
    Filter(String),
    ClearFilter,
    /// Load an entry's text. Does **not** mark it read -- the window sends
    /// `SetRead` when `[reading] mark_read_on_open` says so.
    OpenEntry(EntryId),
    CloseEntry,
    SetRead {
        entries: Vec<EntryId>,
        read: bool,
    },
    MarkAllRead(Selection),
    SetStarred {
        entry: EntryId,
        on: bool,
    },
    Refresh(RefreshScope),
    /// Stop the refresh: what is queued is dropped rather than fetched.
    CancelRefresh,
    /// Pull an entry's page again, whatever its article says now.
    Extract(EntryId),
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
    /// A channel URL, an `@handle` or a bare `UC…`.
    YoutubeAdd(String),
    YoutubeImportTakeout(PathBuf),
    YoutubeSync {
        cookies_from_browser: Option<String>,
    },
    ImportNewsboat {
        urls: PathBuf,
        cache: Option<PathBuf>,
    },
    ImportOpml(PathBuf),
    ExportOpml(PathBuf),
    /// No, and never ask again.
    DismissImportOffer,
    /// Sugar for `OpenFeed(Selection::Search(q))`: search results are a page
    /// like any other, and a level of the window's stack like any other.
    Search(String),
    /// The entry's own link: a video to the player, everything else to the
    /// browser.
    OpenExternal(EntryId),
    /// A link from inside an article.
    OpenUrl {
        url: String,
        kind: OpenKind,
    },
    /// Fetch a picture the open article carries, at no more than `max_w` by
    /// `max_h` pixels. Idempotent: the window sends it for every picture on
    /// screen every frame, and the core answers the second one with nothing.
    FetchPicture {
        url: String,
        max_w: u32,
        max_h: u32,
    },
    /// Hand a picture to the desktop: the cached file where the bytes have
    /// arrived, and the URL where they have not.
    OpenPicture(String),
    SetSetting(Setting),
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteLevel {
    Info,
    Warning,
    Error,
}

/// Something to tell the reader in the status line.
#[derive(Debug, Clone)]
pub struct Note {
    pub level: NoteLevel,
    /// Repeats of the same key replace rather than stack, so a feed that
    /// keeps failing does not fill the status line with the same line over
    /// and over.
    pub key: Option<&'static str>,
    pub text: String,
}

impl Note {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            level: NoteLevel::Info,
            key: None,
            text: text.into(),
        }
    }

    pub fn warning(key: &'static str, text: impl Into<String>) -> Self {
        Self {
            level: NoteLevel::Warning,
            key: Some(key),
            text: text.into(),
        }
    }

    pub fn error(key: &'static str, text: impl Into<String>) -> Self {
        Self {
            level: NoteLevel::Error,
            key: Some(key),
            text: text.into(),
        }
    }
}

/// A notification that something changed. Never the change itself.
#[derive(Debug, Clone)]
pub enum Event {
    /// `State::feeds` or `State::folders` changed.
    Feeds,
    /// `State::page` changed.
    Entries,
    /// `State::article` for this entry changed -- loaded, or an extraction
    /// landed behind it.
    Article(EntryId),
    /// Mirrors `State::refresh`.
    Progress {
        done: usize,
        total: usize,
    },
    Note(Note),
    /// `State::pictures` changed -- one arrived, one failed, or they were
    /// all dropped. The window re-reads them and lays the article out again
    /// if a size it did not know is now known.
    Pictures,
    /// `State::import_offer` changed.
    ImportOffer,
    /// Events were dropped. Whatever the window believes about its
    /// incremental state is now suspect; re-read everything.
    Refresh,
}

/// The sending half of the event channel, with the drop bookkeeping.
#[derive(Clone)]
pub struct EventSink {
    tx: crossbeam_channel::Sender<Event>,
    dropped: Arc<AtomicU64>,
    needs_refresh: Arc<AtomicBool>,
}

impl EventSink {
    pub fn new(tx: crossbeam_channel::Sender<Event>, dropped: Arc<AtomicU64>) -> Self {
        Self {
            tx,
            dropped,
            needs_refresh: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn send(&self, event: Event) {
        if self.needs_refresh.swap(false, Ordering::Relaxed)
            && self.tx.try_send(Event::Refresh).is_err()
        {
            self.needs_refresh.store(true, Ordering::Relaxed);
        }

        if self.tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            self.needs_refresh.store(true, Ordering::Relaxed);
        }
    }
}

/// The window's view of the core.
pub struct Handle {
    senders: Senders,
    events: crossbeam_channel::Receiver<Event>,
    state: Arc<RwLock<State>>,
    sink: EventSink,
    /// The net threads first and the database thread last, which is the
    /// order `Drop` joins them in: the last mark-read is written by the
    /// database thread, and it must still be running when the net pool
    /// finishes folding whatever it had in flight.
    threads: Vec<std::thread::JoinHandle<()>>,
    /// Checked by a net thread between jobs, so a shutdown does not wait for
    /// a queue to drain.
    cancel: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}

/// Everything a [`Handle`] needs, so a fixture can build one with no threads
/// behind it at all -- the window's `ui::fake`, which drains the job queues
/// on the test thread through `worker::{perform_db, perform_net, finish}`.
pub struct HandleParts {
    pub senders: Senders,
    pub events: crossbeam_channel::Receiver<Event>,
    pub state: Arc<RwLock<State>>,
    pub sink: EventSink,
    pub threads: Vec<std::thread::JoinHandle<()>>,
    pub cancel: Arc<AtomicBool>,
    pub dropped: Arc<AtomicU64>,
}

impl Handle {
    /// Open the database, load the feed list, and start the threads.
    pub fn spawn(cfg: WireConfig, db_path: PathBuf, offer: Option<ImportOffer>) -> Result<Handle> {
        let http: Arc<dyn super::net::Http> = Arc::new(super::net::Live::new(&cfg));
        Self::spawn_with(cfg, db_path, offer, http)
    }

    /// The same, against a network of somebody else's choosing -- the replay
    /// directory `--replay` serves, and what the headless driver test runs
    /// the whole core against with no socket in sight.
    pub fn spawn_with(
        cfg: WireConfig,
        db_path: PathBuf,
        offer: Option<ImportOffer>,
        http: Arc<dyn super::net::Http>,
    ) -> Result<Handle> {
        let db = Db::open(&db_path)?;

        // Read synchronously, before a thread exists to be waited on: the
        // first frame the window draws already has the feed list in it
        // rather than an empty panel for however long the database thread
        // takes to pick the job up.
        let rows = feeds::list_feeds_with_unread(&db)?;
        let folders = feeds::folders(&db)?;
        // Offered only on a list with nothing in it, and only once ever:
        // after an import STAR/WIRE owns the list, and an offer that comes
        // back after being refused is a bug rather than a reminder.
        let offer = match offer {
            Some(offer) if rows.is_empty() && !feeds::newsboat_settled(&db)? => Some(offer),
            _ => None,
        };

        let mut initial = State::new(&cfg);
        initial.feeds = rows;
        initial.folders = folders;
        initial.import_offer = offer.clone();
        initial.last_refresh = stamped(&db, schema::meta::LAST_REFRESH);
        initial.last_retention = stamped(&db, schema::meta::LAST_RETENTION);

        let (db_tx, db_rx) = crossbeam_channel::bounded(JOB_CAPACITY);
        let (urgent_tx, urgent_rx) = crossbeam_channel::bounded(JOB_CAPACITY);
        let (background_tx, background_rx) = crossbeam_channel::bounded(JOB_CAPACITY);
        let (event_tx, event_rx) = crossbeam_channel::bounded(EVENT_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let sink = EventSink::new(event_tx, Arc::clone(&dropped));
        let senders = Senders {
            db: db_tx,
            urgent: urgent_tx,
            background: background_tx,
        };
        let state = Arc::new(RwLock::new(initial));
        let cancel = Arc::new(AtomicBool::new(false));

        let parallel = cfg.fetch.parallel.max(1);
        let pool = worker::NetPool {
            events: sink.clone(),
            state: Arc::clone(&state),
            cfg: cfg.clone(),
            senders: senders.clone(),
            http,
            cancel: Arc::clone(&cancel),
        };
        let mut threads = Vec::with_capacity(parallel + 1);
        for index in 0..parallel {
            threads.push(worker::spawn_net(
                index,
                urgent_rx.clone(),
                background_rx.clone(),
                pool.clone(),
            ));
        }
        threads.push(worker::spawn_db(
            db_rx,
            sink.clone(),
            Arc::clone(&state),
            senders.clone(),
            db,
        ));

        let handle = Handle {
            senders,
            events: event_rx,
            state,
            sink,
            threads,
            cancel,
            dropped,
        };

        if offer.is_some() {
            handle.sink.send(Event::ImportOffer);
        } else if cfg.fetch.refresh_on_start {
            // Not while an import is being offered: the list is about to
            // change, and fetching the four feeds somebody already had is
            // work the answer would throw away.
            //
            // The job rather than `Command::Refresh`, which is the same
            // question without the second half: a refresh somebody pressed
            // a key for offers the failed extractions in its scope again,
            // and one that happens because the program started must not --
            // the attempt ceiling would then last exactly one session, and
            // a page that cannot be read would cost a request every launch.
            // Generation zero because nothing has been cancelled yet.
            handle.senders.dispatch(Job::Db(DbJob::Due {
                scope: RefreshScope::All,
                generation: 0,
            }));
        }
        Ok(handle)
    }

    /// Whether there is a newsboat list worth offering to import.
    ///
    /// Called before [`spawn`](Self::spawn), which is the only way round
    /// that works: the answer is a path the window puts in an overlay, and
    /// `spawn` decides whether to keep the offer by looking at the database.
    pub fn probe_newsboat() -> Option<ImportOffer> {
        let home = std::env::home_dir()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::probe_newsboat_in(&home)
    }

    /// The same, under a directory of the caller's choosing.
    pub fn probe_newsboat_in(home: &Path) -> Option<ImportOffer> {
        let urls = super::import::newsboat::default_urls_paths(home)
            .into_iter()
            .find(|p| p.is_file())?;
        let cache = super::import::newsboat::default_cache_paths(home)
            .into_iter()
            .find(|p| p.is_file());
        Some(ImportOffer { urls, cache })
    }

    /// Build a handle around something that is not the real core.
    pub fn from_parts(parts: HandleParts) -> Handle {
        Handle {
            senders: parts.senders,
            events: parts.events,
            state: parts.state,
            sink: parts.sink,
            threads: parts.threads,
            cancel: parts.cancel,
            dropped: parts.dropped,
        }
    }

    /// Ask the core to do something.
    ///
    /// Applies the pure transition under the write lock, dispatches whatever
    /// jobs it produced, and lets the lock go before anything is sent -- a
    /// worker taking the lock to report a result must never wait on a
    /// channel send this function is blocked on.
    pub fn send(&self, command: Command) {
        let effects = {
            let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
            state::apply(&mut state, Change::Command(command))
        };
        for job in effects.jobs {
            self.senders.dispatch(job);
        }
        for event in effects.events {
            self.sink.send(event);
        }
    }

    /// Everything queued, for the once-a-frame drain.
    pub fn drain(&self) -> crossbeam_channel::TryIter<'_, Event> {
        self.events.try_iter()
    }

    /// The truth. Copy what the frame needs and drop the guard before
    /// drawing: a worker takes the write lock to fold its result in, and a
    /// read guard held across a render stalls it.
    pub fn state(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn events_dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

fn stamped(db: &Db, key: &str) -> Option<jiff::Timestamp> {
    let value = db.meta(key).ok()??;
    let secs: i64 = value.trim().parse().ok()?;
    jiff::Timestamp::from_second(secs).ok()
}

impl Drop for Handle {
    fn drop(&mut self) {
        // The flag first, so a thread between jobs stops without needing to
        // see a message; then one `Shutdown` per thread, because each takes
        // exactly one and a thread mid-fetch has to find one waiting.
        self.cancel.store(true, Ordering::Relaxed);
        let net = self.threads.len().saturating_sub(1);
        for _ in 0..net {
            let _ = self.senders.urgent.try_send(Job::Shutdown);
            let _ = self.senders.background.try_send(Job::Shutdown);
        }
        let _ = self.senders.db.try_send(Job::Shutdown);

        let deadline = Instant::now() + SHUTDOWN_GRACE;
        for thread in std::mem::take(&mut self.threads) {
            while !thread.is_finished() {
                if Instant::now() >= deadline {
                    tracing::warn!("a wire worker did not stop within {SHUTDOWN_GRACE:?}");
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if let Err(e) = thread.join() {
                tracing::error!("a wire worker panicked: {e:?}");
            }
        }
    }
}

/// Sugar the window uses often enough to be worth naming here rather than
/// writing out at every call site.
impl Handle {
    pub fn open(&self, sel: Selection) {
        self.send(Command::OpenFeed(sel));
    }

    pub fn reload_feeds(&self) {
        self.senders.dispatch(Job::Db(DbJob::LoadFeeds));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::feed::EntryRow;
    use crate::wire::net::Replay;
    use crate::wire::testing;
    use crate::wire::worker::Senders;
    use std::sync::atomic::AtomicU64;

    fn parts() -> (
        Handle,
        crossbeam_channel::Sender<Event>,
        crossbeam_channel::Receiver<Job>,
    ) {
        let (db_tx, db_rx) = crossbeam_channel::bounded(8);
        let (urgent_tx, _urgent_rx) = crossbeam_channel::bounded(8);
        let (background_tx, _background_rx) = crossbeam_channel::bounded(8);
        let (event_tx, event_rx) = crossbeam_channel::bounded(8);
        let dropped = Arc::new(AtomicU64::new(0));
        let handle = Handle::from_parts(HandleParts {
            senders: Senders {
                db: db_tx,
                urgent: urgent_tx,
                background: background_tx,
            },
            events: event_rx,
            state: Arc::new(RwLock::new(State::new(&WireConfig::default()))),
            sink: EventSink::new(event_tx.clone(), Arc::clone(&dropped)),
            threads: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            dropped,
        });
        (handle, event_tx, db_rx)
    }

    #[test]
    fn the_drain_yields_everything_queued_and_then_stops() {
        let (handle, events, _jobs) = parts();
        events.send(Event::Feeds).unwrap();
        events.send(Event::Entries).unwrap();
        assert_eq!(handle.drain().count(), 2);
        assert_eq!(handle.drain().count(), 0);
    }

    #[test]
    fn sending_a_command_queues_the_job_before_the_event() {
        let (handle, _events, jobs) = parts();
        handle.send(Command::OpenFeed(Selection::Starred));
        assert!(matches!(
            jobs.try_recv(),
            Ok(Job::Db(crate::wire::worker::DbJob::LoadEntries { .. }))
        ));
        assert!(handle.drain().any(|e| matches!(e, Event::Entries)));
    }

    /// STAR/CORD's rule, kept: an event that does not fit is counted, and
    /// the next one that does is preceded by a `Refresh` so the window knows
    /// to stop trusting what it missed.
    #[test]
    fn events_that_do_not_fit_are_dropped_and_followed_by_a_refresh() {
        let (event_tx, event_rx) = crossbeam_channel::bounded(2);
        let dropped = Arc::new(AtomicU64::new(0));
        let sink = EventSink::new(event_tx, Arc::clone(&dropped));

        sink.send(Event::Feeds);
        sink.send(Event::Feeds);
        sink.send(Event::Feeds);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);

        let _ = event_rx.try_recv();
        let _ = event_rx.try_recv();
        sink.send(Event::Entries);

        assert!(
            matches!(event_rx.try_recv(), Ok(Event::Refresh)),
            "a window that missed an event was not told to re-read"
        );
        assert!(matches!(event_rx.try_recv(), Ok(Event::Entries)));
    }

    fn replay() -> Arc<dyn super::super::net::Http> {
        Arc::new(Replay::open(&testing::testdata().join("replay")).expect("the replay directory"))
    }

    /// A database with the replay directory's own feed list in it, which is
    /// what `--replay` seeds an empty one with.
    fn seeded(dir: &Path) -> PathBuf {
        let path = dir.join("wire.db");
        let db = Db::open(&path).expect("a database");
        let replay =
            Replay::open(&testing::testdata().join("replay")).expect("the replay directory");
        for url in replay.feeds() {
            let canonical = crate::wire::youtube::canonicalise(url).expect("a fixture URL");
            feeds::add(
                &db,
                &canonical.url,
                None,
                canonical.kind,
                canonical.title.as_deref(),
                None,
            )
            .expect("adding a feed");
        }
        path
    }

    fn cfg() -> WireConfig {
        let mut cfg = WireConfig::default();
        // The test drives the refresh itself, so that "nothing has happened
        // yet" is a state it can actually observe.
        cfg.fetch.refresh_on_start = false;
        cfg
    }

    /// Poll the state until `f` holds, draining events as the window would.
    ///
    /// A deadline rather than a fixed sleep: the whole of this runs against
    /// a directory of files, so it finishes in milliseconds, and a machine
    /// under load should wait rather than fail.
    fn wait(handle: &Handle, what: &str, mut f: impl FnMut(&State) -> bool) -> Vec<Event> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = Vec::new();
        loop {
            seen.extend(handle.drain());
            if f(&handle.state()) {
                return seen;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn row_titled<'a>(rows: &'a [EntryRow], needle: &str) -> &'a EntryRow {
        rows.iter()
            .find(|r| r.title.contains(needle))
            .unwrap_or_else(|| panic!("no row whose title holds {needle:?}"))
    }

    /// The whole core, on its real threads, with a directory of files where
    /// the network would be: a refresh end to end, a page, an article
    /// somebody reads, a read mark, and an import.
    #[test]
    fn the_core_refreshes_pages_reads_and_imports_with_no_window_and_no_socket() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let db_path = seeded(dir.path());
        let handle = Handle::spawn_with(cfg(), db_path, None, replay()).expect("the core starts");

        // Loaded synchronously by `spawn`, so the first frame is never empty
        // for want of a job being picked up.
        let feed_count = handle.state().feeds.len();
        assert_eq!(feed_count, 5, "the replay directory's own feed list");

        handle.send(Command::Refresh(RefreshScope::All));
        // `last_refresh` is stamped when one *starts*, so this waits for a
        // refresh that has both begun and ended rather than for the moment
        // before the first job is picked up. The counters are not the test:
        // they go back to nothing behind the last event, and that event is
        // what is asserted below.
        let events = wait(&handle, "the refresh to finish", |s| {
            s.last_refresh.is_some() && !s.refresh.running && s.fetching.is_empty()
        });
        assert!(
            events.iter().any(|e| matches!(
                e,
                Event::Progress { done, total } if done == total && *total > 0
            )),
            "the bar reached n of n"
        );
        assert!(handle.state().fetching.is_empty());

        handle.send(Command::OpenFeed(Selection::All));
        wait(&handle, "the page to fill", |s| !s.page.rows.is_empty());
        let rows = handle.state().page.rows.clone();
        assert!(rows.len() >= 8, "{} rows", rows.len());

        // The entry whose page is in the replay directory: it is pulled and
        // reduced without anybody asking, because it arrived in a refresh.
        let entry = row_titled(&rows, "borrow checker").id;
        handle.send(Command::OpenEntry(entry));
        wait(&handle, "the article to be extracted", |s| {
            s.article
                .as_ref()
                .is_some_and(|a| a.status == crate::wire::feed::ArticleStatus::Extracted)
        });
        let article = handle.state().article.clone().expect("an article");
        assert_eq!(article.entry, entry);
        assert!(
            article.markdown.contains("fn longest"),
            "the whole page, not the feed's two sentences:\n{}",
            article.markdown
        );

        // Reading one entry moves the count, and the count survives being
        // read back out of the database.
        let before = handle.state().unread_total();
        handle.send(Command::SetRead {
            entries: vec![entry],
            read: true,
        });
        assert_eq!(handle.state().unread_total(), before - 1, "at once");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            handle.reload_feeds();
            std::thread::sleep(Duration::from_millis(10));
            let _ = handle.drain().count();
            let now = handle.state().unread_total();
            if now == before - 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the read mark never reached the database: {now} unread, wanted {}",
                before - 1
            );
        }

        // And a newsboat list, imported into a database that already has
        // four of its feeds under other spellings.
        handle.send(Command::ImportNewsboat {
            urls: testing::testdata().join("import/urls"),
            cache: None,
        });
        wait(&handle, "the import to land", |s| s.last_import.is_some());
        let report = handle.state().last_import.clone().expect("a report");
        assert!(report.added >= 10, "{report:?}");
        assert!(report.already_there >= 1, "{report:?}");
        assert!(report.videos >= 6, "{report:?}");
        wait(&handle, "the new feeds to appear", |s| {
            s.feeds.len() > feed_count
        });

        // `Drop` joins every thread, and does it well within the grace
        // period rather than merely by it.
        let started = Instant::now();
        drop(handle);
        assert!(
            started.elapsed() < SHUTDOWN_GRACE,
            "shutting down took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_cancelled_refresh_stops_the_bar() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let db_path = seeded(dir.path());
        let handle = Handle::spawn_with(cfg(), db_path, None, replay()).expect("the core starts");
        handle.send(Command::Refresh(RefreshScope::All));
        handle.send(Command::CancelRefresh);
        wait(&handle, "the bar to stop", |s| !s.refresh.running);
        assert!(handle.state().refresh_generation >= 1);
    }

    #[test]
    fn the_offer_is_dropped_when_there_are_already_feeds() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let db_path = seeded(dir.path());
        let offer = Some(ImportOffer {
            urls: testing::testdata().join("import/urls"),
            cache: None,
        });
        let handle = Handle::spawn_with(cfg(), db_path, offer, replay()).expect("the core starts");
        assert!(
            handle.state().import_offer.is_none(),
            "after an import STAR/WIRE owns the list; the offer is a first-run question"
        );
    }

    #[test]
    fn the_offer_stands_on_an_empty_list_and_never_comes_back_once_refused() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let db_path = dir.path().join("wire.db");
        let offer = Some(ImportOffer {
            urls: testing::testdata().join("import/urls"),
            cache: None,
        });
        let handle = Handle::spawn_with(cfg(), db_path.clone(), offer.clone(), replay())
            .expect("the core starts");
        assert_eq!(handle.state().import_offer, offer);
        assert!(handle.drain().any(|e| matches!(e, Event::ImportOffer)));

        handle.send(Command::DismissImportOffer);
        wait(&handle, "the refusal to be written down", |_| true);
        std::thread::sleep(Duration::from_millis(50));
        drop(handle);

        let handle = Handle::spawn_with(cfg(), db_path, offer, replay()).expect("the core starts");
        assert!(
            handle.state().import_offer.is_none(),
            "an offer that comes back after being refused is a bug, not a reminder"
        );
    }

    #[test]
    fn probing_finds_a_newsboat_list_where_newsboat_keeps_one() {
        let home = tempfile::tempdir().expect("a temporary home");
        assert!(Handle::probe_newsboat_in(home.path()).is_none());

        std::fs::create_dir_all(home.path().join(".config/newsboat")).unwrap();
        std::fs::write(
            home.path().join(".config/newsboat/urls"),
            "https://example.org/feed.xml\n",
        )
        .unwrap();
        let offer = Handle::probe_newsboat_in(home.path()).expect("an offer");
        assert!(offer.urls.ends_with(".config/newsboat/urls"));
        assert!(offer.cache.is_none(), "there is no cache beside it");

        std::fs::create_dir_all(home.path().join(".local/share/newsboat")).unwrap();
        std::fs::write(home.path().join(".local/share/newsboat/cache.db"), b"").unwrap();
        let offer = Handle::probe_newsboat_in(home.path()).expect("an offer");
        assert!(
            offer.cache.is_some(),
            "the read marks and the titles are there"
        );
    }

    #[test]
    fn a_command_that_names_a_url_can_still_be_debug_printed() {
        let command = Command::OpenUrl {
            url: "https://example.org/x".into(),
            kind: OpenKind::Browser,
        };
        assert!(format!("{command:?}").contains("example.org"));
    }
}
