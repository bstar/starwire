//! The `/` filter: narrowing the rows already on screen.
//!
//! Not to be confused with search. **Search** is FTS5 over every article ever
//! extracted (`db::articles::search`), it costs a query, and its results are
//! a stack level of their own. **This** is a fuzzy match over the two hundred
//! rows already loaded, it costs nothing, and it runs on every keystroke
//! while the filter field is open. Both exist because they answer different
//! questions: "where was that thing about lifetimes" and "which of these
//! forty is the one about the borrow checker".
//!
//! `nucleo-matcher`, as in STAR/FOLD: the same matcher Helix uses, as a
//! library rather than as a picker. This supplies the ordering and the panel
//! draws it.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

use super::feed::EntryRow;

/// Indices into `rows`, best match first.
///
/// An empty query matches everything, in the order it was given -- which is
/// what makes clearing the filter with `esc` put the list back exactly as it
/// was rather than re-sorting it.
pub fn filter(query: &str, rows: &[EntryRow]) -> Vec<usize> {
    // The feed name is part of what is matched, so that typing `phoronix`
    // into the filter on an aggregate list narrows to one source without
    // having to go back and pick it.
    let haystacks: Vec<String> = rows
        .iter()
        .map(|row| match &row.author {
            Some(author) => format!("{} {} {}", row.title, row.feed_title, author),
            None => format!("{} {}", row.title, row.feed_title),
        })
        .collect();
    matches(query, &haystacks)
}

/// The same match, over whatever strings the caller has: the window's
/// SOURCES list filters its folder and feed names through this, so the two
/// `/` filters behave identically and there is one matcher configured in
/// one place.
pub fn matches(query: &str, haystacks: &[String]) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..haystacks.len()).collect();
    }

    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(
        query.trim(),
        // Smart case, as everywhere else in the family: a lowercase query
        // matches anything, and typing a capital means you meant it.
        CaseMatching::Smart,
        Normalization::Smart,
    );

    let mut scored: Vec<(u32, usize)> = haystacks
        .iter()
        .enumerate()
        .filter_map(|(index, haystack)| {
            let mut buf = Vec::new();
            let utf32 = nucleo_matcher::Utf32Str::new(haystack, &mut buf);
            pattern
                .score(utf32, &mut matcher)
                .map(|score| (score, index))
        })
        .collect();

    // Descending by score, and by the original position where two rows score
    // the same -- so the newest of two equally good matches is first, which
    // is the order the list was already in.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, index)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::feed::{ArticleStatus, EntryId, EntryKind, FeedId};

    fn row(id: i64, title: &str, feed: &str, author: Option<&str>) -> EntryRow {
        EntryRow {
            id: EntryId(id),
            feed_id: FeedId(1),
            feed_title: feed.into(),
            title: title.into(),
            author: author.map(str::to_string),
            url: None,
            published: None,
            kind: EntryKind::Article,
            read: false,
            starred: false,
            article_status: ArticleStatus::Pending,
            thumbnail_url: None,
        }
    }

    fn rows() -> Vec<EntryRow> {
        vec![
            row(
                1,
                "Why the borrow checker says no",
                "Example Journal",
                Some("Jane Example"),
            ),
            row(2, "A new release of Phoronix Test Suite", "Phoronix", None),
            row(
                3,
                "Lifetimes are not about lifetime",
                "Example Journal",
                None,
            ),
            row(4, "Something else entirely", "Lobsters", None),
        ]
    }

    #[test]
    fn an_empty_query_is_every_row_in_the_order_it_came() {
        let rows = rows();
        assert_eq!(filter("", &rows), vec![0, 1, 2, 3]);
        assert_eq!(filter("   ", &rows), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_query_narrows_to_what_matches() {
        let rows = rows();
        let got = filter("borrow", &rows);
        assert_eq!(got, vec![0]);
    }

    #[test]
    fn the_feed_name_is_matched_too() {
        let rows = rows();
        let got = filter("phoronix", &rows);
        assert!(got.contains(&1), "{got:?}");
    }

    #[test]
    fn an_author_is_matched_too() {
        let rows = rows();
        let got = filter("jane", &rows);
        assert_eq!(got, vec![0]);
    }

    #[test]
    fn matching_is_fuzzy_rather_than_a_substring_search() {
        let rows = rows();
        // The letters of "lftms", in order, inside "Lifetimes".
        let got = filter("lftms", &rows);
        assert!(got.contains(&2), "{got:?}");
    }

    #[test]
    fn case_is_smart() {
        let rows = rows();
        assert!(filter("phoronix", &rows).contains(&1));
        assert!(filter("Phoronix", &rows).contains(&1));
        assert!(
            filter("PHORONIX", &rows).is_empty(),
            "typing capitals means you meant them"
        );
    }

    /// The window's SOURCES list comes through here with folder and feed
    /// names, and gets the same match the entries do.
    #[test]
    fn plain_names_match_the_same_way() {
        let names: Vec<String> = ["All", "Unread", "Tech", "Phoronix", "Hacker News"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(matches("pho", &names), vec![3]);
        assert_eq!(matches("", &names), vec![0, 1, 2, 3, 4]);
        assert!(matches("zzzz", &names).is_empty());
    }

    #[test]
    fn nothing_matching_is_an_empty_list_rather_than_everything() {
        let rows = rows();
        assert!(filter("zzzzqqq", &rows).is_empty());
    }

    #[test]
    fn an_empty_list_filters_to_an_empty_list() {
        assert!(filter("anything", &[]).is_empty());
        assert!(filter("", &[]).is_empty());
    }

    proptest::proptest! {
        /// Everything here is somebody's typing against somebody else's
        /// titles. The claims are that it returns, and that every index it
        /// returns is one the caller can use -- an out-of-range index would
        /// panic in the panel that draws it.
        #[test]
        fn every_index_returned_is_in_range(query: String, count in 0usize..40) {
            let rows: Vec<EntryRow> = (0..count as i64)
                .map(|n| row(n, &format!("Title {n}"), "Feed", None))
                .collect();
            let got = filter(&query, &rows);
            proptest::prop_assert!(got.len() <= rows.len());
            for index in &got {
                proptest::prop_assert!(*index < rows.len());
            }
            let mut sorted = got.clone();
            sorted.sort_unstable();
            sorted.dedup();
            proptest::prop_assert_eq!(sorted.len(), got.len(), "an index twice");
        }

        #[test]
        fn arbitrary_titles_never_panic(titles: Vec<String>, query: String) {
            let rows: Vec<EntryRow> = titles
                .iter()
                .enumerate()
                .map(|(n, t)| row(n as i64, t, "Feed", None))
                .collect();
            let _ = filter(&query, &rows);
        }
    }
}
