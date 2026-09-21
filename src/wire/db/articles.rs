//! What was made of an entry: the text the reader shows, and the full-text
//! index over it.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use super::{now, stamp, Db};
use crate::wire::extract::{ArticleResult, Policy};
use crate::wire::feed::{ArticleStatus, ArticleView, EntryId, EntryRow, ParsedEntry};

/// Create the `article` row that belongs to a newly inserted entry.
///
/// Every entry has one from the moment it exists, rather than one appearing
/// on first open: `pending` is a scan of this table, and an entry with no row
/// here would never be offered to an extractor. For a video or a post the row
/// is complete on creation -- the feed's own text *is* the text, and there is
/// nothing to fetch.
///
/// **A row waiting to be extracted gets the feed's text too**, which is what
/// makes a failure degrade to something rather than to nothing. `put` writes
/// `COALESCE(excluded.markdown, article.markdown)`, so a failed extraction --
/// which carries no markdown of its own -- leaves this text in place and the
/// reader shows it under the reason. Every one of the eighty-five failures in
/// the reference database had text in the feed and none of it was on screen.
pub(super) fn create_pending(
    conn: &rusqlite::Connection,
    entry: EntryId,
    policy: Policy,
    parsed: &ParsedEntry,
) -> Result<()> {
    let markdown = parsed
        .content_html
        .as_deref()
        .map(|html| crate::wire::extract::from_feed_content(html, parsed.url.as_deref()))
        .filter(|md| !md.trim().is_empty());
    let status = match policy {
        Policy::Extract => ArticleStatus::Pending,
        Policy::Video => ArticleStatus::NotApplicable,
        Policy::FeedContent => ArticleStatus::FeedContent,
    };
    conn.execute(
        "INSERT INTO article(entry_id, status, title, markdown, byline, source_url, extracted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(entry_id) DO NOTHING",
        params![
            entry.0,
            status.as_i64(),
            parsed.title,
            markdown,
            parsed.author,
            parsed.url,
            if status.is_pending() {
                None
            } else {
                Some(now())
            },
        ],
    )?;
    Ok(())
}

/// Store what an extraction came to.
///
/// `attempts` is incremented rather than set, because the value that matters
/// is how many times this entry has cost a request: three is where
/// [`MAX_ATTEMPTS`] stops trying, and resetting it on each attempt would make
/// a page that fails in a new way every time cost requests for ever.
pub fn put(db: &Db, entry: EntryId, result: &ArticleResult) -> Result<()> {
    let attempts: i64 = db
        .conn
        .query_row(
            "SELECT attempts FROM article WHERE entry_id = ?1",
            [entry.0],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0)
        + 1;
    let retry_after = result
        .retry_in_secs
        .filter(|_| attempts < MAX_ATTEMPTS)
        .map(|base| now() + backoff_secs(base, attempts));

    db.conn.execute(
        "INSERT INTO article(entry_id, status, title, markdown, byline, site_name, image_url,
                             excerpt, source_url, extracted_at, attempts, error, retry_after)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?12)
         ON CONFLICT(entry_id) DO UPDATE SET
           status       = excluded.status,
           title        = COALESCE(excluded.title, article.title),
           markdown     = COALESCE(excluded.markdown, article.markdown),
           byline       = COALESCE(excluded.byline, article.byline),
           site_name    = COALESCE(excluded.site_name, article.site_name),
           image_url    = COALESCE(excluded.image_url, article.image_url),
           excerpt      = COALESCE(excluded.excerpt, article.excerpt),
           source_url   = COALESCE(excluded.source_url, article.source_url),
           extracted_at = excluded.extracted_at,
           attempts     = article.attempts + 1,
           error        = excluded.error,
           retry_after  = excluded.retry_after",
        params![
            entry.0,
            result.status.as_i64(),
            result.title,
            result.markdown,
            result.byline,
            result.site_name,
            result.image_url,
            result.excerpt,
            result.source_url,
            now(),
            result.error,
            retry_after,
        ],
    )?;
    Ok(())
}

