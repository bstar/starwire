//! The one seam over the HTML-to-Markdown converter.
//!
//! **Nothing else in the tree names `htmd`.** That is the whole reason this
//! file exists, and it is a decision rather than tidiness.
//!
//! `htmd` is on `html5ever` 0.38; `dom_smoothie`, which does the readability
//! pass before this one, reaches `html5ever` 0.39 through `dom_query`. So the
//! binary carries two HTML tokenizers. That is accepted for 0.0.1 -- both are
//! permissively licensed, neither is in a hot path measured in milliseconds,
//! and the duplicate is written down in `deny.toml` rather than ignored --
//! but it is a thing to fix, and the fix is to write a serializer over the
//! `dom_query` tree that is already in the graph. Behind this one function,
//! that is one file's worth of work and no change anywhere else. With
//! `htmd::convert` called from four places it would be four.
//!
//! The second reason is taste. Turndown-family converters each have their own
//! opinions about how to spell an emphasis or where to put a list marker,
//! and the reader's own tests are written against *this* function's output.
//! Swapping the converter is then a matter of making those tests pass again.

use anyhow::Result;

/// HTML in, CommonMark out.
///
/// `base` is only used for the error message; resolving relative links is
/// [`super::normalise::normalise`]'s job, because it has to happen to the
/// feed's own text as well and that never goes through a converter.
pub fn to_markdown(html: &str, base: Option<&url::Url>) -> Result<String> {
    converter().convert(html).map_err(|e| match base {
        Some(url) => anyhow::anyhow!("converting {url} to markdown: {e}"),
        None => anyhow::anyhow!("converting to markdown: {e}"),
    })
}

/// The converter, configured once.
///
/// `skip_tags` covers the furniture readability leaves behind on a page it
/// scored generously: a `<form>` in the middle of an article is a newsletter
/// box, an `<iframe>` is an embed the terminal cannot show, and `<script>`
/// and `<style>` are never text however they got here. Each of them
/// otherwise arrives as a paragraph of nothing, or as the literal text of a
/// script.
///
/// `<noscript>` is **not** in that list, which is the one entry worth
/// explaining. A page that loads its pictures with JavaScript puts the real
/// `<img>` inside a `<noscript>` for everybody else, and this program is
/// everybody else: skipping the tag threw away the only copy of the picture
/// that had a URL in it. `scripting_enabled(false)` is the other half --
/// without it `html5ever` treats the contents of a `<noscript>` as raw text,
/// which would arrive as the literal characters `<img src=...>`.
fn converter() -> htmd::HtmlToMarkdown {
    htmd::HtmlToMarkdown::builder()
        .options(htmd::options::Options {
            // A hard line break as a trailing backslash rather than as two
            // trailing spaces. `normalise::collapse` trims trailing
            // whitespace off every line -- it has to, or a page indented with
            // spaces is half a screen of them -- and that silently deleted
            // every hard break in every article: not one survived in the
            // reference database's three hundred and thirty.
            br_style: htmd::options::BrStyle::Backslash,
            ..htmd::options::Options::default()
        })
        .scripting_enabled(false)
        .skip_tags(vec![
            "script", "style", "iframe", "form", "button", "svg", "nav", "footer",
        ])
        // Registered after `skip_tags`, because the last handler for a tag is
        // the one that runs and `skip_tags` is itself a handler.
        .add_handler(vec!["a"], anchor)
        .build()
}

/// A link, with two cases the default handler gets wrong for a reader.
///
/// **A link with nothing visible in it disappears.** 121 of the 333 extracted
/// articles in the reference database carried at least one `[](url)`: a
/// permalink anchor, a bare `<a name>`, an icon whose `<svg>` this converter
/// skips. In the reader each one is an empty pair of brackets with a link
/// number beside it, and following it is how somebody finds out it went
/// nowhere interesting.
///
/// **A link around nothing but a picture becomes the picture.** That is the
/// usual markup for a figure that opens larger, and `[![](x)](x)` in a
/// terminal is two link numbers and no picture. WP-8 draws the picture; this
/// is what leaves it something to draw.
fn anchor(
    handlers: &dyn htmd::element_handler::Handlers,
    element: htmd::Element,
) -> Option<htmd::element_handler::HandlerResult> {
    let inner = handlers.walk_children(element.node).content;
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }
    if is_one_image(trimmed) {
        return Some(trimmed.to_string().into());
    }
    handlers.fallback(element)
}

