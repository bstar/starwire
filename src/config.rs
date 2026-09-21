//! `config.toml`, and the template written beside it on a first run.
//!
//! Every field has a default, and a file that omits a table gets the whole
//! table's defaults -- the file is hand-edited, and a key nobody has typed
//! yet should cost a preference rather than a startup. Nothing here is a
//! secret, so this file is safe to copy between machines and safe to paste
//! into a bug report.
//!
//! Six tables, in two groups. `[fetch]`, `[articles]`, `[player]` and
//! `[youtube]` are the core's, and [`Config::core`] is the one place a key in
//! this file becomes a setting under `src/wire/`. `[ui]` and `[reading]` are
//! the window's and are read by `src/ui/`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::wire;

/// The whole of `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: Ui,
    pub reading: Reading,
    pub fetch: Fetch,
    pub articles: Articles,
    pub player: Player,
    pub youtube: Youtube,
}

/// How the column looks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    /// A theme id, or `"system"` to follow the desktop.
    pub theme: String,
    /// `auto`, `kitty`, `blocks` or `off`.
    pub graphics: String,
    /// Blank columns and rows kept around the whole layout, for terminals
    /// whose window has no padding of its own.
    pub padding_x: u16,
    pub padding_y: u16,
    /// How many rows a focused list opens to while an article is open behind
    /// it -- the "peek", which is what keeps the reader visible when the
    /// cursor moves down a long entry list.
    pub list_rows: u16,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: "catppuccin-mocha".into(),
            graphics: "auto".into(),
            padding_x: 0,
            padding_y: 0,
            list_rows: 12,
        }
    }
}

/// How an article reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reading {
    /// Columns of text, centred in the reader. `<` and `>` step it.
    pub width: u16,
    pub show_byline: bool,
    /// Opening an entry marks it read. `m` undoes it.
    pub mark_read_on_open: bool,
}

impl Default for Reading {
    fn default() -> Self {
        Self {
            // Eighty, which is what typography has said for a century and
            // what every one of these articles was written to be read at.
            width: 80,
            show_byline: true,
            mark_read_on_open: true,
        }
    }
}

/// How and how often feeds are pulled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fetch {
    pub refresh_minutes: u32,
    pub parallel: usize,
    pub timeout_secs: u64,
    pub max_feed_bytes: u64,
    pub min_host_interval_secs: u64,
    pub user_agent_extra: String,
    pub refresh_on_start: bool,
}

impl Default for Fetch {
    fn default() -> Self {
        let core = wire::FetchConfig::default();
        Self {
            refresh_minutes: core.refresh_minutes,
            parallel: core.parallel,
            timeout_secs: core.timeout_secs,
            max_feed_bytes: core.max_feed_bytes,
            min_host_interval_secs: core.min_host_interval_secs,
            user_agent_extra: core.user_agent_extra,
            refresh_on_start: core.refresh_on_start,
        }
    }
}

/// What happens to an entry once it has arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Articles {
    pub extract: bool,
    pub max_article_bytes: u64,
    pub max_markdown_bytes: usize,
    pub keep_days: u32,
    pub max_entries_per_feed: usize,
    pub images: bool,
    pub page_size: usize,
}

impl Default for Articles {
    fn default() -> Self {
        let core = wire::ArticlesConfig::default();
        Self {
            extract: core.extract,
            max_article_bytes: core.max_article_bytes,
            max_markdown_bytes: core.max_markdown_bytes,
            keep_days: core.keep_days,
            max_entries_per_feed: core.max_entries_per_feed,
            images: core.images,
            page_size: core.page_size,
        }
    }
}

/// What opens a link and what plays a video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Player {
    /// argv -- never a shell line. The URL is one more argument.
    pub video: Vec<String>,
    /// Empty is the desktop's own opener: `open` on macOS, `xdg-open`
    /// elsewhere.
    pub browser: Vec<String>,
}

