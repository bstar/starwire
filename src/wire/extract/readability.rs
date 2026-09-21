//! Finding the article inside the page.
//!
//! A wrapper over `dom_smoothie`, which is a port of Mozilla's Readability --
//! the same algorithm behind Firefox's reader view, and the reason the pages
//! this works on are the pages people already expect to work. What is added
//! here is the gate in front of it and the shape of what comes out.
//!
//! The gate matters. `is_probably_readable` is a cheap scan that asks whether
//! there is enough prose in the document to be worth scoring at all; a
//! homepage, a search results page, a login wall and a paywall stub all fail
//! it. Without it, readability happily returns the navigation of a homepage
//! as an "article", and the reader shows a list of link text where the piece
//! should be. Failing the gate is not an error -- it is the signal to try the
//! page's own JSON-LD, and then to fall back to whatever the feed carried.
//!
//! The numbers the gate uses are loosened from the library's defaults, and
//! `config` says what was measured to choose them. The short version: those
//! defaults are for a button in a browser, pressed on a page somebody is
//! already looking at, and a feed carries short news items every day.

use anyhow::Result;

/// What was found in a page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extracted {
    pub title: Option<String>,
    pub byline: Option<String>,
    pub site_name: Option<String>,
    pub excerpt: Option<String>,
    pub image_url: Option<String>,
    /// The article's own HTML, cleaned. Still HTML: turning it into markdown
    /// is [`super::markdown::to_markdown`]'s job, behind its own seam.
    pub content_html: String,
    /// Roughly how much text there was, which is what tells a near-empty
    /// result from a real one.
    pub length: usize,
}

/// Why a page yielded nothing.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// The gate said no. The common case, and not really an error: a
    /// homepage, a link to a comment thread, a paywall stub.
    #[error("the page does not read like an article")]
    NotReadable,
    /// Readability ran and found nothing worth keeping.
    #[error("nothing that reads like an article was found in the page")]
    Empty,
    #[error("{0}")]
    Failed(String),
}

/// The least text a result may have and still be an article.
///
/// Four hundred characters is about a short paragraph and a half. Below
/// that, what readability found is almost always a cookie banner, a
/// "subscribe to read" stub, or the one sentence above a paywall -- and the
/// feed's own summary is usually longer and always more honest.
const MIN_LENGTH: usize = 400;

/// Run readability over a page.
///
/// `url` is the address the page was finally served from, after redirects:
/// readability uses it to resolve the page's own relative links, and getting
/// it wrong means every link in the article points at the wrong host.
pub fn extract(html: &str, url: Option<&str>) -> Result<Extracted, ExtractError> {
    let mut readability = dom_smoothie::Readability::new(html, url, Some(config()))
        .map_err(|e| ExtractError::Failed(e.to_string()))?;

    let readable = readability.is_probably_readable();
    let scored = if readable {
        Some(
            readability
                .parse()
                .map_err(|e| ExtractError::Failed(e.to_string()))?,
        )
    } else {
        None
    };

    if let Some(article) = scored {
        let content_html = article.content.to_string();
        if article.length >= MIN_LENGTH && !content_html.trim().is_empty() {
            return Ok(Extracted {
                title: nonempty(article.title),
                byline: article.byline.and_then(nonempty),
                site_name: article.site_name.and_then(nonempty),
                excerpt: article.excerpt.and_then(nonempty),
                image_url: article.image.and_then(nonempty),
                content_html,
                length: article.length,
            });
        }
    }

    // The page's own account of itself, before giving up. A site that
    // renders its article with JavaScript often still puts the whole of it
    // in a `<script type="application/ld+json">` for a search engine to
    // read, and that is the one thing on such a page worth having.
    if let Some(found) = json_ld::article(html) {
        if found.body.chars().count() >= MIN_LENGTH {
            return Ok(found.into());
        }
    }

    Err(if readable {
        ExtractError::Empty
    } else {
        ExtractError::NotReadable
    })
}

