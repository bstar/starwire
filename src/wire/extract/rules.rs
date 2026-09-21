//! What a page turned out to be, and what to take off it: the two tables of
//! per-site knowledge, and nothing that fetches.
//!
//! Everything here is **data plus a function that applies it**. That is the
//! whole design: a site that grows a new footer is a line in a table and a
//! fixture, not a branch somewhere in the pipeline, and the next person to
//! read this file can see the entire list of things this program believes
//! about particular websites in one place.
//!
//! Two kinds of knowledge live here. A **stub** is a page that answered 200
//! and gave nothing -- the first two paragraphs above a wall -- which is
//! worth telling apart from an article, because the feed's own summary is
//! usually the same text and always more honest about being a summary. A
//! **rule** is furniture a site puts inside its own article element, which
//! readability therefore keeps: a share bar, a "more like this" list, a
//! patron link.

/// Phrases a paywall stub ends with.
///
/// Matched against the tail rather than the whole text, because these are
/// what a page says *instead of the rest of the article*: an article that
/// mentions subscribing in its third paragraph is an article. Case is
/// ignored; the shortest is deliberately distinctive enough to stand alone.
const STUB_PHRASES: &[&str] = &[
    "this post is for paid subscribers",
    "this post is for paying subscribers",
    "already a paid subscriber",
    "subscribe to continue",
    "subscribe to keep reading",
    "keep reading with a 7-day free trial",
    "sign in to read",
    "continue reading with",
    "to continue reading",
    "this article is for subscribers",
    "become a paid subscriber",
    "already have an account? sign in",
];

/// How much of the end of the text a stub phrase has to be in.
const TAIL: usize = 500;

/// Sites whose pages answer 200 with the first paragraph or two of an
/// article and keep the rest.
///
/// A short list, and only the ones that behave this way -- a hard wall
/// answers 401 or 403 and is a failure rather than a stub, which is what the
/// ten NYT, WSJ and Reuters entries in the reference database did. The five
/// Bloomberg articles there are the measured case: 494, 617, 753 and 880
/// bytes of text each, none of which is an article, all of which were stored
/// as successes.
///
/// This list never stops a page being fetched. It only says that a *short*
/// result from one of these hosts is a stub rather than a short article,
/// which is a judgement that would be wrong on `archlinux.org`.
const METERED: &[&str] = &[
    "bloomberg.com",
    "wsj.com",
    "nytimes.com",
    "ft.com",
    "economist.com",
    "theatlantic.com",
    "newyorker.com",
    "washingtonpost.com",
    "businessinsider.com",
    "barrons.com",
    "theinformation.com",
    "seekingalpha.com",
    "thetimes.co.uk",
    "telegraph.co.uk",
];

/// What a page from a metered site has to exceed to be an article.
///
/// A thousand characters is about three paragraphs. Every Bloomberg stub
/// measured was under nine hundred, and every real article on any of these
/// sites is several thousand.
const STUB_LENGTH: usize = 1000;

/// Whether what came back is a wall rather than a piece.
///
/// The host is asked for because the length test is only safe with it: a
/// six-hundred-character result from `archlinux.org` is a news item, and the
/// same from `bloomberg.com` is the free sample.
pub fn is_paywall_stub(markdown: &str, host: &str) -> bool {
    let tail = tail_of(markdown).to_ascii_lowercase();
    if STUB_PHRASES.iter().any(|phrase| tail.contains(phrase)) {
        return true;
    }
    is_metered(host) && markdown.chars().count() < STUB_LENGTH
}

fn tail_of(markdown: &str) -> &str {
    let trimmed = markdown.trim_end();
    let mut at = trimmed.len().saturating_sub(TAIL);
    while at > 0 && !trimmed.is_char_boundary(at) {
        at -= 1;
    }
    &trimmed[at..]
}

/// Whether a host is one of the metered ones, by whole host or by a
/// dot-prefixed tail, so `www.bloomberg.com` matches and `notbloomberg.com`
/// does not.
fn is_metered(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    METERED
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phrase_a_page_ends_on_is_what_makes_it_a_stub() {
        let substack = "`Hello! Editor Matt` here; this is Dracula Weekly, Week 19.\n\n\
             OK bac…\n\n## This post is for paid subscribers\n\n\
             [Already a paid subscriber? **Sign in**](https://substack.com/sign-in)";
        assert!(is_paywall_stub(substack, "draculadaily.substack.com"));
    }

    #[test]
    fn an_article_that_mentions_subscribing_is_still_an_article() {
        let article = format!(
            "# On newsletters\n\nSubscribe to continue is the phrase every one of \
             them ends on, and this piece is about why.\n\n{}",
            "A paragraph of the actual argument. ".repeat(60)
        );
        assert!(
            !is_paywall_stub(&article, "example.org"),
            "the phrase was in the first paragraph, not instead of the rest"
        );
    }

    #[test]
    fn a_short_page_from_a_metered_site_is_the_free_sample() {
        // The shape of the five Bloomberg articles in the reference
        // database: a dateline and two paragraphs, under nine hundred bytes.
        let stub = format!(
            "September 15, 2026 at 10:31 PM UTC\n\n{}",
            "A sentence of the opening. ".repeat(20)
        );
        assert!(stub.len() < STUB_LENGTH);
        assert!(is_paywall_stub(&stub, "www.bloomberg.com"));

        // The same length from a site that publishes short news items is a
        // short news item.
        assert!(!is_paywall_stub(&stub, "archlinux.org"));
        assert!(!is_paywall_stub(&stub, "notbloomberg.com"));

        // And a real article on a metered site is not a stub either.
        let whole = "A paragraph of the piece. ".repeat(100);
        assert!(!is_paywall_stub(&whole, "www.bloomberg.com"));
    }

    #[test]
    fn nothing_at_all_is_not_a_stub_for_a_site_that_is_not_metered() {
        assert!(!is_paywall_stub("", "example.org"));
        assert!(is_paywall_stub("", "www.wsj.com"));
    }

    proptest::proptest! {
        /// The input is somebody else's page, reduced. It may be anything.
        #[test]
        fn arbitrary_markdown_never_panics(s: String, host: String) {
            let _ = is_paywall_stub(&s, &host);
        }
    }
}
