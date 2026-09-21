//! One feed's round trip: ask, read, parse, and say what happened.
//!
//! Split into a part that touches the network ([`fetch`]) and a part that is
//! pure ([`parse`]), because the second is where every interesting failure
//! lives and the first is the one that needs a socket. `parse` takes bytes
//! and returns a [`crate::wire::feed::ParsedFeed`] or an error, and it is
//! what the property test below throws arbitrary bytes at.

use anyhow::Result;
use url::Url;

use super::db::feeds::Conditional;
use super::feed::ParsedFeed;
use super::net::{Http, RequestOptions};

/// What one fetch came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    /// New bytes, parsed.
    Fetched {
        feed: ParsedFeed,
        conditional: Conditional,
        bytes: usize,
    },
    /// The server said nothing has changed. The good case, and the reason a
    /// refresh of forty-one feeds costs almost nothing.
    NotModified,
    /// It did not work. `retry_after` is the server's own answer where it
    /// gave one.
    Failed {
        error: String,
        status: Option<u16>,
        retry_after: Option<i64>,
    },
}

impl FetchOutcome {
    pub fn is_failure(&self) -> bool {
        matches!(self, FetchOutcome::Failed { .. })
    }

    /// How many entries arrived, for the log line.
    pub fn entry_count(&self) -> Option<usize> {
        match self {
            FetchOutcome::Fetched { feed, .. } => Some(feed.entries.len()),
            _ => None,
        }
    }
}

/// Why bytes were not a feed.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("the body was empty")]
    Empty,
    #[error("the body is not a feed: {0}")]
    NotAFeed(String),
}

/// Bytes to a feed. Pure, total, and the one place `feed-rs` is called.
///
/// `base` is what a feed's relative links are resolved against, which the
/// parser wants because an Atom feed may carry `<link href="/posts/one"/>`
/// and expect the reader to know where it came from.
pub fn parse(bytes: &[u8], base: Option<&Url>) -> Result<ParsedFeed, ParseError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(ParseError::Empty);
    }
    let parser = feed_rs::parser::Builder::new()
        .base_uri(base.map(Url::as_str))
        // Left off deliberately. `feed-rs`'s sanitiser strips what it thinks
        // is dangerous markup, and this program does not render HTML at all:
        // it runs the body through readability and a markdown converter,
        // both of which drop scripts themselves, and what the sanitiser
        // takes out on the way past is sometimes the article's own markup.
        .sanitize_content(false)
        .build();
    parser
        .parse(bytes)
        .map(super::feed::from_parsed)
        .map_err(|e| ParseError::NotAFeed(e.to_string()))
}

/// Ask a server for a feed.
///
/// Never returns `Err` for anything the network did: a refused connection, a
/// 500, a body that is not a feed and a body that is too large all come back
/// as [`FetchOutcome::Failed`], because every one of them is a thing to
/// record against the feed and show in the list rather than a thing to stop
/// a refresh over. The one `Err` is a URL that will not parse.
pub fn fetch(
    http: &dyn Http,
    url: &str,
    conditional: &Conditional,
    max_bytes: u64,
) -> Result<FetchOutcome> {
    let parsed = Url::parse(url).map_err(|e| anyhow::anyhow!("{url} is not a URL: {e}"))?;

    let options = RequestOptions::feed(max_bytes)
        .conditional(conditional.etag.clone(), conditional.last_modified.clone());

    let response = match http.get(&parsed, &options) {
        Ok(r) => r,
        Err(e) => {
            return Ok(FetchOutcome::Failed {
                error: e.to_string(),
                status: None,
                retry_after: None,
            })
        }
    };

    if response.is_not_modified() {
        return Ok(FetchOutcome::NotModified);
    }
    if !response.is_ok() {
        return Ok(FetchOutcome::Failed {
            error: describe_status(response.status),
            status: Some(response.status),
            retry_after: response.retry_after,
        });
    }

    let bytes = response.body.len();
    // A 200 carrying a web page instead of a feed. Reddit does this when it
    // is rate limiting: rather than a 429 it serves its own "take a break"
    // HTML, and feed-rs then reports "no root element", which sends whoever
    // reads the feed's error line looking for a broken feed instead of for
    // a reader asking too often.
    if looks_like_html(response.content_type.as_deref(), &response.body) {
        return Ok(FetchOutcome::Failed {
            error: "not a feed (HTML page; the site may be rate limiting)".into(),
            status: Some(response.status),
            retry_after: response.retry_after,
        });
    }
    let base = Url::parse(&response.final_url).unwrap_or(parsed);
    match parse(&response.body, Some(&base)) {
        Ok(feed) => Ok(FetchOutcome::Fetched {
            feed,
            conditional: Conditional {
                etag: response.etag,
                last_modified: response.last_modified,
            },
            bytes,
        }),
        Err(e) => Ok(FetchOutcome::Failed {
            error: e.to_string(),
            status: Some(response.status),
            retry_after: None,
        }),
    }
}

