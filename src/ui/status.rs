//! The one row at the bottom.
//!
//! Three fields, STAR/FOLD's `ui/status.rs` with the middle taught about a
//! refresh. The left is a fixed reminder that the help exists. The middle is
//! transient: a note for three seconds, else the refresh bar, else the key
//! hints for whichever module has the keyboard. The right is **stable** --
//! the source, where the cursor is in it, how much of it has been read, and
//! what can draw pictures -- because it is the part somebody glances at
//! without stopping what they are doing, and a field that moves is a field
//! that has to be read rather than glanced at.
//!
//! Its geometry is computed once, by [`fields`], and both the renderer and
//! the mouse come through it.

use std::time::{Duration, Instant};

use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::{Modifier, Style};

use super::panels::{fit, rgb, width_of};
use super::theme::Theme;
use crate::wire::NoteLevel;

/// How long a note holds the middle field before the refresh bar or the
/// hints come back.
pub const NOTE_FOR: Duration = Duration::from_secs(3);

/// What a click on the status line lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Help,
    /// The refresh bar, while it is what the middle field shows.
    Progress,
    /// The right-hand field's source-and-count word, which goes to SOURCES.
    Count,
}

pub struct View<'a> {
    pub theme: &'a Theme,
    pub note: Option<&'a (String, NoteLevel, Instant)>,
    pub now: Instant,
    /// `"⠋ refreshing  12 of 41"`, while one is running.
    pub progress: Option<&'a str>,
    /// `(key, description)` pairs -- `("n", "next unread")` -- drawn the
    /// same way the `?` cell to their left is.
    pub hints: &'a [(&'a str, &'a str)],
    /// `"Hacker News · 1/40 · 3%"`: the source, the cursor, how much of it
    /// has been read.
    pub right: &'a str,
    pub graphics: &'a str,
}

/// The hint pairs joined back into one line, one space between a key and its
/// word and two between pairs.
fn hints_text(hints: &[(&str, &str)]) -> String {
    hints
        .iter()
        .map(|(key, desc)| format!("{key} {desc}"))
        .collect::<Vec<_>>()
        .join("  ")
}

/// What the middle field is presently showing -- decides both its colour and
/// whether a click on it means [`Hit::Progress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MiddleKind {
    Note(NoteLevel),
    Progress,
    Hints,
}

impl View<'_> {
    /// A fresh note first (urgent and transient), else the refresh (urgent
    /// and ongoing), else the key hints (what is there the rest of the
    /// time).
    fn middle(&self) -> (String, MiddleKind) {
        if let Some((text, level, at)) = self.note {
            if self.now.duration_since(*at) < NOTE_FOR {
                return (text.clone(), MiddleKind::Note(*level));
            }
        }
        if let Some(p) = self.progress {
            return (p.to_string(), MiddleKind::Progress);
        }
        (hints_text(self.hints), MiddleKind::Hints)
    }

    /// The stable right-hand field, held to `max_w`: the graphics word goes
    /// first, then the source-and-count line loses its head. Returns the
    /// field and the count word as it was drawn.
    fn right_field(&self, max_w: u16) -> (String, String) {
        let join = |parts: &[&str]| {
            parts
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join("  ")
        };
        let full = join(&[self.right, self.graphics]);
        if width_of(&full) <= max_w {
            return (full, self.right.to_string());
        }
        if width_of(self.right) <= max_w {
            return (self.right.to_string(), self.right.to_string());
        }
        let count = elide_head(self.right, max_w);
        (count.clone(), count)
    }
}

/// `…/the/end` -- the tail of `text` that fits in `width`, with an ellipsis
/// in front of it.
fn elide_head(text: &str, width: u16) -> String {
    if width_of(text) <= width {
        return text.to_string();
    }
    if width < 2 {
        return String::new();
    }
    let keep = usize::from(width - 1);
    let clusters: Vec<&str> = starkit::wrap::clusters(text).map(|(_, c)| c).collect();
    let mut back = Vec::new();
    let mut used = 0usize;
    for c in clusters.iter().rev() {
        let cw = usize::from(width_of(c));
        if used + cw > keep {
            break;
        }
        back.push(*c);
        used += cw;
    }
    let mut out = String::from("\u{2026}");
    for c in back.iter().rev() {
        out.push_str(c);
    }
    out
}

/// The fewest columns the middle field keeps against a long right-hand one:
/// enough for the refresh bar and its two numbers.
const MIDDLE_MIN: u16 = 24;

const HELP: &str = "? help";

/// Where each field sits. The renderer draws from this and the mouse tests
/// against it, so a word that was not drawn cannot be clicked.
pub struct Fields {
    pub help: Rect,
    /// Whichever of note / progress / hints is currently shown.
    pub middle: Rect,
    /// The source-and-count word inside the right-hand field.
    pub count: Rect,
}

