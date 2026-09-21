//! What arrived: storing a parsed feed, paging the list, the three read-state
//! writes, and the sweep that stops the file growing without end.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use super::{articles, now, stamp, Db};
use crate::wire::feed::{
    ArticleStatus, EntryId, EntryKind, EntryRow, FeedId, FeedKind, FolderId, ParsedEntry,
    ParsedFeed, Selection,
};

/// One page of the ENTRIES list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub rows: Vec<EntryRow>,
    /// How many rows the selection has in total, which is what the status
    /// line's `n/len` counts and what tells `MoreEntries` when to stop.
    pub total: i64,
}

/// What storing one fetched feed came to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stored {
    /// Entries that were not in the table before. The extraction queue is
    /// fed from exactly this list, which is why a refetch of an unchanged
    /// feed costs no scraping at all.
    pub new: Vec<EntryId>,
    /// Of those, the ones an extractor should look at -- articles with a URL,
    /// not videos or posts.
    pub extractable: Vec<(EntryId, String)>,
    /// How many of the new ones were marked read on the way in by an
    /// imported newsboat read mark.
    pub marked_read: usize,
}

/// Store what a fetch parsed.
///
/// Idempotent on `(feed_id, guid)`: a feed refetched five minutes later
/// upserts every row it still carries and inserts nothing. That is what
/// makes two writers -- an open window and a `starwire fetch` from a timer --
/// safe against each other without a lock beyond SQLite's own.
///
/// The update deliberately does **not** touch `read` or `starred`. A feed
/// that revises an entry's title in place must not un-read it; the only
/// things a later fetch is allowed to correct are the text and the metadata.
pub fn upsert_parsed(
    db: &mut Db,
    feed_id: FeedId,
    kind: FeedKind,
    parsed: &ParsedFeed,
) -> Result<Stored> {
    let tx = db.conn.transaction()?;
    let at = now();
    let mut out = Stored::default();

    // The feed's own URL, for matching newsboat's read marks, which are keyed
    // by the URL that was in the `urls` file.
    let feed_urls: Vec<String> = {
        let mut stmt = tx.prepare("SELECT url, source_url FROM feed WHERE id = ?1")?;
        let row = stmt
            .query_row([feed_id.0], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .optional()?;
        match row {
            Some((url, source)) => std::iter::once(url).chain(source).collect(),
            None => Vec::new(),
        }
    };

    for entry in &parsed.entries {
        if entry.guid.is_empty() {
            continue;
        }
        let entry_kind = classify(kind, entry);
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM entry WHERE feed_id = ?1 AND guid = ?2",
                params![feed_id.0, entry.guid],
                |r| r.get(0),
            )
            .optional()?;

        if let Some(id) = existing {
            tx.execute(
                "UPDATE entry SET url = ?2, title = ?3, author = ?4, kind = ?5,
                                  published = COALESCE(?6, published),
                                  content_html = COALESCE(?7, content_html),
                                  thumbnail_url = COALESCE(?8, thumbnail_url),
                                  video_id = COALESCE(?9, video_id),
                                  duration_secs = COALESCE(?10, duration_secs)
                 WHERE id = ?1",
                params![
                    id,
                    entry.url,
                    entry.title,
                    entry.author,
                    entry_kind.as_i64(),
                    entry.published.map(|t| t.as_second()),
                    entry.content_html,
                    entry.thumbnail_url,
                    entry.video_id,
                    entry.duration_secs,
                ],
            )?;
            continue;
        }

        // A newsboat read mark waiting for this entry. Matched on guid within
        // the feed's own URLs rather than globally: two feeds can carry the
        // same item, and marking one read should not mark the other.
        let mut read = 0i64;
        for feed_url in &feed_urls {
            let hit: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM imported_read WHERE feed_url = ?1 AND guid = ?2",
                    params![feed_url, entry.guid],
                    |r| r.get(0),
                )
                .optional()?;
            if hit.is_some() {
                tx.execute(
                    "DELETE FROM imported_read WHERE feed_url = ?1 AND guid = ?2",
                    params![feed_url, entry.guid],
                )?;
                read = 1;
                break;
            }
        }

        tx.execute(
            "INSERT INTO entry(feed_id, guid, url, title, author, kind, published, fetched_at,
                               content_html, thumbnail_url, video_id, duration_secs, read)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                feed_id.0,
                entry.guid,
                entry.url,
                entry.title,
                entry.author,
                entry_kind.as_i64(),
                entry.published.map(|t| t.as_second()),
                at,
                entry.content_html,
                entry.thumbnail_url,
                entry.video_id,
                entry.duration_secs,
                read,
            ],
        )?;
        let id = EntryId(tx.last_insert_rowid());
        if read == 1 {
            out.marked_read += 1;
        }

        // The article row is created with the entry, not on first open. It
        // is what `pending(limit)` scans, and an entry with no article row
        // would be invisible to the extractor for ever.
        let policy = crate::wire::extract::policy(kind, entry_kind, entry.url.as_deref());
        articles::create_pending(&tx, id, policy, entry)?;
        if let (true, Some(url)) = (policy.extracts(), entry.url.clone()) {
            out.extractable.push((id, url));
        }
        out.new.push(id);
    }

    tx.commit()?;
    Ok(out)
}

