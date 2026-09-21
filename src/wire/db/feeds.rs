//! Folders and feeds: the list itself, and the bookkeeping one round trip to
//! a server leaves behind.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::{now, schema, stamp, Db};
use crate::wire::feed::{FeedId, FeedKind, FeedRow, Folder, FolderId};

/// The conditional-request headers a feed answered with last time, and the
/// thing that turns a refresh of 41 feeds into 41 round trips and almost no
/// bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conditional {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// What one feed looks like to the fetcher, which needs less than the list
/// does and needs one thing -- the backoff -- the list does not use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueFeed {
    pub id: FeedId,
    pub url: String,
    pub kind: FeedKind,
    pub conditional: Conditional,
    pub backoff_until: Option<i64>,
}

/// Every feed, with its unread count, in the order the source list draws
/// them: foldered feeds first by folder position and name, then the
/// unfoldered ones.
///
/// The count is a correlated subquery over the partial `WHERE read = 0`
/// index rather than a column kept up to date by hand. Measured on the
/// reference list -- 41 feeds, 120k entries -- it is under a millisecond,
/// and a stored count has to survive an import, a retention sweep and a
/// second process writing through WAL, any one of which is a way to show a
/// number that is not true.
pub fn list_feeds_with_unread(db: &Db) -> Result<Vec<FeedRow>> {
    let mut stmt = db.conn.prepare(
        "SELECT f.id, f.url, f.source_url, f.kind, f.title, f.custom_title, f.site_url,
                f.folder_id, f.position, f.last_ok, f.last_error, f.failures, f.backoff_until,
                (SELECT COUNT(*) FROM entry e WHERE e.feed_id = f.id AND e.read = 0)
         FROM feed f
         LEFT JOIN folder fo ON fo.id = f.folder_id
         ORDER BY (f.folder_id IS NULL), fo.position, fo.name COLLATE NOCASE,
                  f.position, COALESCE(f.custom_title, f.title, f.url) COLLATE NOCASE",
    )?;
    let rows = stmt
        .query_map([], row_to_feed)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn row_to_feed(r: &rusqlite::Row<'_>) -> rusqlite::Result<FeedRow> {
    Ok(FeedRow {
        id: FeedId(r.get(0)?),
        url: r.get(1)?,
        source_url: r.get(2)?,
        kind: FeedKind::from_i64(r.get(3)?),
        title: r.get(4)?,
        custom_title: r.get(5)?,
        site_url: r.get(6)?,
        folder: r.get::<_, Option<i64>>(7)?.map(FolderId),
        position: r.get(8)?,
        last_ok: stamp(r.get(9)?),
        error: r.get(10)?,
        failures: r.get(11)?,
        backoff_until: stamp(r.get(12)?),
        unread: r.get(13)?,
    })
}

pub fn folders(db: &Db) -> Result<Vec<Folder>> {
    let mut stmt = db
        .conn
        .prepare("SELECT id, name, position FROM folder ORDER BY position, name COLLATE NOCASE")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Folder {
                id: FolderId(r.get(0)?),
                name: r.get(1)?,
                position: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The folder called `name`, created if there is not one.
///
/// Folders have no separate life: they exist because a feed is in one, and
/// they are made by naming one. There is no "new folder" command anywhere in
/// this program, and that is deliberate -- an empty folder is a thing to
/// tidy up later.
pub fn folder_named(db: &Db, name: &str) -> Result<FolderId> {
    let name = name.trim();
    if let Some(id) = db
        .conn
        .query_row(
            "SELECT id FROM folder WHERE name = ?1 COLLATE NOCASE",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(FolderId(id));
    }
    db.conn.execute(
        "INSERT INTO folder(name, position) VALUES (?1, (SELECT COALESCE(MAX(position), 0) + 1 FROM folder))",
        [name],
    )?;
    Ok(FolderId(db.conn.last_insert_rowid()))
}

/// What `add` did, which the caller reports differently in each case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    /// A new row.
    New(FeedId),
    /// The canonical URL was already there. Not an error: importing the same
    /// list twice, or adding a channel already subscribed to through a
    /// different spelling of its URL, both land here.
    Existing(FeedId),
}

impl Added {
    pub fn id(self) -> FeedId {
        match self {
            Added::New(id) | Added::Existing(id) => id,
        }
    }

    pub fn is_new(self) -> bool {
        matches!(self, Added::New(_))
    }
}

/// Add a feed by its already-canonical URL.
///
/// Canonicalisation -- https, and the YouTube rewrite -- happens in
/// [`crate::wire::youtube::canonicalise`] before this is called, so that the
/// importer, the CLI and the window all normalise the same way and a feed
/// cannot arrive under two spellings.
pub fn add(
    db: &Db,
    url: &str,
    source_url: Option<&str>,
    kind: FeedKind,
    title: Option<&str>,
    folder: Option<FolderId>,
) -> Result<Added> {
    if let Some(id) = db
        .conn
        .query_row("SELECT id FROM feed WHERE url = ?1", [url], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
    {
        let id = FeedId(id);
        // An import that carries a title for a feed already on the list is
        // still worth something: newsboat's cache is the only place several
        // of these have a name at all.
        if let Some(title) = title.filter(|t| !t.trim().is_empty()) {
            db.conn.execute(
                "UPDATE feed SET title = COALESCE(title, ?2) WHERE id = ?1",
                params![id.0, title.trim()],
            )?;
        }
        if let Some(folder) = folder {
            db.conn.execute(
                "UPDATE feed SET folder_id = COALESCE(folder_id, ?2) WHERE id = ?1",
                params![id.0, folder.0],
            )?;
        }
        return Ok(Added::Existing(id));
    }

    db.conn
        .execute(
            "INSERT INTO feed(url, source_url, kind, title, folder_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5,
                     (SELECT COALESCE(MAX(position), 0) + 1 FROM feed), ?6)",
            params![
                url,
                source_url,
                kind.as_i64(),
                title.map(str::trim).filter(|t| !t.is_empty()),
                folder.map(|f| f.0),
                now(),
            ],
        )
        .with_context(|| format!("adding {url}"))?;
    Ok(Added::New(FeedId(db.conn.last_insert_rowid())))
}

/// Remove a feed. Its entries and articles go with it, through the schema's
/// cascade rather than three statements here.
pub fn remove(db: &Db, id: FeedId) -> Result<bool> {
    Ok(db.conn.execute("DELETE FROM feed WHERE id = ?1", [id.0])? > 0)
}

/// Set or clear the reader's own name for a feed. An empty string clears it,
/// which is how the rename overlay says "go back to what the feed calls
/// itself".
pub fn rename(db: &Db, id: FeedId, title: Option<&str>) -> Result<()> {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    db.conn.execute(
        "UPDATE feed SET custom_title = ?2 WHERE id = ?1",
        params![id.0, title],
    )?;
    Ok(())
}

pub fn set_folder(db: &Db, id: FeedId, folder: Option<FolderId>) -> Result<()> {
    db.conn.execute(
        "UPDATE feed SET folder_id = ?2 WHERE id = ?1",
        params![id.0, folder.map(|f| f.0)],
    )?;
    Ok(())
}

pub fn rename_folder(db: &Db, id: FolderId, name: &str) -> Result<()> {
    db.conn.execute(
        "UPDATE folder SET name = ?2 WHERE id = ?1",
        params![id.0, name.trim()],
    )?;
    Ok(())
}

/// Remove a folder. Its feeds are not removed -- they become unfoldered,
/// through the schema's `ON DELETE SET NULL`. Deleting a folder is a tidying
/// action, and one that took the feeds with it would be a trap.
pub fn remove_folder(db: &Db, id: FolderId) -> Result<()> {
    db.conn.execute("DELETE FROM folder WHERE id = ?1", [id.0])?;
    Ok(())
}

pub fn get(db: &Db, id: FeedId) -> Result<Option<FeedRow>> {
    Ok(one(db, "f.id = ?1", params![id.0])?)
}

/// Find a feed by what somebody typed: a row id, or a URL in any of the
/// spellings it might have been stored under.
///
/// The three URL columns are all checked because a feed imported from
/// newsboat is stored under the YouTube URL and the person looking for it
/// has the scriptbarrel one in their `urls` file.
pub fn find(db: &Db, needle: &str) -> Result<Option<FeedRow>> {
    let needle = needle.trim();
    if let Ok(id) = needle.parse::<i64>() {
        if let Some(row) = get(db, FeedId(id))? {
            return Ok(Some(row));
        }
    }
    if let Some(row) = one(db, "f.url = ?1 OR f.source_url = ?1", params![needle])? {
        return Ok(Some(row));
    }
    // A last try with the canonical form, so `starwire remove
    // http://example.org/feed` finds the https row it was stored as.
    if let Ok(canonical) = crate::wire::youtube::canonicalise(needle) {
        if canonical.url != needle {
            return one(db, "f.url = ?1", params![canonical.url]);
        }
    }
    Ok(None)
}

fn one(db: &Db, where_clause: &str, args: impl rusqlite::Params) -> Result<Option<FeedRow>> {
    let sql = format!(
        "SELECT f.id, f.url, f.source_url, f.kind, f.title, f.custom_title, f.site_url,
                f.folder_id, f.position, f.last_ok, f.last_error, f.failures, f.backoff_until,
                (SELECT COUNT(*) FROM entry e WHERE e.feed_id = f.id AND e.read = 0)
         FROM feed f WHERE {where_clause} LIMIT 1"
    );
    Ok(db.conn.query_row(&sql, args, row_to_feed).optional()?)
}

/// Every feed a refresh would fetch, oldest fetch first.
///
/// The order is what makes a refresh of a long list feel right: a feed that
/// has not been looked at since yesterday goes before one fetched five
/// minutes ago, so the numbers that move first are the ones most likely to
/// have changed.
pub fn due(db: &Db, ignore_backoff: bool) -> Result<Vec<DueFeed>> {
    let mut stmt = db.conn.prepare(
        "SELECT id, url, kind, etag, last_modified, backoff_until
         FROM feed ORDER BY (last_fetch IS NOT NULL), last_fetch, id",
    )?;
    let at = now();
    let rows = stmt
        .query_map([], |r| {
            Ok(DueFeed {
                id: FeedId(r.get(0)?),
                url: r.get(1)?,
                kind: FeedKind::from_i64(r.get(2)?),
                conditional: Conditional {
                    etag: r.get(3)?,
                    last_modified: r.get(4)?,
                },
                backoff_until: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter(|f| ignore_backoff || f.backoff_until.is_none_or(|until| until <= at))
        .collect())
}

/// Record a fetch that produced something: a 200 with a body, or a 304.
///
/// Clears the failure count and the backoff, because one good answer is the
/// end of a bad run -- a site that was down for an hour should be back on
/// the fifteen-minute schedule as soon as it answers, not on the four-hour
/// one its outage earned it.
pub fn record_ok(
    db: &Db,
    id: FeedId,
    conditional: &Conditional,
    title: Option<&str>,
    site_url: Option<&str>,
) -> Result<()> {
    let at = now();
    db.conn.execute(
        "UPDATE feed SET last_fetch = ?2, last_ok = ?2, last_error = NULL,
                         failures = 0, backoff_until = NULL,
                         etag = ?3, last_modified = ?4,
                         title = COALESCE(?5, title),
                         site_url = COALESCE(?6, site_url)
         WHERE id = ?1",
        params![
            id.0,
            at,
            conditional.etag,
            conditional.last_modified,
            title.map(str::trim).filter(|t| !t.is_empty()),
            site_url,
        ],
    )?;
    Ok(())
}

/// Record a fetch that failed, and say when to try again.
///
/// `retry_after` is the server's own answer where it gave one -- Reddit
/// does, and honouring it is the difference between being rate limited and
/// being blocked. Otherwise the wait doubles per consecutive failure from
/// the ordinary refresh interval, capped at a day: a feed whose domain has
/// expired should cost one request a day, not ninety-six.
pub fn record_failure(
    db: &Db,
    id: FeedId,
    error: &str,
    refresh_minutes: u32,
    retry_after: Option<i64>,
) -> Result<i64> {
    let failures: i64 = db
        .conn
        .query_row("SELECT failures FROM feed WHERE id = ?1", [id.0], |r| {
            r.get(0)
        })
        .optional()?
        .unwrap_or(0)
        + 1;
    let at = now();
    let until = at + retry_after.unwrap_or_else(|| backoff_secs(refresh_minutes, failures));
    db.conn.execute(
        "UPDATE feed SET last_fetch = ?2, last_error = ?3, failures = ?4, backoff_until = ?5
         WHERE id = ?1",
        params![id.0, at, error, failures, until],
    )?;
    Ok(until)
}

/// `min(refresh_minutes * 2^(failures - 1), 24 h)`, in seconds.
///
/// Saturating rather than shifting: twenty-odd consecutive failures would
/// overflow the shift, and a feed that has been failing for a week is
/// exactly the case this has to survive.
pub fn backoff_secs(refresh_minutes: u32, failures: i64) -> i64 {
    const DAY: i64 = 24 * 60 * 60;
    let base = (refresh_minutes.max(1) as i64) * 60;
    let shift = (failures.max(1) - 1).min(32) as u32;
    base.saturating_mul(1i64 << shift).min(DAY)
}

/// One line in `fetch_log`, and the sweep that keeps it a week.
///
/// A week rather than forever because the only question this table answers
/// is "has this been failing, and since when", and nobody has ever wanted
/// the answer for a feed they removed in March.
pub fn log_fetch(
    db: &Db,
    id: FeedId,
    status: Option<u16>,
    bytes: Option<usize>,
    new_entries: Option<usize>,
    millis: u128,
    error: Option<&str>,
) -> Result<()> {
    db.conn.execute(
        "INSERT INTO fetch_log(feed_id, at, status, bytes, new_entries, millis, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id.0,
            now(),
            status.map(i64::from),
            bytes.map(|b| b as i64),
            new_entries.map(|n| n as i64),
            millis.min(i64::MAX as u128) as i64,
            error,
        ],
    )?;
    Ok(())
}

pub fn sweep_fetch_log(db: &Db) -> Result<usize> {
    const WEEK: i64 = 7 * 24 * 60 * 60;
    Ok(db
        .conn
        .execute("DELETE FROM fetch_log WHERE at < ?1", [now() - WEEK])?)
}

/// Whether the newsboat import offer has already been made and answered.
pub fn newsboat_settled(db: &Db) -> Result<bool> {
    Ok(db.meta(schema::meta::NEWSBOAT_IMPORTED_AT)?.is_some()
        || db.meta(schema::meta::NEWSBOAT_OFFER_DECLINED)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn adding_the_same_url_twice_is_one_feed() {
        let db = db();
        let a = add(&db, "https://example.org/feed", None, FeedKind::Web, None, None).unwrap();
        let b = add(&db, "https://example.org/feed", None, FeedKind::Web, None, None).unwrap();
        assert!(a.is_new());
        assert!(!b.is_new());
        assert_eq!(a.id(), b.id());
        assert_eq!(list_feeds_with_unread(&db).unwrap().len(), 1);
    }

    #[test]
    fn a_second_add_fills_in_a_title_the_first_did_not_have() {
        let db = db();
        let id = add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        add(
            &db,
            "https://e.org/f",
            None,
            FeedKind::Web,
            Some("Named at last"),
            None,
        )
        .unwrap();
        assert_eq!(
            get(&db, id).unwrap().unwrap().title.as_deref(),
            Some("Named at last"),
            "newsboat's cache is the only place several feeds have a name"
        );
    }

    #[test]
    fn a_second_add_does_not_overwrite_a_title_that_is_already_there() {
        let db = db();
        let id = add(
            &db,
            "https://e.org/f",
            None,
            FeedKind::Web,
            Some("First"),
            None,
        )
        .unwrap()
        .id();
        add(
            &db,
            "https://e.org/f",
            None,
            FeedKind::Web,
            Some("Second"),
            None,
        )
        .unwrap();
        assert_eq!(get(&db, id).unwrap().unwrap().title.as_deref(), Some("First"));
    }

    #[test]
    fn a_folder_is_made_by_naming_it_and_found_again_by_name() {
        let db = db();
        let a = folder_named(&db, "Tech").unwrap();
        let b = folder_named(&db, " Tech ").unwrap();
        let c = folder_named(&db, "tech").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c, "folder names are not case sensitive");
        assert_eq!(folders(&db).unwrap().len(), 1);
    }

    #[test]
    fn removing_a_folder_keeps_its_feeds() {
        let db = db();
        let folder = folder_named(&db, "Tech").unwrap();
        let feed = add(
            &db,
            "https://e.org/f",
            None,
            FeedKind::Web,
            None,
            Some(folder),
        )
        .unwrap()
        .id();
        remove_folder(&db, folder).unwrap();
        let row = get(&db, feed).unwrap().unwrap();
        assert!(row.folder.is_none(), "the feed survived, unfoldered");
    }

    #[test]
    fn a_rename_wins_and_an_empty_rename_clears_it() {
        let db = db();
        let id = add(&db, "https://e.org/f", None, FeedKind::Web, Some("Feed"), None)
            .unwrap()
            .id();
        rename(&db, id, Some("Mine")).unwrap();
        assert_eq!(get(&db, id).unwrap().unwrap().display_title(), "Mine");
        rename(&db, id, Some("   ")).unwrap();
        assert_eq!(get(&db, id).unwrap().unwrap().display_title(), "Feed");
    }

    #[test]
    fn find_takes_an_id_a_url_or_the_url_it_was_imported_under() {
        let db = db();
        let id = add(
            &db,
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCabc",
            Some("https://scriptbarrel.com/xml.cgi?channel_id=UCabc&name=Thing"),
            FeedKind::Youtube,
            None,
            None,
        )
        .unwrap()
        .id();
        assert_eq!(find(&db, &id.to_string()).unwrap().unwrap().id, id);
        assert_eq!(
            find(&db, "https://www.youtube.com/feeds/videos.xml?channel_id=UCabc")
                .unwrap()
                .unwrap()
                .id,
            id
        );
        assert_eq!(
            find(&db, "https://scriptbarrel.com/xml.cgi?channel_id=UCabc&name=Thing")
                .unwrap()
                .unwrap()
                .id,
            id,
            "the URL in somebody's urls file still finds the feed it became"
        );
        assert!(find(&db, "https://nowhere.example/").unwrap().is_none());
    }

    #[test]
    fn find_canonicalises_before_giving_up() {
        let db = db();
        let id = add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        assert_eq!(
            find(&db, "http://e.org/f").unwrap().unwrap().id,
            id,
            "http is rewritten on the way in, so it must be on the way to a lookup too"
        );
    }

    #[test]
    fn the_backoff_doubles_and_stops_at_a_day() {
        assert_eq!(backoff_secs(15, 1), 15 * 60);
        assert_eq!(backoff_secs(15, 2), 30 * 60);
        assert_eq!(backoff_secs(15, 3), 60 * 60);
        assert_eq!(backoff_secs(15, 8), 24 * 60 * 60);
        // A feed that has been failing for a week: the shift would overflow
        // long before this.
        assert_eq!(backoff_secs(15, 672), 24 * 60 * 60);
        assert_eq!(backoff_secs(15, i64::MAX), 24 * 60 * 60);
        assert_eq!(backoff_secs(0, 1), 60, "a zero interval is still a minute");
    }

    #[test]
    fn a_failure_sets_a_backoff_and_a_success_clears_it() {
        let db = db();
        let id = add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        record_failure(&db, id, "connection refused", 15, None).unwrap();
        let row = get(&db, id).unwrap().unwrap();
        assert_eq!(row.failures, 1);
        assert_eq!(row.error.as_deref(), Some("connection refused"));
        assert!(row.backoff_until.is_some());
        assert!(due(&db, false).unwrap().is_empty(), "it is backed off");
        assert_eq!(due(&db, true).unwrap().len(), 1, "unless asked for by hand");

        record_ok(&db, id, &Conditional::default(), Some("A feed"), None).unwrap();
        let row = get(&db, id).unwrap().unwrap();
        assert_eq!(row.failures, 0);
        assert!(row.error.is_none());
        assert!(row.backoff_until.is_none());
        assert_eq!(row.title.as_deref(), Some("A feed"));
        assert_eq!(due(&db, false).unwrap().len(), 1);
    }

    #[test]
    fn a_retry_after_from_the_server_wins_over_the_computed_backoff() {
        let db = db();
        let id = add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        let until = record_failure(&db, id, "429", 15, Some(3600)).unwrap();
        let computed = now() + backoff_secs(15, 1);
        assert!(
            until > computed,
            "the server asked for an hour and got fifteen minutes"
        );
    }

    #[test]
    fn due_puts_the_feed_nobody_has_fetched_first() {
        let db = db();
        let a = add(&db, "https://a.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        let b = add(&db, "https://b.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        record_ok(&db, a, &Conditional::default(), None, None).unwrap();
        let order: Vec<FeedId> = due(&db, false).unwrap().into_iter().map(|f| f.id).collect();
        assert_eq!(order, vec![b, a]);
    }

    #[test]
    fn conditional_headers_are_kept_and_handed_back() {
        let db = db();
        let id = add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        let c = Conditional {
            etag: Some("\"abc\"".into()),
            last_modified: Some("Sat, 20 Sep 2026 10:00:00 GMT".into()),
        };
        record_ok(&db, id, &c, None, None).unwrap();
        assert_eq!(due(&db, false).unwrap()[0].conditional, c);
    }

    #[test]
    fn the_fetch_log_is_kept_a_week() {
        let db = db();
        let id = add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        log_fetch(&db, id, Some(200), Some(1024), Some(3), 42, None).unwrap();
        db.conn
            .execute(
                "UPDATE fetch_log SET at = ?1",
                [now() - 8 * 24 * 60 * 60],
            )
            .unwrap();
        assert_eq!(sweep_fetch_log(&db).unwrap(), 1);
    }

    #[test]
    fn removing_a_feed_takes_its_entries_with_it() {
        let db = db();
        let id = add(&db, "https://e.org/f", None, FeedKind::Web, None, None)
            .unwrap()
            .id();
        db.conn
            .execute(
                "INSERT INTO entry(feed_id, guid, title, fetched_at) VALUES (?1, 'g', 't', 0)",
                [id.0],
            )
            .unwrap();
        assert!(remove(&db, id).unwrap());
        let left: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM entry", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
        assert!(!remove(&db, id).unwrap(), "removing it again does nothing");
    }
}