pub fn fields(area: Rect, v: &View<'_>) -> Fields {
    let empty_at = |r: Rect| Rect {
        width: 0,
        height: 0,
        ..r
    };
    if area.height == 0 || area.width == 0 {
        return Fields {
            help: empty_at(area),
            middle: empty_at(area),
            count: empty_at(area),
        };
    }

    let help_w = width_of(HELP).min(area.width);
    let help = Rect {
        x: area.x,
        y: area.y,
        width: help_w,
        height: 1,
    };

    let max_right = area
        .width
        .saturating_sub(help_w + 2)
        .saturating_sub(MIDDLE_MIN)
        .max(area.width.saturating_sub(help_w + 2) / 2);
    let (right, drawn_count) = v.right_field(max_right);
    let right_w = width_of(&right).min(area.width.saturating_sub(help_w + 2));
    let right_x = area.x + area.width - right_w;

    let count = if drawn_count.is_empty() || right_w == 0 {
        empty_at(area)
    } else {
        Rect {
            x: right_x,
            y: area.y,
            width: width_of(&drawn_count).min(right_w),
            height: 1,
        }
    };

    let reserved_right = if right_w > 0 { right_w + 2 } else { 0 };
    let middle_w = area
        .width
        .saturating_sub(help_w + 2)
        .saturating_sub(reserved_right);
    let middle = Rect {
        x: area.x + help_w + 2,
        y: area.y,
        width: middle_w,
        height: if middle_w > 0 { 1 } else { 0 },
    };

    Fields {
        help,
        middle,
        count,
    }
}

pub fn hit(area: Rect, v: &View<'_>, x: u16, y: u16) -> Option<Hit> {
    let f = fields(area, v);
    let inside = |r: Rect| r.width > 0 && y == r.y && x >= r.x && x < r.x + r.width;
    if inside(f.help) {
        return Some(Hit::Help);
    }
    if inside(f.count) {
        return Some(Hit::Count);
    }
    if inside(f.middle) && matches!(v.middle().1, MiddleKind::Progress) {
        return Some(Hit::Progress);
    }
    None
}

pub fn render(area: Rect, buf: &mut Buffer, v: &View<'_>) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let t = v.theme;
    let base = Style::default().fg(rgb(t.status_fg)).bg(rgb(t.status_bg));
    buf.set_style(area, base);

    let f = fields(area, v);
    let (middle_text, kind) = v.middle();
    let max_right = area
        .width
        .saturating_sub(f.help.width + 2)
        .saturating_sub(MIDDLE_MIN)
        .max(area.width.saturating_sub(f.help.width + 2) / 2);
    let (right, _) = v.right_field(max_right);

    if f.help.width > 0 {
        buf.set_string(
            f.help.x,
            f.help.y,
            "?",
            Style::default()
                .fg(rgb(t.hint_key_fg))
                .bg(rgb(t.hint_key_bg))
                .add_modifier(Modifier::BOLD),
        );
        buf.set_string(
            f.help.x + 1,
            f.help.y,
            " help",
            base.fg(rgb(t.hint_desc_fg)),
        );
    }

    if f.middle.width > 0 {
        match kind {
            MiddleKind::Hints => {
                let key_style = Style::default()
                    .fg(rgb(t.hint_key_fg))
                    .bg(rgb(t.hint_key_bg))
                    .add_modifier(Modifier::BOLD);
                let desc_style = base.fg(rgb(t.hint_desc_fg));
                render_hints(buf, f.middle, v.hints, key_style, desc_style);
            }
            MiddleKind::Note(level) => {
                let style = match level {
                    NoteLevel::Error => base.fg(rgb(t.error)),
                    NoteLevel::Warning => base.fg(rgb(t.warn)),
                    NoteLevel::Info => base.fg(rgb(t.accent)),
                };
                buf.set_string(
                    f.middle.x,
                    f.middle.y,
                    fit(&middle_text, f.middle.width),
                    style,
                );
            }
            MiddleKind::Progress => {
                buf.set_string(
                    f.middle.x,
                    f.middle.y,
                    fit(&middle_text, f.middle.width),
                    base.fg(rgb(t.accent)),
                );
            }
        }
    }

    if !right.is_empty() {
        let w = width_of(&right).min(area.width);
        let x = area.x + area.width - w;
        buf.set_string(x, area.y, &right, base);
    }
}