/// What kind of thing an entry is, from the feed it came out of and the link
/// it carries.
///
/// A YouTube feed's entries are videos; so is a link to YouTube from
/// anywhere else, which is why this looks at the URL and not only at the
/// feed. A Reddit feed's entries are posts. Everything else is an article
/// until its link says otherwise.
pub fn classify(feed: FeedKind, entry: &ParsedEntry) -> EntryKind {
    if entry.video_id.is_some() {
        return EntryKind::Video;
    }
    match feed {
        FeedKind::Youtube => EntryKind::Video,
        FeedKind::Reddit => EntryKind::Post,
        FeedKind::Web | FeedKind::Hn => EntryKind::Article,
    }
}

const SELECT_ROW: &str = "SELECT e.id, e.feed_id, COALESCE(f.custom_title, f.title, f.url),
       e.title, e.author, e.url, e.published, e.kind, e.read, e.starred,
       COALESCE(a.status, 0), e.thumbnail_url
FROM entry e
JOIN feed f ON f.id = e.feed_id
LEFT JOIN article a ON a.entry_id = e.id";

pub(super) fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<EntryRow> {
    Ok(EntryRow {
        id: EntryId(r.get(0)?),
        feed_id: FeedId(r.get(1)?),
        feed_title: r.get(2)?,
        title: r.get(3)?,
        author: r.get(4)?,
        url: r.get(5)?,
        published: stamp(r.get(6)?),
        kind: EntryKind::from_i64(r.get(7)?),
        read: r.get::<_, i64>(8)? != 0,
        starred: r.get::<_, i64>(9)? != 0,
        article_status: ArticleStatus::from_i64(r.get(10)?),
        thumbnail_url: r.get(11)?,
    })
}

/// The `WHERE` a selection comes to, and its one bound parameter.
///
/// Returned as a pair rather than interpolated, because the one selection
/// that carries text from a person -- `Search` -- is exactly the one that
/// must not be pasted into SQL.
fn selection_where(sel: &Selection) -> (String, Option<String>) {
    match sel {
        Selection::All => ("1 = 1".into(), None),
        Selection::Feed(id) => (format!("e.feed_id = {}", id.0), None),
        Selection::Folder(id) => (format!("f.folder_id = {}", id.0), None),
        Selection::Starred => ("e.starred = 1".into(), None),
        Selection::Videos => (
            format!("e.kind = {}", EntryKind::Video.as_i64()),
            None,
        ),
        Selection::Search(q) => (
            "e.id IN (SELECT rowid FROM article_fts WHERE article_fts MATCH ?1)".into(),
            Some(articles::fts_query(q)),
        ),
    }
}

