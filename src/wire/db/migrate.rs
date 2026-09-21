//! Bringing a file written by an older build up to [`super::schema::SCHEMA_VERSION`].
//!
//! STAR/AMP's shape: `schema::SCHEMA` is applied first and is idempotent
//! (every statement is `IF NOT EXISTS`), so a new table or a new index needs
//! no code here at all -- it is simply there after the batch runs. What this
//! function is for is the changes that cannot be expressed that way: a column
//! added to an existing table, a value rewritten, an index that has to be
//! dropped before it can be recreated differently.
//!
//! The difference from STAR/AMP matters and is the reason this file is not a
//! stub with a `todo!` in it: STAR/AMP's index is a cache that can be thrown
//! away and rebuilt from the music on disk, so a version it does not
//! recognise is grounds for asking the user to delete it. `wire.db` is not a
//! cache. Read state, stars, and every article ever extracted from a page
//! that may since have gone behind a paywall exist nowhere else. A file this
//! build cannot understand is refused with its version named, rather than
//! rebuilt.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

use super::schema;

/// Move a file at `from` up to the current version.
///
/// `from == 0` is a file that has just been created by `SCHEMA`; there is
/// nothing to move and the version is simply stamped. A version above the
/// current one is a file written by a newer build, which this one cannot
/// safely write to.
pub fn migrate(conn: &Connection, from: i32) -> Result<()> {
    match from {
        0 => {}
        // The arms fall through: a file at 1 runs 1->2 and then 2->3 when
        // there is a 3, which is what makes a build that skipped three
        // releases work.
        1 => one_to_two(conn)?,
        v if v == schema::SCHEMA_VERSION => return Ok(()),
        v if v > schema::SCHEMA_VERSION => anyhow::bail!(
            "{} was written by a newer STAR/WIRE (schema {v}, this build reads {}); \
             upgrade rather than letting this one write to it",
            "wire.db",
            schema::SCHEMA_VERSION
        ),
        v => anyhow::bail!(
            "no way to bring schema {v} up to {}; this build is older than the file",
            schema::SCHEMA_VERSION
        ),
    }
    conn.pragma_update(None, "user_version", schema::SCHEMA_VERSION)?;
    Ok(())
}

/// 0.0.1 to 0.0.2.
///
/// Everything here is a change `SCHEMA` cannot express by itself: a column on
/// a table that already exists, and a value rewritten in place.
fn one_to_two(conn: &Connection) -> Result<()> {
    add_column(conn, "article", "retry_after", "INTEGER")?;
    add_column(conn, "entry", "final_url", "TEXT")?;
    backfill_failed_markdown(conn)?;
    recanonicalise_feed_urls(conn)?;
    Ok(())
}

/// Give every failed extraction the feed's own text.
///
/// 0.0.1 stored `markdown = NULL` when a page did not yield, so an entry
/// behind a paywall read as nothing at all -- although every one of the
/// eighty-five failures in the reference database had text in the feed, and
/// three quarters of them had more than three hundred bytes of it. New rows
/// get it in `articles::create_pending`; the ones already in the file get it
/// here, put through the same `from_feed_content` so the two are the same
/// markdown.
fn backfill_failed_markdown(conn: &Connection) -> Result<()> {
    let rows: Vec<(i64, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT a.entry_id, e.content_html, e.url
             FROM article a JOIN entry e ON e.id = a.entry_id
             WHERE a.status = 3 AND a.markdown IS NULL AND e.content_html IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (entry_id, html, url) in rows {
        let markdown = crate::wire::extract::from_feed_content(&html, url.as_deref());
        if markdown.trim().is_empty() {
            continue;
        }
        conn.execute(
            "UPDATE article SET markdown = ?2 WHERE entry_id = ?1",
            rusqlite::params![entry_id, markdown],
        )?;
    }
    Ok(())
}

/// Put every stored feed URL through the canonicalisation this build does.
///
/// Today that means one thing: `old.reddit.com` becomes `www.reddit.com`,
/// because the old host has started answering a feed request with a login
/// page. A file written by 0.0.1 has the old spelling in it and nothing else
/// would ever change it -- `canonicalise` runs when a feed is *added*.
///
/// `source_url` is deliberately left alone: it is what was typed or imported,
/// it is what a second import of the same `urls` file matches on, and
/// rewriting it would make this migration lose the one record of what the
/// reader actually asked for. `feed.url` is UNIQUE, so a row whose new
/// spelling is already taken is left where it is rather than made into an
/// error -- the reader has both, and removing one is theirs to do.
fn recanonicalise_feed_urls(conn: &Connection) -> Result<()> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, url FROM feed")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, url) in rows {
        let Ok(mut parsed) = url::Url::parse(&url) else {
            continue;
        };
        crate::wire::feed::canonical_url(&mut parsed);
        let rewritten = parsed.to_string();
        if rewritten == url {
            continue;
        }
        let taken: Option<i64> = conn
            .query_row("SELECT id FROM feed WHERE url = ?1", [&rewritten], |r| {
                r.get(0)
            })
            .optional()?;
        if taken.is_some() {
            tracing::warn!(%url, %rewritten, "both spellings of this feed are subscribed to; leaving the old one");
            continue;
        }
        conn.execute(
            "UPDATE feed SET url = ?2 WHERE id = ?1",
            rusqlite::params![id, rewritten],
        )?;
    }
    Ok(())
}

