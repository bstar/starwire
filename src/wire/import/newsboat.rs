//! Reading newsboat's two files.
//!
//! `~/.config/newsboat/urls` is the feed list. Its format is a line per feed:
//! the URL, then any number of tags, of which one beginning with `~` is a
//! title rather than a tag. Tags may be quoted. A line beginning with `#` is
//! a comment, a URL prefixed with `!` is a feed newsboat hides from the list,
//! and three kinds of line are not feeds at all -- `query:` saved searches,
//! `filter:` script-filtered feeds and `exec:` command feeds -- because all
//! three name something newsboat itself computes rather than something to
//! fetch.
//!
//! `~/.local/share/newsboat/cache.db` is the read state. It is opened
//! **read-only and immutable**, and only five columns are ever read from it:
//! `rss_feed(rssurl, title)`, and `rss_item(feedurl, guid, url)` for the rows
//! marked read. Never `rss_item.content` -- the reference cache is 123 MB and
//! almost all of it is article bodies this program is about to fetch fresh
//! copies of anyway.
//!
//! The titles are worth more than they look. The reference `urls` file has no
//! `~title` on any of its forty-one lines, so `rss_feed.title` is the only
//! place thirty-four of those feeds have a name at all before the first
//! fetch.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};

/// One line of a `urls` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlsLine {
    Blank,
    /// `# something`, kept so that a rendered file is the same file.
    Comment(String),
    Feed(FeedLine),
    /// A line that is not a feed to fetch, with the reason it was left out.
    Skipped { raw: String, reason: &'static str },
}

/// A feed line, taken apart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedLine {
    pub url: String,
    /// `!url`: newsboat keeps the feed but hides it from the list. Imported
    /// like any other -- STAR/WIRE has no hidden feeds, and dropping them
    /// would silently lose subscriptions.
    pub hidden: bool,
    /// From a `~Title` tag.
    pub title: Option<String>,
    pub tags: Vec<String>,
}

/// Parse a whole `urls` file.
///
/// Total: every line becomes something, and nothing is an error. A file
/// somebody has hand-edited for ten years has lines in it nobody remembers
/// writing, and an import that refuses the file over one of them is an
/// import that does not happen.
pub fn parse_urls(text: &str) -> Vec<UrlsLine> {
    text.lines().map(parse_line).collect()
}

fn parse_line(raw: &str) -> UrlsLine {
    let line = raw.trim();
    if line.is_empty() {
        return UrlsLine::Blank;
    }
    if let Some(rest) = line.strip_prefix('#') {
        return UrlsLine::Comment(rest.trim().to_string());
    }

    let tokens = tokenise(line);
    let Some(first) = tokens.first() else {
        return UrlsLine::Blank;
    };

    // The three newsboat computes rather than fetches. Checked on the
    // unquoted token, because `"query:Unread:unread = \"yes\""` is one
    // quoted token and the bare form is another.
    for prefix in ["query:", "filter:", "exec:"] {
        if first.starts_with(prefix) {
            return UrlsLine::Skipped {
                raw: line.to_string(),
                reason: match prefix {
                    "query:" => "a newsboat saved search, not a feed",
                    "filter:" => "a script-filtered feed; the script is newsboat's",
                    _ => "a command feed; the command is newsboat's",
                },
            };
        }
    }

    let (hidden, url) = match first.strip_prefix('!') {
        Some(rest) => (true, rest.to_string()),
        None => (false, first.clone()),
    };
    if url.is_empty() {
        return UrlsLine::Skipped {
            raw: line.to_string(),
            reason: "no address on the line",
        };
    }

    let mut title = None;
    let mut tags = Vec::new();
    for token in &tokens[1..] {
        match token.strip_prefix('~') {
            // The last `~title` wins, which is what newsboat does.
            Some(t) if !t.is_empty() => title = Some(t.to_string()),
            Some(_) => {}
            None if !token.is_empty() => tags.push(token.clone()),
            None => {}
        }
    }

    UrlsLine::Feed(FeedLine {
        url,
        hidden,
        title,
        tags,
    })
}

