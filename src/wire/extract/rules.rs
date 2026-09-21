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

/// What one site puts inside its own article, and what to do about it.
///
/// Readability keeps whatever is inside the element it scored highest, and a
/// share bar, a "more like this" list and a patron link are all inside it on
/// the sites below. None of this is guesswork: every line here was counted in
/// the reference database, and the count is in the comment.
pub struct Rule {
    /// The whole host, or a dot-prefixed tail of it.
    pub host_suffix: &'static str,
    /// Lines to drop, matched as a whole line or as the start of one.
    pub strip_lines: &'static [&'static str],
    /// Lines to drop by something they contain anywhere.
    pub strip_containing: &'static [&'static str],
    /// This line, and everything after it.
    pub strip_from: Option<&'static str>,
    /// How to get the author's name out of the byline this site writes.
    pub byline: Option<Byline>,
    /// Whether an article here is one page of several, and how to tell.
    pub next_page: Option<NextPage>,
}

/// The author's name, inside a sentence about when the piece was published.
pub struct Byline {
    pub after: &'static str,
    pub before: &'static str,
}

/// An article split across pages.
///
/// Opt-in per site rather than followed wherever a `rel="next"` turns up. The
/// mechanism is general -- `next_page` reads any of the four ways a page says
/// where the rest of it is -- but following it everywhere would mean this
/// program fetching eight pages for every blog with a paginated archive, and
/// `CONTRIBUTING.md` asks for a reason in the same commit as any change that
/// fetches more.
pub struct NextPage {
    /// Only followed for articles under this path, because a site's news
    /// items are one page and its reviews are nine.
    pub path_prefix: &'static str,
    /// Including the first. Eight is two more than the longest review in the
    /// reference database.
    pub max_pages: usize,
}

const RULES: &[Rule] = &[
    // 53 of 53 GamingOnLinux articles ended with this line.
    Rule {
        host_suffix: "gamingonlinux.com",
        strip_lines: &["Article taken from"],
        strip_containing: &[],
        strip_from: None,
        byline: None,
        next_page: None,
    },
    // 35 of 37 KitGuru articles carried the share bar and the "more like
    // this" list, and 37 of 37 the patron link.
    Rule {
        host_suffix: "kitguru.net",
        strip_lines: &["Share", "[Become a Patron!]"],
        strip_containing: &[],
        strip_from: Some("### Check Also"),
        byline: None,
        next_page: None,
    },
    // The same patron link, from the same plugin.
    Rule {
        host_suffix: "indieretronews.com",
        strip_lines: &["[Become a Patron!]"],
        strip_containing: &[],
        strip_from: None,
        byline: None,
        next_page: None,
    },
    // Hearst's papers put their ad slots inside the article text: ten of
    // them across two SFGate pieces. The other titles run the same template.
    Rule {
        host_suffix: "sfgate.com",
        strip_lines: &["Article continues below this ad"],
        strip_containing: &[],
        strip_from: None,
        byline: None,
        next_page: None,
    },
    Rule {
        host_suffix: "houstonchronicle.com",
        strip_lines: &["Article continues below this ad"],
        strip_containing: &[],
        strip_from: None,
        byline: None,
        next_page: None,
    },
    Rule {
        host_suffix: "chron.com",
        strip_lines: &["Article continues below this ad"],
        strip_containing: &[],
        strip_from: None,
        byline: None,
        next_page: None,
    },
    Rule {
        host_suffix: "timesunion.com",
        strip_lines: &["Article continues below this ad"],
        strip_containing: &[],
        strip_from: None,
        byline: None,
        next_page: None,
    },
    Rule {
        host_suffix: "expressnews.com",
        strip_lines: &["Article continues below this ad"],
        strip_containing: &[],
        strip_from: None,
        byline: None,
        next_page: None,
    },
    // Every Phoronix item opens with the category badge -- an image whose
    // alt text is the word VIRTUALIZATION -- and its byline is a sentence
    // ending in "Add A Comment". Its reviews are nine pages, of which the
    // reference database has page one of three of them.
    Rule {
        host_suffix: "phoronix.com",
        strip_lines: &[],
        strip_containing: &["/assets/categories/"],
        strip_from: None,
        byline: Some(Byline {
            after: "Written by ",
            before: " in ",
        }),
        next_page: Some(NextPage {
            path_prefix: "/review/",
            max_pages: 8,
        }),
    },
];

