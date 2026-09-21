//! The fixture every core test that needs more than one module is built on.
//!
//! An in-memory database seeded from `testdata/feeds`, and a
//! [`crate::wire::net::Replay`] over `testdata/replay`. Not a mock: the
//! database is a real SQLite file in memory running the real schema, the feed
//! parsing is the real parser over the real fixture bytes, and the extractor
//! is the real extractor over the real page. Only the socket is missing.

use std::path::{Path, PathBuf};

use crate::wire::db::{entries, feeds, Db};
use crate::wire::feed::{EntryId, FeedId, FeedKind};
use crate::wire::handle::{EventSink, Handle, HandleParts};
use crate::wire::net::Replay;
use crate::wire::state::State;
use crate::wire::worker::{DbJob, Job, Senders};

/// A seeded database and a replay directory.
pub struct Fixture {
    pub db: Db,
    pub http: Replay,
}

/// The instant fixture timestamps are measured from: 2026-09-20 12:00:00 UTC
/// (a Sunday).
///
/// Pinned so that an age reads `2h` on every machine and on every day the
/// tests are run, the same way STAR/FOLD pins its fixture mtimes.
pub fn now() -> jiff::Timestamp {
    jiff::Timestamp::from_second(1_789_905_600).expect("a fixed instant")
}

pub fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

