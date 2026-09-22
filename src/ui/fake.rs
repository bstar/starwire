//! A core with no threads behind it, for deterministic frames.
//!
//! [`handle`] builds a real [`Handle`] over a real in-memory database and
//! the replay directory, and hands the three job queues to a [`Fake`]
//! instead of to `starwire-db` and the `starwire-net` pool. A test sends a
//! [`Command`] exactly as the window does, then calls [`Fake::pump`] to run
//! whatever that produced: `pump` drains all three lanes through
//! `wire::worker::{perform_db, perform_net, finish}` -- the very functions
//! the real threads call -- synchronously, on the test thread.
//!
//! This is not a second core. `state::apply`, `db::*`, `fetch::parse` and
//! `extract::run` all run for real; what is fake is the absence of threads
//! and of a socket. It is `wire::testing::Driver` with a fixture built for
//! the *window* rather than for the core's own tests: the core's fixture is
//! four feeds parsed out of `testdata/feeds`, and this one is a feed list
//! with a folder in it, an article with every markdown treatment, one page
//! still coming, one that failed, a video, two things already read and one
//! starred -- one of each thing a panel can draw.
//!
//! Everything is measured from [`testing::now`], so an age reads `2h` and
//! `1d` on every machine and on every day the tests are run.

use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::wire::db::{articles, entries, feeds, Db};
use crate::wire::extract::ArticleResult;
use crate::wire::feed::{
    ArticleStatus, EntryId, FeedId, FeedKind, ParsedEntry, ParsedFeed, Selection,
};
use crate::wire::net::Replay;
use crate::wire::state::State;
use crate::wire::testing::{self, Driver};
use crate::wire::{Handle, WireConfig};

/// A [`Handle`] with no worker threads behind it, and the plumbing that
/// stands in for them.
pub struct Fake {
    driver: Driver,
}

/// Build a [`Handle`] over the window's fixture, with a [`Fake`] standing in
/// for the worker threads.
pub fn handle(cfg: &WireConfig) -> (Handle, Fake) {
    let db = seeded();
    let http = Replay::open(&testing::testdata().join("replay")).expect("the replay directory");
    // No disk cache, whatever the caller's config says. `Config::core()`
    // names the *reader's* own `cache/pictures`, and a test that wrote into
    // it would be a test that changes the machine it is run on.
    let cfg = WireConfig {
        pictures_dir: None,
        ..cfg.clone()
    };
    let (handle, driver) = testing::driver_over(&cfg, db, http);
    let mut fake = Fake { driver };
    // The feed list is loaded synchronously, exactly as `Handle::spawn` does
    // it, so the first frame a test draws already has rows in it.
    fake.pump();
    (handle, fake)
}

impl Fake {
    /// Run every queued job, and everything those jobs queue in turn, until
    /// all three lanes are empty. Returns how many jobs ran.
    pub fn pump(&mut self) -> usize {
        self.driver.pump()
    }

    /// One tick of the clock, as the database thread's `recv_timeout` hands
    /// it over -- what drives the background refresh and the retention
    /// sweep.
    pub fn tick(&mut self, at: jiff::Timestamp) {
        self.driver.tick(at);
    }

    pub fn state(&self) -> RwLockReadGuard<'_, State> {
        self.driver.state.read().unwrap_or_else(|e| e.into_inner())
    }

    /// The truth, writable -- for a frame that needs the core in a state no
    /// real run leaves it in for long enough to draw, such as a refresh
    /// stopped at `12 of 41`. Bypasses `state::apply`, so a caller bumps
    /// `State::version` itself for the next `App::tick` to notice.
    pub fn state_mut(&self) -> RwLockWriteGuard<'_, State> {
        self.driver.state.write().unwrap_or_else(|e| e.into_inner())
    }

    pub fn state_arc(&self) -> Arc<RwLock<State>> {
        Arc::clone(&self.driver.state)
    }

    /// The entry whose title contains `needle`, wherever it is in the
    /// fixture. Panics rather than returning an option: a test naming an
    /// entry that is not there is a test that fails confusingly later.
    pub fn entry(&self, needle: &str) -> EntryId {
        entries::page(&self.driver.db, &Selection::All, false, 0, 500)
            .expect("paging the fixture")
            .rows
            .into_iter()
            .find(|e| e.title.contains(needle))
            .unwrap_or_else(|| panic!("no fixture entry whose title holds {needle:?}"))
            .id
    }

    /// The feed whose URL starts with `prefix`.
    pub fn feed(&self, prefix: &str) -> FeedId {
        feeds::list_feeds_with_unread(&self.driver.db)
            .expect("listing the fixture's feeds")
            .into_iter()
            .find(|f| f.url.starts_with(prefix))
            .unwrap_or_else(|| panic!("no fixture feed starting with {prefix}"))
            .id
    }
}

