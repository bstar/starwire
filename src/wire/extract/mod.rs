//! Turning an entry into something worth reading in a terminal.
//!
//! This is the part people came for. A feed gives a title, a link and, if you
//! are lucky, two sentences; what this does is fetch the page behind the
//! link, find the article inside it, and reduce it to clean CommonMark that
//! wraps at whatever width the reader is drawing at.
//!
//! Three stages, each behind its own seam:
//!
//! 1. [`readability`] finds the article inside the page. Mozilla's algorithm,
//!    with a gate in front of it that refuses homepages and paywall stubs.
//! 2. [`markdown`] turns that HTML into CommonMark. The one place `htmd` is
//!    named.
//! 3. [`normalise`] makes the result fit to read and fit to store: blank
//!    lines collapsed, relative links resolved, size capped at a paragraph
//!    boundary.
//!
//! **What is not scraped, and why.** Not everything with a link is worth
//! fetching, and fetching the wrong things is both rude and useless:
//!
//! - **A video** is never fetched. Its description is the text; the video is
//!   the point, and `v` plays it.
//! - **A Reddit post** is never fetched. The feed carries the post body,
//!   the link goes to a comment thread rather than to an article, and Reddit
//!   rate limits hard enough that scraping it would cost the whole feed list
//!   its refreshes.
//! - **A Hacker News item that links back into `news.ycombinator.com`** is
//!   never fetched, for the same reason: that is the comment thread, not the
//!   piece. An `hnrss` item that links out *is* fetched, which is most of
//!   the value of reading HN in a reader at all.
//!
//! There is no `robots.txt` fetch. The politeness here is structural instead:
//! one request at a time per host with a gap between them, a fifteen-second
//! timeout, a two-megabyte cap, an `Accept` header that says HTML, a user
//! agent that names the program and links the repository, and at most three
//! attempts at any one entry ever. This fetches pages a person subscribed to
//! and asked to read, one per entry, once.

pub mod markdown;
pub mod normalise;
pub mod readability;

use anyhow::Result;
use url::Url;

use super::feed::{ArticleStatus, EntryKind, FeedKind};
use super::net::{Http, RequestOptions};

/// What should be done with an entry's link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Fetch the page and reduce it.
    Extract,
    /// Use whatever the feed itself carried.
    FeedContent,
    /// It is a video: the description is the text, and nothing is fetched.
    Video,
}

impl Policy {
    pub fn extracts(self) -> bool {
        matches!(self, Policy::Extract)
    }
}

/// Which of the three an entry gets.
///
/// `extract` being off in the config is handled by the caller rather than
/// here: this answers what *kind* of thing the entry is, and the setting
/// answers whether to bother, and keeping them apart is what lets
/// `starwire extract <url>` work on a single URL with the setting off.
pub fn policy(feed: FeedKind, entry: EntryKind, url: Option<&str>) -> Policy {
    if entry == EntryKind::Video {
        return Policy::Video;
    }
    if entry == EntryKind::Post || feed == FeedKind::Reddit {
        return Policy::FeedContent;
    }
    let Some(url) = url else {
        return Policy::FeedContent;
    };
    let Ok(parsed) = Url::parse(url) else {
        return Policy::FeedContent;
    };
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Policy::FeedContent;
    }
    // An `hnrss` item whose link goes back to Hacker News is the comment
    // thread. The article is whatever the submission pointed at, and when
    // the submission *is* a discussion there is no article at all.
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if host == "news.ycombinator.com" {
        return Policy::FeedContent;
    }
    Policy::Extract
}

/// What an extraction attempt came to, ready for `db::articles::put`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArticleResult {
    pub status: ArticleStatus,
    pub title: Option<String>,
    pub markdown: Option<String>,
    pub byline: Option<String>,
    pub site_name: Option<String>,
    pub image_url: Option<String>,
    pub excerpt: Option<String>,
    pub source_url: Option<String>,
    pub error: Option<String>,
}

/// What `run` needs to know beyond the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_article_bytes: u64,
    pub max_markdown_bytes: usize,
    pub images: bool,
}

impl Default for Limits {
    fn default() -> Self {
        let cfg = super::ArticlesConfig::default();
        Self {
            max_article_bytes: cfg.max_article_bytes,
            max_markdown_bytes: cfg.max_markdown_bytes,
            images: cfg.images,
        }
    }
}

impl From<&super::ArticlesConfig> for Limits {
    fn from(cfg: &super::ArticlesConfig) -> Self {
        Self {
            max_article_bytes: cfg.max_article_bytes,
            max_markdown_bytes: cfg.max_markdown_bytes,
            images: cfg.images,
        }
    }
}

