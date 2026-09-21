//! The command line.
//!
//! `starwire` with no arguments opens the window. Everything else is
//! headless, and that is not a consolation prize: `starwire fetch` from a
//! systemd timer is how a reader's feeds are already up to date when they
//! open it, `starwire extract <url>` is the probe that says whether a site
//! yields to the scraper at all, and `import`, `export` and `youtube` are
//! how a feed list gets in and out without a window ever being drawn.
//!
//! None of these commands touch `Handle` or `State`: they open a
//! [`crate::wire::db::Db`], build an [`crate::wire::net::Http`], and call the
//! same functions the worker threads call.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "starwire",
    version,
    about = "STAR/WIRE — a stack-based terminal news reader",
    long_about = None,
)]
pub struct Cli {
    /// Log at debug level. To the log file, never to the terminal.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Serve a directory of saved responses instead of the network.
    ///
    /// The directory holds an `index.tsv` of `URL<TAB>file` lines and the
    /// files beside it. Nothing reaches a socket while this is set.
    #[arg(long, global = true, value_name = "DIR")]
    pub replay: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Refresh every feed, or one, and exit. For a systemd timer.
    Fetch {
        /// One feed, by URL or by id. Default: all of them.
        #[arg(long, value_name = "URL|ID")]
        feed: Option<String>,

        /// Store what each feed carried and do not fetch any pages.
        #[arg(long)]
        no_extract: bool,
    },

    /// List entries, or feeds.
    List {
        /// Only entries nothing has read.
        #[arg(long)]
        unread: bool,

        /// Only this feed's entries.
        #[arg(long, value_name = "URL|ID")]
        feed: Option<String>,

        /// List the feeds themselves, with their unread counts and last
        /// error, rather than entries.
        #[arg(long)]
        feeds: bool,

        #[arg(long, default_value_t = 50)]
        limit: usize,

        /// One JSON object per line, for a script.
        #[arg(long)]
        json: bool,
    },

    /// Print one entry.
    Show {
        /// The entry id, as `list` prints it.
        entry: i64,

        #[command(flatten)]
        format: ShowFormat,
    },

    /// Subscribe to a feed.
    Add {
        /// A feed URL, a YouTube channel URL, an `@handle`, or a `UC…`.
        url: String,

        #[arg(long, value_name = "TITLE")]
        title: Option<String>,

        #[arg(long, value_name = "FOLDER")]
        folder: Option<String>,
    },

    /// Unsubscribe, and forget its entries.
    Remove {
        /// The feed, by URL or by id.
        feed: String,
    },

    /// Bring a feed list in from somewhere else.
    #[command(subcommand)]
    Import(ImportCommand),

    /// Write the feed list out.
    #[command(subcommand)]
    Export(ExportCommand),

    /// YouTube channels.
    #[command(subcommand)]
    Youtube(YoutubeCommand),

    /// Run the scraper on one page and print the markdown.
    ///
    /// The probe: what a site yields, before subscribing to it.
    Extract { url: String },
}

/// How `show` prints an entry. Exactly one, and the default is the header
/// and the markdown -- which is what a person wants; the other two are for a
/// script.
#[derive(Debug, Args)]
#[group(multiple = false)]
pub struct ShowFormat {
    /// The markdown alone, with no header.
    #[arg(long)]
    pub markdown: bool,

    /// The HTML the feed carried, if it carried any.
    #[arg(long)]
    pub html: bool,

    /// The entry's link, and nothing else.
    #[arg(long)]
    pub url: bool,
}

