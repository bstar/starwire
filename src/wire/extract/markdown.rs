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
    converter()
        .convert(html)
        .map_err(|e| match base {
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
fn converter() -> htmd::HtmlToMarkdown {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "noscript", "iframe", "form", "button", "svg", "nav", "footer",
        ])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        to_markdown(html, None).unwrap()
    }

    #[test]
    fn the_shapes_the_reader_draws_all_come_out_of_it() {
        let got = md(
            r#"<h1>A heading</h1>
<p>A paragraph with <strong>bold</strong>, <em>italic</em> and
<code>inline code</code>, and a <a href="https://example.org/x">link</a>.</p>
<ul><li>One</li><li>Two</li></ul>
<ol><li>First</li><li>Second</li></ol>
<pre><code class="language-rust">fn main() {}</code></pre>
<blockquote><p>Quoted.</p></blockquote>
<hr>
<img src="https://example.org/a.png" alt="A picture">"#,
        );
        assert!(got.contains("# A heading"), "{got}");
        assert!(got.contains("**bold**"), "{got}");
        assert!(got.contains("`inline code`"), "{got}");
        assert!(got.contains("[link](https://example.org/x)"), "{got}");
        assert!(got.contains("One"), "{got}");
        assert!(got.contains("1. First"), "{got}");
        assert!(got.contains("fn main() {}"), "{got}");
        assert!(got.contains("> Quoted."), "{got}");
        assert!(got.contains("![A picture](https://example.org/a.png)"), "{got}");
    }

    #[test]
    fn a_fenced_block_keeps_the_language_the_page_declared() {
        let got = md(r#"<pre><code class="language-rust">let a = 1;</code></pre>"#);
        assert!(got.contains("```rust"), "{got}");
    }

    #[test]
    fn a_table_survives_as_a_table() {
        let got = md(
            "<table><thead><tr><th>A</th><th>B</th></tr></thead>\
             <tbody><tr><td>1</td><td>2</td></tr></tbody></table>",
        );
        assert!(got.contains('|'), "the reader draws tables as columns: {got}");
        assert!(got.contains('A') && got.contains('2'), "{got}");
    }

    #[test]
    fn scripts_styles_and_embeds_are_not_text() {
        let got = md(
            "<p>Before</p>\
             <script>alert('x')</script>\
             <style>.a{color:red}</style>\
             <iframe src=\"https://e.org/embed\"></iframe>\
             <form><button>Subscribe</button></form>\
             <p>After</p>",
        );
        assert!(got.contains("Before") && got.contains("After"), "{got}");
        assert!(!got.contains("alert"), "{got}");
        assert!(!got.contains("color:red"), "{got}");
        assert!(!got.contains("Subscribe"), "{got}");
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