/// The rule for a host, where there is one. The longest matching suffix wins.
pub fn for_host(host: &str) -> Option<&'static Rule> {
    let host = host.to_ascii_lowercase();
    RULES
        .iter()
        .filter(|rule| {
            host == rule.host_suffix || host.ends_with(&format!(".{}", rule.host_suffix))
        })
        .max_by_key(|rule| rule.host_suffix.len())
}

/// Take a site's furniture out of its article.
///
/// Line by line, and **never inside a fenced code block**: a line saying
/// `Share` in a shell transcript is the transcript, and there is a property
/// test. Blank lines left behind are not tidied here -- `normalise` runs
/// after this and collapses them, which is the same work done once.
pub fn strip(markdown: &str, host: &str) -> String {
    let Some(rule) = for_host(host) else {
        return markdown.to_string();
    };
    let mut out = String::with_capacity(markdown.len());
    let mut in_code = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if !in_code && drops(rule, line) {
            continue;
        }
        if !in_code && rule.strip_from.is_some_and(|from| matches(line, from)) {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn drops(rule: &Rule, line: &str) -> bool {
    rule.strip_lines.iter().any(|needle| matches(line, needle))
        || rule
            .strip_containing
            .iter()
            .any(|needle| line.contains(needle))
}

/// A line matches a rule when it *is* the text or *starts with* it. Both are
/// wanted: `Share` is the whole line, and `Article taken from` is followed by
/// a link.
fn matches(line: &str, needle: &str) -> bool {
    let line = line.trim();
    line == needle || line.starts_with(needle)
}

/// The author's name out of a site's byline sentence.
///
/// Phoronix writes `Written by Michael Larabel in AMD on 21 September 2026 at
/// 06:22 AM EDT. Add A Comment`, all of which the reader draws under the
/// headline, and only the first three words of which are the byline.
pub fn byline(byline: &str, host: &str) -> Option<String> {
    let rule = for_host(host)?;
    let shape = rule.byline.as_ref()?;
    let rest = byline.trim().strip_prefix(shape.after)?;
    let name = match rest.find(shape.before) {
        Some(at) => &rest[..at],
        None => rest,
    }
    .trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Where the rest of this article is, if the page says.
///
/// Four ways, in the order they are worth trusting: `<link rel=next>` in the
/// head, an `<a rel=next>`, an `<a>` whose `title` says it, and an `<a>`
/// whose text says it. The guard is the same for all four and is the point of
/// the function: **same origin, and under `path_prefix`**. A "next" link that
/// leaves the site is an advertisement, and one that leaves the path is the
/// site's own navigation.
pub fn next_page(html: &str, from: &url::Url, path_prefix: &str) -> Option<url::Url> {
    const WORDS: &[&str] = &[
        "next page",
        "go to next page",
        "next",
        "»",
        ">>",
        "next »",
        "next >",
    ];

    let document = dom_query::Document::from(html);
    let mut candidates: Vec<String> = Vec::new();
    for selector in [r#"link[rel="next"]"#, r#"a[rel="next"]"#] {
        for node in document.select(selector).nodes() {
            if let Some(href) = node.attr("href") {
                candidates.push(href.to_string());
            }
        }
    }
    for node in document.select("a").nodes() {
        let says = |text: &str| {
            let text = text.trim().to_ascii_lowercase();
            WORDS.contains(&text.as_str())
        };
        let titled = node.attr("title").is_some_and(|title| says(&title));
        if titled || says(&node.text()) {
            if let Some(href) = node.attr("href") {
                candidates.push(href.to_string());
            }
        }
    }

    candidates.into_iter().find_map(|href| {
        let next = from.join(href.trim()).ok()?;
        let same_origin = next.scheme() == from.scheme()
            && next.host_str() == from.host_str()
            && next.port_or_known_default() == from.port_or_known_default();
        let under_prefix = next.path().starts_with(path_prefix);
        (same_origin && under_prefix && next != *from).then_some(next)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lines of some markdown that are inside a fenced block, which is
    /// what the property below is about.
    fn fenced_lines(markdown: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut in_code = false;
        for line in markdown.lines() {
            let fence = line.trim_start().starts_with("```");
            if fence {
                in_code = !in_code;
                continue;
            }
            if in_code {
                out.push(line);
            }
        }
        out
    }

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

    #[test]
    fn a_sites_footer_comes_off_and_the_article_does_not() {
        let md = "The piece itself.\n\nArticle taken from [GamingOnLinux.com.](https://www.gamingonlinux.com/)\n";
        let got = strip(md, "www.gamingonlinux.com");
        assert!(got.contains("The piece itself."), "{got:?}");
        assert!(!got.contains("Article taken from"), "{got:?}");

        // A site with no rule is untouched, whatever is in it.
        assert_eq!(strip(md, "example.org"), md);
    }

    #[test]
    fn everything_after_a_more_like_this_heading_goes() {
        let md = "***KitGuru Says: something.***\n\n\
             [Become a Patron!](https://www.patreon.com/bePatron?u=1)\n\n\
             Share\n\n\
             ### Check Also\n\n\
             [![](https://www.kitguru.net/a.jpg)](https://www.kitguru.net/b/)\n\n\
             ## [Another article entirely](https://www.kitguru.net/b/)\n";
        let got = strip(md, "www.kitguru.net");
        assert!(got.contains("KitGuru Says"), "{got:?}");
        assert!(!got.contains("Patron"), "{got:?}");
        assert!(!got.contains("Share"), "{got:?}");
        assert!(!got.contains("Check Also"), "{got:?}");
        assert!(
            !got.contains("Another article entirely"),
            "the list after the heading survived: {got:?}"
        );
    }

    #[test]
    fn a_category_badge_is_not_part_of_the_article() {
        let md =
            "![VIRTUALIZATION](https://www.phoronix.com/assets/categories/virtualization.webp)\n\n\
             Here's some very intriguing work taking place.\n";
        let got = strip(md, "www.phoronix.com");
        assert!(!got.contains("VIRTUALIZATION"), "{got:?}");
        assert!(got.contains("intriguing work"), "{got:?}");
    }

    #[test]
    fn an_ad_slot_inside_the_prose_comes_out() {
        let md = "A paragraph.\n\nArticle continues below this ad\n\nAnother paragraph.\n";
        let got = strip(md, "www.sfgate.com");
        assert!(!got.contains("below this ad"), "{got:?}");
        assert!(got.contains("Another paragraph."), "{got:?}");
    }

    #[test]
    fn a_line_inside_a_code_block_is_the_authors_whatever_it_says() {
        let md =
            "Before.\n\n```sh\nShare\nArticle taken from nowhere\n### Check Also\n```\n\nAfter.\n";
        let got = strip(md, "www.kitguru.net");
        assert!(
            got.contains("Share\nArticle taken from nowhere\n### Check Also"),
            "{got:?}"
        );
        assert!(got.contains("After."), "{got:?}");
    }

    #[test]
    fn a_byline_sentence_becomes_the_authors_name() {
        assert_eq!(
            byline(
                "Written by Michael Larabel in AMD on 21 September 2026 at 06:22 AM EDT. Add A Comment",
                "www.phoronix.com"
            )
            .as_deref(),
            Some("Michael Larabel")
        );
        // A byline that is not the shape the rule knows is left alone, and so
        // is one from a site with no rule.
        assert_eq!(byline("Michael Larabel", "www.phoronix.com"), None);
        assert_eq!(byline("Written by Somebody in Things", "example.org"), None);
    }

    #[test]
    fn the_next_page_is_the_one_on_this_site_and_under_this_path() {
        let from = url::Url::parse("https://www.phoronix.com/review/a-long-test/1").unwrap();
        let html = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/pages/paged-1.html"),
        )
        .unwrap();
        let next = next_page(&html, &from, "/review/").unwrap();
        assert_eq!(
            next.as_str(),
            "https://www.phoronix.com/review/a-long-test/2"
        );

        // The last page says nothing about a next one.
        let html = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/pages/paged-2.html"),
        )
        .unwrap();
        let from = url::Url::parse("https://www.phoronix.com/review/a-long-test/2").unwrap();
        assert_eq!(next_page(&html, &from, "/review/"), None);
    }

    #[test]
    fn a_next_link_that_leaves_the_site_or_the_path_is_not_followed() {
        let from = url::Url::parse("https://e.org/review/one/1").unwrap();
        for html in [
            r#"<a rel="next" href="https://elsewhere.example/review/one/2">Next Page</a>"#,
            r#"<a rel="next" href="http://e.org/review/one/2">Next Page</a>"#,
            r#"<a rel="next" href="/news/something">Next Page</a>"#,
            r#"<a rel="next" href="/review/one/1">Next Page</a>"#,
        ] {
            assert_eq!(next_page(html, &from, "/review/"), None, "{html}");
        }
    }

    proptest::proptest! {
        /// Stripping never reaches inside a fenced block. The lines in one
        /// are somebody's shell transcript, and a transcript that says
        /// `Share` says `Share`.
        ///
        /// `### Check Also` is deliberately not in the corpus: it is a
        /// `strip_from`, which truncates the rest of the document on purpose
        /// -- a fence after it is inside the "more like this" list, not
        /// inside the article -- and that rule has its own table test.
        #[test]
        fn a_fenced_block_is_never_touched(
            lines in proptest::collection::vec(
                proptest::sample::select(vec![
                    "Share", "Article taken from a place",
                    "Article continues below this ad", "[Become a Patron!](x)",
                    "![A badge](https://www.phoronix.com/assets/categories/amd.webp)",
                    "Ordinary prose.", "", "```", "```sh",
                ]),
                0..24,
            ),
            host in proptest::sample::select(vec![
                "www.kitguru.net", "www.gamingonlinux.com", "www.sfgate.com",
                "www.phoronix.com", "example.org",
            ]),
        ) {
            let md = lines.join("\n");
            let got = strip(&md, host);
            // Every line that was inside a fence in the input is still there,
            // in order, in the output.
            let fenced: Vec<&str> = fenced_lines(&md);
            let kept: Vec<&str> = got.lines().collect();
            let mut at = 0usize;
            for line in fenced {
                let found = kept[at..].iter().position(|k| *k == line);
                proptest::prop_assert!(
                    found.is_some(),
                    "{line:?} was inside a fence and is gone from {got:?}"
                );
                at += found.unwrap() + 1;
            }
        }

        /// And the follower never leaves the origin it started on, whatever
        /// the page says.
        #[test]
        fn the_follower_never_leaves_the_origin(html: String) {
            let from = url::Url::parse("https://e.org/review/one/1").unwrap();
            if let Some(next) = next_page(&html, &from, "/review/") {
                proptest::prop_assert_eq!(next.scheme(), from.scheme());
                proptest::prop_assert_eq!(next.host_str(), from.host_str());
                proptest::prop_assert_eq!(
                    next.port_or_known_default(),
                    from.port_or_known_default()
                );
                proptest::prop_assert!(next.path().starts_with("/review/"), "{}", next);
                proptest::prop_assert_ne!(&next, &from);
            }
        }

        /// The input is somebody else's page, reduced. It may be anything.
        #[test]
        fn arbitrary_markdown_never_panics(s: String, host: String) {
            let _ = is_paywall_stub(&s, &host);
            let _ = strip(&s, &host);
            let _ = byline(&s, &host);
        }
    }
}
