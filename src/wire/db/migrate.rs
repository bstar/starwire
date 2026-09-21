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
use rusqlite::Connection;

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
        // Each future version gets an arm here, and the arms fall through:
        // a file at 1 runs 1->2 then 2->3, which is what makes a build that
        // skipped three releases work.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(schema::PRAGMAS).unwrap();
        conn.execute_batch(schema::SCHEMA).unwrap();
        conn
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
