//! Where STAR/WIRE keeps its files.
//!
//! A name for [`starkit::paths::Paths`], and nothing else -- the layout
//! itself is documented there: everything under one directory,
//! `~/.local/starwire` by default, rather than scattered across the three
//! XDG roots, so the config, the database, the session and the log can be
//! backed up, moved between machines, or deleted by moving one folder.
//!
//! The database is the reason that matters more here than in the siblings.
//! `wire.db` holds every feed, every entry and every article ever extracted,
//! which is the only thing in this program a person would be sorry to lose,
//! and it lives beside the config rather than in a cache directory a cleaner
//! might empty.

// `own_dir` has no caller yet -- a private captures directory would use it,
// and there is not one -- but it is part of the paths vocabulary this file
// exists to name.
#[allow(unused_imports)]
pub use starkit::paths::{own_dir, Paths};

/// This application's directories. `$STARWIRE_DIR` moves everything;
/// `$STARWIRE_CONFIG_DIR` moves only the configuration, which is what a
/// dotfile manager wants.
pub const PATHS: Paths = Paths::new("starwire", "STARWIRE_DIR", "STARWIRE_CONFIG_DIR");

/// The one database file, under the data directory.
///
/// Named here rather than spelled out at each of the four call sites that
/// open it (`main`, the headless subcommands, `Handle::spawn`, the tests),
/// because a second spelling of it is a second database.
pub fn db_file() -> anyhow::Result<std::path::PathBuf> {
    Ok(PATHS.data_dir()?.join("wire.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// STAR/KIT's own tests cover the layout; what is worth asserting here is
    /// that this application's own three strings reach it -- an environment
    /// variable spelled for another program would relocate nothing.
    #[test]
    fn the_directory_override_wins() {
        let dir = tempfile::tempdir().unwrap();
        temp_env("STARWIRE_DIR", dir.path().as_os_str(), || {
            assert_eq!(PATHS.app(), "starwire");
            assert_eq!(PATHS.base_dir().unwrap(), dir.path());
            assert_eq!(PATHS.config_file().unwrap(), dir.path().join("config.toml"));
            assert_eq!(
                PATHS.log_dir().unwrap(),
                dir.path().join("cache"),
                "the log belongs in cache, not beside the config"
            );
            assert!(
                !db_file().unwrap().starts_with(dir.path().join("cache")),
                "the database is the one thing here nobody can rebuild; \
                 it must not sit where a cache cleaner will find it"
            );
        });
    }

    #[test]
    fn everything_hangs_off_one_base_directory() {
        if std::env::var_os("STARWIRE_DIR").is_some()
            || std::env::var_os("STARWIRE_CONFIG_DIR").is_some()
        {
            return;
        }
        let Ok(base) = PATHS.base_dir() else {
            return;
        };
        assert!(base.ends_with("starwire"));
        assert!(PATHS.config_file().unwrap().starts_with(&base));
        assert!(PATHS.cache_dir().unwrap().starts_with(&base));
        assert!(PATHS.log_dir().unwrap().starts_with(&base));
        assert!(PATHS.session_file().unwrap().starts_with(&base));
        assert!(db_file().unwrap().starts_with(&base));
    }

    /// `std::env::set_var` is unsafe in edition 2024 and racy in any edition;
    /// this is its only user, and it still runs on the process-wide table.
    fn temp_env(key: &str, value: &std::ffi::OsStr, f: impl FnOnce()) {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var_os(key);
        // SAFETY: serialised by `LOCK` above, which every caller of this
        // helper takes before touching the environment.
        unsafe { std::env::set_var(key, value) };
        f();
        match old {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
}