/// One page of a selection, newest first.
///
/// Ordered by the published date where there is one and by when the entry
/// was first seen where there is not, then by id: an undated feed still
/// comes out in a stable order, and two entries published in the same second
/// do not swap places between pages.
pub fn page(
    db: &Db,
    sel: &Selection,
    unread_only: bool,
    offset: usize,
    limit: usize,
) -> Result<Page> {
    let (clause, bind) = selection_where(sel);
    // A search for something nothing matches is a valid selection with no
    // rows, not an error; an unparseable FTS query is the same.
    let unread = if unread_only { " AND e.read = 0" } else { "" };

    let total_sql = format!(
        "SELECT COUNT(*) FROM entry e JOIN feed f ON f.id = e.feed_id WHERE {clause}{unread}"
    );
    let rows_sql = format!(
        "{SELECT_ROW} WHERE {clause}{unread}
         ORDER BY COALESCE(e.published, e.fetched_at) DESC, e.id DESC
         LIMIT {limit} OFFSET {offset}"
    );

    let (total, rows) = match &bind {
        Some(q) => {
            let total: i64 = match db.conn.query_row(&total_sql, [q], |r| r.get(0)) {
                Ok(n) => n,
                Err(e) if is_fts_syntax_error(&e) => return Ok(Page::default()),
                Err(e) => return Err(e.into()),
            };
            let mut stmt = db.conn.prepare(&rows_sql)?;
            let rows = stmt
                .query_map([q], row_to_entry)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (total, rows)
        }
        None => {
            let total: i64 = db.conn.query_row(&total_sql, [], |r| r.get(0))?;
            let mut stmt = db.conn.prepare(&rows_sql)?;
            let rows = stmt
                .query_map([], row_to_entry)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (total, rows)
        }
    };
    Ok(Page { rows, total })
}

/// FTS5 raises a plain `SQLITE_ERROR` for a query it cannot parse. The
/// message is the only thing that distinguishes it from a real failure, and
/// a person typing `/` and then `"` should get an empty list rather than a
/// backtrace.
fn is_fts_syntax_error(e: &rusqlite::Error) -> bool {
    let text = e.to_string();
    text.contains("fts5") || text.contains("MATCH")
}

pub fn get(db: &Db, id: EntryId) -> Result<Option<EntryRow>> {
    let sql = format!("{SELECT_ROW} WHERE e.id = ?1");
    Ok(db.conn.query_row(&sql, [id.0], row_to_entry).optional()?)
}

/// The feed's own text for an entry, which is what extraction falls back to.
pub fn content_html(db: &Db, id: EntryId) -> Result<Option<String>> {
    Ok(db
        .conn
        .query_row("SELECT content_html FROM entry WHERE id = ?1", [id.0], |r| {
            r.get(0)
        })
        .optional()?
        .flatten())
}

pub fn set_read(db: &mut Db, ids: &[EntryId], read: bool) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let tx = db.conn.transaction()?;
    let mut changed = 0;
    {
        let mut stmt = tx.prepare("UPDATE entry SET read = ?2 WHERE id = ?1")?;
        for id in ids {
            changed += stmt.execute(params![id.0, i64::from(read)])?;
        }
    }
    tx.commit()?;
    Ok(changed)
}

pub fn set_starred(db: &Db, id: EntryId, starred: bool) -> Result<()> {
    db.conn.execute(
        "UPDATE entry SET starred = ?2 WHERE id = ?1",
        params![id.0, i64::from(starred)],
    )?;
    Ok(())
}

/// Mark everything in a selection read. The one bulk write in the program,
/// and the reason `A` asks first.
pub fn mark_all_read(db: &Db, sel: &Selection) -> Result<usize> {
    let (clause, bind) = selection_where(sel);
    let sql = format!(
        "UPDATE entry SET read = 1 WHERE read = 0 AND id IN
         (SELECT e.id FROM entry e JOIN feed f ON f.id = e.feed_id WHERE {clause})"
    );
    Ok(match bind {
        Some(q) => db.conn.execute(&sql, [q])?,
        None => db.conn.execute(&sql, [])?,
    })
}