/// Whether a page is worth fetching the rest of, without extracting it.
///
/// Used by `starwire extract <url>` to say *why* a page produced nothing,
/// which is the difference between a useful probe and one that prints an
/// empty line.
pub fn is_probably_readable(html: &str) -> bool {
    dom_smoothie::Readability::new(html, None, Some(config()))
        .map(|r| r.is_probably_readable())
        .unwrap_or(false)
}

fn config() -> dom_smoothie::Config {
    dom_smoothie::Config {
        // The gate and the scoring, both loosened from the defaults, and
        // both measured. Of the twenty-five pages in the reference database
        // refused as "does not read like an article", five were Arch Linux
        // news items and three were Substack posts -- real articles of five
        // hundred to a thousand characters, which their feeds prove exist --
        // and the other seventeen were landing pages, login pages and
        // JavaScript shells that these numbers still refuse. A short news
        // item is a normal thing for a feed to carry, and the defaults here
        // (a score of 20 over 140 characters, a 500-character floor) are
        // tuned for a browser button somebody presses on a page they are
        // already looking at.
        readable_min_score: 8.0,
        readable_min_content_length: 140,
        char_threshold: 300,
        // More candidates because the pages this now reaches are short ones,
        // where the difference between the best node and the fourth best is
        // a paragraph rather than a page.
        n_top_candidates: 8,
        // A page that is thirty thousand elements is a search result listing
        // or a generated index, not an article, and scoring all of it is the
        // one way this becomes slow. Readability treats 0 as no limit, which
        // is not what a program fetching arbitrary URLs wants.
        max_elements_to_parse: 30_000,
        // Classes are kept, which is not the obvious choice: they make the
        // HTML handed to the markdown converter noticeably larger and
        // nothing downstream reads any of them except one. That one is
        // `class="language-rust"` on a `<pre><code>`, which is the only
        // place a fenced block's language exists -- strip it and every code
        // block in every article comes out as a bare ``` fence, which the
        // reader then cannot label or colour.
        keep_classes: true,
        // Raw, because the *HTML* is what is wanted here -- the markdown is
        // made by the converter behind its own seam, and letting two things
        // produce markdown would be two sets of opinions about a list
        // marker.
        text_mode: dom_smoothie::TextMode::Raw,
        ..dom_smoothie::Config::default()
    }
}

/// What a page says about itself in `application/ld+json`.
///
/// `dom_smoothie` reads this too -- `Readability::parse_json_ld` is public --
/// but what it returns is a `Metadata`: a title, a byline, an excerpt, an
/// image. The one field wanted here, `articleBody`, is not on it, and the
/// method is only callable on a `Readability` whose document has already
/// been through the scoring pass that may have removed the script. So the
/// document is read again, from the bytes as they arrived.
mod json_ld {
    use super::Extracted;

    /// An article as its own JSON-LD describes it.
    pub struct Article {
        pub title: Option<String>,
        pub byline: Option<String>,
        pub body: String,
    }

    impl From<Article> for Extracted {
        fn from(article: Article) -> Self {
            let length = article.body.chars().count();
            Self {
                title: article.title,
                byline: article.byline,
                site_name: None,
                excerpt: None,
                image_url: None,
                content_html: paragraphs(&article.body),
                length,
            }
        }
    }

    /// How deep the search for an `articleBody` goes.
    ///
    /// JSON-LD nests: an article is often inside an `@graph`, which is
    /// inside an array. Eight is more than any real document needs and
    /// stops a hostile one from being a stack overflow.
    const MAX_DEPTH: usize = 8;