/// Whether a 200's body is a web page rather than a feed.
///
/// Two tests, because neither alone is enough: a server that declares
/// `text/html` is believed, and a server that declares nothing useful --
/// `application/octet-stream`, or no `Content-Type` at all -- is judged by
/// what its first bytes say. The sniff is deliberately narrow: a doctype or
/// an `<html` at the very start, past any leading whitespace and a byte
/// order mark, and nothing cleverer. An Atom feed never begins either way,
/// and a feed that merely *mentions* HTML in an entry is untouched.
fn looks_like_html(content_type: Option<&str>, body: &[u8]) -> bool {
    if content_type.is_some_and(|ct| {
        ct.split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case("text/html")
    }) {
        return true;
    }
    let start = body.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(body);
    let start: &[u8] = match start.iter().position(|b| !b.is_ascii_whitespace()) {
        Some(at) => &start[at..],
        None => return false,
    };
    let head = &start[..start.len().min(64)];
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

/// A status code as something worth putting in a feed's error line.
///
/// The codes a feed reader actually meets, named. "410 Gone" in particular
/// deserves its own sentence: it is the one answer that means stop asking,
/// and a reader that shows it as "the site answered 410" leaves the person
/// to look it up.
fn describe_status(status: u16) -> String {
    match status {
        401 | 403 => format!("{status}: this feed needs credentials, or refuses this reader"),
        404 => "404: there is no feed at this address any more".into(),
        410 => "410: this feed has been withdrawn -- it is not coming back".into(),
        429 => "429: too many requests; slowing down".into(),
        500..=599 => format!("{status}: the server is having trouble"),
        other => format!("the site answered {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::net::Replay;

    fn replay() -> Replay {
        Replay::open(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/replay"))
            .unwrap()
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("testdata/feeds")
                .join(name),
        )
        .unwrap_or_else(|e| panic!("testdata/feeds/{name}: {e}"))
    }

    #[test]
    fn every_feed_format_in_testdata_parses() {
        for name in [
            "atom-basic.xml",
            "rss2-basic.xml",
            "jsonfeed.json",
            "youtube-channel.xml",
            "scriptbarrel.xml",
            "hn-frontpage.xml",
            "hn-newcomments.xml",
            "reddit-sub.xml",
        ] {
            let parsed = parse(&fixture(name), None).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!parsed.entries.is_empty(), "{name} has no entries");
            for entry in &parsed.entries {
                assert!(!entry.guid.is_empty(), "{name}: an entry with no guid");
            }
        }
    }

    #[test]
    fn a_malformed_feed_is_a_failure_and_not_a_panic() {
        let err = parse(&fixture("malformed.xml"), None).unwrap_err();
        assert!(matches!(err, ParseError::NotAFeed(_)), "{err}");
    }

    #[test]
    fn an_empty_body_says_so_rather_than_being_called_not_a_feed() {
        assert!(matches!(parse(b"", None), Err(ParseError::Empty)));
        assert!(matches!(parse(b"  \n\t ", None), Err(ParseError::Empty)));
    }

    #[test]
    fn a_feed_from_the_replay_directory_comes_back_parsed() {
        let out = fetch(
            &replay(),
            "https://example.org/feed.xml",
            &Conditional::default(),
            8_388_608,
        )
        .unwrap();
        match out {
            FetchOutcome::Fetched { feed, bytes, .. } => {
                assert!(bytes > 0);
                assert_eq!(feed.title.as_deref(), Some("Example Journal"));
                assert!(!feed.entries.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_url_the_replay_does_not_have_is_a_recorded_failure_not_an_error() {
        let out = fetch(
            &replay(),
            "https://example.org/nothing.xml",
            &Conditional::default(),
            1024,
        )
        .unwrap();
        assert!(out.is_failure(), "{out:?}");
    }

    /// What Reddit answers with when it is rate limiting: a 200 carrying its
    /// own web page. The error line has to say that rather than feed-rs's
    /// "no root element", which reads like a broken feed.
    #[test]
    fn a_web_page_where_a_feed_should_be_says_the_site_may_be_rate_limiting() {
        let html = b"<!DOCTYPE html>\n<html><head><title>Too many requests</title></head>\
                     <body><p>Take a break for a minute.</p></body></html>";
        assert!(looks_like_html(Some("text/html; charset=utf-8"), html));
        assert!(
            looks_like_html(None, html),
            "a server that declares nothing is judged by its first bytes"
        );
        assert!(looks_like_html(None, b"\xEF\xBB\xBF  \n<html lang=\"en\">"));

        // And the outcome the fetch records.
        let out = fetch(
            &replay(),
            // The replay directory's article page, served where a feed is
            // being asked for: the same bytes, the same content type.
            "https://example.org/posts/borrow-checker",
            &Conditional::default(),
            8_388_608,
        )
        .unwrap();
        match out {
            FetchOutcome::Failed { error, status, .. } => {
                assert_eq!(
                    error,
                    "not a feed (HTML page; the site may be rate limiting)"
                );
                assert_eq!(status, Some(200));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_feed_is_never_mistaken_for_a_page() {
        for name in ["atom-basic.xml", "rss2-basic.xml", "jsonfeed.json"] {
            assert!(
                !looks_like_html(Some("application/xml"), &fixture(name)),
                "{name}"
            );
        }
        assert!(!looks_like_html(None, b""));
        assert!(!looks_like_html(None, b"   \n\t  "));
        // An entry that talks about HTML is not an HTML page.
        assert!(!looks_like_html(
            None,
            b"<?xml version=\"1.0\"?><rss><channel><item><description>&lt;html&gt;</description>"
        ));
    }

    #[test]
    fn something_that_is_not_a_url_is_the_callers_mistake() {
        assert!(fetch(&replay(), "nonsense", &Conditional::default(), 1024).is_err());
    }

    #[test]
    fn every_status_a_feed_reader_meets_has_a_sentence() {
        for (status, wanted) in [
            (403u16, "refuses"),
            (404, "no feed at this address"),
            (410, "not coming back"),
            (429, "too many requests"),
            (503, "having trouble"),
            (418, "418"),
        ] {
            let text = describe_status(status);
            assert!(
                text.to_lowercase().contains(&wanted.to_lowercase()),
                "{status}: {text}"
            );
        }
    }

    #[test]
    fn an_outcome_can_say_how_many_entries_arrived() {
        let out = fetch(
            &replay(),
            "https://example.org/feed.xml",
            &Conditional::default(),
            8_388_608,
        )
        .unwrap();
        assert!(out.entry_count().unwrap() > 0);
        assert_eq!(FetchOutcome::NotModified.entry_count(), None);
    }

    proptest::proptest! {
        /// Arbitrary bytes are what a compromised or merely confused server
        /// sends. This has to return, either way, without taking the
        /// process with it.
        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = parse(&bytes, Some(&Url::parse("https://example.org/").unwrap()));
        }

        /// And so does a very deeply nested document, which is the shape an
        /// XML bomb takes.
        #[test]
        fn deeply_nested_xml_never_panics(depth in 0usize..500) {
            let xml = format!(
                "<rss version=\"2.0\"><channel>{}{}</channel></rss>",
                "<a>".repeat(depth),
                "</a>".repeat(depth)
            );
            let _ = parse(xml.as_bytes(), None);
        }
    }
}