impl Default for Player {
    fn default() -> Self {
        let core = wire::PlayerConfig::default();
        Self {
            video: core.video,
            browser: core.browser,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Youtube {
    pub cookies_from_browser: String,
    pub yt_dlp: String,
}

impl Default for Youtube {
    fn default() -> Self {
        let core = wire::YoutubeConfig::default();
        Self {
            cookies_from_browser: core.cookies_from_browser,
            yt_dlp: core.yt_dlp,
        }
    }
}

impl Config {
    /// Read the file, or the defaults if there is not one.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Write a commented starting file, if there is not one already. A
    /// template rather than a serialised `Config`, which would be correct
    /// and teach nobody anything. Returns whether it created the file.
    pub fn write_template(path: &Path) -> Result<bool> {
        if path.exists() {
            return Ok(false);
        }
        starkit::fs::write_atomic(path, TEMPLATE.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(true)
    }

    /// What the file says, translated into the core's own settings.
    ///
    /// The two sets of structs are separate on purpose -- nothing under
    /// `src/wire/` reads a config file -- and this is the one place a key
    /// added to `config.toml` and not carried across is a key that silently
    /// does nothing.
    pub fn core(&self) -> wire::WireConfig {
        wire::WireConfig {
            fetch: wire::FetchConfig {
                refresh_minutes: self.fetch.refresh_minutes,
                // At least one thread, or a refresh has nowhere to run. A
                // ceiling of sixteen because past that the limit is the
                // per-host gap rather than the thread count, and thirty-two
                // sockets on a feed list of forty is impolite.
                parallel: self.fetch.parallel.clamp(1, 16),
                timeout_secs: self.fetch.timeout_secs.max(1),
                max_feed_bytes: self.fetch.max_feed_bytes.max(1024),
                min_host_interval_secs: self.fetch.min_host_interval_secs,
                user_agent_extra: self.fetch.user_agent_extra.clone(),
                refresh_on_start: self.fetch.refresh_on_start,
            },
            articles: wire::ArticlesConfig {
                extract: self.articles.extract,
                max_article_bytes: self.articles.max_article_bytes.max(1024),
                max_markdown_bytes: self.articles.max_markdown_bytes.max(256),
                keep_days: self.articles.keep_days,
                max_entries_per_feed: self.articles.max_entries_per_feed,
                images: self.articles.images,
                page_size: self.articles.page_size.clamp(10, 5000),
            },
            player: wire::PlayerConfig {
                video: self.player.video.clone(),
                browser: self.player.browser.clone(),
            },
            youtube: wire::YoutubeConfig {
                cookies_from_browser: self.youtube.cookies_from_browser.clone(),
                // An empty `yt_dlp` would try to run the empty string, which
                // fails with a message about a file called "" rather than
                // about a setting.
                yt_dlp: if self.youtube.yt_dlp.trim().is_empty() {
                    wire::YoutubeConfig::default().yt_dlp
                } else {
                    self.youtube.yt_dlp.trim().to_string()
                },
            },
        }
    }
}

const TEMPLATE: &str = r#"# STAR/WIRE configuration.
#
# Everything STAR/WIRE keeps lives under one directory -- this file, the
# database, the session and the log. $STARWIRE_DIR relocates all of it.

[ui]
# "system" follows the desktop. Or name one of the built-in themes; `t` and
# `T` cycle through them while it is running.
theme = "catppuccin-mocha"
# How pictures are drawn, where they are drawn at all: auto, kitty, blocks,
# or off.
graphics = "auto"
# Blank cells around the whole layout, for a terminal whose window has none.
padding_x = 0
padding_y = 0
# How many rows a focused list opens to while an article is open behind it.
list_rows = 12

[reading]
# Columns of text, centred in the reader. `<` and `>` step this by eight.
width = 80
show_byline = true
# Opening an entry marks it read. `m` puts it back.
mark_read_on_open = true

[fetch]
# How often a background refresh of every feed starts, in minutes.
refresh_minutes = 15
# How many feeds are fetched at once.
parallel = 4
timeout_secs = 15
# A feed body larger than this is a failure rather than something to parse.
max_feed_bytes = 8388608
# The gap between two requests to one host. Requests to a host are serial
# regardless; this is how long the next one waits.
min_host_interval_secs = 2
# Appended to "starwire/<version> (+https://github.com/bstar/starwire)", if
# you would rather the sites you read had a way to reach you.
user_agent_extra = ""
refresh_on_start = true

[articles]
# Fetch the linked page and pull the article out of it. false leaves every
# entry on the text its feed carried, which for a full-text feed is the same
# thing and for a summary feed is two sentences.
extract = true
# The most HTML downloaded for one article.
max_article_bytes = 2097152
# Markdown longer than this is cut at a paragraph boundary.
max_markdown_bytes = 524288
# Entries older than this are swept. Starred entries are always kept.
keep_days = 30
# And the second bound, which is the one that matters for a busy feed.
max_entries_per_feed = 2000
# Keep images as images. false leaves the alt text behind as a paragraph.
# Nothing is downloaded either way in this release.
images = true
# How many entries are loaded at a time.
page_size = 200

[player]
# argv, never a shell line. The URL is one more argument.
video = ["mpv", "--terminal=no", "--"]
# Empty is the desktop's own opener: `open` on macOS, `xdg-open` elsewhere.
browser = []

[youtube]
# Which browser yt-dlp should take cookies from, for `starwire youtube sync`.
# Empty means that command is unavailable -- there is no way to read an
# account's subscriptions without them.
cookies_from_browser = ""
yt_dlp = "yt-dlp"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_defaults() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn a_partial_table_keeps_the_rest_of_its_defaults() {
        let c: Config = toml::from_str("[reading]\nwidth = 100\n").unwrap();
        assert_eq!(c.reading.width, 100);
        assert!(c.reading.show_byline);
        assert_eq!(c.fetch, Fetch::default());
    }

    #[test]
    fn the_template_parses_as_the_defaults() {
        let parsed: Config = toml::from_str(TEMPLATE).expect("the template must parse");
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn the_template_names_every_table_the_program_reads() {
        for table in [
            "[ui]",
            "[reading]",
            "[fetch]",
            "[articles]",
            "[player]",
            "[youtube]",
        ] {
            assert!(TEMPLATE.contains(table), "the template is missing {table}");
        }
    }

    #[test]
    fn an_unknown_table_is_ignored_rather_than_refused() {
        let text = "[ui]\npadding_x = 2\n\n[layout]\nsome_future_key = true\n";
        let c: Config = toml::from_str(text).expect("an unknown table still parses");
        assert_eq!(c.ui.padding_x, 2);
    }

    #[test]
    fn the_template_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        assert!(Config::write_template(&path).unwrap());
        std::fs::write(&path, "[ui]\npadding_x = 7\n").unwrap();
        assert!(!Config::write_template(&path).unwrap());
        assert_eq!(Config::load(&path).unwrap().ui.padding_x, 7);
    }

    #[test]
    fn a_missing_file_loads_as_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("nothing.toml")).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn every_setting_the_core_reads_is_carried_across() {
        let cfg = Config {
            fetch: Fetch {
                refresh_minutes: 33,
                parallel: 7,
                timeout_secs: 9,
                max_feed_bytes: 4096,
                min_host_interval_secs: 5,
                user_agent_extra: "contact@example.org".into(),
                refresh_on_start: false,
            },
            articles: Articles {
                extract: false,
                max_article_bytes: 4096,
                max_markdown_bytes: 1024,
                keep_days: 7,
                max_entries_per_feed: 50,
                images: false,
                page_size: 25,
            },
            player: Player {
                video: vec!["vlc".into()],
                browser: vec!["firefox".into()],
            },
            youtube: Youtube {
                cookies_from_browser: "firefox".into(),
                yt_dlp: "/usr/bin/yt-dlp".into(),
            },
            ..Config::default()
        };
        let core = cfg.core();

        assert_eq!(core.fetch.refresh_minutes, 33);
        assert_eq!(core.fetch.parallel, 7);
        assert_eq!(core.fetch.timeout_secs, 9);
        assert_eq!(core.fetch.max_feed_bytes, 4096);
        assert_eq!(core.fetch.min_host_interval_secs, 5);
        assert_eq!(core.fetch.user_agent_extra, "contact@example.org");
        assert!(!core.fetch.refresh_on_start);

        assert!(!core.articles.extract);
        assert_eq!(core.articles.max_article_bytes, 4096);
        assert_eq!(core.articles.max_markdown_bytes, 1024);
        assert_eq!(core.articles.keep_days, 7);
        assert_eq!(core.articles.max_entries_per_feed, 50);
        assert!(!core.articles.images);
        assert_eq!(core.articles.page_size, 25);

        assert_eq!(core.player.video, vec!["vlc"]);
        assert_eq!(core.player.browser, vec!["firefox"]);
        assert_eq!(core.youtube.cookies_from_browser, "firefox");
        assert_eq!(core.youtube.yt_dlp, "/usr/bin/yt-dlp");
    }

    #[test]
    fn the_defaults_agree_with_the_cores_defaults() {
        assert_eq!(Config::default().core(), wire::WireConfig::default());
    }

    #[test]
    fn a_setting_that_would_break_the_program_is_clamped_rather_than_obeyed() {
        let mut cfg = Config::default();
        cfg.fetch.parallel = 0;
        cfg.fetch.timeout_secs = 0;
        cfg.articles.page_size = 0;
        cfg.youtube.yt_dlp = "   ".into();
        let core = cfg.core();
        assert_eq!(core.fetch.parallel, 1, "a refresh needs somewhere to run");
        assert_eq!(core.fetch.timeout_secs, 1);
        assert_eq!(core.articles.page_size, 10);
        assert_eq!(core.youtube.yt_dlp, "yt-dlp");

        cfg.fetch.parallel = 10_000;
        assert_eq!(cfg.core().fetch.parallel, 16);
    }

    #[test]
    fn a_config_round_trips_through_toml() {
        let cfg = Config {
            reading: Reading {
                width: 100,
                show_byline: false,
                mark_read_on_open: false,
            },
            ..Config::default()
        };
        let text = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
    }
}
