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
//! 2. [`images`] makes the pictures' addresses right, which has to happen
//!    while it is still HTML: `srcset`, `<picture>` and `data-src` are
//!    attributes the converter does not read.
//! 3. [`markdown`] turns that HTML into CommonMark. The one place `htmd` is
//!    named.
//! 4. [`normalise`] makes the result fit to read and fit to store: blank
//!    lines collapsed, relative links resolved, size capped at a paragraph
//!    boundary.
//!
//! [`rules`] is the fourth thing, and it is a table rather than a stage: what
//! this program knows about particular websites, in one file, as data.
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

pub mod images;
pub mod markdown;
pub mod normalise;
pub mod readability;
pub mod rules;

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
    /// How long to wait before another attempt is worth making, in seconds,
    /// where one is worth making at all.
    ///
    /// `None` is the ordinary case and means never: a 401, a 403, a 404 and a
    /// page that does not read like an article are all facts about the page
    /// rather than about today, and a browser's user agent is measured to get
    /// the same three codes. `Some` is a 429, a 5xx or a timeout -- twenty-four
    /// of the eighty-five failures in the reference database -- and
    /// `db::articles::put` turns it into the moment the entry rejoins the
    /// queue, doubling it for each attempt already spent.
    pub retry_in_secs: Option<i64>,
}

/// What `run` needs to know beyond the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_article_bytes: u64,
    /// The whole of one page request, in seconds.
    pub timeout_secs: u64,
    pub max_markdown_bytes: usize,
    pub images: bool,
    /// The first delay after a transient failure, in seconds. Doubled per
    /// attempt by `db::articles::put`; the refresh interval, because a reader
    /// who refreshes every fifteen minutes has said what "soon" means to
    /// them.
    pub retry_base_secs: i64,
}

impl Default for Limits {
    fn default() -> Self {
        let cfg = super::ArticlesConfig::default();
        Self {
            max_article_bytes: cfg.max_article_bytes,
            timeout_secs: cfg.timeout_secs,
            max_markdown_bytes: cfg.max_markdown_bytes,
            images: cfg.images,
            retry_base_secs: super::FetchConfig::default().refresh_minutes as i64 * 60,
        }
    }
}

impl Limits {
    /// What one page request asks for.
    fn request(&self) -> RequestOptions {
        RequestOptions::page(self.max_article_bytes).timeout(self.timeout_secs)
    }
}