/// Whether some markdown is exactly one image and nothing else.
fn is_one_image(md: &str) -> bool {
    let Some(rest) = md.strip_prefix("![") else {
        return false;
    };
    let Some(alt_end) = rest.find("](") else {
        return false;
    };
    if rest[..alt_end].contains('[') {
        return false;
    }
    // The target may have one level of nested parentheses in it, which is
    // what a Wikipedia URL looks like.
    let mut depth = 0usize;
    for (offset, ch) in rest[alt_end + 2..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => return alt_end + 2 + offset + 1 == rest.len(),
            ')' => depth -= 1,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        to_markdown(html, None).unwrap()
    }

    #[test]
    fn the_shapes_the_reader_draws_all_come_out_of_it() {
        let got = md(r#"<h1>A heading</h1>
<p>A paragraph with <strong>bold</strong>, <em>italic</em> and
<code>inline code</code>, and a <a href="https://example.org/x">link</a>.</p>
<ul><li>One</li><li>Two</li></ul>
<ol><li>First</li><li>Second</li></ol>
<pre><code class="language-rust">fn main() {}</code></pre>
<blockquote><p>Quoted.</p></blockquote>
<hr>
<img src="https://example.org/a.png" alt="A picture">"#);
        assert!(got.contains("# A heading"), "{got}");
        assert!(got.contains("**bold**"), "{got}");
        assert!(got.contains("`inline code`"), "{got}");
        assert!(got.contains("[link](https://example.org/x)"), "{got}");
        assert!(got.contains("*   One"), "{got}");
        assert!(got.contains("1.  First"), "{got}");
        assert!(got.contains("fn main() {}"), "{got}");
        assert!(got.contains("> Quoted."), "{got}");
        assert!(
            got.contains("![A picture](https://example.org/a.png)"),
            "{got}"
        );
    }

    #[test]
    fn a_fenced_block_keeps_the_language_the_page_declared() {
        let got = md(r#"<pre><code class="language-rust">let a = 1;</code></pre>"#);
        assert!(got.contains("```rust"), "{got}");
    }

    #[test]
    fn a_table_survives_as_a_table() {
        let got = md("<table><thead><tr><th>A</th><th>B</th></tr></thead>\
             <tbody><tr><td>1</td><td>2</td></tr></tbody></table>");
        assert!(
            got.contains('|'),
            "the reader draws tables as columns: {got}"
        );
        assert!(got.contains('A') && got.contains('2'), "{got}");
    }

    #[test]
    fn scripts_styles_and_embeds_are_not_text() {
        let got = md("<p>Before</p>\
             <script>alert('x')</script>\
             <style>.a{color:red}</style>\
             <iframe src=\"https://e.org/embed\"></iframe>\
             <form><button>Subscribe</button></form>\
             <p>After</p>");
        assert!(got.contains("Before") && got.contains("After"), "{got}");
        assert!(!got.contains("alert"), "{got}");
        assert!(!got.contains("color:red"), "{got}");
        assert!(!got.contains("Subscribe"), "{got}");
    }

    #[test]
    fn a_link_with_nothing_visible_in_it_disappears() {
        // A permalink anchor and an icon link, which is where 121 of the 333
        // extracted articles in the reference database got their `[](url)`
        // from.
        let got = md(concat!(
            r##"<p>A heading<a href="#h" class="anchor"></a> and "##,
            r#"<a href="https://e.org/x"><svg viewBox="0 0 1 1"></svg></a> after.</p>"#
        ));
        assert!(!got.contains("[]("), "{got}");
        assert!(got.contains("A heading"), "{got}");
        assert!(got.contains("after"), "{got}");
    }

    #[test]
    fn a_link_that_is_only_a_picture_becomes_the_picture() {
        let got = md(concat!(
            r#"<figure><a href="https://e.org/big.png">"#,
            r#"<img src="https://e.org/small.png" alt="A harbour"></a></figure>"#
        ));
        assert_eq!(got.trim(), "![A harbour](https://e.org/small.png)");

        // With no alt text at all the URL is still kept: WP-8 draws it, and
        // the alt is what a reader falls back to.
        let got = md(r#"<a href="https://e.org/big.png"><img src="https://e.org/s.png"></a>"#);
        assert_eq!(got.trim(), "![](https://e.org/s.png)");

        // A link with a picture *and* words in it is still a link.
        let got = md(r#"<a href="https://e.org/x"><img src="https://e.org/s.png"> Read on</a>"#);
        assert!(got.contains("](https://e.org/x)"), "{got}");
    }

    #[test]
    fn a_hard_break_survives_being_tidied() {
        let got = md("<p>First line<br>second line</p>");
        assert!(
            got.contains("First line\\\nsecond line"),
            "a break has to survive `normalise::collapse`, which trims trailing \
             whitespace: {got:?}"
        );
        let tidied = super::super::normalise::normalise(
            &got,
            None,
            super::super::normalise::Options::default(),
        );
        assert!(tidied.contains("First line\\\nsecond line"), "{tidied:?}");
    }

    #[test]
    fn a_picture_a_page_only_shows_to_a_browser_without_javascript_survives() {
        // The shape every lazy-loading image library emits: a spacer with the
        // real one in a `<noscript>` beside it.
        let got = md(concat!(
            r#"<p><img src="https://e.org/spacer.gif" alt="">"#,
            r#"<noscript><img src="https://e.org/real.jpg" alt="The real one"></noscript></p>"#
        ));
        assert!(
            got.contains("![The real one](https://e.org/real.jpg)"),
            "{got}"
        );
        assert!(
            !got.contains("&lt;img"),
            "the noscript arrived as text rather than as markup: {got}"
        );
    }

    #[test]
    fn an_empty_document_is_an_empty_string_rather_than_an_error() {
        assert_eq!(md(""), "");
        assert_eq!(md("   "), "");
    }

    #[test]
    fn markup_that_is_not_html_at_all_still_returns() {
        // The input is a network response. A page that is a JSON error body
        // served with the wrong content type has to come back as text, not
        // as a panic.
        let got = md("{\"error\": \"not html\"}");
        assert!(got.contains("not html"), "{got}");
    }

    proptest::proptest! {
        /// The converter is handed somebody else's page. It may produce
        /// anything; it may not take the process down.
        #[test]
        fn arbitrary_input_never_panics(s: String) {
            let _ = to_markdown(&s, None);
        }

        /// And nor may deeply nested markup, which is the shape a hostile
        /// page takes when it is trying to blow a recursive parser's stack.
        #[test]
        fn deep_nesting_never_panics(depth in 0usize..400) {
            let html = format!("{}text{}", "<div>".repeat(depth), "</div>".repeat(depth));
            let _ = to_markdown(&html, None);
        }
    }
}
