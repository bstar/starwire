//! Adding a feed: one line, and the core works out what it is.
//!
//! Deliberately one field rather than three. A URL, a `@handle`, a bare
//! `UC…` channel id and a bare hostname all go in the same box, because
//! `wire::youtube::canonicalise` is going to normalise whatever arrives
//! anyway -- and asking somebody to say in advance which of four kinds of
//! string they are about to paste is asking them to know something the
//! program can work out.
//!
//! Nothing is validated here beyond "not empty". A URL that turns out to be
//! a 404 is a fetch failure with a reason on the feed row, which is where a
//! reader can actually do something about it; refusing it in the box would
//! only be a guess made earlier and with less information.

use starkit::chrome::overlay::{self, Anchor};
use starkit::crossterm::event::KeyEvent;
use starkit::input::{Edit, TextInput};
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::Style;

use crate::ui::panels::rgb;
use crate::ui::theme::Theme;

/// What a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Taken,
    Close,
    /// A non-empty line, as typed.
    Submit(String),
}

#[derive(Debug)]
pub struct AddFeed {
    pub input: TextInput,
    /// Why the last submission was refused.
    pub error: Option<&'static str>,
}

impl Default for AddFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl AddFeed {
    pub fn new() -> Self {
        Self {
            input: TextInput::single(),
            error: None,
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> Action {
        match self.input.handle(key) {
            Edit::Submit => {
                let text = self.input.text().trim().to_string();
                if text.is_empty() {
                    self.error = Some("type a URL, a @handle or a channel id");
                    return Action::Taken;
                }
                Action::Submit(text)
            }
            Edit::Cancel => Action::Close,
            Edit::Consumed => {
                self.error = None;
                Action::Taken
            }
            Edit::Ignored => Action::Taken,
        }
    }

    pub fn paste(&mut self, text: &str) {
        self.input.paste(text);
        self.error = None;
    }
}

/// Where the box lands: the field, and the row under it that says why the
/// last submission was refused.
pub fn rect(area: Rect) -> Rect {
    overlay::rect(area, (30, 64), 4, 3, Anchor::Centre)
}

/// Draw it, and hand back where the terminal's own cursor belongs so the
/// caret a person sees is the real one rather than a drawn stand-in.
pub fn render(area: Rect, buf: &mut Buffer, theme: &Theme, a: &mut AddFeed) -> Option<(u16, u16)> {
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
            title: "add a feed",
            detail: None,
            footer: Some("enter add \u{b7} esc cancel"),
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
    let field = Rect {
        x: inner.x + 2,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: 1,
    };
    let cursor = a
        .input
        .render(field, buf, Style::default().fg(rgb(theme.fg)));

    if inner.height > 1 {
        let (text, colour) = match a.error {
            Some(e) => (e, theme.error),
            None => ("https://\u{2026} or @channel", theme.dim),
        };
        buf.set_string(
            inner.x + 2,
            inner.y + 1,
            starkit::text::fit(text, inner.width.saturating_sub(2)),
            Style::default().fg(rgb(colour)),
        );
    }
    cursor
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
    fn a_typed_line_is_submitted_as_it_was_typed() {
        let mut a = AddFeed::new();
        for c in "  @veritasium  ".chars() {
            a.handle(key(c));
        }
        assert_eq!(
            a.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Submit("@veritasium".into())
        );
    }

    #[test]
    fn an_empty_line_is_refused_and_the_box_stays_open() {
        let mut a = AddFeed::new();
        assert_eq!(
            a.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Taken
        );
        assert!(a.error.is_some());
        // And typing anything clears the complaint again.
        a.handle(key('h'));
        assert!(a.error.is_none());
    }

    #[test]
    fn a_paste_lands_in_the_field() {
        let mut a = AddFeed::new();
        a.paste("https://example.org/feed.xml");
        assert_eq!(a.input.text(), "https://example.org/feed.xml");
    }

    #[test]
    fn it_draws_its_title_and_its_hint() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        let mut a = AddFeed::new();
        render(area, &mut buf, &t, &mut a);
        let text: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("ADD A FEED"), "{text}");
        assert!(text.contains("or @channel"), "{text}");
    }
}
