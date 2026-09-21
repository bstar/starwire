//! Finding the URL a picture is actually at, before the markdown is written.
//!
//! A pass over the HTML readability handed back, with `dom_query` -- the same
//! tree `dom_smoothie` is built on, so this is a second walk rather than a
//! second parser. It runs before the markdown converter, which reads `src`,
//! `alt` and `title` on an `<img>` and nothing else: no `srcset`, no
//! `data-src`, no `<picture>`.
//!
//! That is the whole problem. A page that loads its pictures with JavaScript
//! serves an `<img>` whose `src` is a one-pixel spacer or a base64 blur and
//! keeps the real address in `data-src`; a responsive page offers five sizes
//! in `srcset` and leaves `src` as the smallest; a `<picture>` puts the good
//! ones in `<source>` elements and the fallback in the `<img>`. Convert any of
//! those without looking and what lands in the markdown is a spacer.
//!
//! Nothing here fetches anything and nothing here decides how a picture is
//! drawn. This makes the URLs right; drawing them is the reader's.

use dom_query::{Document, Node};

/// The widest a candidate may be and still be worth choosing.
///
/// A `srcset` on a news site routinely offers 3840px for a retina display.
/// The reader is a terminal: two thousand pixels is already more than any
/// cell grid can use, and the difference is megabytes over somebody's
/// connection.
const MAX_WIDTH: u32 = 2000;

/// Attributes a lazy-loading library hides the real address in, in the order
/// they are worth trying.
///
/// `data-src` is the convention every one of them follows; the other two are
/// what the older libraries still in use on WordPress sites write.
const LAZY_ATTRS: &[&str] = &["data-src", "data-original", "data-lazy-src"];

/// Attributes holding a set of candidates rather than one address.
const LAZY_SRCSET_ATTRS: &[&str] = &["srcset", "data-srcset", "data-lazy-srcset"];

/// Rewrite every picture in a document so that its `src` is the address worth
/// having, and give up on the ones that have none.
pub fn sources(html: &str) -> String {
    let document = Document::fragment(html);

    // `<picture>` first: its `<source>` elements are the good candidates and
    // the `<img>` inside it is the fallback, so the best of the sources is
    // hoisted onto the fallback before the `<img>` pass looks at it.
    for picture in document.select("picture").nodes() {
        let inside = picture.descendants();
        let best = inside
            .iter()
            .filter(|node| is_tag(node, "source"))
            .filter_map(|source| srcset_of(source).and_then(|set| best_candidate(&set)))
            .next_back();
        if let (Some(best), Some(img)) = (best, inside.iter().find(|node| is_tag(node, "img"))) {
            img.set_attr("src", &best);
        }
        // The `<source>` elements themselves are neither text nor pictures:
        // they would be nothing in the markdown either way, and taking them
        // out keeps what is left readable.
        for source in inside.iter().filter(|node| is_tag(node, "source")) {
            source.remove_from_parent();
        }
    }

    for img in document.select("img").nodes() {
        if let Some(better) = better_source(img) {
            img.set_attr("src", &better);
        }
        for attr in LAZY_ATTRS.iter().chain(LAZY_SRCSET_ATTRS) {
            img.remove_attr(attr);
        }

        // A picture whose only address is the bytes themselves. The reader
        // cannot fetch it, it counts in full against the size cap -- a blur
        // placeholder is routinely two kilobytes of base64 -- and it says
        // nothing. What it had to say was its alt text.
        let src = img.attr("src").map(|s| s.to_string()).unwrap_or_default();
        if src.trim().is_empty() || src.trim_start().to_ascii_lowercase().starts_with("data:") {
            let alt = img.attr("alt").map(|s| s.to_string()).unwrap_or_default();
            let alt = alt.trim();
            if alt.is_empty() {
                img.remove_from_parent();
            } else {
                img.replace_with_html(escape(alt));
            }
        }
    }

    document.html().to_string()
}

