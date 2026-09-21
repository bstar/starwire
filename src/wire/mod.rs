//! Everything that knows what a feed, an entry and an article are, and
//! nothing that knows a terminal exists.
//!
//! **Nothing under `src/wire/` may reach for `ratatui`, `crossterm` or the UI
//! module.** The test at the bottom of this file greps the module's own
//! sources and fails on the line that broke it. That is the boundary every
//! headless subcommand is built on: `starwire fetch` has to run from a
//! systemd timer with no TTY attached, `starwire extract <url>` has to print
//! markdown into a pipe, and a change to how a row is drawn must not be able
//! to break how a page is read.
//!
//! The shape:
//!
//! - [`feed`] is the vocabulary: the three row ids, what kind of thing a feed
//!   and an entry are, and the conversion from what `feed-rs` parsed.
//! - [`net`] is the one way out to the network, and the only place a URL is
//!   turned into bytes; [`fetch`] is one feed's round trip through it.
//! - [`extract`] is the part people actually came for: a page in, clean
//!   CommonMark out.
//! - [`db`] is the one thing that writes, and the whole of what is kept.
//! - [`youtube`] and [`import`] are the two ways a feed list arrives.
//! - [`open`] hands a link to a browser or a player; [`search`] is the `/`
//!   filter over rows already loaded.
//!
//! The core stores **CommonMark text**, not a parsed document: the UI parses
//! it with `pulldown-cmark` at the width it is drawing, and nothing under
//! here has an opinion about how a heading looks.
//!
//! - [`state`] is the truth and the one function allowed to change it;
//!   [`worker`] is the threads that do everything else; [`handle`] is the
//!   contract the window programs against. None of the three is needed by a
//!   headless subcommand: those open a [`db::Db`], build an [`net::Http`],
//!   and call the same functions the threads call.

pub mod db;
pub mod extract;
pub mod feed;
pub mod fetch;
pub mod handle;
pub mod import;
pub mod net;
pub mod open;
pub mod search;
pub mod state;
#[cfg(test)]
pub mod testing;
pub mod worker;
pub mod youtube;

// The contract the window programs against, spelled here so `src/ui/` says
// `wire::Command` rather than `wire::handle::Command`. Exactly what is
// reached for: a `pub use` nothing has asked for is a warning in a binary
// crate, and the rest of each module is public where it lives.
pub use handle::{Command, Event, Handle, ImportOffer, NoteLevel, OpenKind, RefreshScope, Setting};
pub use state::{RefreshProgress, State};

/// Everything the terminal hands the core once, at startup.
///
/// Built by `config::Config::core`, which is the one place a key in
/// `config.toml` becomes a setting here -- nothing under `src/wire/` reads a
/// config file itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WireConfig {
    pub fetch: FetchConfig,
    pub articles: ArticlesConfig,
    pub player: PlayerConfig,
    pub youtube: YoutubeConfig,
}

/// How and how often feeds are pulled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchConfig {
    /// Minutes between background refreshes of every feed.
    pub refresh_minutes: u32,
    /// How many `starwire-net` threads there are, and so how many feeds are
    /// in flight at once.
    pub parallel: usize,
    pub timeout_secs: u64,
    /// A feed body larger than this is a fetch failure rather than something
    /// to parse. A feed is a summary; one that is eight megabytes is either
    /// broken or not a feed.
    pub max_feed_bytes: u64,
    /// The gap between two requests to the same host. Requests to one host
    /// are serial regardless; this is how long the second one waits after
    /// the first finishes.
    pub min_host_interval_secs: u64,
    /// Appended to the user agent, for a person who wants a contact address
    /// of their own in the logs of the sites they read.
    pub user_agent_extra: String,
    pub refresh_on_start: bool,
    /// Gaps for particular hosts, by host or by a dot-prefixed tail of one.
    /// Beats both the default above and the table `wire::net` keeps of the
    /// hosts a feed list of any size meets, which is what makes "the site I
    /// read has asked me to slow down" a line in a file rather than a
    /// release.
    pub host_intervals: std::collections::BTreeMap<String, u64>,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            refresh_minutes: 15,
            parallel: 4,
            timeout_secs: 15,
            max_feed_bytes: 8_388_608,
            min_host_interval_secs: 2,
            user_agent_extra: String::new(),
            refresh_on_start: true,
            host_intervals: std::collections::BTreeMap::new(),
        }
    }
}

/// What is done with an entry once it has arrived, and how long it is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticlesConfig {
    /// Fetch and reduce the linked page. `false` leaves every entry on the
    /// text the feed itself carried, which for a full-text feed is the same
    /// thing and for a summary feed is two sentences.
    pub extract: bool,
    /// The most HTML downloaded for one article.
    pub max_article_bytes: u64,
    /// The whole of one page request, connection and body included. Longer
    /// than a feed's, because a feed is a file a server has already made and
    /// an article is often rendered when it is asked for -- six of the
    /// reference database's failures were timeouts on heavy pages behind
    /// redirect wrappers.
    pub timeout_secs: u64,
    /// Markdown above this is truncated at a paragraph boundary.
    pub max_markdown_bytes: usize,
    /// Entries older than this are swept, unless starred.
    pub keep_days: u32,
    /// The second bound on retention, and the one that matters for a busy
    /// feed: `hnrss/newcomments` alone is about a hundred thousand rows a
    /// month, and `keep_days` on its own would keep every one of them.
    pub max_entries_per_feed: usize,
    /// Keep `![alt](src)` in the markdown. `false` leaves the alt text
    /// behind as a paragraph instead. Nothing is downloaded either way in
    /// 0.0.1 -- the reader draws a kept image as an `[image: alt]` line.
    pub images: bool,
    pub page_size: usize,
}