/// How long before [`testing::now`] each fixture entry was published, so the
/// ages in a drawn frame are the ones the plan's mocks show.
const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

fn at(secs_ago: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_second(testing::now().as_second() - secs_ago).expect("a fixed instant")
}

/// The window's own fixture: four feeds in two folders and one loose, and
/// one entry of every kind a panel draws.
fn seeded() -> Db {
    let mut db = Db::open_in_memory().expect("an in-memory database");
    let tech = feeds::folder_named(&db, "Tech").expect("a folder");
    let tube = feeds::folder_named(&db, "YouTube").expect("a folder");

    let hn = add(
        &db,
        "https://hnrss.org/frontpage",
        FeedKind::Hn,
        "Hacker News",
        Some(tech),
    );
    let lobsters = add(
        &db,
        "https://lobste.rs/rss",
        FeedKind::Web,
        "Lobsters",
        Some(tech),
    );
    let channel = add(
        &db,
        "https://www.youtube.com/feeds/videos.xml?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw",
        FeedKind::Youtube,
        "Veritasium",
        Some(tube),
    );
    let phoronix = add(
        &db,
        "https://www.phoronix.com/rss.php",
        FeedKind::Web,
        "Phoronix",
        None,
    );

    store(
        &mut db,
        hn,
        FeedKind::Hn,
        &[
            entry(
                "hn-1",
                "Why the borrow checker says no: a field guide to lifetimes",
                Some("https://example.org/posts/borrow-checker"),
                at(2 * HOUR),
            ),
            entry(
                "hn-2",
                "The page that has not arrived yet",
                // No link at all, which is a real thing a feed item can be
                // -- and what keeps this one pending in a fixture that has
                // no network to fetch it with.
                None,
                at(3 * HOUR),
            ),
            entry(
                "hn-3",
                "Behind a paywall",
                Some("https://example.com/paywalled"),
                at(4 * HOUR),
            ),
        ],
    );
    store(
        &mut db,
        lobsters,
        FeedKind::Web,
        &[
            entry(
                "lo-1",
                "Something already read",
                Some("https://lobste.rs/s/aaa"),
                at(DAY),
            ),
            entry(
                "lo-2",
                "And another already read",
                Some("https://lobste.rs/s/bbb"),
                at(DAY),
            ),
        ],
    );
    store(
        &mut db,
        phoronix,
        FeedKind::Web,
        &[
            entry(
                "ph-1",
                "A kernel release worth keeping",
                Some("https://www.phoronix.com/news/one"),
                at(DAY),
            ),
            entry(
                "ph-2",
                "A post with pictures",
                Some("https://www.phoronix.com/news/two"),
                at(2 * DAY),
            ),
        ],
    );
    store(
        &mut db,
        channel,
        FeedKind::Youtube,
        &[ParsedEntry {
            guid: "yt:video:dQw4w9WgXcQ".into(),
            url: Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()),
            title: "How a transistor works".into(),
            author: Some("Veritasium".into()),
            published: Some(at(DAY)),
            video_id: Some("dQw4w9WgXcQ".into()),
            ..ParsedEntry::default()
        }],
    );

    // The one article with every markdown treatment in it.
    let ready = find(&db, "borrow checker");
    articles::put(
        &db,
        ready,
        &ArticleResult {
            status: ArticleStatus::Extracted,
            title: Some("Why the borrow checker says no: a field guide to lifetimes".into()),
            markdown: Some(crate::ui::markdown::FIXTURE_MD.to_string()),
            byline: Some("Jane Example".into()),
            site_name: Some("example.org".into()),
            image_url: None,
            excerpt: None,
            source_url: Some("https://example.org/posts/borrow-checker".into()),
            error: None,
            retry_in_secs: None,
        },
    )
    .expect("storing the fixture article");

    // An article with two pictures in it: one the replay directory answers
    // for, and one carried inline as a `data:` URI, which nothing fetches
    // and which the reader draws as its alt line.
    articles::put(
        &db,
        find(&db, "A post with pictures"),
        &ArticleResult {
            status: ArticleStatus::Extracted,
            title: Some("A post with pictures".into()),
            markdown: Some(PICTURES_MD.to_string()),
            byline: Some("A N Other".into()),
            site_name: Some("phoronix.com".into()),
            image_url: None,
            excerpt: None,
            source_url: Some("https://www.phoronix.com/news/two".into()),
            error: None,
            retry_in_secs: None,
        },
    )
    .expect("storing the picture article");

    // One whose page has not been fetched yet, and one that will not be.
    articles::put(&db, find(&db, "not arrived"), &pending()).expect("a pending article");
    articles::put(&db, find(&db, "paywall"), &failed("paywall")).expect("a failed article");

    // A video's description is its text; nothing is ever extracted for one.
    articles::put(
        &db,
        find(&db, "transistor"),
        &ArticleResult {
            status: ArticleStatus::NotApplicable,
            title: Some("How a transistor works".into()),
            markdown: Some(
                "A hundred years of solid-state physics in twelve minutes, with a \
                 demonstration on a bench and one very patient oscilloscope.\n"
                    .into(),
            ),
            byline: Some("Veritasium".into()),
            site_name: Some("youtube.com".into()),
            image_url: None,
            excerpt: None,
            source_url: Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()),
            error: None,
            retry_in_secs: None,
        },
    )
    .expect("a video description");

    // Three whose feed carried the whole text, which is what a full-text
    // feed looks like and what keeps `Pending` meaning "still coming".
    for (needle, text) in [
        (
            "Something already",
            "A short post, carried whole by its own feed.\n",
        ),
        ("And another", "Another short post, likewise.\n"),
        (
            "kernel release",
            "## What landed\n\nA scheduler change, two drivers and a filesystem fix.\n",
        ),
    ] {
        articles::put(&db, find(&db, needle), &feed_content(text)).expect("feed text");
    }

    // The picture article is read from the start, so that adding it left
    // every unread count in the frame snapshots where it was: what it is
    // there to exercise is the reader, and it is opened by name.
    let read = [
        find(&db, "Something already"),
        find(&db, "And another"),
        find(&db, "A post with pictures"),
    ];
    entries::set_read(&mut db, &read, true).expect("two read entries");
    entries::set_starred(&db, find(&db, "kernel release"), true).expect("a starred entry");

    db
}