pub fn unread_count(db: &Db, sel: &Selection) -> Result<i64> {
    let (clause, bind) = selection_where(sel);
    let sql = format!(
        "SELECT COUNT(*) FROM entry e JOIN feed f ON f.id = e.feed_id
         WHERE e.read = 0 AND {clause}"
    );
    Ok(match bind {
        Some(q) => db.conn.query_row(&sql, [q], |r| r.get(0))?,
        None => db.conn.query_row(&sql, [], |r| r.get(0))?,
    })
}

/// What a retention sweep removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retained {
    pub by_age: usize,
    pub by_count: usize,
    pub read_marks: usize,
}

/// Two bounds, because one is not enough.
///
/// `keep_days` alone leaves a month of `hnrss/newcomments` in the file, which
/// is about a hundred thousand rows; `max_entries_per_feed` alone keeps a
/// quiet blog's entire history for ever. Starred entries survive both --
/// starring something is the one way a reader says "keep this", and a
/// retention policy that ignored it would be a data-loss bug rather than a
/// tidying one.
pub fn retain(db: &mut Db, keep_days: u32, max_per_feed: usize) -> Result<Retained> {
    let tx = db.conn.transaction()?;
    let cutoff = now() - (keep_days as i64) * 24 * 60 * 60;

    let by_age = tx.execute(
        "DELETE FROM entry
         WHERE starred = 0 AND COALESCE(published, fetched_at) < ?1",
        [cutoff],
    )?;

    let by_count = if max_per_feed > 0 {
        tx.execute(
            "DELETE FROM entry WHERE id IN (
               SELECT id FROM (
                 SELECT id, ROW_NUMBER() OVER (
                   PARTITION BY feed_id
                   ORDER BY COALESCE(published, fetched_at) DESC, id DESC
                 ) AS rank
                 FROM entry WHERE starred = 0
               ) WHERE rank > ?1
             )",
            [max_per_feed as i64],
        )?
    } else {
        0
    };

    // Read marks for entries that never turned up. Swept on the same
    // schedule rather than kept for ever: a guid that has not appeared in a
    // month's worth of fetches is one whose entry newsboat saw and this
    // program never will.
    let read_marks = tx.execute(
        "DELETE FROM imported_read WHERE rowid IN (
           SELECT ir.rowid FROM imported_read ir
           LEFT JOIN feed f ON f.url = ir.feed_url OR f.source_url = ir.feed_url
           WHERE f.id IS NULL
         )",
        [],
    )?;

    tx.commit()?;
    Ok(Retained {
        by_age,
        by_count,
        read_marks,
    })
}

/// Hold a newsboat read mark until its entry arrives.
pub fn put_imported_read(db: &rusqlite::Connection, feed_url: &str, guid: &str, url: Option<&str>) -> Result<()> {
    db.execute(
        "INSERT INTO imported_read(feed_url, guid, url) VALUES (?1, ?2, ?3)
         ON CONFLICT(feed_url, guid) DO NOTHING",
        params![feed_url, guid, url],
    )?;
    Ok(())
}

/// Apply the read marks that were imported before the entries they name.
///
/// Called once after an import, for the case the entries were already in the
/// table -- a second import over a list that has been fetched since.
pub fn apply_imported_read(db: &Db) -> Result<usize> {
    Ok(db.conn.execute(
        "UPDATE entry SET read = 1 WHERE read = 0 AND id IN (
           SELECT e.id FROM entry e
           JOIN feed f ON f.id = e.feed_id
           JOIN imported_read ir
             ON (ir.feed_url = f.url OR ir.feed_url = f.source_url) AND ir.guid = e.guid
         )",
        [],
    )?)
}