    /// The first article-shaped object in any of the page's JSON-LD blocks.
    pub fn article(html: &str) -> Option<Article> {
        let document = dom_query::Document::from(html);
        for node in document
            .select(r#"script[type="application/ld+json"]"#)
            .nodes()
        {
            let text = node.text().to_string();
            let Ok(value) = serde_json::from_str::<serde_json::Value>(strip_cdata(&text)) else {
                continue;
            };
            if let Some(found) = find(&value, 0) {
                return Some(found);
            }
        }
        None
    }

    /// Some sites wrap the JSON in a CDATA section, which is XML rather than
    /// JSON and will not parse as it stands.
    fn strip_cdata(text: &str) -> &str {
        let trimmed = text.trim();
        trimmed
            .strip_prefix("<![CDATA[")
            .and_then(|rest| rest.strip_suffix("]]>"))
            .unwrap_or(trimmed)
            .trim()
    }

    fn find(value: &serde_json::Value, depth: usize) -> Option<Article> {
        if depth > MAX_DEPTH {
            return None;
        }
        match value {
            serde_json::Value::Object(object) => {
                if let Some(body) = object.get("articleBody").and_then(|v| v.as_str()) {
                    let body = body.trim();
                    if !body.is_empty() {
                        return Some(Article {
                            title: text(object.get("headline"))
                                .or_else(|| text(object.get("name"))),
                            byline: author(object.get("author")),
                            body: body.to_string(),
                        });
                    }
                }
                object.values().find_map(|v| find(v, depth + 1))
            }
            serde_json::Value::Array(items) => items.iter().find_map(|v| find(v, depth + 1)),
            _ => None,
        }
    }

    fn text(value: Option<&serde_json::Value>) -> Option<String> {
        let text = value?.as_str()?.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    /// `author` is a string, an object with a `name`, or an array of either.
    fn author(value: Option<&serde_json::Value>) -> Option<String> {
        match value? {
            serde_json::Value::String(_) => text(value),
            serde_json::Value::Object(object) => text(object.get("name")),
            serde_json::Value::Array(items) => {
                let names: Vec<String> = items.iter().filter_map(|v| author(Some(v))).collect();
                (!names.is_empty()).then(|| names.join(", "))
            }
            _ => None,
        }
    }

    /// The body back into HTML, because HTML is what the next stage takes.
    ///
    /// `articleBody` is plain text with blank lines between paragraphs, so
    /// this is the one place in the extractor that writes markup rather than
    /// reading it -- and the reason every character is escaped on the way.
    fn paragraphs(body: &str) -> String {
        let mut out = String::with_capacity(body.len() + 32);
        out.push_str("<div>");
        for paragraph in body.split("\n\n") {
            let paragraph = paragraph.trim();
            if paragraph.is_empty() {
                continue;
            }
            out.push_str("<p>");
            for ch in paragraph.chars() {
                match ch {
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '&' => out.push_str("&amp;"),
                    '\n' => out.push_str("<br>"),
                    other => out.push(other),
                }
            }
            out.push_str("</p>");
        }
        out.push_str("</div>");
        out
    }
}

fn nonempty(s: String) -> Option<String> {
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page shaped like a real article: a header and a footer of
    /// furniture, and enough prose in the middle to score.
    fn article_page() -> String {
        let body: String = (1..=8)
            .map(|n| {
                format!(
                    "<p>Paragraph {n} of the piece. It is long enough to be scored as prose \
                     rather than as navigation, which is the whole point of the fixture, and \
                     it says something about lifetimes so that the words are not all the same.</p>"
                )
            })
            .collect();
        format!(
            r#"<!doctype html><html><head>
<title>Why the borrow checker says no</title>
<meta property="og:site_name" content="Example Journal">
<meta property="og:image" content="https://example.org/card.png">
</head><body>
<nav><a href="/">Home</a> <a href="/about">About</a></nav>
<article><h1>Why the borrow checker says no</h1>
<p class="byline">by Jane Example</p>
{body}
</article>
<footer>Copyright somebody</footer>
</body></html>"#
        )
    }

    #[test]
    fn a_page_shaped_like_an_article_gives_up_its_article() {
        let got = extract(&article_page(), Some("https://example.org/posts/one")).unwrap();
        assert!(got.content_html.contains("Paragraph 1 of the piece"));
        assert!(
            !got.content_html.contains("Copyright somebody"),
            "the furniture came with it: {}",
            got.content_html
        );
        assert!(got.length >= MIN_LENGTH);
        assert_eq!(got.title.as_deref(), Some("Why the borrow checker says no"));
    }

    fn page(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/pages")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The five Arch news items and three Substack posts the gate used to
    /// refuse. Short is not the same as not an article, and the feed proves
    /// the article exists.
    #[test]
    fn a_short_news_item_is_an_article() {
        let got = extract(
            &page("short-real.html"),
            Some("https://example.org/news/nft/"),
        )
        .unwrap();
        assert!(
            got.content_html.contains("defaults to the nft backend"),
            "{}",
            got.content_html
        );
        assert!(
            !got.content_html.contains("Donate"),
            "the furniture came with it: {}",
            got.content_html
        );
    }

    /// A page drawn by a script has nothing to score and everything to read,
    /// in its own structured data.
    #[test]
    fn a_page_that_draws_itself_later_gives_up_its_json_ld() {
        let got = extract(&page("jsonld-only.html"), Some("https://example.org/a")).unwrap();
        assert!(
            got.content_html
                .contains("nothing in it for a reader to find"),
            "{}",
            got.content_html
        );
        assert_eq!(
            got.title.as_deref(),
            Some("The whole of it was in the head all along")
        );
        assert_eq!(got.byline.as_deref(), Some("Jane Example"));
        assert!(
            got.content_html.matches("<p>").count() >= 3,
            "the blank lines between paragraphs were lost: {}",
            got.content_html
        );
        assert!(
            !got.content_html.contains("Loading"),
            "{}",
            got.content_html
        );
    }

    #[test]
    fn the_json_ld_body_is_escaped_rather_than_pasted_in() {
        let html = format!(
            r#"<html><head><script type="application/ld+json">
            {{"@type": "Article", "articleBody": "{}"}}
            </script></head><body><div id="root"></div></body></html>"#,
            format_args!(
                "A body with <b>markup</b> and an & in it. {}",
                "Padding to get it past the floor. ".repeat(20)
            )
        );
        let got = extract(&html, None).unwrap();
        assert!(!got.content_html.contains("<b>"), "{}", got.content_html);
        assert!(
            got.content_html.contains("&lt;b&gt;markup&lt;/b&gt;"),
            "{}",
            got.content_html
        );
        assert!(
            got.content_html.contains("an &amp; in it"),
            "{}",
            got.content_html
        );
    }

    #[test]
    fn a_navigation_page_is_refused_rather_than_returned_as_an_article() {
        let html = r#"<!doctype html><html><head><title>Home</title></head><body>
<nav><ul>
<li><a href="/a">One</a></li><li><a href="/b">Two</a></li>
<li><a href="/c">Three</a></li><li><a href="/d">Four</a></li>
</ul></nav></body></html>"#;
        let err = extract(html, Some("https://example.org/")).unwrap_err();
        assert!(
            matches!(err, ExtractError::NotReadable | ExtractError::Empty),
            "{err}"
        );
        assert!(!is_probably_readable(html));
    }

    #[test]
    fn a_paywall_stub_is_refused_because_it_is_too_short_to_be_the_piece() {
        let html = r#"<!doctype html><html><body><article>
<h1>Something behind a wall</h1>
<p>Subscribers only. Sign in to read the rest of this article.</p>
</article></body></html>"#;
        let err = extract(html, None).unwrap_err();
        assert!(
            matches!(err, ExtractError::NotReadable | ExtractError::Empty),
            "{err}"
        );
    }

    #[test]
    fn an_empty_document_is_refused_rather_than_crashing() {
        assert!(extract("", None).is_err());
        assert!(extract("<html></html>", None).is_err());
        assert!(!is_probably_readable(""));
    }

    #[test]
    fn a_bad_document_url_is_reported_rather_than_ignored() {
        // Readability wants an absolute URL to resolve links against, and a
        // relative one is a programming mistake worth surfacing.
        let err = extract(&article_page(), Some("not a url"));
        assert!(matches!(err, Err(ExtractError::Failed(_))), "{err:?}");
    }

    proptest::proptest! {
        /// Somebody else's page, in whatever state it arrived in.
        #[test]
        fn arbitrary_input_never_panics(s: String) {
            let _ = extract(&s, Some("https://example.org/"));
            let _ = is_probably_readable(&s);
        }
    }
}
