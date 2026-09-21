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
//! should be. Failing the gate is not an error -- it is the signal to fall
//! back to whatever the feed itself carried.

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

    if !readability.is_probably_readable() {
        return Err(ExtractError::NotReadable);
    }

    let article = readability
        .parse()
        .map_err(|e| ExtractError::Failed(e.to_string()))?;

    let content_html = article.content.to_string();
    if article.length < MIN_LENGTH || content_html.trim().is_empty() {
        return Err(ExtractError::Empty);
    }

    Ok(Extracted {
        title: nonempty(article.title),
        byline: article.byline.and_then(nonempty),
        site_name: article.site_name.and_then(nonempty),
        excerpt: article.excerpt.and_then(nonempty),
        image_url: article.image.and_then(nonempty),
        content_html,
        length: article.length,
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