/// An article with the two kinds of picture in it: one that can be fetched,
/// and one that never will be.
pub const PICTURES_MD: &str = "\
Two pictures, and some words to put them among.

![a kernel graph](https://example.org/pictures/hero.png)

The first is a real file in the replay directory; the second is carried in
the markdown itself, which nothing fetches.

![an inline diagram](data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==)

And a line after them both.
";

fn add(
    db: &Db,
    url: &str,
    kind: FeedKind,
    title: &str,
    folder: Option<crate::wire::feed::FolderId>,
) -> FeedId {
    let id = feeds::add(db, url, None, kind, Some(title), folder)
        .expect("adding a fixture feed")
        .id();
    feeds::record_ok(db, id, &feeds::Conditional::default(), Some(title), None)
        .expect("recording a fetch");
    id
}

fn store(db: &mut Db, feed: FeedId, kind: FeedKind, rows: &[ParsedEntry]) {
    let parsed = ParsedFeed {
        title: None,
        site_url: None,
        entries: rows.to_vec(),
    };
    entries::upsert_parsed(db, feed, kind, &parsed).expect("storing fixture entries");
}

fn entry(guid: &str, title: &str, url: Option<&str>, published: jiff::Timestamp) -> ParsedEntry {
    ParsedEntry {
        guid: guid.into(),
        url: url.map(str::to_string),
        title: title.into(),
        author: None,
        published: Some(published),
        ..ParsedEntry::default()
    }
}

fn pending() -> ArticleResult {
    ArticleResult {
        status: ArticleStatus::Pending,
        title: None,
        markdown: None,
        byline: None,
        site_name: None,
        image_url: None,
        excerpt: None,
        source_url: None,
        error: None,
        retry_in_secs: None,
    }
}

fn feed_content(markdown: &str) -> ArticleResult {
    ArticleResult {
        status: ArticleStatus::FeedContent,
        markdown: Some(markdown.to_string()),
        ..pending()
    }
}

fn failed(why: &str) -> ArticleResult {
    ArticleResult {
        status: ArticleStatus::Failed,
        error: Some(why.into()),
        ..pending()
    }
}

fn find(db: &Db, needle: &str) -> EntryId {
    entries::page(db, &Selection::All, false, 0, 500)
        .expect("paging the fixture")
        .rows
        .into_iter()
        .find(|e| e.title.contains(needle))
        .unwrap_or_else(|| panic!("no fixture entry whose title holds {needle:?}"))
        .id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::feed::EntryKind;
    use crate::wire::Command;

    #[test]
    fn the_fixture_has_one_of_everything_a_panel_can_draw() {
        let cfg = WireConfig::default();
        let (handle, mut fk) = handle(&cfg);
        assert_eq!(handle.state().feeds.len(), 4);
        assert_eq!(handle.state().folders.len(), 2);

        handle.send(Command::OpenFeed(Selection::All));
        fk.pump();
        let state = handle.state();
        let rows = &state.page.rows;
        assert_eq!(rows.len(), 8, "{rows:#?}");
        assert_eq!(rows.iter().filter(|r| r.read).count(), 3);
        assert_eq!(rows.iter().filter(|r| r.starred).count(), 1);
        assert_eq!(
            rows.iter().filter(|r| r.kind == EntryKind::Video).count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|r| r.article_status == ArticleStatus::Failed)
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|r| r.article_status == ArticleStatus::Pending)
                .count(),
            1
        );
    }

    /// The ages the plan's mocks show, which is what pinning the clock buys.
    #[test]
    fn the_pinned_clock_makes_the_ages_read_2h_and_1d() {
        let cfg = WireConfig::default();
        let (handle, mut fk) = handle(&cfg);
        handle.send(Command::OpenFeed(Selection::All));
        fk.pump();
        let state = handle.state();
        let row = state
            .page
            .rows
            .iter()
            .find(|r| r.title.contains("borrow checker"))
            .expect("the fixture article");
        assert_eq!(
            crate::ui::panels::entries::age(row.published.unwrap(), testing::now()),
            "2h"
        );
        let video = state
            .page
            .rows
            .iter()
            .find(|r| r.kind == EntryKind::Video)
            .expect("the fixture video");
        assert_eq!(
            crate::ui::panels::entries::age(video.published.unwrap(), testing::now()),
            "1d"
        );
    }

    #[test]
    fn opening_the_ready_article_yields_the_markdown_fixture() {
        let cfg = WireConfig::default();
        let (handle, mut fk) = handle(&cfg);
        handle.send(Command::OpenFeed(Selection::All));
        fk.pump();
        let id = fk.entry("borrow checker");
        handle.send(Command::OpenEntry(id));
        fk.pump();
        let article = handle.state().article.clone().expect("an article");
        assert_eq!(article.status, ArticleStatus::Extracted);
        assert!(
            article.markdown.contains("fn longest"),
            "{}",
            article.markdown
        );
    }

    /// The pending one stays pending: it has no link, so there is nothing
    /// for the core to queue an extraction against.
    #[test]
    fn the_pending_article_stays_pending_through_a_pump() {
        let cfg = WireConfig::default();
        let (handle, mut fk) = handle(&cfg);
        handle.send(Command::OpenFeed(Selection::All));
        fk.pump();
        let id = fk.entry("not arrived");
        handle.send(Command::OpenEntry(id));
        fk.pump();
        let article = handle.state().article.clone().expect("an article row");
        assert_eq!(article.status, ArticleStatus::Pending);
        assert!(article.markdown.is_empty());
    }

    /// The two kinds of picture an article carries, end to end through the
    /// real core: one the replay directory answers for, and one carried as
    /// a `data:` URI, which is refused where it is asked about rather than
    /// by a request that could only fail.
    #[test]
    fn the_picture_article_fetches_one_and_refuses_the_other() {
        use crate::wire::PictureState;

        let hero = "https://example.org/pictures/hero.png";
        let inline = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";
        assert!(PICTURES_MD.contains(hero), "the fixture carries it");
        assert!(PICTURES_MD.contains(inline));

        let cfg = WireConfig::default();
        let (handle, mut fk) = handle(&cfg);
        handle.send(Command::OpenFeed(Selection::All));
        fk.pump();
        let id = fk.entry("A post with pictures");
        handle.send(Command::OpenEntry(id));
        fk.pump();

        // What the reader's own draw pass would ask for, at the box a
        // picture of this size is given.
        for url in [hero, inline] {
            handle.send(Command::FetchPicture {
                url: url.to_string(),
                max_w: 640,
                max_h: 160,
            });
        }
        fk.pump();

        let state = handle.state();
        match state.pictures.get(hero) {
            Some(PictureState::Ready(picture)) => {
                assert_eq!(picture.natural, (64, 32));
                assert_eq!(picture.image.dimensions(), (64, 32), "never upscaled");
                assert_eq!(picture.path, None, "the fixture keeps nothing on disk");
            }
            other => panic!("the hero picture is {other:?}"),
        }
        match state.pictures.get(inline) {
            Some(PictureState::Failed(why)) => assert!(why.contains("not https"), "{why}"),
            other => panic!("the inline picture is {other:?}"),
        }
    }

    #[test]
    fn the_failed_one_says_why() {
        let cfg = WireConfig::default();
        let (handle, mut fk) = handle(&cfg);
        handle.send(Command::OpenFeed(Selection::All));
        fk.pump();
        let id = fk.entry("paywall");
        handle.send(Command::OpenEntry(id));
        fk.pump();
        let article = handle.state().article.clone().expect("an article row");
        assert_eq!(article.status, ArticleStatus::Failed);
        assert_eq!(article.error.as_deref(), Some("paywall"));
    }
}