/// Split a line into tokens, honouring double quotes and backslash escapes.
///
/// newsboat's own tokeniser, near enough: a quoted token may contain spaces,
/// `\"` is a literal quote inside one, and `\\` is a backslash. Everything
/// else after a backslash is that character, which is what keeps a Windows
/// path or a stray backslash in a title from eating the rest of the line.
fn tokenise(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut in_quotes = false;
    let mut chars = line.chars();

    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                started = true;
                match chars.next() {
                    Some(next) => current.push(next),
                    // A trailing backslash is a literal one rather than the
                    // start of an escape that never arrives.
                    None => current.push('\\'),
                }
            }
            '"' => {
                started = true;
                in_quotes = !in_quotes;
            }
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                started = true;
                current.push(c);
            }
        }
    }
    if started {
        out.push(current);
    }
    out
}

/// A token, quoted if it needs to be.
fn render_token(token: &str) -> String {
    let needs_quotes = token.is_empty()
        || token.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\');
    if !needs_quotes {
        return token.to_string();
    }
    let escaped = token.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// A parsed line, written back out.
///
/// Exists so that the round trip can be tested: a parser for a format this
/// loose is only as trustworthy as the proof that it did not lose anything,
/// and `parse(render(line)) == line` over generated lines is that proof.
pub fn render_line(line: &UrlsLine) -> String {
    match line {
        UrlsLine::Blank => String::new(),
        UrlsLine::Comment(text) => format!("# {text}"),
        UrlsLine::Skipped { raw, .. } => raw.clone(),
        UrlsLine::Feed(feed) => {
            let mut out = String::new();
            if feed.hidden {
                out.push('!');
            }
            out.push_str(&render_token(&feed.url));
            for tag in &feed.tags {
                out.push(' ');
                out.push_str(&render_token(tag));
            }
            if let Some(title) = &feed.title {
                out.push(' ');
                out.push_str(&render_token(&format!("~{title}")));
            }
            out
        }
    }
}

/// What was read out of `cache.db`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheRows {
    /// `rssurl` to `title`. The only place most feeds have a name.
    pub titles: HashMap<String, String>,
    /// The entries newsboat has marked read.
    pub read: Vec<ReadMark>,
}

/// One read entry, keyed the way newsboat keys it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadMark {
    pub feed_url: String,
    pub guid: String,
    pub url: Option<String>,
}

/// Read newsboat's cache, without writing to it and without reading the
/// bodies.
///
/// `SQLITE_OPEN_READ_ONLY` plus `immutable=1`: read-only stops this from
/// touching a file another program owns, and immutable stops SQLite creating
/// a `-wal` or `-shm` beside it -- which it will do even on a read, and
/// which on a file newsboat is running against is at best confusing and at
/// worst a corrupted cache.
pub fn read_cache(path: &Path) -> Result<CacheRows> {
    anyhow::ensure!(path.exists(), "{} is not there", path.display());
    let uri = format!(
        "file:{}?immutable=1&mode=ro",
        path.to_string_lossy().replace('?', "%3f").replace('#', "%23")
    );
    let conn = rusqlite::Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening {}", path.display()))?;
    read_cache_conn(&conn)
}

/// The same, against an already-open connection. What the tests drive, so
/// that the fixture can be built in memory from newsboat's own DDL rather
/// than by checking a 123 MB file into the repository.
pub fn read_cache_conn(conn: &rusqlite::Connection) -> Result<CacheRows> {
    let mut titles = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT rssurl, title FROM rss_feed")
            .context("reading rss_feed; is this a newsboat cache?")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (url, title) = row?;
            if let Some(title) = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
                titles.insert(url, title);
            }
        }
    }

    let mut read = Vec::new();
    {
        // Five columns, none of them `content`. The reference cache has
        // 121,117 rows in `rss_item` and is 123 MB; reading the bodies would
        // be most of that for text this program is about to fetch again.
        let mut stmt = conn
            .prepare(
                "SELECT feedurl, guid, url FROM rss_item
                 WHERE unread = 0 AND (deleted IS NULL OR deleted = 0)",
            )
            .context("reading rss_item; is this a newsboat cache?")?;
        let rows = stmt.query_map([], |r| {
            Ok(ReadMark {
                feed_url: r.get(0)?,
                guid: r.get(1)?,
                url: r.get(2)?,
            })
        })?;
        for row in rows {
            let mark = row?;
            if !mark.feed_url.is_empty() && !mark.guid.is_empty() {
                read.push(mark);
            }
        }
    }

    Ok(CacheRows { titles, read })
}

/// The paths newsboat uses, in the order it looks for them.
pub fn default_urls_paths(home: &Path) -> Vec<std::path::PathBuf> {
    vec![
        home.join(".config/newsboat/urls"),
        home.join(".newsboat/urls"),
    ]
}