/// The best address this `<img>` carries, if it is not the one in `src`.
fn better_source(img: &Node) -> Option<String> {
    // A candidate set beats a single address: it is what the page would have
    // chosen for a wide window.
    if let Some(best) = srcset_of(img).and_then(|set| best_candidate(&set)) {
        return Some(best);
    }
    for attr in LAZY_ATTRS {
        if let Some(value) = img.attr(attr) {
            let value = value.trim().to_string();
            if !value.is_empty() && !value.to_ascii_lowercase().starts_with("data:") {
                return Some(value);
            }
        }
    }
    None
}

/// Whichever of the candidate-set attributes this element carries.
fn srcset_of(node: &Node) -> Option<String> {
    LAZY_SRCSET_ATTRS
        .iter()
        .find_map(|attr| node.attr(attr))
        .map(|set| set.to_string())
}

fn is_tag(node: &Node, name: &str) -> bool {
    node.node_name()
        .is_some_and(|tag| tag.as_ref().eq_ignore_ascii_case(name))
}

/// The widest candidate in a `srcset` that is not wider than [`MAX_WIDTH`].
///
/// Candidates are separated by commas, which is also a character URLs are
/// allowed to contain -- the split here is the common case rather than the
/// specification's, which would need the whole URL grammar to do properly.
/// A candidate that does not parse is skipped, not guessed at.
fn best_candidate(srcset: &str) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    let mut fallback: Option<String> = None;
    for candidate in srcset.split(',') {
        let mut parts = candidate.split_whitespace();
        let Some(url) = parts.next() else {
            continue;
        };
        if url.to_ascii_lowercase().starts_with("data:") {
            continue;
        }
        let width = parts.next().and_then(parse_descriptor);
        match width {
            Some(width) if width <= MAX_WIDTH => {
                if best.as_ref().is_none_or(|(widest, _)| width > *widest) {
                    best = Some((width, url.to_string()));
                }
            }
            // Too wide, or no descriptor at all -- a one-candidate `srcset`
            // is written without one. Kept in case nothing better turns up.
            _ => {
                if fallback.is_none() {
                    fallback = Some(url.to_string());
                }
            }
        }
    }
    best.map(|(_, url)| url).or(fallback)
}

/// `640w` as a width, and `2x` as the width it stands for.
///
/// A density descriptor is turned into a notional width so that the two kinds
/// can be compared: `1x` is a picture at its natural size, which for this
/// purpose is worth about a thousand pixels, and `3x` is one nobody in a
/// terminal needs.
fn parse_descriptor(descriptor: &str) -> Option<u32> {
    let descriptor = descriptor.trim();
    if let Some(width) = descriptor.strip_suffix('w') {
        return width.trim().parse().ok();
    }
    if let Some(density) = descriptor.strip_suffix('x') {
        let density: f32 = density.trim().parse().ok()?;
        if density <= 0.0 {
            return None;
        }
        return Some((density * 1000.0) as u32);
    }
    None
}

