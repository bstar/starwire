//! The ENTRIES module: the list for whichever source is chosen.
//!
//! One row per entry: the marks, the headline, which feed it came from, and
//! how old it is. [`columns`] is the one place the widths are decided --
//! [`render`] draws from it and [`hit`] tests against it -- and it is where
//! the two things a narrow terminal gives up are written down.
//!
//! The feed column is the interesting one. It only appears at eighty
//! columns or more, and only on an *aggregate* source: on one feed's own
//! list every row would say the same thing, and a column of forty identical
//! words is forty columns of headline given away for nothing.

use starkit::chrome::frame::{self, Badge, Tone};
use starkit::chrome::scrollbar;
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::{Modifier, Style};
use starkit::theme::color::Rgb;

use super::{fit, rgb, width_of, words, ModuleId};
use crate::ui::theme::Theme;
use crate::ui::{Bar, Bars};
use crate::wire::feed::{ArticleStatus, EntryKind};

/// One row of the ENTRIES list, already worded by whoever built it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRow {
    pub unread: bool,
    pub starred: bool,
    pub kind: EntryKind,
    pub title: String,
    /// The feed's own name, drawn only in the cases [`columns`] allows.
    pub feed: String,
    /// `2h`, `1d`, `3w` -- see [`age`].
    pub age: String,
    pub status: ArticleStatus,
}

pub struct View<'a> {
    pub theme: &'a Theme,
    pub focused: bool,
    pub folded: bool,
    pub rows: &'a [EntryRow],
    pub cursor: usize,
    pub scroll: usize,
    /// The source's own name, after `ENTRIES —` on the border.
    pub source: &'a str,
    /// Whether the source draws rows from more than one feed, which is what
    /// earns the feed column.
    pub aggregate: bool,
    /// `12 unread`, or the refresh spinner while one is running.
    pub badge: Option<(String, Tone)>,
    /// `▸ Hacker News › 1 of 40 · <the cursor row>` -- the folded line, and
    /// the crumb row above the list when it is open.
    pub crumb: String,
    pub loading: bool,
    /// The `/` filter, drawn at the right of the crumb row -- see
    /// [`super::Filter`] for what the field being open changes about it.
    pub filter: Option<super::Filter<'a>>,
    /// What an empty list says. `choose a source` before anything has been
    /// opened, `nothing here` for a source that has nothing in it.
    pub empty: &'a str,
}

pub struct Split {
    pub crumb: Rect,
    pub list: Rect,
}

/// One blank column kept at the right, matching the header row's own.
const RIGHT_PAD: u16 = 1;

pub fn split(body: Rect, folded: bool) -> Split {
    let zero = |r: Rect| Rect {
        width: 0,
        height: 0,
        ..r
    };
    if body.height == 0 || body.width == 0 {
        return Split {
            crumb: zero(body),
            list: zero(body),
        };
    }
    // One blank column kept at the right, matching the header row's own,
    // so a count and the word above it line up and neither touches the
    // border.
    let body = Rect {
        width: body.width.saturating_sub(RIGHT_PAD),
        ..body
    };
    let crumb = Rect { height: 1, ..body };
    if folded {
        return Split {
            crumb,
            list: zero(body),
        };
    }
    Split {
        crumb,
        list: Rect {
            y: body.y + 1,
            height: body.height - 1,
            ..body
        },
    }
}

pub fn visible_rows(area: Rect, folded: bool) -> usize {
    let body = frame::body(area, &words(ModuleId::Entries));
    usize::from(split(body, folded).list.height)
}

/// The two marks before a headline, and the space after them.
const GLYPH_W: u16 = 3;
/// `2h`, `12d`, `40wk` -- four columns is every age this ever prints.
const AGE_W: u16 = 4;
/// The feed name, when there is room for one.
const FEED_W: u16 = 18;
const GAP: u16 = 2;
/// Below this the feed column is not worth the headline it costs.
pub const FEED_COLUMN_FROM: u16 = 80;

/// Where each column starts and how wide it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cols {
    pub show_feed: bool,
    pub glyph_w: u16,
    pub title_w: u16,
    pub feed_w: u16,
    pub age_w: u16,
}

