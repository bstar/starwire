//! The modules of the column, and the words on their headers.
//!
//! A module is a pure widget over a small view struct, as in STAR/FOLD and
//! STAR/CORD. Three of them here, in the order a feed list is drilled
//! through: the sources, the entries of whichever source is chosen, and the
//! article.
//!
//! The reader is a *module*, not a level of the stack, which is what makes
//! `n` and `p` able to replace the article in place without the frame ids
//! churning -- see `ui/stack.rs`. It folds like the two lists do, to a line
//! that says what is open in it.

pub mod entries;
pub mod reader;
pub mod sources;

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use starkit::chrome::header;
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::Style;

use super::keymap::Module;

// The border, corners, titles and header row are STAR/KIT's, and so are the
// small text helpers every panel would otherwise keep its own copy of.
// Re-exported under their old names so the panels to come keep the imports
// the family's other applications already have.
pub use starkit::chrome::rgb;
pub use starkit::text::fit;
pub use starkit::wrap::width_of;

/// The three modules of the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleId {
    Sources,
    Entries,
    Reader,
}

/// The column, top to bottom -- draw order, tab order and the order the feed
/// list is drilled through, deliberately the same order.
pub const COLUMN: [ModuleId; 3] = [ModuleId::Sources, ModuleId::Entries, ModuleId::Reader];

impl ModuleId {
    pub fn index(self) -> usize {
        match self {
            ModuleId::Sources => 0,
            ModuleId::Entries => 1,
            ModuleId::Reader => 2,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            ModuleId::Sources => "sources",
            ModuleId::Entries => "entries",
            ModuleId::Reader => "reader",
        }
    }

    pub fn module(self) -> Module {
        match self {
            ModuleId::Sources => Module::Sources,
            ModuleId::Entries => Module::Entries,
            ModuleId::Reader => Module::Reader,
        }
    }
}

/// A word on a module's header row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    /// `‹`: back one level -- where a browser keeps it.
    Back,
    Add,
    Refresh,
    Unread,
    Filter,
    Star,
    Browser,
}

impl header::Word for Word {
    fn word(self) -> Cow<'static, str> {
        match self {
            Word::Back => "\u{2039}".into(),
            Word::Add => "add".into(),
            Word::Refresh => "refresh".into(),
            Word::Unread => "unread".into(),
            Word::Filter => "filter".into(),
            Word::Star => "star".into(),
            Word::Browser => "browser".into(),
        }
    }
}

/// What each module offers, right-aligned, dropped from the left as it
/// narrows.
///
/// SOURCES has no `‹` because there is nothing behind it: it is the root of
/// the column and is never popped.
pub fn words(module: ModuleId) -> Vec<Word> {
    match module {
        ModuleId::Sources => vec![Word::Add, Word::Refresh],
        ModuleId::Entries => vec![Word::Back, Word::Unread, Word::Filter],
        ModuleId::Reader => vec![Word::Back, Word::Star, Word::Browser],
    }
}

/// The application's name as the top of the window says it, letter-spaced,
/// exactly as the rest of the family says its own.
pub const HEADING: &str = "S T A R / W I R E";

/// Cut a string to `width` columns by taking out its middle, so both ends
/// survive: a long headline keeps the part that says what it is about and
/// the part that says how it ends.
pub fn elide_middle(text: &str, width: u16) -> String {
    let w = width_of(text);
    if w <= width {
        return text.to_string();
    }
    if width < 4 {
        return fit(text, width);
    }
    let keep = usize::from(width - 1);
    let head = keep / 2;
    let tail = keep - head;
    let clusters: Vec<&str> = starkit::wrap::clusters(text).map(|(_, c)| c).collect();
    let mut out = String::new();
    let mut used = 0usize;
    for c in &clusters {
        let cw = usize::from(width_of(c));
        if used + cw > head {
            break;
        }
        out.push_str(c);
        used += cw;
    }
    out.push('\u{2026}');
    let mut back = Vec::new();
    let mut used = 0usize;
    for c in clusters.iter().rev() {
        let cw = usize::from(width_of(c));
        if used + cw > tail {
            break;
        }
        back.push(*c);
        used += cw;
    }
    for c in back.iter().rev() {
        out.push_str(c);
    }
    out
}

