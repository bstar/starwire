//! Making markdown out of a converter's markdown fit to read and fit to
//! store, and making a URL out of a URL fit to key on.
//!
//! Everything here is pure and total: no IO, no failure case, and the same
//! input gives the same output twice. That is what lets the property tests
//! below say something worth knowing -- the input is somebody else's page,
//! and "it is idempotent and never longer than the cap" is a claim about
//! every page rather than about the four in `testdata`.

use url::Url;

/// What [`normalise`] is allowed to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The most bytes of markdown to keep. Anything longer is cut at a
    /// paragraph boundary.
    pub max_bytes: usize,
    /// Keep `![alt](src)` as an image. `false` leaves the alt text behind as
    /// a plain paragraph, for a reader who does not want a line of
    /// `[image: …]` every third paragraph.
    ///
    /// Either way nothing is *fetched*: pictures inside articles are not in
    /// 0.0.1, and the reader draws a kept image as `[image: alt]`.
    pub images: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_bytes: 524_288,
            images: true,
        }
    }
}

/// Query parameters that identify a reader rather than a resource.
///
/// Stripped because the URL is a key here -- it is what an entry is
/// deduplicated on and what `o` opens -- and two links to one article that
/// differ only in which newsletter they came from are one article. The list
/// is the common analytics families and nothing speculative: a parameter
/// that might be tracking and might be a page selector stays, because losing
/// a real one turns a link into a 404.
const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "utm_name",
    "utm_reader",
    "utm_social",
    "utm_brand",
    // Facebook, Google, Microsoft, Mailchimp, HubSpot, Yandex.
    "fbclid",
    "gclid",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "mc_cid",
    "mc_eid",
    "_hsenc",
    "_hsmi",
    "hsCtaTracking",
    "yclid",
    "_openstat",
    // Instapaper, Pocket and the feed-reader family's own.
    "igshid",
    "vero_id",
    "vero_conv",
    "ref_src",
    "ref_url",
    "s_cid",
    "cmpid",
    "ncid",
];

/// A URL with the tracking parameters taken off, and nothing else changed.
///
/// Deliberately conservative. The fragment is kept -- it is how a link into
/// a long page reaches the right heading. The path is untouched, including a
/// trailing slash, because `/a` and `/a/` are different resources on more
/// servers than they are the same one. A URL that will not parse comes back
/// as it went in: a string that is not a URL is still the only link this
/// entry has.
pub fn clean_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let Ok(mut url) = Url::parse(trimmed) else {
        return trimmed.to_string();
    };

    let all: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let kept: Vec<&(String, String)> = all
        .iter()
        .filter(|(k, _)| !TRACKING_PARAMS.iter().any(|t| k.eq_ignore_ascii_case(t)))
        .collect();

    // Nothing to remove means nothing to rewrite. Re-serialising a query
    // that was already fine would change `?a=b%20c` into `?a=b+c` -- the
    // same query, a different string, and this string is a key.
    if kept.len() == all.len() {
        return url.to_string();
    }

    if kept.is_empty() {
        // `set_query(Some(""))` leaves a bare `?` behind, which is a
        // different string from no query at all and would defeat the whole
        // point of cleaning.
        url.set_query(None);
    } else {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in kept {
            serializer.append_pair(k, v);
        }
        url.set_query(Some(&serializer.finish()));
    }
    url.to_string()
}

/// Tidy a converter's markdown and bring it within the size cap.
///
/// In order: collapse the runs of blank lines a converter leaves behind,
/// make every relative link and image absolute against the page it came
/// from, drop or keep images, and then cut at a paragraph boundary if what
/// is left is still too long.
///
/// The cut is last because it is the only step that loses anything, and
/// cutting after the other three means the boundary is a boundary in the
/// text a reader will actually see.
pub fn normalise(markdown: &str, base: Option<&Url>, options: Options) -> String {
    let mut out = collapse(markdown);
    if let Some(base) = base {
        out = absolutise(&out, base);
    }
    if !options.images {
        out = drop_images(&out);
    }
    let mut out = truncate_at_paragraph(&out, options.max_bytes);
    // The cut can land just after a hard break, which then breaks nothing and
    // would otherwise be a lone backslash at the end of the article.
    undangle(&mut out);
    out
}

