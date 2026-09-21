//! Searching every article ever extracted.
//!
//! The results are not a list this overlay holds: submitting pushes a
//! `Level::Entries{ Selection::Search(q) }` onto the stack and sends
//! `OpenFeed` for it, so a search is a level like any other -- it can be
//! backed out of, jumped away from and returned to, and the entry keys work
//! in it because it *is* the entries module.
//!
//! Not to be confused with `/`, which narrows the rows already on screen and
//! is not an overlay at all. `wire/search.rs`'s own doc has the distinction.

use starkit::chrome::overlay::{self, Anchor};
use starkit::crossterm::event::KeyEvent;
use starkit::input::{Edit, TextInput};
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::Style;

use crate::ui::panels::rgb;
use crate::ui::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Taken,
    Close,
    Submit(String),
}

#[derive(Debug)]
pub struct Search {
    pub input: TextInput,
}

impl Default for Search {
    fn default() -> Self {
        Self::new()
    }
}

impl Search {
    pub fn new() -> Self {
        Self {
            input: TextInput::single(),
        }
    }

    /// Opened from the `/ Search…` row in SOURCES with whatever was searched
    /// for last, so refining a search is two keys rather than retyping it.
    pub fn with(query: &str) -> Self {
        Self {
            input: TextInput::single().with_text(query),
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> Action {
        match self.input.handle(key) {
            Edit::Submit => {
                let text = self.input.text().trim().to_string();
                // An empty search is not a refusal worth a red line: it is
                // somebody changing their mind, and closing is what they
                // meant.
                if text.is_empty() {
                    Action::Close
                } else {
                    Action::Submit(text)
                }
            }
            Edit::Cancel => Action::Close,
            Edit::Consumed | Edit::Ignored => Action::Taken,
        }
    }

    pub fn paste(&mut self, text: &str) {
        self.input.paste(text);
    }
}

pub fn rect(area: Rect) -> Rect {
    overlay::rect(area, (30, 64), 3, 3, Anchor::Centre)
}

pub fn render(area: Rect, buf: &mut Buffer, theme: &Theme, s: &mut Search) -> Option<(u16, u16)> {
    let r = rect(area);
    if r.width < 10 || r.height < 3 {
        return None;
    }
    let core: &starkit::theme::Theme = theme;
    let inner = overlay::render(
        r,
        buf,
        &overlay::Overlay {
            theme: core,
            title: "search",
            detail: None,
            footer: Some("enter search \u{b7} esc cancel"),
        },
    );
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    buf.set_string(
        inner.x,
        inner.y,
        "\u{203a} ",
        Style::default().fg(rgb(theme.dim)),
    );
    s.input.render(
        Rect {
            x: inner.x + 2,
            y: inner.y,
            width: inner.width.saturating_sub(2),
            height: 1,
        },
        buf,
        Style::default().fg(rgb(theme.fg)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::tests_support::theme;
    use starkit::crossterm::event::{KeyCode, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn a_query_is_submitted_trimmed() {
        let mut s = Search::new();
        for c in " lifetimes ".chars() {
            s.handle(key(c));
        }
        assert_eq!(
            s.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Submit("lifetimes".into())
        );
    }

    #[test]
    fn an_empty_query_closes_rather_than_searching_for_nothing() {
        let mut s = Search::new();
        assert_eq!(
            s.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Close
        );
    }

    #[test]
    fn it_opens_on_what_was_searched_for_last() {
        let s = Search::with("borrow checker");
        assert_eq!(s.input.text(), "borrow checker");
    }

    #[test]
    fn it_draws_its_title() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        let mut s = Search::with("lifetimes");
        render(area, &mut buf, &t, &mut s);
        let text: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("SEARCH"), "{text}");
        assert!(text.contains("lifetimes"), "{text}");
    }
}