/// Alt text as HTML, because it is going back into a document.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/pages")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Every `src` an `<img>` in some HTML ends up with.
    fn srcs(html: &str) -> Vec<String> {
        let document = Document::fragment(html);
        document
            .select("img")
            .nodes()
            .iter()
            .map(|img| img.attr("src").map(|s| s.to_string()).unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_lazy_loaded_picture_gives_up_its_real_address() {
        let got = sources(
            r#"<p><img src="https://e.org/spacer.gif" data-src="https://e.org/real.jpg" alt="A harbour"></p>"#,
        );
        assert_eq!(srcs(&got), ["https://e.org/real.jpg"]);
        assert!(got.contains("A harbour"), "the alt text was lost: {got}");
        assert!(
            !got.contains("data-src"),
            "the attribute is spent and would otherwise be in the tree: {got}"
        );
    }

    #[test]
    fn the_widest_candidate_that_is_not_absurd_wins() {
        let got = sources(concat!(
            r#"<img src="https://e.org/small.jpg" alt="" srcset=""#,
            "https://e.org/320.jpg 320w, https://e.org/1280.jpg 1280w, ",
            r#"https://e.org/3840.jpg 3840w">"#
        ));
        assert_eq!(srcs(&got), ["https://e.org/1280.jpg"]);
    }

    #[test]
    fn a_srcset_of_densities_is_understood_too() {
        let got = sources(
            r#"<img src="https://e.org/one.jpg" srcset="https://e.org/one.jpg 1x, https://e.org/two.jpg 2x" alt="">"#,
        );
        assert_eq!(srcs(&got), ["https://e.org/two.jpg"]);

        // Every candidate too wide: the first is better than nothing.
        let got =
            sources(r#"<img src="https://e.org/s.jpg" srcset="https://e.org/huge.jpg 4000w">"#);
        assert_eq!(srcs(&got), ["https://e.org/huge.jpg"]);
    }

    #[test]
    fn a_picture_element_hands_its_best_source_to_its_fallback() {
        let got = sources(concat!(
            "<picture>",
            r#"<source type="image/avif" srcset="https://e.org/a.avif 1200w">"#,
            r#"<source type="image/webp" srcset="https://e.org/a.webp 1200w">"#,
            r#"<img src="https://e.org/a.jpg" alt="A harbour">"#,
            "</picture>"
        ));
        assert_eq!(srcs(&got), ["https://e.org/a.webp"]);
        assert!(!got.contains("<source"), "{got}");
    }

    #[test]
    fn a_picture_that_is_only_bytes_leaves_its_alt_text_behind() {
        let got = sources(concat!(
            r#"<p><img src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///" alt="A harbour"> "#,
            r#"<img src="DATA:image/png;base64,iVBOR" alt=""></p>"#
        ));
        assert!(srcs(&got).is_empty(), "{got}");
        assert!(got.contains("A harbour"), "{got}");
        assert!(!got.contains("base64"), "{got}");
    }

    #[test]
    fn an_alt_text_with_markup_in_it_is_escaped_on_the_way_back_in() {
        let got =
            sources(r#"<img src="data:image/gif;base64,R0l" alt="<script>alert(1)</script>">"#);
        assert!(!got.contains("<script>"), "{got}");
        assert!(got.contains("&lt;script&gt;"), "{got}");
    }

    #[test]
    fn a_picture_that_is_already_right_is_left_alone() {
        let html = r#"<p><img src="https://e.org/a.png" alt="A harbour" title="Dusk"></p>"#;
        let got = sources(html);
        assert_eq!(srcs(&got), ["https://e.org/a.png"]);
        assert!(got.contains("Dusk"), "{got}");
        // `http:` is left as it is. The reader will not fetch it -- the agent
        // refuses plaintext -- and showing the alt text for one is WP-8's.
        let got = sources(r#"<img src="http://e.org/a.png" alt="Old">"#);
        assert_eq!(srcs(&got), ["http://e.org/a.png"]);
    }

    #[test]
    fn the_fixture_page_of_lazy_pictures_comes_out_with_real_addresses() {
        let got = sources(&page("lazy-images.html"));
        let srcs = srcs(&got);
        assert!(srcs.iter().all(|src| !src.starts_with("data:")), "{srcs:?}");
        assert!(
            srcs.iter().any(|src| src.ends_with("harbour-1280.jpg")),
            "{srcs:?}"
        );
        assert!(
            srcs.iter().any(|src| src.ends_with("bridge.webp")),
            "{srcs:?}"
        );
        assert!(
            srcs.iter().any(|src| src.ends_with("lighthouse.jpg")),
            "{srcs:?}"
        );
        assert!(
            !srcs.iter().any(|src| src.contains("spacer")),
            "a spacer survived: {srcs:?}"
        );
    }

    proptest::proptest! {
        /// The input is somebody else's page. Whatever is in it, no picture
        /// comes out of here carrying its own bytes: they cannot be fetched,
        /// they count in full against the size cap, and a blur placeholder is
        /// routinely two kilobytes of base64.
        #[test]
        fn no_picture_ever_comes_out_as_its_own_bytes(s: String) {
            let got = sources(&s);
            for src in srcs(&got) {
                proptest::prop_assert!(
                    !src.trim_start().to_ascii_lowercase().starts_with("data:"),
                    "{src}"
                );
            }
        }

        /// And the pass is total: an arbitrary document goes through it.
        #[test]
        fn arbitrary_input_never_panics(s: String) {
            let _ = sources(&s);
        }
    }
}