impl From<&super::ArticlesConfig> for Limits {
    fn from(cfg: &super::ArticlesConfig) -> Self {
        Self {
            max_article_bytes: cfg.max_article_bytes,
            timeout_secs: cfg.timeout_secs,
            max_markdown_bytes: cfg.max_markdown_bytes,
            images: cfg.images,
            ..Self::default()
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

    let response = match http.get(&parsed, &limits.request()) {
        Ok(r) => r,
        Err(e) => {
            let retry = transport_is_transient(&e).then_some(limits.retry_base_secs);
            return Ok(failed(url, e.to_string()).retrying(retry));
        }
    };
    if !response.is_ok() {
        let retry = status_is_transient(response.status).then(|| {
            // A server that says how long to wait is believed where it asks
            // for longer than the delay this would have chosen anyway.
            response
                .retry_after
                .unwrap_or(0)
                .max(limits.retry_base_secs)
        });
        // Named against the host that answered rather than the one that was
        // asked. Seventeen of the reference failures read `feedpress.me
        // answered 429` when the request feedpress redirected to was the one
        // being refused, which sends whoever reads it to the wrong site.
        let reason = format!(
            "{} answered {}",
            host_of(&response.final_url),
            response.status
        );
        return Ok(failed(&response.final_url, reason).retrying(retry));
    }

    // A server that answers a request for HTML with a PDF or an image is
    // answering honestly; there is just nothing here that can read it.
    if let Some(ct) = &response.content_type {
        let ct = ct.to_ascii_lowercase();
        if !ct.contains("html") && !ct.contains("xml") && !ct.contains("text/") {
            return Ok(failed(
                url,
                format!("the page is {ct}, not something to read"),
            ));
        }
    }

    let final_url = Url::parse(&response.final_url).unwrap_or(parsed);
    let html = response.text();

    let extracted = match readability::extract(&html, Some(final_url.as_str())) {
        Ok(e) => e,
        Err(e) => return Ok(failed(final_url.as_str(), e.to_string())),
    };

    let host = final_url.host_str().unwrap_or("").to_string();

    // An article the site split across pages, put back together. Only where a
    // rule says this site does that, and only while the pages stay on the
    // same host and under the same path.
    let mut article_html = extracted.content_html.clone();
    article_html.push_str(&follow_pages(http, &html, &final_url, limits));

    // The pictures' addresses, before the converter -- which reads `src` and
    // nothing else, so a lazy-loaded page would otherwise reduce to a list of
    // spacers.
    let content_html = images::sources(&article_html);

    let md = match markdown::to_markdown(&content_html, Some(&final_url)) {
        Ok(md) => md,
        Err(e) => return Ok(failed(final_url.as_str(), e.to_string())),
    };
    // The site's own furniture, which readability kept because the site puts
    // it inside the article.
    let md = rules::strip(&md, &host);
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

    // A wall that answered 200. Stored as the feed's own text with the
    // reason beside it rather than as a success: five Bloomberg articles in
    // the reference database are four hundred to nine hundred bytes of free
    // sample each, and every one of them was counted as an article that had
    // been extracted.
    if rules::is_paywall_stub(&md, &host) {
        return Ok(ArticleResult {
            status: ArticleStatus::FeedContent,
            title: extracted.title,
            // None, so that `articles::put` keeps the text the entry already
            // had. The feed's summary and the free sample are usually the
            // same two paragraphs, and the summary does not pretend to be
            // the article.
            markdown: None,
            byline: tidy_byline(extracted.byline, &host),
            site_name: extracted.site_name,
            image_url: extracted.image_url,
            excerpt: extracted.excerpt,
            source_url: Some(final_url.to_string()),
            error: Some("paywall".to_string()),
            retry_in_secs: None,
        });
    }

    Ok(ArticleResult {
        status: ArticleStatus::Extracted,
        title: extracted.title,
        markdown: Some(md),
        byline: tidy_byline(extracted.byline, &host),
        site_name: extracted.site_name,
        image_url: extracted.image_url,
        excerpt: extracted.excerpt,
        source_url: Some(final_url.to_string()),
        error: None,
        retry_in_secs: None,
    })
}

/// Fetch the rest of an article a site split across pages.
///
/// Returns the extra HTML, which is empty in every case but the one a rule
/// asks for. Every page costs a lease like any other request, the chain stops
/// at the first page that does not answer, and a page that points back at one
/// already read ends it: a pagination loop must not be eight requests.
fn follow_pages(http: &dyn Http, first_html: &str, first_url: &Url, limits: Limits) -> String {
    let host = first_url.host_str().unwrap_or("");
    let Some(next_page) = rules::for_host(host).and_then(|rule| rule.next_page.as_ref()) else {
        return String::new();
    };
    if !first_url.path().starts_with(next_page.path_prefix) {
        return String::new();
    }

    let mut extra = String::new();
    let mut seen = vec![first_url.clone()];
    let mut page_html = first_html.to_string();
    let mut page_url = first_url.clone();

    while seen.len() < next_page.max_pages {
        let Some(next) = rules::next_page(&page_html, &page_url, next_page.path_prefix) else {
            break;
        };
        if seen.contains(&next) {
            break;
        }
        let Ok(response) = http.get(&next, &limits.request()) else {
            break;
        };
        if !response.is_ok() {
            break;
        }
        let html = response.text();
        let landed = Url::parse(&response.final_url).unwrap_or_else(|_| next.clone());
        if let Ok(more) = readability::extract(&html, Some(landed.as_str())) {
            extra.push_str(&more.content_html);
        }
        seen.push(next);
        page_html = html;
        page_url = landed;
    }
    extra
}

/// A site's byline sentence reduced to the author's name, where a rule knows
/// how.
fn tidy_byline(byline: Option<String>, host: &str) -> Option<String> {
    let byline = byline?;
    Some(rules::byline(&byline, host).unwrap_or(byline))
}

/// A URL's host, as an error message should name it, or `the site` when
/// there is not one to name.
fn host_of(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .map(|host| host.strip_prefix("www.").unwrap_or(&host).to_string())
        .unwrap_or_else(|| "the site".to_string())
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

impl ArticleResult {
    /// Say that this failure is worth another attempt after `secs`.
    fn retrying(mut self, secs: Option<i64>) -> Self {
        self.retry_in_secs = secs;
        self
    }
}

/// Whether a status code is a fact about today rather than about the page.
///
/// Measured rather than assumed: of the eighty-five failures in the reference
/// database, the seventeen 429s were one host being asked too fast and the
/// 401s, 402s and 403s answered a browser's user agent with the same code.
/// So a 429 and a 5xx come back and the rest do not.
fn status_is_transient(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// And the same question for a failure that never reached a status code.
///
/// A timeout is the one worth retrying: six of the reference failures were
/// heavy pages behind redirect wrappers, and the same page fetched again
/// without the queue behind it usually answers. A refused connection, a
/// certificate that does not verify and a body over the cap are not.
fn transport_is_transient(error: &super::net::NetError) -> bool {
    use super::net::NetError;
    match error {
        // A login wall and a chain that will not end are both facts about
        // the address rather than about the minute.
        NetError::TooLarge(_)
        | NetError::NoReplay(_)
        | NetError::Wall(_)
        | NetError::TooManyRedirects(_) => false,
        NetError::Transport(text) => {
            let text = text.to_ascii_lowercase();
            text.contains("timeout") || text.contains("timed out")
        }
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
    // A feed's `<description>` is allowed to be plain text rather than HTML,
    // and 429 of the 431 video entries in the reference database are: a
    // YouTube description, laid out with blank lines and single newlines,
    // which the converter has no way to see. 322 of them arrived in the
    // reader as one unbroken paragraph.
    let md = if is_plain_text(html) {
        plain_text(html)
    } else {
        markdown::to_markdown(html, base.as_ref()).unwrap_or_else(|_| html.to_string())
    };
    normalise::normalise(&md, base.as_ref(), normalise::Options::default())
}

/// Whether a feed's text carries no markup at all.
///
/// Looks for a `<` that begins a tag rather than for a `<` -- a description
/// saying `a < b` or `<3` is still plain text, and those are not rare in a
/// YouTube description.
fn is_plain_text(text: &str) -> bool {
    !text.as_bytes().windows(2).any(|pair| {
        pair[0] == b'<' && (pair[1].is_ascii_alphabetic() || pair[1] == b'/' || pair[1] == b'!')
    })
}

/// Plain text as markdown that keeps the shape it was written in.
///
/// A blank line is a paragraph break, as it is everywhere. A single newline
/// is a *hard* break, because in a description written by hand it is a line
/// the author chose -- a list of chapter times, a row of links, an address.
/// Reflowing those into a paragraph is what the reader used to do.
fn plain_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut first = true;
    for paragraph in text.split("\n\n") {
        let lines: Vec<&str> = paragraph
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        if !first {
            out.push_str("\n\n");
        }
        first = false;
        // A trailing backslash is a hard break; `normalise::collapse` knows
        // to keep one and to take off the last, which breaks nothing.
        out.push_str(&lines.join("\\\n"));
    }
    out
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
            policy(
                FeedKind::Reddit,
                EntryKind::Article,
                Some("https://e.org/a")
            ),
            Policy::FeedContent,
            "the feed kind decides even when the entry looks like an article"
        );
    }

    #[test]
    fn a_hacker_news_item_is_extracted_only_when_it_links_out() {
        assert_eq!(
            policy(
                FeedKind::Hn,
                EntryKind::Article,
                Some("https://e.org/piece")
            ),
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
        assert!(
            md.contains("Every value has exactly one owner."),
            "the list: {md}"
        );
        assert!(md.contains("```rust"), "the fenced block's language: {md}");
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

    /// The pre-pass, through the whole pipeline: the markdown carries the
    /// addresses the pictures are really at, not the spacers the page served.
    #[test]
    fn a_page_of_lazy_loaded_pictures_comes_out_with_addresses_that_work() {
        let http = Replay::open(&replay_dir()).unwrap();
        let got = run(
            &http,
            "https://example.org/posts/lazy-pictures",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(got.status, ArticleStatus::Extracted, "{:?}", got.error);
        let md = got.markdown.unwrap();
        assert!(md.contains("harbour-1280.jpg"), "{md}");
        assert!(md.contains("bridge.webp"), "{md}");
        assert!(md.contains("lighthouse.jpg"), "{md}");
        assert!(!md.contains("spacer.gif"), "{md}");
        assert!(!md.contains("data:"), "{md}");
        assert!(
            md.contains("A ferry, eventually"),
            "the alt text of the one with no address: {md}"
        );
    }

    /// The page follower, end to end: a review split across two pages comes
    /// out as one article, with the badge and the byline sentence cleaned up
    /// by the same site's rule.
    #[test]
    fn a_review_split_across_pages_comes_back_as_one_article() {
        let http = Replay::open(&replay_dir()).unwrap();
        let got = run(
            &http,
            "https://www.phoronix.com/review/a-long-test/1",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(got.status, ArticleStatus::Extracted, "{:?}", got.error);
        let md = got.markdown.unwrap();
        assert!(md.contains("sets out what was tested"), "page one: {md}");
        assert!(
            md.contains("throughput went up by a fifth"),
            "page two: {md}"
        );
        assert!(
            !md.contains("HARDWARE"),
            "the category badge came with it: {md}"
        );
        assert!(
            !md.contains("sponsor.example"),
            "a link out of the site was followed or kept: {md}"
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
    fn a_failure_names_the_host_that_answered() {
        assert_eq!(host_of("https://www.nytimes.com/2026/a"), "nytimes.com");
        assert_eq!(host_of("https://feedpress.me/link/1"), "feedpress.me");
        assert_eq!(host_of("not a url"), "the site");
    }

    #[test]
    fn a_429_and_a_timeout_come_back_and_a_403_does_not() {
        assert!(status_is_transient(429));
        assert!(status_is_transient(503));
        for status in [401, 402, 403, 404, 410, 451] {
            assert!(!status_is_transient(status), "{status}");
        }

        use crate::wire::net::NetError;
        assert!(transport_is_transient(&NetError::Transport(
            "timeout: global".into()
        )));
        assert!(!transport_is_transient(&NetError::TooLarge(1)));
        assert!(!transport_is_transient(&NetError::Wall(
            "https://e.org/login".into()
        )));
        assert!(!transport_is_transient(&NetError::Transport(
            "configured for https only: http://e.org/".into()
        )));
    }

    /// A page that answers 200 with two paragraphs and an invitation is not
    /// a success. The reader keeps the feed's own text, and `paywall` is what
    /// the banner says.
    #[test]
    fn a_paywall_stub_is_the_feeds_text_with_a_reason_rather_than_an_article() {
        let http = Replay::open(&replay_dir()).unwrap();
        let got = run(
            &http,
            "https://example.org/posts/paywalled",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(got.status, ArticleStatus::FeedContent, "{:?}", got.error);
        assert_eq!(got.error.as_deref(), Some("paywall"));
        assert!(
            got.markdown.is_none(),
            "the stub was stored over the feed's text: {:?}",
            got.markdown
        );
        // The metadata is still worth having: it is the real headline.
        assert_eq!(
            got.title.as_deref(),
            Some("Something happened in a market today")
        );
        assert_eq!(got.retry_in_secs, None, "a wall is not a bad minute");
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

    /// What 322 of the 410 YouTube descriptions in the reference database
    /// arrived as: one paragraph, because every newline the author wrote was
    /// reflowed away.
    #[test]
    fn a_plain_text_description_keeps_the_lines_it_was_written_with() {
        let got = from_feed_content(
            "\"A quotation.\"\n\nSomebody said it.\n\n\
             » Subscribe: https://example.org/s\n\
             » The newsletter: https://example.org/n\n\
             » On the radio: https://example.org/r",
            None,
        );
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines[0], "\"A quotation.\"");
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], "Somebody said it.");
        assert!(
            lines[4].ends_with('\\'),
            "the three links are three lines, not one paragraph: {got:?}"
        );
        assert!(lines[5].ends_with('\\'), "{got:?}");
        assert!(
            !lines[6].ends_with('\\'),
            "the last line of a paragraph breaks nothing: {got:?}"
        );
    }

    #[test]
    fn a_description_that_mentions_a_less_than_sign_is_still_plain_text() {
        assert!(is_plain_text("a < b, and 3 < 4"));
        assert!(is_plain_text("I <3 this"));
        assert!(!is_plain_text("<p>Markup.</p>"));
        assert!(!is_plain_text("Text with <br> in it"));
        assert!(!is_plain_text("<!-- a comment -->"));

        // And the plain one keeps its punctuation rather than gaining
        // markup's escapes.
        let got = from_feed_content("Comparing a < b, on two\nlines.", None);
        assert!(got.contains("a < b"), "{got}");
        assert!(got.contains("two\\\nlines."), "{got:?}");
    }

    #[test]
    fn the_limits_come_from_the_config_rather_than_from_a_second_set_of_numbers() {
        let cfg = super::super::ArticlesConfig {
            max_article_bytes: 7,
            timeout_secs: 45,
            max_markdown_bytes: 11,
            images: false,
            ..super::super::ArticlesConfig::default()
        };
        let limits = Limits::from(&cfg);
        assert_eq!(limits.max_article_bytes, 7);
        assert_eq!(limits.timeout_secs, 45);
        assert_eq!(limits.max_markdown_bytes, 11);
        assert!(!limits.images);
    }
}