/// The widths at this panel width. `aggregate` is whether the source draws
/// from more than one feed -- see the module doc for why it decides this.
pub fn columns(width: u16, aggregate: bool) -> Cols {
    let show_feed = aggregate && width >= FEED_COLUMN_FROM;
    let mut fixed = GLYPH_W + GAP + AGE_W;
    if show_feed {
        fixed += FEED_W + GAP;
    }
    Cols {
        show_feed,
        glyph_w: GLYPH_W,
        title_w: width.saturating_sub(fixed),
        feed_w: if show_feed { FEED_W } else { 0 },
        age_w: AGE_W,
    }
}

/// How long ago, in at most four columns.
///
/// Rounded down and never given a decimal: a reader glancing at a list wants
/// "this morning" or "last week", and `2.4h` says neither any better than
/// `2h` does. Anything still in the future -- a feed with an optimistic
/// clock, which is commoner than it sounds -- reads `now`.
pub fn age(published: jiff::Timestamp, now: jiff::Timestamp) -> String {
    let secs = (now.as_second() - published.as_second()).max(0);
    let mins = secs / 60;
    let hours = mins / 60;
    let days = hours / 24;
    if mins < 1 {
        "now".into()
    } else if mins < 60 {
        format!("{mins}m")
    } else if hours < 24 {
        format!("{hours}h")
    } else if days < 14 {
        format!("{days}d")
    } else if days < 365 {
        format!("{}w", days / 7)
    } else {
        format!("{}y", days / 365)
    }
}

pub fn render(area: Rect, buf: &mut Buffer, v: &View<'_>, bars: &mut Bars) {
    let t = v.theme;
    let word_list = words(ModuleId::Entries);
    let core: &starkit::theme::Theme = t;
    let badge = v.badge.as_ref().map(|(text, tone)| Badge {
        text: text.as_str(),
        tone: *tone,
    });
    let body = frame::frame(
        area,
        buf,
        &frame::Frame {
            theme: core,
            focused: v.focused,
            title: ModuleId::Entries.title(),
            detail: (!v.source.is_empty()).then_some(v.source),
            heading: false,
            badge,
            footer: None,
            words: &word_list,
        },
    );

    let s = split(body, v.folded);
    if s.crumb.height > 0 {
        render_crumb(s.crumb, buf, t, v);
    }
    if s.list.height > 0 {
        render_list(s.list, buf, t, v);
    }

    let track = scrollbar::track(area, s.list);
    bars.draw(
        Bar::Entries,
        track,
        buf,
        t,
        v.rows.len() as u32,
        v.scroll as u32,
    );
}

fn render_crumb(area: Rect, buf: &mut Buffer, t: &Theme, v: &View<'_>) {
    let right = v.filter.map(|f| format!("/{}", f.text)).unwrap_or_default();
    let right_w = width_of(&right).min(area.width);
    let left_w = area.width.saturating_sub(right_w + 1);
    buf.set_string(
        area.x,
        area.y,
        fit(&v.crumb, left_w),
        Style::default().fg(rgb(t.accent)),
    );
    if right_w > 0 {
        buf.set_string(
            area.x + area.width - right_w,
            area.y,
            right,
            super::filter_style(t, v.filter.is_some_and(|f| f.typing)),
        );
    }
}

fn render_list(area: Rect, buf: &mut Buffer, t: &Theme, v: &View<'_>) {
    if v.loading && v.rows.is_empty() {
        starkit::chrome::empty(area, buf, t, "reading\u{2026}");
        return;
    }
    if v.rows.is_empty() {
        starkit::chrome::empty(area, buf, t, v.empty);
        return;
    }
    let cols = columns(area.width, v.aggregate);
    for (i, row) in v
        .rows
        .iter()
        .enumerate()
        .skip(v.scroll)
        .take(usize::from(area.height))
    {
        let y = area.y + u16::try_from(i - v.scroll).unwrap_or(0);
        render_row(area, y, buf, t, &cols, row, v.focused && i == v.cursor);
    }
}