impl Default for ArticlesConfig {
    fn default() -> Self {
        Self {
            extract: true,
            max_article_bytes: 2_097_152,
            timeout_secs: 30,
            max_markdown_bytes: 524_288,
            keep_days: 30,
            max_entries_per_feed: 2000,
            images: true,
            page_size: 200,
        }
    }
}

/// argv for the two external programs. Never a shell line -- a title with a
/// semicolon in it is a title, not a chance to inject a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerConfig {
    /// What a video entry is handed to. The URL is one more argument.
    pub video: Vec<String>,
    /// What a link is handed to. Empty is the desktop's own opener.
    pub browser: Vec<String>,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            // `--terminal=no` because mpv otherwise takes over the terminal
            // this program is drawing in, and `--` because a URL that begins
            // with a dash is still a URL.
            video: vec!["mpv".into(), "--terminal=no".into(), "--".into()],
            browser: Vec::new(),
        }
    }
}

/// How YouTube channels are resolved and synced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YoutubeConfig {
    /// Passed to `yt-dlp --cookies-from-browser` when it is not empty. There
    /// is no way to read an account's real subscription list without them.
    pub cookies_from_browser: String,
    pub yt_dlp: String,
}

impl Default for YoutubeConfig {
    fn default() -> Self {
        Self {
            cookies_from_browser: String::new(),
            yt_dlp: "yt-dlp".into(),
        }
    }
}

impl WireConfig {
    /// The `User-Agent` every request in this program goes out with.
    ///
    /// Naming the program and linking the repository is not decoration.
    /// Reddit answers 429 to a client it cannot identify, and several of the
    /// sites in a typical feed list will block a default agent string
    /// outright; a name and a contact URL is what lets an administrator who
    /// is unhappy about the traffic find out who to ask.
    pub fn user_agent(&self) -> String {
        let base = concat!(
            "starwire/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/bstar/starwire)"
        );
        let extra = self.fetch.user_agent_extra.trim();
        if extra.is_empty() {
            base.to_string()
        } else {
            format!("{base} {extra}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule this module exists to keep. Copied from `fold/mod.rs` in
    /// STAR/FOLD, which took it from STAR/CORD: a grep rather than a crate
    /// boundary, because `starwire` is a single binary crate and there is no
    /// `wire` crate for Cargo to keep `ratatui` out of.
    #[test]
    fn nothing_in_the_core_knows_about_the_terminal() {
        const FORBIDDEN: &[&str] = &[
            "use ratatui",   // NO-TERMINAL-HERE
            "ratatui::",     // NO-TERMINAL-HERE
            "use crossterm", // NO-TERMINAL-HERE
            "crossterm::",   // NO-TERMINAL-HERE
            "crate::ui",     // NO-TERMINAL-HERE
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("wire");
        let mut offences = Vec::new();
        let mut files = 0usize;

        walk(&root, &mut |path| {
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                return;
            }
            files += 1;
            let Ok(text) = std::fs::read_to_string(path) else {
                return;
            };
            for (number, line) in text.lines().enumerate() {
                if line.contains("NO-TERMINAL-HERE") {
                    continue;
                }
                for needle in FORBIDDEN {
                    if line.contains(needle) {
                        offences.push(format!(
                            "{}:{}: {}",
                            path.display(),
                            number + 1,
                            line.trim()
                        ));
                    }
                }
            }
        });

        assert!(
            files > 10,
            "only {files} files were scanned; the walk is wrong"
        );
        assert!(
            offences.is_empty(),
            "the wire core reached for the terminal:\n{}",
            offences.join("\n")
        );
    }

    fn walk(dir: &std::path::Path, f: &mut impl FnMut(&std::path::Path)) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, f);
            } else {
                f(&path);
            }
        }
    }

    #[test]
    fn the_user_agent_names_the_program_and_where_to_complain() {
        let cfg = WireConfig::default();
        let ua = cfg.user_agent();
        assert!(ua.starts_with("starwire/"), "{ua}");
        assert!(ua.contains("github.com/bstar/starwire"), "{ua}");
        assert!(ua.contains(env!("CARGO_PKG_VERSION")), "{ua}");
    }

    #[test]
    fn the_user_agent_extra_is_appended_rather_than_replacing() {
        let mut cfg = WireConfig::default();
        cfg.fetch.user_agent_extra = "  contact@example.org  ".into();
        let ua = cfg.user_agent();
        assert!(ua.starts_with("starwire/"), "{ua}");
        assert!(ua.ends_with("contact@example.org"), "{ua}");
    }
}