pub fn default_cache_paths(home: &Path) -> Vec<std::path::PathBuf> {
    vec![
        home.join(".local/share/newsboat/cache.db"),
        home.join(".newsboat/cache.db"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(line: &str) -> FeedLine {
        match parse_line(line) {
            UrlsLine::Feed(f) => f,
            other => panic!("{line:?} parsed as {other:?}"),
        }
    }

    #[test]
    fn a_bare_url_is_a_feed_with_nothing_else_on_it() {
        let got = feed("https://example.org/feed.xml");
        assert_eq!(got.url, "https://example.org/feed.xml");
        assert!(!got.hidden);
        assert!(got.title.is_none());
        assert!(got.tags.is_empty());
    }

    #[test]
    fn tags_and_a_title_come_off_the_line() {
        let got = feed(r#"https://example.org/feed.xml tech "two words" ~"My Title""#);
        assert_eq!(got.tags, vec!["tech", "two words"]);
        assert_eq!(got.title.as_deref(), Some("My Title"));
    }

    #[test]
    fn an_unquoted_title_works_too() {
        let got = feed("https://example.org/feed.xml ~Title");
        assert_eq!(got.title.as_deref(), Some("Title"));
    }

    #[test]
    fn a_hidden_feed_is_still_a_feed() {
        let got = feed("!https://example.org/feed.xml tech");
        assert!(got.hidden);
        assert_eq!(got.url, "https://example.org/feed.xml");
        assert_eq!(got.tags, vec!["tech"]);
    }

    #[test]
    fn comments_and_blank_lines_are_kept_as_what_they_are() {
        assert_eq!(parse_line(""), UrlsLine::Blank);
        assert_eq!(parse_line("   "), UrlsLine::Blank);
        assert_eq!(
            parse_line("# Hacker News"),
            UrlsLine::Comment("Hacker News".into())
        );
        assert_eq!(parse_line("#"), UrlsLine::Comment(String::new()));
    }

    #[test]
    fn the_three_kinds_of_line_newsboat_computes_are_skipped_with_a_reason() {
        for line in [
            r#""query:Unread:unread = \"yes\"""#,
            "query:Everything:title =~ \".\"",
            "filter:~/bin/strip.sh:https://example.org/feed",
            "exec:~/bin/make-a-feed.sh",
        ] {
            match parse_line(line) {
                UrlsLine::Skipped { reason, .. } => assert!(!reason.is_empty(), "{line}"),
                other => panic!("{line:?} parsed as {other:?}"),
            }
        }
    }

    #[test]
    fn an_escaped_quote_inside_a_tag_survives() {
        let got = feed(r#"https://e.org/f "a \"quoted\" tag""#);
        assert_eq!(got.tags, vec![r#"a "quoted" tag"#]);
    }

    #[test]
    fn a_trailing_backslash_does_not_eat_the_line() {
        let got = feed(r"https://e.org/f tag\");
        assert_eq!(got.tags, vec![r"tag\"]);
    }

    #[test]
    fn the_reference_urls_fixture_parses_into_feeds_and_comments() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/import/urls"),
        )
        .unwrap();
        let lines = parse_urls(&text);
        let feeds: Vec<&FeedLine> = lines
            .iter()
            .filter_map(|l| match l {
                UrlsLine::Feed(f) => Some(f),
                _ => None,
            })
            .collect();
        assert!(feeds.len() >= 10, "{}", feeds.len());
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, UrlsLine::Comment(c) if c.contains("YouTube"))),
            "the comment groups are what a later milestone turns into folders"
        );
        assert!(
            lines.iter().any(|l| matches!(l, UrlsLine::Skipped { .. })),
            "the fixture carries a query: line so the skip path is exercised"
        );
    }

    /// A newsboat cache, built from newsboat's own DDL rather than checked
    /// in: the real one is 123 MB, and the columns that matter are five.
    fn newsboat_cache() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE rss_feed (
               rssurl VARCHAR(1024) PRIMARY KEY NOT NULL,
               url VARCHAR(1024) NOT NULL,
               title VARCHAR(1024) NOT NULL,
               lastmodified INTEGER(11) NOT NULL DEFAULT 0,
               is_rtl INTEGER(1) NOT NULL DEFAULT 0,
               etag VARCHAR(128) NOT NULL DEFAULT '');
             CREATE TABLE rss_item (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               guid VARCHAR(64) NOT NULL,
               title VARCHAR(1024) NOT NULL,
               author VARCHAR(1024) NOT NULL,
               url VARCHAR(1024) NOT NULL,
               feedurl VARCHAR(1024) NOT NULL,
               pubDate INTEGER NOT NULL,
               content VARCHAR(65535) NOT NULL,
               unread INTEGER(1) NOT NULL,
               enclosure_url VARCHAR(1024),
               enclosure_type VARCHAR(1024),
               enqueued INTEGER(1) NOT NULL DEFAULT 0,
               flags VARCHAR(52),
               deleted INTEGER(1) NOT NULL DEFAULT 0,
               base VARCHAR(128) NOT NULL DEFAULT '');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO rss_feed(rssurl, url, title) VALUES
             ('https://example.org/feed.xml', 'https://example.org/', 'Example Journal')",
            [],
        )
        .unwrap();
        let mut insert = conn
            .prepare(
                "INSERT INTO rss_item(guid, title, author, url, feedurl, pubDate, content, unread, deleted)
                 VALUES (?1, 't', 'a', ?2, ?3, 0, 'a very large body', ?4, ?5)",
            )
            .unwrap();
        insert
            .execute(rusqlite::params!["g1", "https://example.org/1", "https://example.org/feed.xml", 0, 0])
            .unwrap();
        insert
            .execute(rusqlite::params!["g2", "https://example.org/2", "https://example.org/feed.xml", 1, 0])
            .unwrap();
        insert
            .execute(rusqlite::params!["g3", "https://example.org/3", "https://example.org/feed.xml", 0, 1])
            .unwrap();
        drop(insert);
        conn
    }

    #[test]
    fn the_cache_gives_up_its_titles_and_its_read_marks_and_nothing_else() {
        let conn = newsboat_cache();
        let got = read_cache_conn(&conn).unwrap();
        assert_eq!(
            got.titles.get("https://example.org/feed.xml").map(String::as_str),
            Some("Example Journal")
        );
        assert_eq!(got.read.len(), 1, "only the unread=0, undeleted row");
        assert_eq!(got.read[0].guid, "g1");
        assert_eq!(got.read[0].feed_url, "https://example.org/feed.xml");
    }

    #[test]
    fn a_file_that_is_not_a_newsboat_cache_says_so() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let err = read_cache_conn(&conn).unwrap_err().to_string();
        assert!(err.contains("newsboat cache"), "{err}");
    }

    #[test]
    fn a_cache_that_is_not_there_says_which_file() {
        let err = read_cache(std::path::Path::new("/nowhere/at/all/cache.db"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("/nowhere/at/all/cache.db"), "{err}");
    }

    #[test]
    fn newsboats_own_paths_are_looked_for_in_its_own_order() {
        let home = std::path::Path::new("/home/someone");
        assert_eq!(
            default_urls_paths(home)[0],
            home.join(".config/newsboat/urls")
        );
        assert_eq!(
            default_cache_paths(home)[0],
            home.join(".local/share/newsboat/cache.db")
        );
    }

    proptest::proptest! {
        /// A ten-year-old hand-edited file, in whatever state it is in.
        #[test]
        fn arbitrary_text_never_panics(s: String) {
            let lines = parse_urls(&s);
            proptest::prop_assert_eq!(lines.len(), s.lines().count());
        }

        /// Writing a parsed line back out and parsing it again gives the
        /// same line. This is the proof that the tokeniser did not lose a
        /// quote, a backslash or a `~`.
        #[test]
        fn a_feed_line_survives_being_written_out_and_read_back(
            url in "[a-z][a-z0-9:/._-]{0,40}",
            hidden: bool,
            title in proptest::option::of("[^\\x00-\\x1f]{0,30}"),
            tags in proptest::collection::vec("[^\\x00-\\x1f]{0,20}", 0..4),
        ) {
            let line = UrlsLine::Feed(FeedLine {
                url,
                hidden,
                // An all-whitespace or empty title is not a title: newsboat
                // would drop the `~` token as empty, and so does this.
                title: title.filter(|t| !t.trim().is_empty()).map(|t| t.trim().to_string()),
                tags: tags
                    .into_iter()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect(),
            });
            let rendered = render_line(&line);
            let back = parse_line(&rendered);
            proptest::prop_assert_eq!(&line, &back, "rendered as {:?}", rendered);
        }
    }
}