/// Whether a table already has a column, so that a migration adding one is
/// safe to run against a file that has been through it before -- SQLite has
/// no `ADD COLUMN IF NOT EXISTS`, and a second `ALTER TABLE` is an error
/// rather than a no-op.
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Add a column to an existing table, if it is not already there.
fn add_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    if has_column(conn, table, column)? {
        return Ok(());
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(schema::PRAGMAS).unwrap();
        conn.execute_batch(schema::SCHEMA).unwrap();
        conn
    }

    /// 0.0.1's `SCHEMA`, with its comments taken out and every statement
    /// otherwise verbatim.
    ///
    /// A copy rather than a reference to the real one on purpose: the whole
    /// question a migration test asks is whether this build can read a file
    /// written by the last one, and pointing both sides at the same string
    /// answers a different, much easier question.
    const V1_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS folder (
  id       INTEGER PRIMARY KEY,
  name     TEXT NOT NULL UNIQUE,
  position INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS feed (
  id            INTEGER PRIMARY KEY,
  url           TEXT NOT NULL UNIQUE,
  source_url    TEXT,
  kind          INTEGER NOT NULL,            -- 0 web, 1 youtube, 2 reddit, 3 hn
  title         TEXT,
  custom_title  TEXT,
  site_url      TEXT,
  folder_id     INTEGER REFERENCES folder(id) ON DELETE SET NULL,
  position      INTEGER NOT NULL DEFAULT 0,
  etag          TEXT,
  last_modified TEXT,
  last_fetch    INTEGER,
  last_ok       INTEGER,
  last_error    TEXT,
  failures      INTEGER NOT NULL DEFAULT 0,
  backoff_until INTEGER,
  added_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS feed_folder_idx ON feed(folder_id, position);
CREATE INDEX IF NOT EXISTS feed_due_idx    ON feed(last_fetch);
CREATE TABLE IF NOT EXISTS entry (
  id            INTEGER PRIMARY KEY,
  feed_id       INTEGER NOT NULL REFERENCES feed(id) ON DELETE CASCADE,
  guid          TEXT NOT NULL,
  url           TEXT,
  title         TEXT NOT NULL DEFAULT '',
  author        TEXT,
  kind          INTEGER NOT NULL DEFAULT 0,  -- 0 article, 1 video, 2 post
  published     INTEGER,
  fetched_at    INTEGER NOT NULL,
  content_html  TEXT,
  thumbnail_url TEXT,
  video_id      TEXT,
  duration_secs INTEGER,
  read          INTEGER NOT NULL DEFAULT 0,
  starred       INTEGER NOT NULL DEFAULT 0,
  UNIQUE(feed_id, guid)
);
CREATE INDEX IF NOT EXISTS entry_feed_pub_idx ON entry(feed_id, published DESC);
CREATE INDEX IF NOT EXISTS entry_unread_idx   ON entry(feed_id) WHERE read = 0;
CREATE INDEX IF NOT EXISTS entry_url_idx      ON entry(url);
CREATE INDEX IF NOT EXISTS entry_starred_idx  ON entry(starred) WHERE starred = 1;
CREATE INDEX IF NOT EXISTS entry_kind_pub_idx ON entry(kind, published DESC);
CREATE INDEX IF NOT EXISTS entry_pub_idx      ON entry(published DESC);
CREATE TABLE IF NOT EXISTS article (
  entry_id     INTEGER PRIMARY KEY REFERENCES entry(id) ON DELETE CASCADE,
  status       INTEGER NOT NULL,
  title        TEXT,
  markdown     TEXT,
  byline       TEXT,
  site_name    TEXT,
  image_url    TEXT,
  excerpt      TEXT,
  source_url   TEXT,
  extracted_at INTEGER,
  attempts     INTEGER NOT NULL DEFAULT 0,
  error        TEXT
);
CREATE INDEX IF NOT EXISTS article_pending_idx ON article(status) WHERE status = 0;
CREATE VIRTUAL TABLE IF NOT EXISTS article_fts USING fts5(
  title,
  markdown,
  content='article',
  content_rowid='entry_id',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS article_ai AFTER INSERT ON article BEGIN
  INSERT INTO article_fts(rowid, title, markdown)
  VALUES (new.entry_id, new.title, new.markdown);
END;
CREATE TRIGGER IF NOT EXISTS article_ad AFTER DELETE ON article BEGIN
  INSERT INTO article_fts(article_fts, rowid, title, markdown)
  VALUES ('delete', old.entry_id, old.title, old.markdown);
END;
CREATE TRIGGER IF NOT EXISTS article_au AFTER UPDATE ON article BEGIN
  INSERT INTO article_fts(article_fts, rowid, title, markdown)
  VALUES ('delete', old.entry_id, old.title, old.markdown);
  INSERT INTO article_fts(rowid, title, markdown)
  VALUES (new.entry_id, new.title, new.markdown);
END;
CREATE TABLE IF NOT EXISTS youtube_channel (
  channel_id TEXT PRIMARY KEY,
  title      TEXT,
  handle     TEXT,
  feed_id    INTEGER REFERENCES feed(id) ON DELETE CASCADE,
  source     INTEGER NOT NULL,              -- 0 feed import, 1 add, 2 takeout, 3 ytsubs
  added_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS youtube_feed_idx ON youtube_channel(feed_id);
CREATE TABLE IF NOT EXISTS fetch_log (
  id          INTEGER PRIMARY KEY,
  feed_id     INTEGER REFERENCES feed(id) ON DELETE CASCADE,
  at          INTEGER NOT NULL,
  status      INTEGER,
  bytes       INTEGER,
  new_entries INTEGER,
  millis      INTEGER,
  error       TEXT
);
CREATE INDEX IF NOT EXISTS fetch_log_at_idx ON fetch_log(at);
CREATE TABLE IF NOT EXISTS imported_read (
  feed_url TEXT NOT NULL,
  guid     TEXT NOT NULL,
  url      TEXT,
  PRIMARY KEY (feed_url, guid)
);
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT
);
"#;

    /// A file as 0.0.1 left it: the old DDL, a feed, an entry with text in
    /// it, and a failed extraction storing nothing.
    fn v1_file() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(schema::PRAGMAS).unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        conn.execute_batch(
            "INSERT INTO feed(id, url, kind, added_at) VALUES (1, 'https://e.org/f', 0, 0);
             INSERT INTO entry(id, feed_id, guid, url, title, fetched_at, content_html)
             VALUES (1, 1, 'a', 'https://e.org/a', 'Behind a wall', 0,
                     '<p>Two sentences, and a <a href=\"/rest\">link</a> to the rest.</p>'),
                    (2, 1, 'b', 'https://e.org/b', 'Nothing at all', 0, NULL);
             INSERT INTO article(entry_id, status, attempts, error)
             VALUES (1, 3, 1, 'the site answered 403'),
                    (2, 3, 1, 'the site answered 403');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn
    }

    /// What `Db::init` does, so the test runs the real path rather than
    /// `migrate` on its own: the current schema first, because it is
    /// idempotent and adds every new table by itself, and only then the
    /// changes it cannot express.
    fn open_as_the_program_does(conn: &Connection) {
        conn.execute_batch(schema::SCHEMA).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        migrate(conn, version).unwrap();
    }

    #[test]
    fn a_file_from_0_0_1_comes_up_to_the_current_version() {
        let conn = v1_file();
        open_as_the_program_does(&conn);
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, schema::SCHEMA_VERSION);
    }

    #[test]
    fn a_failure_from_0_0_1_is_given_the_text_its_feed_carried() {
        let conn = v1_file();
        open_as_the_program_does(&conn);

        let markdown: Option<String> = conn
            .query_row("SELECT markdown FROM article WHERE entry_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let markdown = markdown.expect("the feed's own text");
        assert!(markdown.contains("Two sentences"), "{markdown}");
        assert!(
            markdown.contains("https://e.org/rest"),
            "the link was not resolved against the entry: {markdown}"
        );

        // An entry whose feed carried nothing has nothing to be given, and
        // that is not an error.
        let empty: Option<String> = conn
            .query_row("SELECT markdown FROM article WHERE entry_id = 2", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(empty, None);
    }

    #[test]
    fn a_reddit_feed_from_0_0_1_is_moved_to_the_host_that_still_answers() {
        let conn = v1_file();
        conn.execute_batch(
            "INSERT INTO feed(id, url, source_url, kind, added_at)
             VALUES (2, 'https://old.reddit.com/r/rust/.rss',
                        'http://old.reddit.com/r/rust/.rss', 2, 0),
                    (3, 'https://example.org/feed.xml', NULL, 0, 0);",
        )
        .unwrap();
        open_as_the_program_does(&conn);

        let (url, source): (String, Option<String>) = conn
            .query_row("SELECT url, source_url FROM feed WHERE id = 2", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(url, "https://www.reddit.com/r/rust/.rss");
        assert_eq!(
            source.as_deref(),
            Some("http://old.reddit.com/r/rust/.rss"),
            "what was imported is what a second import matches on"
        );

        let other: String = conn
            .query_row("SELECT url FROM feed WHERE id = 3", [], |r| r.get(0))
            .unwrap();
        assert_eq!(other, "https://example.org/feed.xml");
    }

    #[test]
    fn a_reddit_feed_subscribed_to_under_both_names_keeps_both() {
        let conn = v1_file();
        conn.execute_batch(
            "INSERT INTO feed(id, url, kind, added_at)
             VALUES (2, 'https://old.reddit.com/r/rust/.rss', 2, 0),
                    (3, 'https://www.reddit.com/r/rust/.rss', 2, 0);",
        )
        .unwrap();
        // The rewrite would collide with a row that is already there, and
        // `feed.url` is UNIQUE. Leaving it is right: removing somebody's
        // subscription is not a migration's business.
        open_as_the_program_does(&conn);
        let urls: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT url FROM feed WHERE id IN (2, 3) ORDER BY id")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.map(Result::unwrap).collect()
        };
        assert_eq!(
            urls,
            [
                "https://old.reddit.com/r/rust/.rss",
                "https://www.reddit.com/r/rust/.rss"
            ]
        );
    }

    #[test]
    fn migrating_twice_changes_nothing() {
        let conn = v1_file();
        open_as_the_program_does(&conn);
        let after_once: Option<String> = conn
            .query_row("SELECT markdown FROM article WHERE entry_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        open_as_the_program_does(&conn);
        let after_twice: Option<String> = conn
            .query_row("SELECT markdown FROM article WHERE entry_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(after_once, after_twice);
    }

    #[test]
    fn a_new_file_is_stamped_with_the_current_version() {
        let conn = fresh();
        migrate(&conn, 0).unwrap();
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, schema::SCHEMA_VERSION);
    }

    #[test]
    fn a_file_already_at_the_current_version_is_left_alone() {
        let conn = fresh();
        migrate(&conn, schema::SCHEMA_VERSION).unwrap();
    }

    #[test]
    fn a_file_from_a_newer_build_is_refused_rather_than_rewritten() {
        let conn = fresh();
        let err = migrate(&conn, schema::SCHEMA_VERSION + 1).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("newer"), "{text}");
        assert!(
            !text.contains("delete"),
            "this database is not a cache and must never suggest deleting it: {text}"
        );
    }
}