/// Whether a folder holds any feed at all, which is what stops the source
/// list drawing an empty row for one the last feed just left.
pub fn folder_is_empty(db: &Db, id: FolderId) -> Result<bool> {
    let n: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM feed WHERE folder_id = ?1",
        [id.0],
        |r| r.get(0),
    )?;
    Ok(n == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::db::feeds;
    use crate::wire::feed::ParsedEntry;

    fn seeded() -> (Db, FeedId) {
        let mut db = Db::open_in_memory().unwrap();
        let id = feeds::add(&db, "https://e.org/f", None, FeedKind::Web, Some("Feed"), None)
            .unwrap()
            .id();
        let parsed = ParsedFeed {
            title: Some("Feed".into()),
            site_url: None,
            entries: (1..=3)
                .map(|n| ParsedEntry {
                    guid: format!("g{n}"),
                    url: Some(format!("https://e.org/{n}")),
                    title: format!("Entry {n}"),
                    published: Some(jiff::Timestamp::from_second(1_800_000_000 + n).unwrap()),
                    content_html: Some(format!("<p>Body {n}</p>")),
                    ..ParsedEntry::default()
                })
                .collect(),
        };
        upsert_parsed(&mut db, id, FeedKind::Web, &parsed).unwrap();
        (db, id)
    }

    #[test]
    fn storing_a_feed_twice_inserts_nothing_the_second_time() {
        let (mut db, id) = seeded();
        let parsed = ParsedFeed {
            entries: vec![ParsedEntry {
                guid: "g1".into(),
                title: "Entry 1".into(),
                ..ParsedEntry::default()
            }],
            ..ParsedFeed::default()
        };
        let stored = upsert_parsed(&mut db, id, FeedKind::Web, &parsed).unwrap();
        assert!(stored.new.is_empty());
        assert_eq!(page(&db, &Selection::All, false, 0, 50).unwrap().total, 3);
    }

    #[test]
    fn a_later_fetch_may_correct_the_text_but_never_the_read_state() {
        let (mut db, id) = seeded();
        let first = page(&db, &Selection::All, false, 0, 50).unwrap().rows[0].id;
        set_read(&mut db, &[first], true).unwrap();
        set_starred(&db, first, true).unwrap();

        let parsed = ParsedFeed {
            entries: vec![ParsedEntry {
                guid: "g3".into(),
                title: "Entry 3, corrected".into(),
                content_html: Some("<p>Better body</p>".into()),
                ..ParsedEntry::default()
            }],
            ..ParsedFeed::default()
        };
        upsert_parsed(&mut db, id, FeedKind::Web, &parsed).unwrap();

        let row = get(&db, first).unwrap().unwrap();
        assert_eq!(row.title, "Entry 3, corrected");
        assert!(row.read, "a revised title must not un-read an entry");
        assert!(row.starred);
    }

    #[test]
    fn a_page_is_newest_first_and_total_counts_the_whole_selection() {
        let (db, _) = seeded();
        let p = page(&db, &Selection::All, false, 0, 2).unwrap();
        assert_eq!(p.total, 3);
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].title, "Entry 3");
        assert_eq!(p.rows[1].title, "Entry 2");
        let p2 = page(&db, &Selection::All, false, 2, 2).unwrap();
        assert_eq!(p2.rows.len(), 1);
        assert_eq!(p2.rows[0].title, "Entry 1");
    }

    #[test]
    fn unread_only_narrows_the_page_and_the_total_together() {
        let (mut db, _) = seeded();
        let first = page(&db, &Selection::All, false, 0, 50).unwrap().rows[0].id;
        set_read(&mut db, &[first], true).unwrap();
        let p = page(&db, &Selection::All, true, 0, 50).unwrap();
        assert_eq!(p.total, 2);
        assert_eq!(p.rows.len(), 2);
        assert_eq!(unread_count(&db, &Selection::All).unwrap(), 2);
    }

    #[test]
    fn every_selection_returns_the_rows_it_names() {
        let mut db = Db::open_in_memory().unwrap();
        let folder = feeds::folder_named(&db, "Tech").unwrap();
        let web = feeds::add(
            &db,
            "https://e.org/f",
            None,
            FeedKind::Web,
            None,
            Some(folder),
        )
        .unwrap()
        .id();
        let tube = feeds::add(
            &db,
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCa",
            None,
            FeedKind::Youtube,
            None,
            None,
        )
        .unwrap()
        .id();
        upsert_parsed(
            &mut db,
            web,
            FeedKind::Web,
            &ParsedFeed {
                entries: vec![ParsedEntry {
                    guid: "a".into(),
                    title: "An article".into(),
                    url: Some("https://e.org/a".into()),
                    ..ParsedEntry::default()
                }],
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        upsert_parsed(
            &mut db,
            tube,
            FeedKind::Youtube,
            &ParsedFeed {
                entries: vec![ParsedEntry {
                    guid: "v".into(),
                    title: "A video".into(),
                    url: Some("https://www.youtube.com/watch?v=abc".into()),
                    video_id: Some("abc".into()),
                    ..ParsedEntry::default()
                }],
                ..ParsedFeed::default()
            },
        )
        .unwrap();

        assert_eq!(page(&db, &Selection::All, false, 0, 50).unwrap().total, 2);
        assert_eq!(
            page(&db, &Selection::Feed(web), false, 0, 50).unwrap().rows[0].title,
            "An article"
        );
        assert_eq!(
            page(&db, &Selection::Folder(folder), false, 0, 50)
                .unwrap()
                .total,
            1
        );
        let videos = page(&db, &Selection::Videos, false, 0, 50).unwrap();
        assert_eq!(videos.total, 1);
        assert_eq!(videos.rows[0].title, "A video");
        assert_eq!(videos.rows[0].kind, EntryKind::Video);
        assert_eq!(page(&db, &Selection::Starred, false, 0, 50).unwrap().total, 0);
    }

    #[test]
    fn a_video_linked_from_a_blog_is_still_a_video() {
        let entry = ParsedEntry {
            guid: "x".into(),
            url: Some("https://www.youtube.com/watch?v=abc".into()),
            video_id: Some("abc".into()),
            ..ParsedEntry::default()
        };
        assert_eq!(classify(FeedKind::Web, &entry), EntryKind::Video);
        assert_eq!(
            classify(FeedKind::Reddit, &ParsedEntry::default()),
            EntryKind::Post
        );
        assert_eq!(
            classify(FeedKind::Hn, &ParsedEntry::default()),
            EntryKind::Article
        );
    }

    #[test]
    fn mark_all_read_empties_the_unread_count_of_its_selection_only() {
        let (mut db, feed) = seeded();
        let other = feeds::add(&db, "https://o.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        upsert_parsed(
            &mut db,
            other,
            FeedKind::Web,
            &ParsedFeed {
                entries: vec![ParsedEntry {
                    guid: "z".into(),
                    title: "Elsewhere".into(),
                    ..ParsedEntry::default()
                }],
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        assert_eq!(mark_all_read(&db, &Selection::Feed(feed)).unwrap(), 3);
        assert_eq!(unread_count(&db, &Selection::Feed(feed)).unwrap(), 0);
        assert_eq!(unread_count(&db, &Selection::Feed(other)).unwrap(), 1);
        assert_eq!(
            mark_all_read(&db, &Selection::Feed(feed)).unwrap(),
            0,
            "a second sweep has nothing to do"
        );
    }

    #[test]
    fn retention_keeps_starred_entries_whatever_the_bounds_say() {
        let (mut db, _) = seeded();
        let rows = page(&db, &Selection::All, false, 0, 50).unwrap().rows;
        set_starred(&db, rows[0].id, true).unwrap();
        db.conn
            .execute("UPDATE entry SET published = 0, fetched_at = 0", [])
            .unwrap();
        let out = retain(&mut db, 30, 2000).unwrap();
        assert_eq!(out.by_age, 2);
        let left = page(&db, &Selection::All, false, 0, 50).unwrap();
        assert_eq!(left.total, 1);
        assert!(left.rows[0].starred);
    }

    #[test]
    fn retention_caps_a_busy_feed_by_count_as_well_as_by_age() {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(&db, "https://hnrss.org/newcomments", None, FeedKind::Hn, None, None)
            .unwrap()
            .id();
        let parsed = ParsedFeed {
            entries: (0..50)
                .map(|n| ParsedEntry {
                    guid: format!("g{n}"),
                    title: format!("Comment {n}"),
                    published: Some(jiff::Timestamp::from_second(1_800_000_000 + n).unwrap()),
                    ..ParsedEntry::default()
                })
                .collect(),
            ..ParsedFeed::default()
        };
        upsert_parsed(&mut db, feed, FeedKind::Hn, &parsed).unwrap();
        let out = retain(&mut db, 36500, 10).unwrap();
        assert_eq!(out.by_age, 0, "nothing is old enough");
        assert_eq!(out.by_count, 40);
        let left = page(&db, &Selection::All, false, 0, 100).unwrap();
        assert_eq!(left.total, 10);
        assert_eq!(left.rows[0].title, "Comment 49", "the newest are kept");
    }

    #[test]
    fn an_imported_read_mark_waits_for_its_entry_and_is_consumed_by_it() {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        put_imported_read(&db.conn, "https://e.org/f", "g2", None).unwrap();

        let parsed = ParsedFeed {
            entries: (1..=2)
                .map(|n| ParsedEntry {
                    guid: format!("g{n}"),
                    title: format!("Entry {n}"),
                    ..ParsedEntry::default()
                })
                .collect(),
            ..ParsedFeed::default()
        };
        let stored = upsert_parsed(&mut db, feed, FeedKind::Web, &parsed).unwrap();
        assert_eq!(stored.marked_read, 1);
        assert_eq!(unread_count(&db, &Selection::All).unwrap(), 1);
        let left: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM imported_read", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "a consumed mark is gone");
    }

    #[test]
    fn an_imported_read_mark_matches_the_url_the_feed_was_imported_under() {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(
            &db,
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCa",
            Some("https://scriptbarrel.com/xml.cgi?channel_id=UCa&name=Thing"),
            FeedKind::Youtube,
            None,
            None,
        )
        .unwrap()
        .id();
        put_imported_read(
            &db.conn,
            "https://scriptbarrel.com/xml.cgi?channel_id=UCa&name=Thing",
            "v1",
            None,
        )
        .unwrap();
        let stored = upsert_parsed(
            &mut db,
            feed,
            FeedKind::Youtube,
            &ParsedFeed {
                entries: vec![ParsedEntry {
                    guid: "v1".into(),
                    title: "A video".into(),
                    ..ParsedEntry::default()
                }],
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        assert_eq!(stored.marked_read, 1);
    }

    #[test]
    fn imported_read_can_also_be_applied_to_entries_already_there() {
        let (db, _) = seeded();
        put_imported_read(&db.conn, "https://e.org/f", "g1", None).unwrap();
        assert_eq!(apply_imported_read(&db).unwrap(), 1);
        assert_eq!(unread_count(&db, &Selection::All).unwrap(), 2);
    }

    #[test]
    fn an_unparseable_search_is_an_empty_page_rather_than_an_error() {
        let (db, _) = seeded();
        let p = page(&db, &Selection::Search("\"unclosed".into()), false, 0, 50).unwrap();
        assert_eq!(p.total, 0);
        assert!(p.rows.is_empty());
    }

    #[test]
    fn an_entry_with_no_guid_is_skipped_rather_than_stored_under_an_empty_key() {
        let mut db = Db::open_in_memory().unwrap();
        let feed = feeds::add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        let stored = upsert_parsed(
            &mut db,
            feed,
            FeedKind::Web,
            &ParsedFeed {
                entries: vec![
                    ParsedEntry::default(),
                    ParsedEntry {
                        guid: "ok".into(),
                        ..ParsedEntry::default()
                    },
                ],
                ..ParsedFeed::default()
            },
        )
        .unwrap();
        assert_eq!(stored.new.len(), 1);
    }
}
