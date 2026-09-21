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
use crate::wire::net::Replay;

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
}