/// Draw the key hints into `area`, a key at a time in `key_style` and each
/// word after it in `desc_style` -- the same treatment [`render`] gives the
/// `?` cell beside them, just repeated for every pair.
fn render_hints(
    buf: &mut Buffer,
    area: Rect,
    hints: &[(&str, &str)],
    key_style: Style,
    desc_style: Style,
) {
    if area.width == 0 {
        return;
    }
    let mut text = String::new();
    let mut key_ranges = Vec::with_capacity(hints.len());
    for (i, (key, desc)) in hints.iter().enumerate() {
        if i > 0 {
            text.push_str("  ");
        }
        let start = text.len();
        text.push_str(key);
        key_ranges.push((start, text.len()));
        text.push(' ');
        text.push_str(desc);
    }

    let mut used = 0u16;
    for (start, cluster) in starkit::wrap::clusters(&text) {
        let w = width_of(cluster);
        if used + w > area.width {
            break;
        }
        let style = if key_ranges.iter().any(|&(s, e)| start >= s && start < e) {
            key_style
        } else {
            desc_style
        };
        buf.set_string(area.x + used, area.y, cluster, style);
        used += w;
    }
    // `fit` always pads a field out to its full width; matched here so a
    // shorter line this frame still clears whatever a longer one left behind
    // on the row this widget reuses.
    if used < area.width {
        buf.set_string(
            area.x + used,
            area.y,
            " ".repeat(usize::from(area.width - used)),
            desc_style,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::tests_support::theme;

    fn view<'a>(
        theme: &'a Theme,
        note: Option<&'a (String, NoteLevel, Instant)>,
        now: Instant,
    ) -> View<'a> {
        View {
            theme,
            note,
            now,
            progress: Some("\u{280b} refreshing  12 of 41"),
            hints: &[("n", "next unread"), ("m", "read"), ("s", "star")],
            right: "Hacker News \u{b7} 1/40 \u{b7} 3%",
            graphics: "kitty",
        }
    }

    #[test]
    fn the_middle_field_prefers_a_fresh_note_then_the_refresh_then_hints() {
        let t = theme("terminal");
        let at = Instant::now();
        let note = ("41 feeds imported".to_string(), NoteLevel::Info, at);

        let fresh = view(&t, Some(&note), at + Duration::from_secs(1));
        assert_eq!(fresh.middle().0, "41 feeds imported");

        let stale_but_running = view(&t, Some(&note), at + NOTE_FOR + Duration::from_millis(1));
        assert!(stale_but_running.middle().0.contains("12 of 41"));

        let mut idle = view(&t, None, at);
        idle.progress = None;
        assert_eq!(idle.middle().0, hints_text(idle.hints));
    }

    #[test]
    fn a_hint_key_is_bold_and_its_word_is_not() {
        let t = theme("terminal");
        let mut v = view(&t, None, Instant::now());
        v.progress = None;
        let area = Rect::new(0, 0, 100, 1);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &v);
        let f = fields(area, &v);

        let key_cell = &buf[(f.middle.x, f.middle.y)];
        assert_eq!(key_cell.symbol(), "n");
        assert!(key_cell.style().add_modifier.contains(Modifier::BOLD));
        assert_eq!(key_cell.style().bg, Some(rgb(t.hint_key_bg)));

        let desc_cell = &buf[(f.middle.x + 2, f.middle.y)];
        assert_eq!(desc_cell.symbol(), "n");
        assert!(!desc_cell.style().add_modifier.contains(Modifier::BOLD));
        assert_eq!(desc_cell.style().bg, Some(rgb(t.status_bg)));
    }

    /// Colouring the hints must not move a single column of them.
    #[test]
    fn colouring_the_hints_does_not_move_the_text() {
        let t = theme("terminal");
        let mut v = view(&t, None, Instant::now());
        v.progress = None;
        for width in [10u16, 20, 24, 40, 100] {
            let area = Rect::new(0, 0, width, 1);
            let f = fields(area, &v);
            let mut buf = Buffer::empty(area);
            render(area, &mut buf, &v);
            let drawn: String = (0..f.middle.width)
                .map(|i| buf[(f.middle.x + i, f.middle.y)].symbol().to_string())
                .collect();
            assert_eq!(
                drawn,
                fit(&hints_text(v.hints), f.middle.width),
                "width {width}"
            );
        }
    }

    #[test]
    fn help_sits_at_the_left_edge_and_the_count_is_clickable() {
        let t = theme("terminal");
        let v = view(&t, None, Instant::now());
        let area = Rect::new(0, 0, 100, 1);
        let f = fields(area, &v);
        assert_eq!(f.help.x, 0);
        assert!(f.count.width > 0);
        assert_eq!(hit(area, &v, 0, 0), Some(Hit::Help));
        assert_eq!(hit(area, &v, f.count.x, 0), Some(Hit::Count));
    }

    /// The refresh bar is the one middle field a click means something on.
    #[test]
    fn a_click_on_the_refresh_bar_answers_progress() {
        let t = theme("terminal");
        let v = view(&t, None, Instant::now());
        let area = Rect::new(0, 0, 100, 1);
        let f = fields(area, &v);
        assert_eq!(hit(area, &v, f.middle.x, 0), Some(Hit::Progress));

        let mut idle = view(&t, None, Instant::now());
        idle.progress = None;
        assert_eq!(hit(area, &idle, f.middle.x, 0), None);
    }

    /// Narrow enough that the graphics word has to go, and then the source
    /// name loses its head rather than the whole field vanishing.
    #[test]
    fn the_right_field_gives_up_the_graphics_word_before_the_count() {
        let t = theme("terminal");
        let v = view(&t, None, Instant::now());
        assert!(v.right_field(200).0.contains("kitty"));
        assert_eq!(v.right_field(24).0, v.right);
        assert!(v.right_field(10).0.starts_with('\u{2026}'));
    }
}