/// Fetch one page and reduce it.
///
/// Every failure comes back as an [`ArticleResult`] with
/// [`ArticleStatus::Failed`] and a reason rather than as an `Err`: the
/// caller's job is to store the outcome either way, the reader shows the
/// reason beside an offer to open the page in a browser, and an entry whose
/// page is a paywall is still an entry worth having in the list.
///
/// The one `Err` is a URL that will not parse, which is a bug in whatever
/// produced it rather than something the network did.
pub fn run(http: &dyn Http, url: &str, limits: Limits) -> Result<ArticleResult> {
    let parsed = Url::parse(url).map_err(|e| anyhow::anyhow!("{url} is not a URL: {e}"))?;

    let response = match http.get(&parsed, &RequestOptions::page(limits.max_article_bytes)) {
        Ok(r) => r,
        Err(e) => return Ok(failed(url, e.to_string())),
    };
    if !response.is_ok() {
        return Ok(failed(url, format!("the site answered {}", response.status)));
    }

    // A server that answers a request for HTML with a PDF or an image is
    // answering honestly; there is just nothing here that can read it.
    if let Some(ct) = &response.content_type {
        let ct = ct.to_ascii_lowercase();
        if !ct.contains("html") && !ct.contains("xml") && !ct.contains("text/") {
            return Ok(failed(url, format!("the page is {ct}, not something to read")));
        }
    }

    let final_url = Url::parse(&response.final_url).unwrap_or(parsed);
    let html = response.text();

    let extracted = match readability::extract(&html, Some(final_url.as_str())) {
        Ok(e) => e,
        Err(e) => return Ok(failed(final_url.as_str(), e.to_string())),
    };

    let md = match markdown::to_markdown(&extracted.content_html, Some(&final_url)) {
        Ok(md) => md,
        Err(e) => return Ok(failed(final_url.as_str(), e.to_string())),
    };
    let md = normalise::normalise(
        &md,
        Some(&final_url),
        normalise::Options {
            max_bytes: limits.max_markdown_bytes,
            images: limits.images,
        },
    );

    if md.trim().is_empty() {
        return Ok(failed(
            final_url.as_str(),
            "the page reduced to nothing".to_string(),
        ));
    }

    Ok(ArticleResult {
        status: ArticleStatus::Extracted,
        title: extracted.title,
        markdown: Some(md),
        byline: extracted.byline,
        site_name: extracted.site_name,
        image_url: extracted.image_url,
        excerpt: extracted.excerpt,
        source_url: Some(final_url.to_string()),
        error: None,
    })
}

fn failed(url: &str, reason: String) -> ArticleResult {
    tracing::debug!(url, reason, "extraction did not yield");
    ArticleResult {
        status: ArticleStatus::Failed,
        source_url: Some(url.to_string()),
        error: Some(reason),
        ..ArticleResult::default()
    }
}