fn render_row(
    area: Rect,
    y: u16,
    buf: &mut Buffer,
    t: &Theme,
    cols: &Cols,
    row: &EntryRow,
    cursor: bool,
) {
    let (fg, bg) = if cursor {
        (Some(rgb(t.row_cursor_fg)), Some(rgb(t.row_cursor_bg)))
    } else {
        (None, None)
    };
    if let Some(bg) = bg {
        buf.set_string(
            area.x,
            y,
            " ".repeat(usize::from(area.width)),
            Style::default().bg(bg),
        );
    }
    let style_for = |fallback: Rgb| {
        let mut s = Style::default().fg(fg.unwrap_or_else(|| rgb(fallback)));
        if let Some(bg) = bg {
            s = s.bg(bg);
        }
        s
    };

    let mut x = area.x;
    buf.set_string(
        x,
        y,
        if row.unread { "\u{25cf}" } else { " " },
        style_for(t.wire.unread_fg),
    );
    x += 1;
    // One column for the second mark, in order of what a reader most wants
    // to see: a star is a decision somebody made, a video is a different
    // thing entirely, and a failed extraction is a warning about what
    // `enter` will show.
    let (mark, mark_fg) = if row.starred {
        ("\u{2605}", t.wire.star_fg)
    } else if row.kind == EntryKind::Video {
        ("\u{25b6}", t.wire.video_fg)
    } else if row.status == ArticleStatus::Failed {
        ("!", t.error)
    } else {
        (" ", t.dim)
    };
    buf.set_string(x, y, mark, style_for(mark_fg));
    x += 2;

    let title_fg = if row.unread { t.wire.unread_fg } else { t.dim };
    let mut title_style = style_for(title_fg);
    if row.unread {
        title_style = title_style.add_modifier(Modifier::BOLD);
    }
    buf.set_string(
        x,
        y,
        super::elide_middle(&row.title, cols.title_w),
        title_style,
    );
    x += cols.title_w;

    if cols.show_feed {
        x += GAP;
        buf.set_string(
            x,
            y,
            fit(&super::elide_middle(&row.feed, cols.feed_w), cols.feed_w),
            style_for(t.row_meta_fg),
        );
        x += cols.feed_w;
    }

    x += GAP;
    let age = fit_right(&row.age, cols.age_w);
    buf.set_string(x, y, age, style_for(t.row_meta_fg));
}

