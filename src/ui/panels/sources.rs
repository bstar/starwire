//! The SOURCES module: the special rows, the folders and the feeds.
//!
//! Two things share the body: one line of crumbs saying how the feed list
//! was drilled into, and the rows of whichever level is open. [`split`] is
//! the one function that decides how tall each is -- [`render`] draws from
//! it and [`hit`] tests against it, so a click can never land on a row the
//! renderer did not draw.
//!
//! Folded, the whole module is that one crumb line with the cursor row's
//! name on the end of it: `▸ sources › Tech › Hacker News`, and the count
//! right-aligned. That is deliberately the same line the expanded panel
//! draws above its list, minus the cursor's name, so folding a module does
//! not change what it says about itself.

use starkit::chrome::frame::{self, Badge, Tone};
use starkit::chrome::scrollbar;
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::{Modifier, Style};
use starkit::theme::color::Rgb;

use super::{fit, rgb, width_of, words, ModuleId};
use crate::ui::theme::Theme;
use crate::ui::{Bar, Bars};

/// What sort of thing a row names, which is what decides its colour and
/// what `enter` on it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// All, Unread, Starred, Videos, `Search…` -- the rows that are a query
    /// rather than a feed.
    Special,
    Folder,
    Feed,
    /// A feed whose last fetch failed. Drawn in `error`, and its `extra`
    /// says why.
    Failed,
}

/// One row of the SOURCES list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRow {
    /// One column before the name: `●`, `★`, `▶`, `⌕`, `▸` for a folder, or
    /// a space for a plain feed.
    pub glyph: &'static str,
    pub name: String,
    /// Unread, right-aligned. `None` draws nothing, which is what the
    /// `Search…` row wants.
    pub count: Option<i64>,
    /// After the name, in `dim`: a fetch error, or `fetching…`.
    pub extra: Option<String>,
    pub kind: RowKind,
    /// Nothing unread here. Drawn in `dim` so a list of forty feeds shows
    /// the four worth looking at.
    pub dim: bool,
}

impl SourceRow {
    pub fn feed(name: impl Into<String>, unread: i64) -> Self {
        Self {
            glyph: " ",
            name: name.into(),
            count: Some(unread),
            extra: None,
            kind: RowKind::Feed,
            dim: unread == 0,
        }
    }
}

/// One folded level above the active one: `sources`, `Tech`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub name: String,
    pub count: Option<usize>,
}

pub struct View<'a> {
    pub theme: &'a Theme,
    pub focused: bool,
    pub folded: bool,
    /// Every level from the root to the active one, the active one last.
    pub crumbs: &'a [Crumb],
    pub rows: &'a [SourceRow],
    pub cursor: usize,
    pub scroll: usize,
    /// The right-hand end of the crumb line: `41 feeds · 312 unread`, or
    /// the `/` filter while one is on -- see [`render_crumbs`].
    pub summary: String,
    pub filter: Option<super::Filter<'a>>,
    pub loading: bool,
}

/// The crumb separator, and the mark that opens the line.
const ARROW: char = '\u{25b8}';
const SEP: &str = " \u{203a} ";

pub struct Split {
    pub crumbs: Rect,
    pub list: Rect,
}

/// One blank column kept at the right, matching the header row's own so a
/// count and the word above it line up and neither touches the border.
const RIGHT_PAD: u16 = 1;

