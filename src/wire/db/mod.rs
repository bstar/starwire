//! The one thing in this program that writes.
//!
//! One `rusqlite::Connection`, owned by whoever opened it -- the `starwire-db`
//! thread when the window is up, the calling thread when a headless
//! subcommand is running. Nothing here spawns anything and nothing here takes
//! a lock: the single-writer rule is kept by there being one `Db` and one
//! thread holding it, not by a mutex.
//!
//! The submodules are the verbs, grouped by what they touch:
//! [`feeds`] the list, [`entries`] what arrived, [`articles`] what was made of
//! it, [`youtube`] the channel table beside the feeds.

pub mod articles;
pub mod entries;
pub mod feeds;
pub mod migrate;
pub mod schema;
pub mod youtube;

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

/// A handle on `wire.db`.
pub struct Db {
    pub conn: Connection,
}

impl Db {
    /// Open, creating the file and its directory if they are not there.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening the database at {}", path.display()))?;
        Self::init(conn)
    }

    /// An empty database in memory. What every test in the tree runs against,
    /// and what `--replay` fetches into when no directory is named.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(schema::PRAGMAS)
            .context("applying pragmas")?;
        // Applied before the version is read, and idempotent, so a file
        // missing a table added in a later version simply gains it. Only the
        // changes that cannot be written that way reach `migrate`.
        conn.execute_batch(schema::SCHEMA)
            .context("creating the schema")?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        migrate::migrate(&conn, version)?;
        Ok(Self { conn })
    }

    /// Read one `meta` value.
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = stmt.query([key])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        })
    }

    /// Write one `meta` value.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (key, value),
        )?;
        Ok(())
    }

    /// `PRAGMA data_version`, which changes when *another connection* has
    /// committed to this file.
    ///
    /// This is how the window notices a `starwire fetch` run from a systemd
    /// timer while it was open. Cheap enough to ask once a second, which is
    /// the whole reason it is used rather than a file watch: WAL means the
    /// file's mtime is not the question.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }
}

/// Now, as the seconds the database stores.
///
/// One function rather than `Timestamp::now().as_second()` at thirty call
/// sites, and the thing `testing` pins so a fixture's ages do not depend on
/// what day the test is run.
pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// A stored unix second back into a timestamp, tolerating the nonsense a
/// hand-edited or corrupted row could hold.
pub fn stamp(secs: Option<i64>) -> Option<jiff::Timestamp> {
    secs.and_then(|s| jiff::Timestamp::from_second(s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_database_has_every_table_and_the_current_version() {
        let db = Db::open_in_memory().unwrap();
        let mut stmt = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view') ORDER BY name")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for want in [
            "article",
            "article_fts",
            "entry",
            "feed",
            "fetch_log",
            "folder",
            "imported_read",
            "meta",
            "youtube_channel",
        ] {
            assert!(names.iter().any(|n| n == want), "missing {want}: {names:?}");
        }
        let v: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, schema::SCHEMA_VERSION);
    }

    #[test]
    fn foreign_keys_are_on_so_a_removed_feed_takes_its_entries_with_it() {
        let db = Db::open_in_memory().unwrap();
        let on: i64 = db
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            on, 1,
            "the cascade rules in the schema are decoration without this"
        );
    }

    #[test]
    fn meta_round_trips_and_overwrites() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.meta("nothing").unwrap(), None);
        db.set_meta("k", "one").unwrap();
        assert_eq!(db.meta("k").unwrap().as_deref(), Some("one"));
        db.set_meta("k", "two").unwrap();
        assert_eq!(db.meta("k").unwrap().as_deref(), Some("two"));
    }

    #[test]
    fn opening_a_file_twice_keeps_what_was_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("wire.db");
        {
            let db = Db::open(&path).unwrap();
            db.set_meta("k", "kept").unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.meta("k").unwrap().as_deref(), Some("kept"));
    }
}