/// One fixture feed file's bytes.
pub fn feed_bytes(name: &str) -> Vec<u8> {
    let path = testdata().join("feeds").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// One fixture page's text.
pub fn page(name: &str) -> String {
    let path = testdata().join("pages").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

impl Fixture {
    /// Four feeds and everything their fixtures carry: an ordinary web feed
    /// with three entries, a workbench blog, Hacker News, and a YouTube
    /// channel. Enough that every [`crate::wire::feed::Selection`] has rows
    /// in it.
    pub fn seeded() -> Self {
        let mut db = Db::open_in_memory().expect("an in-memory database");
        let tech = feeds::folder_named(&db, "Tech").expect("a folder");

        let seeds: [(&str, FeedKind, &str, Option<_>); 4] = [
            (
                "https://example.org/feed.xml",
                FeedKind::Web,
                "atom-basic.xml",
                Some(tech),
            ),
            (
                "https://workbench.example/rss.xml",
                FeedKind::Web,
                "rss2-basic.xml",
                Some(tech),
            ),
            (
                "https://hnrss.org/frontpage",
                FeedKind::Hn,
                "hn-frontpage.xml",
                None,
            ),
            (
                "https://www.youtube.com/feeds/videos.xml?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw",
                FeedKind::Youtube,
                "youtube-channel.xml",
                None,
            ),
        ];

        for (url, kind, fixture, folder) in seeds {
            let id = feeds::add(&db, url, None, kind, None, folder)
                .expect("adding a fixture feed")
                .id();
            let parsed = crate::wire::fetch::parse(&feed_bytes(fixture), None)
                .unwrap_or_else(|e| panic!("{fixture}: {e}"));
            feeds::record_ok(
                &db,
                id,
                &feeds::Conditional::default(),
                parsed.title.as_deref(),
                parsed.site_url.as_deref(),
            )
            .expect("recording the fetch");
            entries::upsert_parsed(&mut db, id, kind, &parsed).expect("storing the entries");
        }

        let http = Replay::open(&testdata().join("replay")).expect("the replay directory");
        Self { db, http }
    }

    /// The feed whose URL starts with `prefix`. Panics rather than returning
    /// an option: a test naming a feed that is not in the fixture is a test
    /// that will fail confusingly three lines later.
    pub fn feed(&self, prefix: &str) -> FeedId {
        feeds::list_feeds_with_unread(&self.db)
            .expect("listing feeds")
            .into_iter()
            .find(|f| f.url.starts_with(prefix))
            .unwrap_or_else(|| panic!("no fixture feed starting with {prefix}"))
            .id
    }

    /// The entry whose title contains `needle`.
    pub fn entry(&self, needle: &str) -> EntryId {
        entries::page(&self.db, &crate::wire::feed::Selection::All, false, 0, 500)
            .expect("paging entries")
            .rows
            .into_iter()
            .find(|e| e.title.contains(needle))
            .unwrap_or_else(|| panic!("no fixture entry whose title holds {needle:?}"))
            .id
    }
}

/// A running core with no threads behind it.
///
/// Everything a [`crate::wire::Handle`] has -- the state, the senders, the
/// event sink -- and the receiving ends of the three job queues, so that a
/// test (and the window's own fixture in a later package) can run the real
/// `worker::{perform_db, perform_net}` inline on its own thread and fold
/// each result in through the real `state::apply`. No timing, no sleeping,
/// no socket, and the same code the threads run.
pub struct Driver {
    pub state: std::sync::Arc<std::sync::RwLock<State>>,
    pub senders: Senders,
    pub sink: EventSink,
    pub db: Db,
    pub http: Replay,
    pub cfg: crate::wire::WireConfig,
    db_jobs: crossbeam_channel::Receiver<Job>,
    urgent: crossbeam_channel::Receiver<Job>,
    background: crossbeam_channel::Receiver<Job>,
}

/// The parts of a thread-free core, and the handle that drives it.
///
/// The database is seeded from `testdata/feeds` and the network is the
/// replay directory, so a test that calls this has four feeds, their
/// entries, and an answer for every URL any of them links to.
pub fn driver(cfg: &crate::wire::WireConfig) -> (Handle, Driver) {
    let fixture = Fixture::seeded();
    driver_over(cfg, fixture.db, fixture.http)
}

/// The same, over a database and a replay directory of the caller's own.
pub fn driver_over(cfg: &crate::wire::WireConfig, db: Db, http: Replay) -> (Handle, Driver) {
    let (db_tx, db_jobs) = crossbeam_channel::bounded(1024);
    let (urgent_tx, urgent) = crossbeam_channel::bounded(1024);
    let (background_tx, background) = crossbeam_channel::bounded(1024);
    let (event_tx, events) = crossbeam_channel::bounded(4096);
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sink = EventSink::new(event_tx, std::sync::Arc::clone(&dropped));
    let senders = Senders {
        db: db_tx,
        urgent: urgent_tx,
        background: background_tx,
    };
    let state = std::sync::Arc::new(std::sync::RwLock::new(State::new(cfg)));

    let handle = Handle::from_parts(HandleParts {
        senders: senders.clone(),
        events,
        state: std::sync::Arc::clone(&state),
        sink: sink.clone(),
        threads: Vec::new(),
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        dropped,
    });
    // So that a driver starts where `Handle::spawn` would: with the feed
    // list already in `State`.
    senders.dispatch(Job::Db(DbJob::LoadFeeds));

    let driver = Driver {
        state,
        senders,
        sink,
        db,
        http,
        cfg: cfg.clone(),
        db_jobs,
        urgent,
        background,
    };
    (handle, driver)
}

impl Driver {
    /// Run every queued job, and everything those jobs queue in turn, until
    /// all three lanes are empty. Returns how many jobs ran.
    ///
    /// Urgent before the database before the background, which is the same
    /// order the real threads settle into and the order that makes a test
    /// reading a queue after one `pump` see what the window would.
    pub fn pump(&mut self) -> usize {
        // A ceiling rather than a `while`: a bug that makes two jobs imply
        // each other should fail a test in a second, not hang the suite.
        const CEILING: usize = 10_000;
        let mut ran = 0usize;
        while ran < CEILING {
            let Some(job) = self.next() else { break };
            ran += 1;
            match job {
                Job::Db(job) => {
                    for done in crate::wire::worker::perform_db(job, &mut self.db) {
                        crate::wire::worker::finish(done, &self.state, &self.sink, &self.senders);
                    }
                }
                Job::Net(job, _) => {
                    for done in
                        crate::wire::worker::perform_net(job, &self.http, &self.cfg, &self.state)
                    {
                        crate::wire::worker::finish(done, &self.state, &self.sink, &self.senders);
                    }
                }
                Job::Shutdown => break,
            }
        }
        ran
    }

    /// One tick of the clock, as the database thread's `recv_timeout` would
    /// hand it over.
    pub fn tick(&mut self, at: jiff::Timestamp) {
        crate::wire::worker::finish(
            crate::wire::worker::Done::Tick(at),
            &self.state,
            &self.sink,
            &self.senders,
        );
    }

    fn next(&self) -> Option<Job> {
        self.urgent
            .try_recv()
            .or_else(|_| self.db_jobs.try_recv())
            .or_else(|_| self.background.try_recv())
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::feed::{EntryKind, Selection};

    #[test]
    fn the_fixture_has_rows_for_every_selection_the_lists_can_show() {
        let f = Fixture::seeded();
        let all = entries::page(&f.db, &Selection::All, false, 0, 500).unwrap();
        assert!(all.total >= 8, "{}", all.total);

        let videos = entries::page(&f.db, &Selection::Videos, false, 0, 500).unwrap();
        assert_eq!(videos.total, 1);
        assert_eq!(videos.rows[0].kind, EntryKind::Video);

        let tech = feeds::folders(&f.db).unwrap()[0].id;
        assert!(
            entries::page(&f.db, &Selection::Folder(tech), false, 0, 500)
                .unwrap()
                .total
                > 0
        );

        let one = f.feed("https://example.org/");
        assert_eq!(
            entries::page(&f.db, &Selection::Feed(one), false, 0, 500)
                .unwrap()
                .total,
            3
        );
    }

    #[test]
    fn every_fixture_feed_has_a_title_after_being_stored() {
        let f = Fixture::seeded();
        for feed in feeds::list_feeds_with_unread(&f.db).unwrap() {
            assert!(
                feed.title.is_some(),
                "{} came out of its fixture with no title",
                feed.url
            );
        }
    }

    #[test]
    fn nothing_in_the_fixture_is_read_yet() {
        let f = Fixture::seeded();
        let all = entries::page(&f.db, &Selection::All, false, 0, 500).unwrap();
        assert_eq!(
            entries::unread_count(&f.db, &Selection::All).unwrap(),
            all.total
        );
    }

    #[test]
    fn the_replay_directory_answers_what_the_fixture_feeds_link_to() {
        let f = Fixture::seeded();
        let got = crate::wire::extract::run(
            &f.http,
            "https://example.org/posts/borrow-checker",
            crate::wire::extract::Limits::default(),
        )
        .unwrap();
        assert_eq!(got.status, crate::wire::feed::ArticleStatus::Extracted);
    }

    #[test]
    fn the_pinned_instant_is_the_one_the_doc_comment_names() {
        assert_eq!(now().to_string(), "2026-09-20T12:00:00Z");
    }

    #[test]
    fn naming_a_feed_and_an_entry_finds_them() {
        let f = Fixture::seeded();
        let _ = f.feed("https://hnrss.org/");
        let _ = f.entry("borrow checker");
    }

    /// The driver is the window's fixture in miniature: the real `apply`,
    /// the real `perform_db` and `perform_net`, and no thread to wait on.
    #[test]
    fn the_driver_runs_the_whole_core_on_the_test_thread() {
        let cfg = crate::wire::WireConfig::default();
        let (handle, mut driver) = driver(&cfg);
        driver.pump();
        assert_eq!(handle.state().feeds.len(), 4, "the fixture's own feeds");

        handle.send(crate::wire::Command::OpenFeed(Selection::All));
        driver.pump();
        assert!(!handle.state().page.rows.is_empty());

        let entry = handle
            .state()
            .page
            .rows
            .iter()
            .find(|r| r.title.contains("borrow checker"))
            .expect("the fixture entry")
            .id;
        handle.send(crate::wire::Command::OpenEntry(entry));
        driver.pump();
        let article = handle.state().article.clone().expect("an article");
        assert_eq!(article.status, crate::wire::feed::ArticleStatus::Extracted);
        assert!(
            article.markdown.contains("fn longest"),
            "{}",
            article.markdown
        );
    }

    #[test]
    fn a_tick_through_the_driver_sweeps_and_tops_the_queue_up() {
        let cfg = crate::wire::WireConfig::default();
        let (handle, mut driver) = driver(&cfg);
        driver.pump();
        driver.tick(now());
        let ran = driver.pump();
        assert!(ran > 0, "a tick on a fresh database has work to do");
        assert!(handle.state().last_retention.is_some());
    }
}