/// The feed's own text, as markdown.
///
/// Every entry gets this on the way in, so that an entry is readable the
/// moment it is stored, before anything has been fetched and whether or not
/// extraction ever succeeds. For a full-text feed it is the whole article
/// and nothing else ever needs to happen.
pub fn from_feed_content(html: &str, url: Option<&str>) -> String {
    let base = url.and_then(|u| Url::parse(u).ok());
    // A feed's `<description>` is allowed to be plain text rather than HTML.
    // The converter handles both -- text with no tags in it comes back as
    // itself -- so there is no sniffing to do here.
    let md = markdown::to_markdown(html, base.as_ref()).unwrap_or_else(|_| html.to_string());
    normalise::normalise(&md, base.as_ref(), normalise::Options::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::net::Replay;

    #[test]
    fn a_video_is_never_fetched_whatever_it_links_to() {
        assert_eq!(
            policy(
                FeedKind::Youtube,
                EntryKind::Video,
                Some("https://www.youtube.com/watch?v=abc")
            ),
            Policy::Video
        );
        assert_eq!(
            policy(FeedKind::Web, EntryKind::Video, Some("https://e.org/a")),
            Policy::Video,
            "a video linked from a blog is still a video"
        );
    }

    #[test]
    fn reddit_is_never_scraped() {
        assert_eq!(
            policy(
                FeedKind::Reddit,
                EntryKind::Post,
                Some("https://old.reddit.com/r/rust/comments/x/")
            ),
            Policy::FeedContent
        );
        assert_eq!(
            policy(FeedKind::Reddit, EntryKind::Article, Some("https://e.org/a")),
            Policy::FeedContent,
            "the feed kind decides even when the entry looks like an article"
        );
    }

    #[test]
    fn a_hacker_news_item_is_extracted_only_when_it_links_out() {
        assert_eq!(
            policy(FeedKind::Hn, EntryKind::Article, Some("https://e.org/piece")),
            Policy::Extract
        );
        assert_eq!(
            policy(
                FeedKind::Hn,
                EntryKind::Article,
                Some("https://news.ycombinator.com/item?id=1")
            ),
            Policy::FeedContent,
            "that is the comment thread, not the article"
        );
        assert_eq!(
            policy(
                FeedKind::Hn,
                EntryKind::Article,
                Some("https://www.news.ycombinator.com/item?id=1")
            ),
            Policy::FeedContent
        );
    }

    #[test]
    fn an_entry_with_no_link_or_an_unfetchable_one_uses_the_feeds_own_text() {
        assert_eq!(
            policy(FeedKind::Web, EntryKind::Article, None),
            Policy::FeedContent
        );
        assert_eq!(
            policy(FeedKind::Web, EntryKind::Article, Some("not a url")),
            Policy::FeedContent
        );
        assert_eq!(
            policy(
                FeedKind::Web,
                EntryKind::Article,
                Some("magnet:?xt=urn:btih:abc")
            ),
            Policy::FeedContent,
            "this program speaks http and nothing else"
        );
    }

    fn replay_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/replay")
    }

    #[test]
    fn a_real_page_from_the_replay_directory_comes_out_as_markdown() {
        let http = Replay::open(&replay_dir()).unwrap();
        let got = run(
            &http,
            "https://example.org/posts/borrow-checker",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(got.status, ArticleStatus::Extracted, "{:?}", got.error);
        let md = got.markdown.unwrap();
        assert!(md.contains("Three rules"), "the heading: {md}");
        assert!(md.contains("* Every value has exactly one owner."), "the list: {md}");
        assert!(md.contains("fn longest"), "the code block: {md}");
        assert!(
            md.contains("https://doc.rust-lang.org/nomicon/"),
            "the link: {md}"
        );
        assert!(
            !md.contains("Subscribe to the newsletter"),
            "the furniture came with it: {md}"
        );
    }

    #[test]
    fn a_page_that_is_not_an_article_fails_with_a_reason_rather_than_an_error() {
        let http = Replay::open(&replay_dir()).unwrap();
        let got = run(
            &http,
            "https://example.org/posts/unreadable",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(got.status, ArticleStatus::Failed);
        assert!(got.error.is_some());
        assert!(got.markdown.is_none());
    }

    #[test]
    fn a_url_with_nothing_behind_it_fails_with_a_reason() {
        let http = Replay::open(&replay_dir()).unwrap();
        let got = run(&http, "https://example.org/nowhere", Limits::default()).unwrap();
        assert_eq!(got.status, ArticleStatus::Failed);
        assert!(got.error.unwrap().contains("replay"));
    }

    #[test]
    fn something_that_is_not_a_url_is_the_callers_mistake_and_is_reported_as_one() {
        let http = Replay::open(&replay_dir()).unwrap();
        assert!(run(&http, "not a url", Limits::default()).is_err());
    }

    #[test]
    fn the_markdown_is_never_longer_than_the_cap() {
        let http = Replay::open(&replay_dir()).unwrap();
        let got = run(
            &http,
            "https://example.org/posts/borrow-checker",
            Limits {
                max_markdown_bytes: 200,
                ..Limits::default()
            },
        )
        .unwrap();
        assert!(got.markdown.unwrap().len() <= 200);
    }

    #[test]
    fn a_feeds_own_text_becomes_markdown_with_absolute_links() {
        let got = from_feed_content(
            r#"<p>Hello with a <a href="/x">link</a>.</p>"#,
            Some("https://example.org/posts/one"),
        );
        assert!(got.contains("[link](https://example.org/x)"), "{got}");
    }

    #[test]
    fn a_feeds_plain_text_description_survives_as_itself() {
        let got = from_feed_content("What the video is about.", None);
        assert_eq!(got, "What the video is about.");
    }

    #[test]
    fn the_limits_come_from_the_config_rather_than_from_a_second_set_of_numbers() {
        let cfg = super::super::ArticlesConfig {
            max_article_bytes: 7,
            max_markdown_bytes: 11,
            images: false,
            ..super::super::ArticlesConfig::default()
        };
        let limits = Limits::from(&cfg);
        assert_eq!(limits.max_article_bytes, 7);
        assert_eq!(limits.max_markdown_bytes, 11);
        assert!(!limits.images);
    }
}