#[derive(Debug, Subcommand)]
pub enum ImportCommand {
    /// Feeds, and read state, from newsboat.
    Newsboat {
        /// Default: `~/.config/newsboat/urls`, then `~/.newsboat/urls`.
        #[arg(long, value_name = "PATH")]
        urls: Option<PathBuf>,

        /// Default: `~/.local/share/newsboat/cache.db`. Read-only, and only
        /// the titles and the read marks are read out of it.
        #[arg(long, value_name = "PATH")]
        cache: Option<PathBuf>,

        /// Print what would happen and write nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Feeds from an OPML file.
    Opml {
        file: PathBuf,

        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ExportCommand {
    /// The feed list as OPML, on stdout unless a file is named.
    Opml { file: Option<PathBuf> },
}

#[derive(Debug, Subcommand)]
pub enum YoutubeCommand {
    /// Subscribe to a channel.
    Add {
        /// A channel URL, a video URL, an `@handle`, or a `UC…`.
        what: String,
    },

    /// Subscribe to everything in a Google Takeout `subscriptions.csv`.
    ImportTakeout {
        file: PathBuf,

        #[arg(long)]
        dry_run: bool,
    },

    /// Subscribe to what the account is actually subscribed to, through
    /// yt-dlp and the browser's cookies.
    Sync {
        /// Which browser to take cookies from. Overrides
        /// `[youtube] cookies_from_browser`.
        #[arg(long, value_name = "BROWSER")]
        cookies_from_browser: Option<String>,

        #[arg(long)]
        dry_run: bool,
    },

    /// The channels already subscribed to.
    List,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn the_definition_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_arguments_opens_the_window() {
        let cli = Cli::parse_from(["starwire"]);
        assert!(cli.command.is_none());
        assert!(!cli.verbose);
        assert!(cli.replay.is_none());
    }

    #[test]
    fn replay_and_verbose_are_accepted_before_and_after_a_subcommand() {
        let before = Cli::parse_from(["starwire", "--replay", "d", "--verbose", "fetch"]);
        assert_eq!(before.replay.as_deref(), Some(std::path::Path::new("d")));
        assert!(before.verbose);
        let after = Cli::parse_from(["starwire", "fetch", "--replay", "d", "--verbose"]);
        assert_eq!(after.replay.as_deref(), Some(std::path::Path::new("d")));
        assert!(after.verbose);
    }

    #[test]
    fn fetch_takes_one_feed_or_all_of_them() {
        match Cli::parse_from(["starwire", "fetch"]).command {
            Some(Command::Fetch { feed, no_extract }) => {
                assert!(feed.is_none());
                assert!(!no_extract);
            }
            other => panic!("{other:?}"),
        }
        match Cli::parse_from(["starwire", "fetch", "--feed", "3", "--no-extract"]).command {
            Some(Command::Fetch { feed, no_extract }) => {
                assert_eq!(feed.as_deref(), Some("3"));
                assert!(no_extract);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn list_defaults_to_fifty_entries_and_takes_the_four_narrowings() {
        match Cli::parse_from(["starwire", "list"]).command {
            Some(Command::List {
                unread,
                feed,
                feeds,
                limit,
                json,
            }) => {
                assert!(!unread && feed.is_none() && !feeds && !json);
                assert_eq!(limit, 50);
            }
            other => panic!("{other:?}"),
        }
        match Cli::parse_from([
            "starwire", "list", "--unread", "--feed", "x", "--limit", "5", "--json",
        ])
        .command
        {
            Some(Command::List {
                unread,
                feed,
                limit,
                json,
                ..
            }) => {
                assert!(unread && json);
                assert_eq!(feed.as_deref(), Some("x"));
                assert_eq!(limit, 5);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn show_takes_an_entry_id_and_at_most_one_format() {
        match Cli::parse_from(["starwire", "show", "12", "--markdown"]).command {
            Some(Command::Show { entry, format }) => {
                assert_eq!(entry, 12);
                assert!(format.markdown);
                assert!(!format.html && !format.url);
            }
            other => panic!("{other:?}"),
        }
        assert!(
            Cli::try_parse_from(["starwire", "show", "1", "--markdown", "--html"]).is_err(),
            "two formats at once is not a question with an answer"
        );
    }

    #[test]
    fn import_newsboat_finds_its_own_files_unless_told_otherwise() {
        match Cli::parse_from(["starwire", "import", "newsboat"]).command {
            Some(Command::Import(ImportCommand::Newsboat {
                urls,
                cache,
                dry_run,
            })) => {
                assert!(urls.is_none() && cache.is_none() && !dry_run);
            }
            other => panic!("{other:?}"),
        }
        match Cli::parse_from([
            "starwire",
            "import",
            "newsboat",
            "--urls",
            "u",
            "--cache",
            "c",
            "--dry-run",
        ])
        .command
        {
            Some(Command::Import(ImportCommand::Newsboat {
                urls,
                cache,
                dry_run,
            })) => {
                assert_eq!(urls.as_deref(), Some(std::path::Path::new("u")));
                assert_eq!(cache.as_deref(), Some(std::path::Path::new("c")));
                assert!(dry_run);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn export_opml_goes_to_stdout_unless_a_file_is_named() {
        match Cli::parse_from(["starwire", "export", "opml"]).command {
            Some(Command::Export(ExportCommand::Opml { file })) => assert!(file.is_none()),
            other => panic!("{other:?}"),
        }
        match Cli::parse_from(["starwire", "export", "opml", "out.opml"]).command {
            Some(Command::Export(ExportCommand::Opml { file })) => {
                assert_eq!(file.as_deref(), Some(std::path::Path::new("out.opml")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_youtube_subcommand_parses() {
        assert!(matches!(
            Cli::parse_from(["starwire", "youtube", "add", "@name"]).command,
            Some(Command::Youtube(YoutubeCommand::Add { .. }))
        ));
        assert!(matches!(
            Cli::parse_from(["starwire", "youtube", "import-takeout", "s.csv"]).command,
            Some(Command::Youtube(YoutubeCommand::ImportTakeout { .. }))
        ));
        match Cli::parse_from([
            "starwire",
            "youtube",
            "sync",
            "--cookies-from-browser",
            "firefox",
        ])
        .command
        {
            Some(Command::Youtube(YoutubeCommand::Sync {
                cookies_from_browser,
                ..
            })) => assert_eq!(cookies_from_browser.as_deref(), Some("firefox")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            Cli::parse_from(["starwire", "youtube", "list"]).command,
            Some(Command::Youtube(YoutubeCommand::List))
        ));
    }

    #[test]
    fn add_and_remove_take_what_a_person_would_type() {
        match Cli::parse_from([
            "starwire",
            "add",
            "@veritasium",
            "--folder",
            "Video",
            "--title",
            "V",
        ])
        .command
        {
            Some(Command::Add { url, title, folder }) => {
                assert_eq!(url, "@veritasium");
                assert_eq!(title.as_deref(), Some("V"));
                assert_eq!(folder.as_deref(), Some("Video"));
            }
            other => panic!("{other:?}"),
        }
        match Cli::parse_from(["starwire", "remove", "3"]).command {
            Some(Command::Remove { feed }) => assert_eq!(feed, "3"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn extract_takes_one_url() {
        match Cli::parse_from(["starwire", "extract", "https://example.org/x"]).command {
            Some(Command::Extract { url }) => assert_eq!(url, "https://example.org/x"),
            other => panic!("{other:?}"),
        }
    }
}