/// How long a transient failure waits, doubling per attempt already spent.
///
/// The same shape as the per-feed backoff in `db::feeds`, and capped the same
/// way: whatever the base, a third attempt is never more than a day out. The
/// cap matters because the base is a configured number and somebody's
/// `refresh_minutes` is a day.
pub fn backoff_secs(base: i64, attempts: i64) -> i64 {
    const DAY: i64 = 24 * 60 * 60;
    let shift = attempts.clamp(1, 16) as u32;
    base.max(1)
        .saturating_mul(1i64.checked_shl(shift).unwrap_or(i64::MAX))
        .clamp(60, DAY)
}

/// One entry's text, as the reader shows it.
pub fn get(db: &Db, entry: EntryId) -> Result<Option<ArticleView>> {
    Ok(db
        .conn
        .query_row(
            "SELECT a.status, COALESCE(a.title, e.title), a.markdown, a.byline, a.site_name,
                    COALESCE(a.source_url, e.url), a.image_url, a.extracted_at, a.error
             FROM article a JOIN entry e ON e.id = a.entry_id
             WHERE a.entry_id = ?1",
            [entry.0],
            |r| {
                Ok(ArticleView {
                    entry,
                    status: ArticleStatus::from_i64(r.get(0)?),
                    title: r.get(1)?,
                    markdown: std::sync::Arc::from(
                        r.get::<_, Option<String>>(2)?.unwrap_or_default().as_str(),
                    ),
                    byline: r.get(3)?,
                    site_name: r.get(4)?,
                    url: r.get(5)?,
                    image_url: r.get(6)?,
                    extracted_at: stamp(r.get(7)?),
                    error: r.get(8)?,
                })
            },
        )
        .optional()?)
}

/// The most attempts one entry costs before it is left alone.
///
/// Three, and not configurable: a page that has failed three times is a
/// paywall, a login wall or a site that does not want to be read by this,
/// and the answer to all three is `o` rather than a fourth request. It is a
/// real ceiling as of 0.0.2 -- until the retry classes below existed, nothing
/// but `e` ever offered a failed row again and this number never came up.
pub const MAX_ATTEMPTS: i64 = 3;

/// The rows the extraction queue is allowed to take.
///
/// Two kinds: a page nothing has been tried on, and a failure whose reason
/// was a fact about today rather than about the page -- a 429, a 5xx or a
/// timeout, which `extract::run` marks by giving the result a
/// `retry_in_secs` and `put` turns into the moment named here. Everything
/// else stays where it is: retrying a 403 hourly costs a request and helps
/// nobody, and `e` in the reader is there for the case where somebody
/// disagrees.
const QUEUE_WHERE: &str = "a.attempts < ?1 AND e.url IS NOT NULL
     AND (a.status = 0
          OR (a.status = 3 AND a.retry_after IS NOT NULL AND a.retry_after <= ?2))";