/// Collapse trailing whitespace and runs of blank lines.
///
/// `htmd` leaves three or four blank lines wherever the page had nested
/// `<div>`s around a paragraph, which in a terminal at eighty columns is
/// half a screen of nothing.
///
/// Trailing whitespace has to go -- a page indented with spaces is otherwise
/// half a screen of them -- and that is exactly why the converter is
/// configured to spell a hard break as a trailing backslash rather than as
/// two trailing spaces: this function would eat the spaces, and did, in every
/// article in the reference database. A backslash at the end of a *paragraph*
/// breaks nothing and is taken off, because a stray backslash on screen looks
/// like a bug in the extractor.
fn collapse(markdown: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut blank_run = 0usize;
    let mut in_code = false;
    for line in markdown.lines() {
        let trimmed = line.trim_end();
        // Inside a fenced block every line is the author's, blank ones
        // included: collapsing them would change what the code means.
        if trimmed.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if !in_code && trimmed.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
            if let Some(last) = lines.last_mut() {
                undangle(last);
            }
        } else {
            blank_run = 0;
        }
        lines.push(trimmed.to_string());
    }
    if !in_code {
        if let Some(last) = lines.last_mut() {
            undangle(last);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out.trim_start_matches('\n').trim_end().to_string()
}

/// Take a hard break off the end of a paragraph, where it breaks nothing.
///
/// One backslash only: two is an escaped backslash, which is a character the
/// author wrote.
fn undangle(line: &mut String) {
    if line.ends_with('\\') && !line.ends_with("\\\\") {
        line.pop();
        while line.ends_with(' ') || line.ends_with('\t') {
            line.pop();
        }
    }
}

/// Make every link and image target absolute against the page.
///
/// A relative `href` is meaningless once the markdown is out of the page it
/// came from: the reader has no document to resolve it against, and `o1`
/// would open nothing. Done by hand rather than with a markdown parser
/// because the core is deliberately parser-free -- the reader is where
/// CommonMark is understood -- and the shape being rewritten is exactly
/// `](target)`, which is a scan rather than a parse.
fn absolutise(markdown: &str, base: &Url) -> String {
    let bytes = markdown.as_bytes();
    let mut out = String::with_capacity(markdown.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            if let Some(close) = find_close_paren(markdown, i + 2) {
                let target = &markdown[i + 2..close];
                // A title after the target (`](url "title")`) is kept as it
                // is; only the URL half is resolved.
                let (link, rest) = match target.find(char::is_whitespace) {
                    Some(at) => (&target[..at], &target[at..]),
                    None => (target, ""),
                };
                let resolved = resolve(link, base);
                out.push_str("](");
                out.push_str(&resolved);
                out.push_str(rest);
                out.push(')');
                i = close + 1;
                continue;
            }
        }
        // Push whole characters: indexing by byte would split a multi-byte
        // one, and the text here is somebody else's prose.
        let ch = markdown[i..].chars().next().expect("i is a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The `)` that closes a link target, respecting one level of nesting --
/// which is all a Wikipedia URL needs, and Wikipedia URLs are the reason
/// this is not `find(')')`.
fn find_close_paren(s: &str, from: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, ch) in s[from..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(from + offset),
            ')' => depth -= 1,
            '\n' => return None,
            _ => {}
        }
    }
    None
}

fn resolve(link: &str, base: &Url) -> String {
    let trimmed = link.trim();
    if trimmed.is_empty() {
        return link.to_string();
    }
    // A fragment on its own points inside the page, which the reader does
    // not have. Resolving it against the base at least opens the right page
    // at the right anchor in a browser.
    match base.join(trimmed) {
        Ok(url) => url.to_string(),
        Err(_) => link.to_string(),
    }
}

/// Turn `![alt](src)` into `alt`, dropping images that have no alt text at
/// all rather than leaving an empty paragraph where one was.
fn drop_images(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(at) = rest.find("![") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(alt_end) = after.find(']') else {
            out.push_str(&rest[at..]);
            return out;
        };
        let alt = &after[..alt_end];
        let tail = &after[alt_end..];
        if tail.starts_with("](") {
            if let Some(close) = find_close_paren(tail, 2) {
                out.push_str(alt);
                rest = &tail[close + 1..];
                continue;
            }
        }
        out.push_str(&rest[at..at + 2]);
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    // Dropping an image that was alone on its line leaves the line blank,
    // which the collapse above would have removed had it run after.
    collapse(&out)
}

/// Cut to at most `max` bytes, at the last paragraph break that fits.
///
/// A paragraph rather than a line or a character because the reader is going
/// to draw this as prose, and a cut mid-sentence reads as a bug in the
/// extractor rather than as a limit. Where there is no paragraph break
/// within the cap at all -- one very long paragraph -- it falls back to the
/// last line break, and then to a character boundary, so the result is
/// always valid UTF-8 and never longer than the cap.
pub fn truncate_at_paragraph(markdown: &str, max: usize) -> String {
    if markdown.len() <= max {
        return markdown.to_string();
    }
    let window = &markdown[..floor_char_boundary(markdown, max)];
    if let Some(at) = window.rfind("\n\n") {
        return window[..at].trim_end().to_string();
    }
    if let Some(at) = window.rfind('\n') {
        return window[..at].trim_end().to_string();
    }
    window.trim_end().to_string()
}

fn floor_char_boundary(s: &str, at: usize) -> usize {
    let mut at = at.min(s.len());
    while at > 0 && !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.org/posts/one").unwrap()
    }

    #[test]
    fn tracking_parameters_go_and_real_ones_stay() {
        assert_eq!(
            clean_url("https://example.org/a?utm_source=news&page=2&fbclid=xyz"),
            "https://example.org/a?page=2"
        );
        assert_eq!(
            clean_url("https://example.org/a?utm_source=news"),
            "https://example.org/a",
            "no parameters left means no question mark"
        );
        assert_eq!(
            clean_url("https://example.org/a?id=7&v=2"),
            "https://example.org/a?id=7&v=2"
        );
    }

    #[test]
    fn a_fragment_and_a_trailing_slash_are_kept() {
        assert_eq!(
            clean_url("https://example.org/a/#section"),
            "https://example.org/a/#section"
        );
        assert_ne!(
            clean_url("https://example.org/a"),
            clean_url("https://example.org/a/"),
            "these are different resources on more servers than not"
        );
    }

    #[test]
    fn something_that_is_not_a_url_comes_back_unharmed() {
        assert_eq!(clean_url("  not a url  "), "not a url");
        assert_eq!(clean_url(""), "");
    }

    #[test]
    fn tracking_parameters_are_matched_without_regard_to_case() {
        assert_eq!(
            clean_url("https://example.org/a?UTM_Source=x&keep=1"),
            "https://example.org/a?keep=1"
        );
    }

    #[test]
    fn runs_of_blank_lines_become_one() {
        let got = collapse("A\n\n\n\n\nB\n\n\n");
        assert_eq!(got, "A\n\nB");
    }

    #[test]
    fn a_hard_break_survives_and_a_dangling_one_does_not() {
        // The converter spells a break as a trailing backslash precisely so
        // that trimming trailing whitespace cannot eat it.
        let got = collapse("First\\\nsecond\n\nA paragraph.\\\n\n\nAnd another.\\");
        assert!(got.contains("First\\\nsecond"), "{got:?}");
        assert!(
            !got.contains("A paragraph.\\"),
            "a break at the end of a paragraph breaks nothing: {got:?}"
        );
        assert!(!got.ends_with('\\'), "{got:?}");

        // Two backslashes are an escaped backslash, which the author wrote.
        let got = collapse("Ends in a backslash: \\\\\n\nAfter.");
        assert!(got.contains("backslash: \\\\"), "{got:?}");
    }

    #[test]
    fn blank_lines_inside_a_code_block_are_the_authors() {
        let md = "Text\n\n```rust\nfn a() {}\n\n\nfn b() {}\n```\n";
        let got = collapse(md);
        assert!(got.contains("fn a() {}\n\n\nfn b() {}"), "{got}");
    }

    #[test]
    fn relative_links_and_images_become_absolute() {
        let md = "See [two](../two) and [three](/three) and ![pic](img/a.png).";
        let got = normalise(md, Some(&base()), Options::default());
        assert!(got.contains("(https://example.org/two)"), "{got}");
        assert!(got.contains("(https://example.org/three)"), "{got}");
        assert!(
            got.contains("(https://example.org/posts/img/a.png)"),
            "{got}"
        );
    }

    #[test]
    fn an_absolute_link_is_left_alone() {
        let md = "[out](https://other.example/x?a=1#f)";
        let got = normalise(md, Some(&base()), Options::default());
        assert!(got.contains("https://other.example/x?a=1#f"), "{got}");
    }

    #[test]
    fn a_link_target_with_brackets_in_it_survives() {
        // The Wikipedia case, which is why the scan counts nesting.
        let md = "[Rust](https://en.wikipedia.org/wiki/Rust_(programming_language))";
        let got = normalise(md, Some(&base()), Options::default());
        assert!(
            got.contains("Rust_(programming_language))"),
            "the closing paren of the URL was mistaken for the end of the link: {got}"
        );
    }

    #[test]
    fn a_link_title_after_the_target_is_kept() {
        let md = r#"[a](/x "A title")"#;
        let got = normalise(md, Some(&base()), Options::default());
        assert!(
            got.contains(r#"(https://example.org/x "A title")"#),
            "{got}"
        );
    }

    #[test]
    fn images_off_leaves_the_alt_text_behind() {
        let md = "Before\n\n![A harbour at dusk](https://e.org/a.png)\n\nAfter";
        let got = normalise(
            md,
            None,
            Options {
                images: false,
                ..Options::default()
            },
        );
        assert!(got.contains("A harbour at dusk"), "{got}");
        assert!(!got.contains("!["), "{got}");
        assert!(!got.contains("a.png"), "{got}");
    }

    #[test]
    fn images_on_keeps_them_for_the_reader_to_draw_as_a_line() {
        let md = "![alt](https://e.org/a.png)";
        let got = normalise(md, None, Options::default());
        assert_eq!(got, md);
    }

    #[test]
    fn the_cut_lands_on_a_paragraph_boundary() {
        let md = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        // The last break that fits, not the first: the cut keeps as much as
        // it can and still lands between paragraphs.
        let got = truncate_at_paragraph(md, 40);
        assert_eq!(got, "First paragraph.\n\nSecond paragraph.");
        assert!(got.len() <= 40);
        assert_eq!(truncate_at_paragraph(md, 20), "First paragraph.");
    }

    #[test]
    fn one_very_long_paragraph_still_comes_out_inside_the_cap() {
        let md = "x".repeat(1000);
        let got = truncate_at_paragraph(&md, 100);
        assert_eq!(got.len(), 100);
    }

    #[test]
    fn the_cut_never_splits_a_character() {
        let md = "é".repeat(500);
        for max in [1, 2, 3, 99, 101] {
            let got = truncate_at_paragraph(&md, max);
            assert!(got.len() <= max);
            assert!(std::str::from_utf8(got.as_bytes()).is_ok());
        }
    }

    #[test]
    fn text_shorter_than_the_cap_is_untouched() {
        assert_eq!(truncate_at_paragraph("short", 100), "short");
    }

    proptest::proptest! {
        /// The three claims the rest of the pipeline relies on. The input is
        /// whatever a converter made of whatever a page held, so these have
        /// to hold for arbitrary strings rather than for the fixtures.
        #[test]
        fn normalise_is_idempotent_bounded_and_still_text(s: String, max in 0usize..2048) {
            let options = Options { max_bytes: max, images: true };
            let once = normalise(&s, Some(&base()), options);
            proptest::prop_assert!(once.len() <= max, "{} > {}", once.len(), max);
            let twice = normalise(&once, Some(&base()), options);
            proptest::prop_assert_eq!(&once, &twice, "normalising twice changed it");
            proptest::prop_assert!(std::str::from_utf8(once.as_bytes()).is_ok());
        }

        #[test]
        fn dropping_images_is_idempotent(s: String) {
            let options = Options { max_bytes: usize::MAX, images: false };
            let once = normalise(&s, None, options);
            let twice = normalise(&once, None, options);
            proptest::prop_assert_eq!(once, twice);
        }

        #[test]
        fn cleaning_a_url_is_idempotent_and_never_invents_one(s: String) {
            let once = clean_url(&s);
            proptest::prop_assert_eq!(&once, &clean_url(&once));
            if Url::parse(s.trim()).is_err() {
                proptest::prop_assert_eq!(once, s.trim().to_string());
            }
        }

        #[test]
        fn a_cut_is_never_longer_than_the_cap(s: String, max in 0usize..512) {
            let got = truncate_at_paragraph(&s, max);
            proptest::prop_assert!(got.len() <= max || s.len() <= max);
        }
    }
}