/// The one line a folded module draws: what is currently open in it.
pub fn summary_row(body: Rect, buf: &mut Buffer, text: &str, style: Style) {
    if body.height == 0 || body.width == 0 {
        return;
    }
    buf.set_string(body.x, body.y, fit(text, body.width), style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_is_in_the_column_once() {
        for m in COLUMN {
            assert_eq!(
                COLUMN.iter().filter(|&&q| q == m).count(),
                1,
                "{m:?} is not in the column exactly once"
            );
            assert_eq!(COLUMN[m.index()], m, "{m:?} indexes somebody else's rect");
        }
    }

    #[test]
    fn a_module_serialises_as_its_lowercase_name() {
        assert_eq!(serialised(ModuleId::Sources), "sources");
        assert_eq!(serialised(ModuleId::Reader), "reader");
    }

    fn serialised(id: ModuleId) -> String {
        // No serde_json here; toml round-trips the same derive.
        #[derive(Serialize)]
        struct Wrap {
            id: ModuleId,
        }
        let text = toml::to_string(&Wrap { id }).unwrap();
        text.trim()
            .trim_start_matches("id = \"")
            .trim_end_matches('"')
            .to_string()
    }

    #[test]
    fn every_module_maps_to_its_own_key_module() {
        let mut seen = Vec::new();
        for m in COLUMN {
            let k = m.module();
            assert!(!seen.contains(&k), "{k:?} is claimed by two modules");
            seen.push(k);
        }
    }

    #[test]
    fn a_summary_row_is_one_fitted_line() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        summary_row(area, &mut buf, "a rather long summary", Style::default());
        let drawn: String = (0..10).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(drawn, "a rather l");
    }

    /// The header words are what the plan's mocks show, and each one is a
    /// thing the module it sits on can actually do.
    #[test]
    fn no_module_offers_a_word_it_cannot_honour() {
        for m in COLUMN {
            for w in words(m) {
                let allowed = match m {
                    ModuleId::Sources => matches!(w, Word::Add | Word::Refresh),
                    ModuleId::Entries => matches!(w, Word::Back | Word::Unread | Word::Filter),
                    ModuleId::Reader => matches!(w, Word::Back | Word::Star | Word::Browser),
                };
                assert!(allowed, "{m:?} offers {w:?}");
            }
        }
        assert!(
            !words(ModuleId::Sources).contains(&Word::Back),
            "the sources are the root of the column; there is nothing behind them"
        );
    }

    #[test]
    fn eliding_the_middle_keeps_both_ends() {
        let title = "Why the borrow checker says no: a field guide to lifetimes";
        let cut = elide_middle(title, 24);
        assert_eq!(width_of(&cut), 24);
        assert!(cut.starts_with("Why the bor"), "{cut:?}");
        assert!(cut.ends_with("lifetimes"), "{cut:?}");
        assert!(cut.contains('\u{2026}'));

        // Short enough already: untouched.
        assert_eq!(elide_middle("short", 20), "short");
        // Too narrow for both ends and an ellipsis: `fit` takes over and
        // cuts from the left, which is all three columns will hold.
        assert_eq!(elide_middle("abcdef", 3), "abc");
    }

    /// Widths are measured in clusters, not characters, so a title with an
    /// emoji in it does not overrun the column it was cut for.
    #[test]
    fn eliding_measures_in_columns_and_not_characters() {
        let text = "aaaa\u{1f469}\u{200d}\u{1f4bb}bbbb";
        for width in 4u16..=14 {
            let cut = elide_middle(text, width);
            assert!(width_of(&cut) <= width, "{cut:?} at {width}");
        }
    }
}