/// Entries whose page is still worth fetching, newest first.
///
/// Newest first because that is what the reader is about to open. An
/// extraction queue that worked through the backlog oldest-first would spend
/// its first minutes on last week's articles while the ones just fetched sat
/// pending.
pub fn pending(db: &Db, limit: usize) -> Result<Vec<(EntryId, String)>> {
    let sql = format!(
        "SELECT a.entry_id, e.url FROM article a
         JOIN entry e ON e.id = a.entry_id
         WHERE {QUEUE_WHERE}
         ORDER BY COALESCE(e.published, e.fetched_at) DESC, e.id DESC
         LIMIT ?3"
    );
    let mut stmt = db.conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![MAX_ATTEMPTS, now(), limit as i64], |r| {
            Ok((EntryId(r.get(0)?), r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// How many entries are still waiting to be extracted, for the status line.
pub fn pending_count(db: &Db) -> Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) FROM article a JOIN entry e ON e.id = a.entry_id
         WHERE {QUEUE_WHERE}"
    );
    Ok(db
        .conn
        .query_row(&sql, params![MAX_ATTEMPTS, now()], |r| r.get(0))?)
}

/// Turn what somebody typed into an FTS5 query.
///
/// Every token is quoted as a phrase, which is the whole point: FTS5's query
/// language treats `AND`, `OR`, `NOT`, `NEAR`, `*`, `^`, `:` and a bare `"`
/// as syntax, and a search for `C++` or for `"` would otherwise be a syntax
/// error rather than a search. Quoting also means a search for `rust AND
/// borrow` looks for those three words, which is what someone typing it into
/// a search box meant.
///
/// The tokens are joined by implicit AND -- FTS5's default -- so every word
/// has to appear somewhere in the title or the body.
pub fn fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Full-text search over stored articles, best match first.
///
/// `bm25` rather than insertion order: a word in a title should outrank the
/// same word four thousand words into a different article, and the ranking
/// function is the only thing that knows that. The weights favour the title
/// ten to one.
pub fn search(db: &Db, query: &str, limit: usize) -> Result<Vec<EntryRow>> {
    let q = fts_query(query);
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT e.id, e.feed_id, COALESCE(f.custom_title, f.title, f.url),
                e.title, e.author, e.url, e.published, e.kind, e.read, e.starred,
                COALESCE(a.status, 0), e.thumbnail_url
         FROM article_fts
         JOIN entry e ON e.id = article_fts.rowid
         JOIN feed f ON f.id = e.feed_id
         LEFT JOIN article a ON a.entry_id = e.id
         WHERE article_fts MATCH ?1
         ORDER BY bm25(article_fts, 10.0, 1.0)
         LIMIT {limit}"
    );
    let mut stmt = match db.conn.prepare(&sql) {
        Ok(stmt) => stmt,
        Err(_) => return Ok(Vec::new()),
    };
    let rows = match stmt.query_map([&q], super::entries::row_to_entry) {
        Ok(rows) => rows.collect::<rusqlite::Result<Vec<_>>>(),
        Err(_) => return Ok(Vec::new()),
    };
    Ok(rows.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::db::{entries, feeds};
    use crate::wire::feed::{FeedKind, ParsedFeed, Selection};

    fn seeded() -> Db {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(
            &db,
            "https://e.org/f",
            None,
            FeedKind::Web,
            Some("Feed"),
            None,
        )
        .unwrap()
        .id();
        entries::upsert_parsed(
            &mut db,
            feed,
            FeedKind::Web,
            &ParsedFeed {
                entries: vec![
                    ParsedEntry {
                        guid: "a".into(),
                        url: Some("https://e.org/a".into()),
                        title: "Lifetimes, a field guide".into(),
                        ..ParsedEntry::default()
                    },
                    ParsedEntry {
                        guid: "b".into(),
                        url: Some("https://e.org/b".into()),
                        title: "Something else entirely".into(),
                        ..ParsedEntry::default()
                    },
                ],
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        db
    }

    fn first(db: &Db) -> EntryId {
        entries::page(db, &Selection::All, false, 0, 1)
            .unwrap()
            .rows[0]
            .id
    }

    #[test]
    fn an_entry_is_pending_until_something_is_put_for_it() {
        let db = seeded();
        assert_eq!(pending_count(&db).unwrap(), 2);
        let id = first(&db);
        let view = get(&db, id).unwrap().unwrap();
        assert_eq!(view.status, ArticleStatus::Pending);
        assert!(view.markdown.is_empty());

        put(
            &db,
            id,
            &ArticleResult {
                status: ArticleStatus::Extracted,
                title: Some("A title".into()),
                markdown: Some("# A title\n\nBody.".into()),
                ..ArticleResult::default()
            },
        )
        .unwrap();
        let view = get(&db, id).unwrap().unwrap();
        assert_eq!(view.status, ArticleStatus::Extracted);
        assert!(view.markdown.contains("Body."));
        assert!(view.extracted_at.is_some());
        assert_eq!(pending_count(&db).unwrap(), 1);
    }

    /// The whole of WP-7's first item, end to end: an entry whose page has
    /// not been fetched yet is already readable, and a failure leaves that
    /// text where it was instead of emptying the row.
    #[test]
    fn a_failed_extraction_falls_back_to_the_text_the_feed_carried() {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        entries::upsert_parsed(
            &mut db,
            feed,
            FeedKind::Web,
            &ParsedFeed {
                entries: vec![ParsedEntry {
                    guid: "a".into(),
                    url: Some("https://e.org/a".into()),
                    title: "Behind a wall".into(),
                    content_html: Some("<p>Two sentences, and a link to the rest.</p>".into()),
                    ..ParsedEntry::default()
                }],
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        let id = first(&db);

        let waiting = get(&db, id).unwrap().unwrap();
        assert_eq!(waiting.status, ArticleStatus::Pending);
        assert!(
            waiting.markdown.contains("Two sentences"),
            "an entry is readable before anything is fetched: {:?}",
            waiting.markdown
        );

        put(
            &db,
            id,
            &ArticleResult {
                status: ArticleStatus::Failed,
                error: Some("the site answered 403".into()),
                ..ArticleResult::default()
            },
        )
        .unwrap();
        let failed = get(&db, id).unwrap().unwrap();
        assert_eq!(failed.status, ArticleStatus::Failed);
        assert!(
            failed.markdown.contains("Two sentences"),
            "the failure emptied the row: {:?}",
            failed.markdown
        );
        assert_eq!(failed.error.as_deref(), Some("the site answered 403"));
    }

    #[test]
    fn attempts_accumulate_so_a_hopeless_page_stops_costing_requests() {
        let db = seeded();
        let id = first(&db);
        for _ in 0..MAX_ATTEMPTS {
            put(
                &db,
                id,
                &ArticleResult {
                    status: ArticleStatus::Pending,
                    error: Some("403".into()),
                    ..ArticleResult::default()
                },
            )
            .unwrap();
        }
        let left = pending(&db, 10).unwrap();
        assert!(
            !left.iter().any(|(e, _)| *e == id),
            "three failures and it is left alone"
        );
    }

    /// The retry classes, which is the whole of WP-7's second item: a 429
    /// comes back, a 403 does not, and neither costs a request before its
    /// time.
    #[test]
    fn a_transient_failure_comes_round_again_and_a_permanent_one_does_not() {
        let db = seeded();
        let rows = entries::page(&db, &Selection::All, false, 0, 10)
            .unwrap()
            .rows;
        let (transient, permanent) = (rows[0].id, rows[1].id);

        put(
            &db,
            transient,
            &ArticleResult {
                status: ArticleStatus::Failed,
                error: Some("the site answered 429".into()),
                retry_in_secs: Some(900),
                ..ArticleResult::default()
            },
        )
        .unwrap();
        put(
            &db,
            permanent,
            &ArticleResult {
                status: ArticleStatus::Failed,
                error: Some("the site answered 403".into()),
                ..ArticleResult::default()
            },
        )
        .unwrap();

        // Neither is offered yet: the 429 has a delay to sit out and the 403
        // has nothing to wait for.
        let queued: Vec<EntryId> = pending(&db, 10)
            .unwrap()
            .into_iter()
            .map(|(e, _)| e)
            .collect();
        assert!(queued.is_empty(), "{queued:?}");

        // With its delay passed, the 429 rejoins the queue and the 403 stays
        // where it is.
        db.conn
            .execute(
                "UPDATE article SET retry_after = ?2 WHERE entry_id = ?1",
                params![transient.0, now() - 1],
            )
            .unwrap();
        let queued: Vec<EntryId> = pending(&db, 10)
            .unwrap()
            .into_iter()
            .map(|(e, _)| e)
            .collect();
        assert_eq!(queued, vec![transient]);
        assert_eq!(pending_count(&db).unwrap(), 1);
    }

    #[test]
    fn a_transient_failure_stops_being_retried_at_the_attempt_ceiling() {
        let db = seeded();
        let id = first(&db);
        for _ in 0..MAX_ATTEMPTS {
            put(
                &db,
                id,
                &ArticleResult {
                    status: ArticleStatus::Failed,
                    error: Some("timeout: global".into()),
                    retry_in_secs: Some(900),
                    ..ArticleResult::default()
                },
            )
            .unwrap();
        }
        let retry: Option<i64> = db
            .conn
            .query_row(
                "SELECT retry_after FROM article WHERE entry_id = ?1",
                [id.0],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(retry, None, "three attempts is where it is left alone");
        assert!(
            !pending(&db, 10).unwrap().iter().any(|(e, _)| *e == id),
            "and it is not offered again"
        );
    }

    #[test]
    fn the_delay_doubles_per_attempt_and_stops_at_a_day() {
        assert_eq!(backoff_secs(900, 1), 1800);
        assert_eq!(backoff_secs(900, 2), 3600);
        assert_eq!(backoff_secs(900, 3), 7200);
        assert_eq!(backoff_secs(900, 20), 24 * 60 * 60);
        assert_eq!(backoff_secs(0, 1), 60, "a zero base is still a minute");
        assert_eq!(backoff_secs(i64::MAX, 1), 24 * 60 * 60);
    }

    #[test]
    fn pending_offers_the_newest_first() {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        entries::upsert_parsed(
            &mut db,
            feed,
            FeedKind::Web,
            &ParsedFeed {
                entries: (1..=3)
                    .map(|n| ParsedEntry {
                        guid: format!("g{n}"),
                        url: Some(format!("https://e.org/{n}")),
                        title: format!("Entry {n}"),
                        published: Some(jiff::Timestamp::from_second(1_800_000_000 + n).unwrap()),
                        ..ParsedEntry::default()
                    })
                    .collect(),
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        let urls: Vec<String> = pending(&db, 10)
            .unwrap()
            .into_iter()
            .map(|(_, u)| u)
            .collect();
        assert_eq!(urls[0], "https://e.org/3");
    }

    #[test]
    fn an_entry_with_no_link_is_never_offered_for_extraction() {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        entries::upsert_parsed(
            &mut db,
            feed,
            FeedKind::Web,
            &ParsedFeed {
                entries: vec![ParsedEntry {
                    guid: "no-link".into(),
                    title: "Linkless".into(),
                    ..ParsedEntry::default()
                }],
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        assert!(pending(&db, 10).unwrap().is_empty());
    }

    #[test]
    fn a_video_is_complete_the_moment_it_is_stored() {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(
            &db,
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCa",
            None,
            FeedKind::Youtube,
            None,
            None,
        )
        .unwrap()
        .id();
        entries::upsert_parsed(
            &mut db,
            feed,
            FeedKind::Youtube,
            &ParsedFeed {
                entries: vec![ParsedEntry {
                    guid: "v".into(),
                    url: Some("https://www.youtube.com/watch?v=abc".into()),
                    title: "A video".into(),
                    content_html: Some("What it is about.".into()),
                    video_id: Some("abc".into()),
                    ..ParsedEntry::default()
                }],
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        let id = first(&db);
        let view = get(&db, id).unwrap().unwrap();
        assert_eq!(view.status, ArticleStatus::NotApplicable);
        assert!(view.markdown.contains("What it is about."));
        assert_eq!(pending_count(&db).unwrap(), 0, "a video is never fetched");
    }

    #[test]
    fn the_index_follows_what_is_stored() {
        let db = seeded();
        let id = first(&db);
        assert!(search(&db, "borrow", 10).unwrap().is_empty());
        put(
            &db,
            id,
            &ArticleResult {
                status: ArticleStatus::Extracted,
                title: Some("Lifetimes, a field guide".into()),
                markdown: Some("The borrow checker is a patient reviewer.".into()),
                ..ArticleResult::default()
            },
        )
        .unwrap();
        let hits = search(&db, "borrow", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, id);

        // And follows a correction: the trigger has to remove the old tokens
        // as well as add the new ones, which is the part an external-content
        // table gets wrong when the delete trigger is missing.
        put(
            &db,
            id,
            &ArticleResult {
                status: ArticleStatus::Extracted,
                title: Some("Lifetimes, a field guide".into()),
                markdown: Some("Entirely different words now.".into()),
                ..ArticleResult::default()
            },
        )
        .unwrap();
        assert!(
            search(&db, "borrow", 10).unwrap().is_empty(),
            "the old text is out of the index"
        );
        assert_eq!(search(&db, "different", 10).unwrap().len(), 1);
    }

    #[test]
    fn a_removed_entry_leaves_nothing_in_the_index() {
        let db = seeded();
        let id = first(&db);
        put(
            &db,
            id,
            &ArticleResult {
                status: ArticleStatus::Extracted,
                markdown: Some("findable".into()),
                ..ArticleResult::default()
            },
        )
        .unwrap();
        assert_eq!(search(&db, "findable", 10).unwrap().len(), 1);
        db.conn
            .execute("DELETE FROM entry WHERE id = ?1", [id.0])
            .unwrap();
        assert!(search(&db, "findable", 10).unwrap().is_empty());
    }

    #[test]
    fn a_title_outranks_a_body() {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        entries::upsert_parsed(
            &mut db,
            feed,
            FeedKind::Web,
            &ParsedFeed {
                entries: (1..=2)
                    .map(|n| ParsedEntry {
                        guid: format!("g{n}"),
                        url: Some(format!("https://e.org/{n}")),
                        title: format!("Entry {n}"),
                        ..ParsedEntry::default()
                    })
                    .collect(),
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        let rows = entries::page(&db, &Selection::All, false, 0, 10)
            .unwrap()
            .rows;
        put(
            &db,
            rows[0].id,
            &ArticleResult {
                status: ArticleStatus::Extracted,
                title: Some("Nothing to see".into()),
                markdown: Some(format!("{} kestrel", "filler ".repeat(500))),
                ..ArticleResult::default()
            },
        )
        .unwrap();
        put(
            &db,
            rows[1].id,
            &ArticleResult {
                status: ArticleStatus::Extracted,
                title: Some("Kestrel".into()),
                markdown: Some("Short.".into()),
                ..ArticleResult::default()
            },
        )
        .unwrap();
        let hits = search(&db, "kestrel", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, rows[1].id, "the one with it in the title wins");
    }

    #[test]
    fn a_query_that_is_punctuation_is_a_search_and_not_a_syntax_error() {
        let db = seeded();
        // Every one of these is FTS5 syntax if it is not quoted.
        for q in ["AND", "OR", "NOT", "NEAR", "*", "\"", "C++", "a:b", "^x"] {
            let out = search(&db, q, 10);
            assert!(out.is_ok(), "{q} raised {:?}", out.err());
        }
        assert!(search(&db, "   ", 10).unwrap().is_empty());
    }

    proptest::proptest! {
        /// Whatever somebody types into the search box, the query it becomes
        /// is one FTS5 can parse. Run against a real index rather than by
        /// reading the string, because the only authority on what FTS5
        /// accepts is FTS5.
        #[test]
        fn any_typed_query_is_a_query_fts_accepts(s: String) {
            let db = Db::open_in_memory().unwrap();
            let q = fts_query(&s);
            if q.is_empty() {
                return Ok(());
            }
            let out: rusqlite::Result<i64> = db.conn.query_row(
                "SELECT COUNT(*) FROM article_fts WHERE article_fts MATCH ?1",
                [&q],
                |r| r.get(0),
            );
            proptest::prop_assert!(out.is_ok(), "{:?} from {:?}", out.err(), q);
        }
    }
}
