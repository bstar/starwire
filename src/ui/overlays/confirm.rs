//! "Are you sure?" -- two shapes of it: removing a feed, and marking a
//! whole source read.
//!
//! The box is STAR/KIT's `chrome::confirm`, answered with `y`/`n` rather
//! than `enter` for the reason that widget documents: a dialogue whose
//! default key is the one already under a reader's thumb is a dialogue that
//! answers a keystroke meant for whatever came before it.
//!
//! Both questions are destructive in the one way this program can be.
//! Removing a feed takes its entries and its extracted articles with it, and
//! `wire.db` is not a cache; marking forty entries read cannot be undone in
//! one key the way `m` undoes one.

use starkit::chrome::confirm;
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;

use crate::ui::theme::Theme;
use crate::wire::feed::{FeedId, Selection};

use super::Pending;

/// A question, what its two answers are called, and what saying yes does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub title: String,
    /// Logical lines; [`layout`] wraps each to the box's width, so a caller
    /// writes a sentence and never a column count.
    pub body: Vec<String>,
    pub yes: &'static str,
    pub no: &'static str,
    pub pending: Pending,
}

impl Confirm {
    /// `d` in SOURCES. The entries go with the feed, and so does every
    /// article ever pulled out of a page for it -- which is the part worth
    /// saying, because nothing else in this program keeps a copy.
    pub fn remove_feed(feed: FeedId, name: &str) -> Self {
        Self {
            title: "remove the feed".into(),
            body: vec![
                format!("{name} will be removed."),
                "Its entries and everything extracted for them go with it.".into(),
            ],
            yes: "remove",
            no: "keep",
            pending: Pending::RemoveFeed(feed),
        }
    }

    /// `A`, in either list. The count is what the question is really about:
    /// marking four read is nothing, and marking four hundred read is the
    /// afternoon somebody meant to spend reading them.
    pub fn mark_all_read(sel: Selection, source: &str, n: i64) -> Self {
        let noun = if n == 1 { "entry" } else { "entries" };
        Self {
            title: "mark read".into(),
            body: vec![format!(
                "{n} unread {noun} in {source} will be marked read."
            )],
            yes: "mark",
            no: "keep",
            pending: Pending::MarkAllRead(sel),
        }
    }
}

/// This question, as the shared widget spells it.
fn kit(c: &Confirm) -> confirm::Confirm {
    confirm::Confirm {
        title: c.title.clone(),
        body: c.body.clone(),
        yes: c.yes,
        no: c.no,
    }
}

/// Where the box lands and where its two answers sit, read straight through
/// to `chrome::confirm`'s own so a click cannot disagree with what was drawn.
pub(super) fn layout(area: Rect, c: &Confirm) -> Option<confirm::Layout> {
    confirm::layout(area, &kit(c))
}

pub fn render(area: Rect, buf: &mut Buffer, theme: &Theme, c: &Confirm) {
    confirm::render(area, buf, theme, &kit(c));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::tests_support::theme;

    fn dump(buf: &Buffer, area: Rect) -> String {
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn one_entry_reads_as_singular() {
        let c = Confirm::mark_all_read(Selection::All, "All", 1);
        assert!(c.body[0].contains("1 unread entry "), "{:?}", c.body);
        let c = Confirm::mark_all_read(Selection::All, "All", 12);
        assert!(c.body[0].contains("12 unread entries "), "{:?}", c.body);
    }

    #[test]
    fn each_question_names_its_own_answers_and_its_own_subject() {
        let c = Confirm::remove_feed(FeedId(1), "Phoronix");
        assert_eq!(c.yes, "remove");
        assert!(c.body[0].contains("Phoronix"));
        assert_eq!(
            Confirm::mark_all_read(Selection::Videos, "Videos", 3).yes,
            "mark"
        );
    }

    #[test]
    fn the_box_draws_its_title_body_and_answers() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 60, 21);
        let mut buf = Buffer::empty(area);
        render(
            area,
            &mut buf,
            &t,
            &Confirm::remove_feed(FeedId(1), "Lobsters"),
        );
        let text = dump(&buf, area);
        assert!(text.contains("REMOVE THE FEED"), "{text}");
        assert!(text.contains("Lobsters"), "{text}");
        assert!(text.contains("y remove"), "{text}");
        assert!(text.contains("n keep"), "{text}");
    }

    #[test]
    fn a_long_body_wraps_rather_than_overruns_the_box() {
        let area = Rect::new(0, 0, 60, 21);
        let c = Confirm::remove_feed(FeedId(1), &"a very long feed name ".repeat(6));
        let l = layout(area, &c).expect("it fits");
        assert!(l.body_rows.len() > 1);
        for row in &l.body_rows {
            assert!(starkit::wrap::width_of(row) < l.inner.width, "{row:?}");
        }
    }
}