/// One row of crumbs, and the rest for the list. A folded module has the
/// crumb row and nothing else, which is the whole of what folding means
/// here.
pub fn split(body: Rect, folded: bool) -> Split {
    let zero = |r: Rect| Rect {
        width: 0,
        height: 0,
        ..r
    };
    if body.height == 0 || body.width == 0 {
        return Split {
            crumbs: zero(body),
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
    let crumbs = Rect { height: 1, ..body };
    if folded {
        return Split {
            crumbs,
            list: zero(body),
        };
    }
    Split {
        crumbs,
        list: Rect {
            y: body.y + 1,
            height: body.height - 1,
            ..body
        },
    }
}

/// How many list rows fit, for the app's scroll clamping and page keys.
pub fn visible_rows(area: Rect, folded: bool) -> usize {
    let body = frame::body(area, &words(ModuleId::Sources));
    usize::from(split(body, folded).list.height)
}

/// The crumb line's text and the columns each crumb occupies in it -- read
/// by both [`render`] and [`hit`], so a click cannot disagree with where a
/// crumb was drawn. The trailing name (a folded module's cursor row) is not
/// a crumb and gets no box.
fn crumb_layout(
    x0: u16,
    crumbs: &[Crumb],
    trailing: Option<&str>,
) -> (String, Vec<(usize, u16, u16)>) {
    let mut line = format!("{ARROW} ");
    let mut boxes = Vec::with_capacity(crumbs.len());
    let mut x = x0 + width_of(&line);
    for (i, c) in crumbs.iter().enumerate() {
        if i > 0 {
            line.push_str(SEP);
            x += width_of(SEP);
        }
        let w = width_of(&c.name);
        boxes.push((i, x, w));
        line.push_str(&c.name);
        x += w;
    }
    if let Some(name) = trailing {
        line.push_str(SEP);
        line.push_str(name);
    }
    (line, boxes)
}

/// What the folded line puts on the end of the crumbs: the row the cursor
/// is on, so a folded module still says what is chosen in it.
fn trailing<'a>(v: &'a View<'a>) -> Option<&'a str> {
    v.folded
        .then(|| v.rows.get(v.cursor).map(|r| r.name.as_str()))
        .flatten()
}

pub fn render(area: Rect, buf: &mut Buffer, v: &View<'_>, bars: &mut Bars) {
    let t = v.theme;
    let word_list = words(ModuleId::Sources);
    // The core theme type -- a struct literal is not a coercion site, so the
    // deref from this crate's own `Theme` is spelled out here.
    let core: &starkit::theme::Theme = t;
    let body = frame::frame(
        area,
        buf,
        &frame::Frame {
            theme: core,
            focused: v.focused,
            title: super::HEADING,
            detail: None,
            heading: true,
            badge: Some(Badge {
                text: ModuleId::Sources.title(),
                tone: Tone::Dim,
            }),
            footer: None,
            words: &word_list,
        },
    );

    let s = split(body, v.folded);
    if s.crumbs.height > 0 {
        render_crumbs(s.crumbs, buf, t, v);
    }
    if s.list.height > 0 {
        render_list(s.list, buf, t, v);
    }

    let track = scrollbar::track(area, s.list);
    bars.draw(
        Bar::Sources,
        track,
        buf,
        t,
        v.rows.len() as u32,
        v.scroll as u32,
    );
}

/// The crumbs, and at the right either the summary or the `/` filter.
///
/// The filter takes the summary's place rather than sitting beside it: at
/// the sixty-column floor there is room for one of them, and while a list is
/// narrowed the thing worth saying is what narrowed it -- the feed count is
/// the same number it was a moment ago. It is where the ENTRIES list draws
/// its own filter, so the two read alike.
fn render_crumbs(area: Rect, buf: &mut Buffer, t: &Theme, v: &View<'_>) {
    let (line, _) = crumb_layout(area.x, v.crumbs, trailing(v));
    let filtering = v.filter.is_some();
    let right = match v.filter {
        Some(f) => format!("/{}", f.text),
        None => v.summary.clone(),
    };
    let right_w = width_of(&right).min(area.width);
    let left_w = area.width.saturating_sub(right_w + 1);
    buf.set_string(
        area.x,
        area.y,
        fit(&line, left_w),
        Style::default().fg(rgb(t.accent)),
    );
    if right_w > 0 {
        let style = if filtering {
            super::filter_style(t, v.filter.is_some_and(|f| f.typing))
        } else {
            Style::default().fg(rgb(t.dim))
        };
        buf.set_string(
            area.x + area.width - right_w,
            area.y,
            fit(&right, right_w),
            style,
        );
    }
}

/// The count column: wide enough for four digits, which is more unread than
/// anybody reads and fewer columns than a name gives up lightly.
const COUNT_W: u16 = 5;
const GLYPH_W: u16 = 2;

fn kind_fg(t: &Theme, row: &SourceRow) -> Rgb {
    match row.kind {
        RowKind::Failed => t.error,
        RowKind::Special => t.accent,
        RowKind::Folder => t.wire.heading_fg,
        RowKind::Feed if row.dim => t.dim,
        RowKind::Feed => t.row_fg,
    }
}

fn render_list(area: Rect, buf: &mut Buffer, t: &Theme, v: &View<'_>) {
    if v.loading {
        starkit::chrome::empty(area, buf, t, "reading\u{2026}");
        return;
    }
    if v.rows.is_empty() {
        starkit::chrome::empty(area, buf, t, "no feeds yet \u{2014} a adds one");
        return;
    }
    for (i, row) in v
        .rows
        .iter()
        .enumerate()
        .skip(v.scroll)
        .take(usize::from(area.height))
    {
        let y = area.y + u16::try_from(i - v.scroll).unwrap_or(0);
        render_row(area, y, buf, t, row, v.focused && i == v.cursor);
    }
}

fn render_row(area: Rect, y: u16, buf: &mut Buffer, t: &Theme, row: &SourceRow, cursor: bool) {
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

    let count = row
        .count
        .filter(|n| *n > 0)
        .map(|n| n.to_string())
        .unwrap_or_default();
    let count_w = if count.is_empty() { 0 } else { COUNT_W };
    let name_w = area.width.saturating_sub(GLYPH_W + count_w);

    let glyph_fg = match row.kind {
        RowKind::Failed => t.error,
        _ => kind_fg(t, row),
    };
    buf.set_string(area.x, y, fit(row.glyph, GLYPH_W), style_for(glyph_fg));

    let mut name = row.name.clone();
    if let Some(extra) = &row.extra {
        name.push_str("  ");
        name.push_str(extra);
    }
    let mut name_style = style_for(kind_fg(t, row));
    // An unread count is what makes a source worth opening, so the name
    // carrying one is the one drawn at full weight.
    if !row.dim && row.kind != RowKind::Special {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    buf.set_string(
        area.x + GLYPH_W,
        y,
        super::elide_middle(&name, name_w),
        name_style,
    );

    if count_w > 0 {
        let x = area.x + area.width - count_w;
        buf.set_string(
            x,
            y,
            format!("{count:>width$}", width = usize::from(count_w)),
            style_for(t.wire.unread_fg),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Crumb(usize),
    Row(usize),
}

pub fn hit(area: Rect, v: &View<'_>, x: u16, y: u16) -> Option<Hit> {
    let body = frame::body(area, &words(ModuleId::Sources));
    let s = split(body, v.folded);
    if s.crumbs.height > 0 && y == s.crumbs.y {
        let (_, boxes) = crumb_layout(s.crumbs.x, v.crumbs, trailing(v));
        return boxes
            .into_iter()
            .find(|&(_, bx, bw)| x >= bx && x < bx + bw)
            .map(|(i, _, _)| Hit::Crumb(i));
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

    fn rows() -> Vec<SourceRow> {
        vec![
            SourceRow {
                glyph: "\u{25cf}",
                name: "All".into(),
                count: Some(312),
                extra: None,
                kind: RowKind::Special,
                dim: false,
            },
            SourceRow {
                glyph: "\u{25b8}",
                name: "Tech".into(),
                count: Some(41),
                extra: None,
                kind: RowKind::Folder,
                dim: false,
            },
            SourceRow::feed("Hacker News", 12),
            SourceRow {
                glyph: " ",
                name: "Phoronix".into(),
                count: Some(0),
                extra: Some("timed out".into()),
                kind: RowKind::Failed,
                dim: true,
            },
        ]
    }

    fn view<'a>(t: &'a Theme, rows: &'a [SourceRow], folded: bool) -> View<'a> {
        View {
            theme: t,
            focused: true,
            folded,
            crumbs: &CRUMBS,
            rows,
            cursor: 2,
            scroll: 0,
            summary: "41 feeds \u{b7} 312 unread".into(),
            filter: None,
            loading: false,
        }
    }

    static CRUMBS: std::sync::LazyLock<Vec<Crumb>> = std::sync::LazyLock::new(|| {
        vec![
            Crumb {
                name: "sources".into(),
                count: Some(6),
            },
            Crumb {
                name: "Tech".into(),
                count: Some(2),
            },
        ]
    });

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

    /// The rule every panel in this family keeps: a click answers the row
    /// the renderer drew there, at every height the panel can have.
    #[test]
    fn hit_agrees_with_render() {
        let t = theme("terminal");
        let rows = rows();
        for height in [6u16, 12, 20] {
            for folded in [false, true] {
                let area = Rect::new(0, 0, 60, height);
                let v = view(&t, &rows, folded);
                let mut buf = Buffer::empty(area);
                render(area, &mut buf, &v, &mut Bars::new());

                let body = frame::body(area, &words(ModuleId::Sources));
                let s = split(body, folded);
                for y in area.y..area.y + area.height {
                    match hit(area, &v, body.x + 3, y) {
                        Some(Hit::Row(i)) => {
                            assert!(
                                s.list.height > 0 && y >= s.list.y,
                                "row off the list at {y}"
                            );
                            assert_eq!(i, v.scroll + usize::from(y - s.list.y));
                            let drawn: String = (0..area.width)
                                .map(|x| buf[(x, y)].symbol().to_string())
                                .collect();
                            let name = &rows[i].name;
                            assert!(drawn.contains(name.as_str()), "{y}: {drawn:?}");
                        }
                        Some(Hit::Crumb(i)) => {
                            assert_eq!(y, s.crumbs.y);
                            assert!(i < CRUMBS.len());
                        }
                        None => {}
                    }
                }
            }
        }
    }

    #[test]
    fn a_folded_module_says_what_is_chosen_in_it() {
        let t = theme("terminal");
        let rows = rows();
        let area = Rect::new(0, 0, 70, 4);
        let v = view(&t, &rows, true);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &v, &mut Bars::new());
        let text = dump(&buf, area);
        assert!(
            text.contains("sources \u{203a} Tech \u{203a} Hacker News"),
            "{text}"
        );
        assert!(text.contains("312 unread"), "{text}");
    }

    #[test]
    fn a_failed_feed_says_why_beside_its_name() {
        let t = theme("terminal");
        let rows = rows();
        let area = Rect::new(0, 0, 60, 12);
        let v = view(&t, &rows, false);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &v, &mut Bars::new());
        let text = dump(&buf, area);
        assert!(text.contains("Phoronix  timed out"), "{text}");
    }

    /// The `/` filter takes the summary's place, and says which of its two
    /// states it is in with the colour: the accent while the field is open,
    /// so the row reads as a mode, and `warn` for one merely still on.
    #[test]
    fn a_filter_replaces_the_summary_and_says_it_is_a_mode() {
        let t = theme("terminal");
        let rows = rows();
        let area = Rect::new(0, 0, 60, 12);
        let crumbs = split(frame::body(area, &words(ModuleId::Sources)), false).crumbs;
        for (typing, want) in [(true, rgb(t.accent)), (false, rgb(t.warn))] {
            let v = View {
                filter: Some(crate::ui::panels::Filter {
                    text: "pho",
                    typing,
                }),
                ..view(&t, &rows, false)
            };
            let mut buf = Buffer::empty(area);
            render(area, &mut buf, &v, &mut Bars::new());
            let text = dump(&buf, area);
            assert!(text.contains("/pho"), "{typing}: {text}");
            assert!(!text.contains("312 unread"), "{typing}: {text}");

            let x = (0..area.width)
                .find(|x| buf[(*x, crumbs.y)].symbol() == "/")
                .expect("the filter is on the crumb row");
            let cell = &buf[(x, crumbs.y)];
            assert_eq!(cell.style().fg, Some(want), "{typing}");
            assert_eq!(
                cell.style().add_modifier.contains(Modifier::BOLD),
                typing,
                "{typing}"
            );
        }
    }

    #[test]
    fn an_empty_list_offers_the_key_that_fixes_it() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 60, 12);
        let v = View {
            rows: &[],
            ..view(&t, &[], false)
        };
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &v, &mut Bars::new());
        assert!(dump(&buf, area).contains("a adds one"));
    }
}