fn fit_right(text: &str, width: u16) -> String {
    let fitted = fit(text, width);
    let trimmed = fitted.trim_end();
    let pad = width.saturating_sub(width_of(trimmed));
    format!("{}{trimmed}", " ".repeat(usize::from(pad)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The crumb line, which jumps back up the column.
    Crumb,
    Row(usize),
}

pub fn hit(area: Rect, v: &View<'_>, x: u16, y: u16) -> Option<Hit> {
    let body = frame::body(area, &words(ModuleId::Entries));
    let s = split(body, v.folded);
    if s.crumb.height > 0 && y == s.crumb.y && x >= s.crumb.x && x < s.crumb.x + s.crumb.width {
        return Some(Hit::Crumb);
    }
    if s.list.height > 0 && y >= s.list.y && y < s.list.y + s.list.height {
        let row = v.scroll + usize::from(y - s.list.y);
        return (row < v.rows.len()).then_some(Hit::Row(row));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::tests_support::theme;

    fn rows() -> Vec<EntryRow> {
        vec![
            EntryRow {
                unread: true,
                starred: false,
                kind: EntryKind::Article,
                title: "Why the borrow checker says no: a field guide to lifetimes".into(),
                feed: "Hacker News".into(),
                age: "2h".into(),
                status: ArticleStatus::Extracted,
            },
            EntryRow {
                unread: false,
                starred: true,
                kind: EntryKind::Article,
                title: "A short one".into(),
                feed: "Lobsters".into(),
                age: "1d".into(),
                status: ArticleStatus::Extracted,
            },
            EntryRow {
                unread: true,
                starred: false,
                kind: EntryKind::Video,
                title: "How a transistor works".into(),
                feed: "Veritasium".into(),
                age: "3d".into(),
                status: ArticleStatus::NotApplicable,
            },
        ]
    }

    fn view<'a>(t: &'a Theme, rows: &'a [EntryRow], folded: bool) -> View<'a> {
        View {
            theme: t,
            focused: true,
            folded,
            rows,
            cursor: 0,
            scroll: 0,
            source: "Hacker News",
            aggregate: true,
            badge: Some(("12 unread".into(), Tone::Dim)),
            crumb: "\u{25b8} Hacker News \u{203a} 1 of 40".into(),
            loading: false,
            filter: None,
            empty: "nothing here",
        }
    }

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
    fn hit_agrees_with_render() {
        let t = theme("terminal");
        let rows = rows();
        for width in [60u16, 80, 100] {
            for folded in [false, true] {
                let area = Rect::new(0, 0, width, 12);
                let v = view(&t, &rows, folded);
                let mut buf = Buffer::empty(area);
                render(area, &mut buf, &v, &mut Bars::new());
                let body = frame::body(area, &words(ModuleId::Entries));
                let s = split(body, folded);
                for y in area.y..area.y + area.height {
                    match hit(area, &v, body.x + 4, y) {
                        Some(Hit::Row(i)) => {
                            assert_eq!(i, v.scroll + usize::from(y - s.list.y));
                            let drawn: String = (0..area.width)
                                .map(|x| buf[(x, y)].symbol().to_string())
                                .collect();
                            let head: String = rows[i].title.chars().take(8).collect();
                            assert!(drawn.contains(&head), "{width}/{y}: {drawn:?}");
                        }
                        Some(Hit::Crumb) => assert_eq!(y, s.crumb.y),
                        None => {}
                    }
                }
            }
        }
    }

    /// The feed column is the one thing that appears and disappears with the
    /// width, and it is never drawn on a single feed's own list.
    #[test]
    fn the_feed_column_is_earned_by_width_and_by_the_source() {
        assert!(!columns(60, true).show_feed);
        assert!(columns(80, true).show_feed);
        assert!(columns(100, true).show_feed);
        assert!(!columns(100, false).show_feed, "one feed's own list");

        // What it costs, and that the headline gets it back.
        let with = columns(100, true);
        let without = columns(100, false);
        assert_eq!(without.title_w, with.title_w + FEED_W + GAP);
    }

    #[test]
    fn every_column_fits_inside_the_panel() {
        for width in 20u16..=140 {
            for aggregate in [true, false] {
                let c = columns(width, aggregate);
                let used = c.glyph_w
                    + c.title_w
                    + if c.show_feed { c.feed_w + GAP } else { 0 }
                    + GAP
                    + c.age_w;
                assert!(
                    used <= width.max(GLYPH_W + GAP + AGE_W),
                    "{width}/{aggregate}"
                );
            }
        }
    }

    #[test]
    fn an_age_is_never_more_than_four_columns() {
        let now = jiff::Timestamp::from_second(1_800_000_000).unwrap();
        let ago = |secs: i64| {
            age(
                jiff::Timestamp::from_second(1_800_000_000 - secs).unwrap(),
                now,
            )
        };
        assert_eq!(ago(30), "now");
        assert_eq!(ago(300), "5m");
        assert_eq!(ago(7200), "2h");
        assert_eq!(ago(86_400), "1d");
        assert_eq!(ago(86_400 * 30), "4w");
        assert_eq!(ago(86_400 * 800), "2y");
        // A feed whose clock runs fast.
        assert_eq!(
            age(jiff::Timestamp::from_second(1_800_000_600).unwrap(), now),
            "now"
        );
        for secs in [
            0i64,
            59,
            61,
            3600,
            86_399,
            86_400 * 13,
            86_400 * 364,
            86_400 * 4000,
        ] {
            assert!(width_of(&ago(secs)) <= AGE_W, "{secs}: {}", ago(secs));
        }
    }

    #[test]
    fn the_marks_say_starred_video_and_failed() {
        let t = theme("terminal");
        let mut rows = rows();
        rows[0].status = ArticleStatus::Failed;
        let area = Rect::new(0, 0, 80, 12);
        let v = view(&t, &rows, false);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &v, &mut Bars::new());
        let text = dump(&buf, area);
        assert!(text.contains("\u{25cf}!"), "the failed one: {text}");
        assert!(text.contains(" \u{2605}"), "the starred one: {text}");
        assert!(text.contains("\u{25cf}\u{25b6}"), "the video: {text}");
    }
}
