//! The READER module: the article, wrapped and centred.
//!
//! The wrapping itself is `ui/markdown`'s -- a [`Rendered`] arrives already
//! laid out for a width, with its link spans recorded -- so this module
//! decides three things and draws: how wide the text is, where it sits, and
//! what to say when there is no article to draw.
//!
//! ## The width, and why it is not the panel's
//!
//! [`text_cols`] is `min([reading] width, body - 2)`: the reader's own
//! setting until the panel is too narrow for it, and then the panel less one
//! column each side so the text never touches the border. The result is
//! centred, which is the whole reason the setting exists -- a hundred and
//! sixty column terminal reading an article at a hundred and sixty columns
//! is a terminal nobody reads an article in.
//!
//! ## Pictures are placed here and drawn elsewhere
//!
//! [`picture_rects`] turns the slots `markdown::layout` recorded into the
//! rectangles they occupy on this frame, cropped to the part of the body
//! that is on screen. It does not draw them: a graphics protocol has to go
//! down after the text and before the overlays, which is a pass over the
//! whole frame rather than a panel's business -- see `ui/pictures.rs` and
//! `App::draw`. What it does do is share [`head`] and [`text_rect`] with
//! [`render`] and [`hit`], so the three cannot disagree about where a row
//! is.
//!
//! ## The head does not scroll
//!
//! The title and the byline are drawn above the body and stay there;
//! `scroll` is a row of the *article*, not of the panel. An article whose
//! title has scrolled away is an article you have to scroll back up to
//! identify, and the three rows it costs are cheaper than that.

use std::sync::Arc;

use starkit::chrome::frame::{self, Badge, Tone};
use starkit::chrome::scrollbar;
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::{Modifier, Style};
use starkit::ratatui::text::Line;

use super::{rgb, width_of, words, ModuleId};
use crate::ui::markdown::layout::Rendered;
use crate::ui::theme::Theme;
use crate::ui::{Bar, Bars};
use crate::wire::feed::{ArticleStatus, EntryKind};

/// What the bottom border says while the reader has the keyboard.
pub const FOOTER: &str = "space page \u{b7} n next \u{b7} o<n> link \u{b7} esc close";

/// The blank column kept each side of the text, which is what stops a line
/// of prose from touching the panel border.
const MARGIN: u16 = 1;

pub struct View<'a> {
    pub theme: &'a Theme,
    pub focused: bool,
    pub folded: bool,
    /// The article's own title, wrapped above the body.
    pub title: &'a str,
    /// `author · site · date · 6 min`, when `[reading] show_byline` is on
    /// and there is one.
    pub byline: Option<&'a str>,
    pub kind: EntryKind,
    pub status: ArticleStatus,
    /// Why extraction failed, for [`ArticleStatus::Failed`].
    pub error: Option<&'a str>,
    pub rendered: Option<Arc<Rendered>>,
    pub scroll: usize,
    /// `[reading] width`.
    pub reading_width: u16,
    /// Whether there is an entry open at all.
    pub open: bool,
    /// The entry is open but its text has not arrived from the database
    /// yet. One frame, usually; the state exists so that frame says
    /// something rather than claiming the article is empty.
    pub loading: bool,
}

/// How many columns of text the body gets: the reader's setting, or the
/// panel less a margin each side when that is narrower.
pub fn text_cols(body_width: u16, reading_width: u16) -> u16 {
    reading_width.min(body_width.saturating_sub(MARGIN * 2))
}

/// Where the text sits inside the body: [`text_cols`] wide, centred.
pub fn text_rect(body: Rect, reading_width: u16) -> Rect {
    let width = text_cols(body.width, reading_width);
    Rect {
        x: body.x + (body.width.saturating_sub(width)) / 2,
        width,
        ..body
    }
}

/// The body, and the part of it the article's own rows are drawn in.
///
/// The one piece of arithmetic [`render`], [`hit`] and [`picture_rects`]
/// all depend on. `None` when there is nothing open or no room to draw it.
fn text_area(area: Rect, v: &View<'_>) -> Option<(Rect, Rect)> {
    let body = frame::body(area, &words(ModuleId::Reader));
    if body.height == 0 || body.width == 0 || !v.open {
        return None;
    }
    let text = text_rect(body, v.reading_width);
    let head_rows = u16::try_from(head(v, text.width).0.len()).unwrap_or(0);
    let y = text.y + head_rows;
    let rest = Rect {
        y,
        height: (body.y + body.height).saturating_sub(y),
        ..text
    };
    Some((body, rest))
}

/// Where one of this article's pictures is on the screen right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visible {
    /// Which of the article's pictures this is.
    pub index: usize,
    /// The cells it covers, already cut to the part of the body on screen.
    pub rect: Rect,
    /// How many of the picture's own rows the scroll took off the top. The
    /// pixels for those rows are cropped away before anything is encoded: a
    /// rectangle cannot start above the panel, so the top has to be cut out
    /// of the picture rather than out of the rectangle.
    pub cut_top: u16,
    /// Whether anything was cut at all, top, bottom or side. A clipped
    /// placement is drawn as half blocks in every protocol but kitty's.
    pub clipped: bool,
}

/// Every picture of this article that is on screen, in document order.
pub fn picture_rects(area: Rect, v: &View<'_>) -> Vec<Visible> {
    let Some((_, rest)) = text_area(area, v) else {
        return Vec::new();
    };
    let Some(rendered) = v.rendered.as_deref() else {
        return Vec::new();
    };
    if rest.height == 0 || rest.width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (index, slot) in rendered.pictures.iter().enumerate() {
        let top = usize::from(slot.row);
        let bottom = top + usize::from(slot.rows);
        if bottom <= v.scroll {
            continue;
        }
        let cut_top = u16::try_from(v.scroll.saturating_sub(top)).unwrap_or(u16::MAX);
        let rows = slot.rows.saturating_sub(cut_top);
        let Ok(above) = u16::try_from(top.saturating_sub(v.scroll)) else {
            continue;
        };
        if rows == 0 || above >= rest.height {
            continue;
        }
        let y = rest.y + above;
        let height = rows.min(rest.y + rest.height - y);
        let x = rest.x + slot.col.min(rest.width);
        let width = slot.cols.min((rest.x + rest.width).saturating_sub(x));
        if height == 0 || width == 0 {
            continue;
        }
        out.push(Visible {
            index,
            rect: Rect {
                x,
                y,
                width,
                height,
            },
            cut_top,
            clipped: cut_top > 0 || height < rows || width < slot.cols,
        });
    }
    out
}

/// The rows above the article: the title, the byline, and a blank. The
/// caller's `scroll` counts from the first row *after* these, and both
/// [`render`] and [`hit`] measure them the same way.
fn head(v: &View<'_>, width: u16) -> (Vec<String>, usize) {
    if !v.open || width == 0 {
        return (Vec::new(), 0);
    }
    let wrapped = |text: &str| -> Vec<String> {
        starkit::wrap::wrap(text, width)
            .iter()
            .map(|row| row.drawn(text).to_string())
            .collect()
    };
    let mut out = wrapped(v.title);
    if out.is_empty() {
        out.push(String::new());
    }
    // How many rows the title took, so `render` can draw exactly those in
    // bold and the byline under them in its own colour.
    let title_rows = out.len();
    if let Some(byline) = v.byline {
        out.extend(wrapped(byline));
    }
    out.push(String::new());
    (out, title_rows)
}

/// The badge on the top border: what state this article is in.
pub fn status_word(v: &View<'_>) -> Option<(&'static str, Tone)> {
    if !v.open {
        return None;
    }
    if v.kind == EntryKind::Video {
        return Some(("video", Tone::Accent));
    }
    Some(match v.status {
        ArticleStatus::Pending => ("extracting\u{2026}", Tone::Warn),
        ArticleStatus::Failed => ("failed", Tone::Warn),
        _ => ("ready", Tone::Ok),
    })
}

pub fn render(area: Rect, buf: &mut Buffer, v: &View<'_>, bars: &mut Bars) {
    let t = v.theme;
    let word_list = words(ModuleId::Reader);
    let core: &starkit::theme::Theme = t;
    let badge = status_word(v).map(|(text, tone)| Badge { text, tone });
    let detail = v.open.then_some(v.title);
    let body = frame::frame(
        area,
        buf,
        &frame::Frame {
            theme: core,
            focused: v.focused,
            title: ModuleId::Reader.title(),
            detail,
            heading: false,
            badge,
            footer: v.focused.then_some(FOOTER),
            words: &word_list,
        },
    );
    if body.height == 0 || body.width == 0 {
        return;
    }

    if !v.open {
        starkit::chrome::empty(body, buf, t, "nothing open");
        return;
    }

    let text = text_rect(body, v.reading_width);
    let mut y = text.y;
    let (head_rows, title_rows) = head(v, text.width);
    for (i, line) in head_rows.iter().enumerate() {
        if y >= body.y + body.height {
            return;
        }
        let style = if i < title_rows {
            Style::default().fg(rgb(t.fg)).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(rgb(t.wire.byline_fg))
        };
        buf.set_string(text.x, y, line, style);
        y += 1;
    }

    let rest = Rect {
        y,
        height: (body.y + body.height).saturating_sub(y),
        ..text
    };
    if rest.height == 0 {
        return;
    }

    match body_kind(v) {
        Body::Loading => {
            centred(
                rest,
                buf,
                "reading\u{2026}",
                Style::default().fg(rgb(t.dim)),
            );
        }
        Body::Pending => {
            centred(
                rest,
                buf,
                "extracting\u{2026}",
                Style::default().fg(rgb(t.dim)),
            );
        }
        Body::Failed(reason) => {
            centred(rest, buf, reason, Style::default().fg(rgb(t.error)));
            let hint = Rect {
                y: rest.y + rest.height / 2 + 2,
                height: 1,
                ..rest
            };
            if hint.y < body.y + body.height {
                centred(
                    hint,
                    buf,
                    "o opens it in the browser",
                    Style::default().fg(rgb(t.dim)),
                );
            }
        }
        Body::Lines(rendered) => {
            draw_lines(rest, buf, &rendered.lines, v.scroll);
            if v.kind == EntryKind::Video {
                let hint_y = rest.y
                    + u16::try_from(rendered.lines.len().saturating_sub(v.scroll)).unwrap_or(0)
                    + 1;
                if hint_y < body.y + body.height {
                    buf.set_string(
                        rest.x,
                        hint_y,
                        "v plays it",
                        Style::default().fg(rgb(t.wire.video_fg)),
                    );
                }
            }
        }
        Body::Nothing => {
            centred(rest, buf, "no text", Style::default().fg(rgb(t.dim)));
        }
    }

    let total = v.rendered.as_ref().map(|r| r.lines.len()).unwrap_or(0) as u32;
    let track = scrollbar::track(area, rest);
    bars.draw(Bar::Reader, track, buf, t, total, v.scroll as u32);
}

enum Body<'a> {
    Loading,
    Pending,
    Failed(&'a str),
    Lines(&'a Rendered),
    Nothing,
}

fn body_kind<'a>(v: &'a View<'a>) -> Body<'a> {
    match v.rendered.as_deref() {
        Some(r) if !r.lines.is_empty() => Body::Lines(r),
        _ if v.loading => Body::Loading,
        // Nothing to draw, and a reason for it: an extraction still running
        // says so, a failed one says why.
        _ if v.status == ArticleStatus::Pending => Body::Pending,
        _ if v.status == ArticleStatus::Failed => {
            Body::Failed(v.error.unwrap_or("the page could not be read"))
        }
        _ => Body::Nothing,
    }
}

fn draw_lines(area: Rect, buf: &mut Buffer, lines: &[Line<'static>], scroll: usize) {
    for (row, line) in lines
        .iter()
        .skip(scroll)
        .take(usize::from(area.height))
        .enumerate()
    {
        let y = area.y + u16::try_from(row).unwrap_or(0);
        let mut x = area.x;
        for span in &line.spans {
            let room = (area.x + area.width).saturating_sub(x);
            if room == 0 {
                break;
            }
            // Cut rather than fitted: `markdown::layout` already pads a
            // code row out to the full width with the background on it, and
            // padding again here would paint whatever style the last span
            // carried across the rest of the line.
            let text = cut(&span.content, room);
            let drawn = width_of(&text);
            buf.set_string(x, y, text, span.style);
            x += drawn.max(1);
        }
    }
}

/// As much of `text` as fits in `width`, with no padding.
fn cut(text: &str, width: u16) -> String {
    if width_of(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0u16;
    for (_, cluster) in starkit::wrap::clusters(text) {
        let w = width_of(cluster);
        if used + w > width {
            break;
        }
        out.push_str(cluster);
        used += w;
    }
    out
}

fn inside(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn centred(area: Rect, buf: &mut Buffer, text: &str, style: Style) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let fitted = starkit::text::fit(text, area.width);
    let trimmed = fitted.trim_end();
    let x = area.x + area.width.saturating_sub(width_of(trimmed)) / 2;
    buf.set_string(x, area.y + area.height / 2, trimmed, style);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Link *n*, as the article numbers it.
    Link(u16),
    /// The picture at this index in the article's own order.
    Picture(usize),
    Scrollbar,
}

/// What a click in the reader landed on.
///
/// Every span is offset by the head rows and by `scroll`, which is the one
/// arithmetic [`render`] and this have to agree about; both take it from
/// [`head`] rather than from a constant.
pub fn hit(area: Rect, v: &View<'_>, x: u16, y: u16) -> Option<Hit> {
    let body = frame::body(area, &words(ModuleId::Reader));
    let (_, rest) = text_area(area, v)?;
    if y < rest.y || y >= body.y + body.height {
        return None;
    }
    if x >= body.x + body.width {
        return Some(Hit::Scrollbar);
    }
    // Pictures first. A picture's rows are blank, so nothing else is under
    // one -- but a link whose row a picture covers would otherwise answer
    // for a click nowhere near it.
    if let Some(place) = picture_rects(area, v)
        .into_iter()
        .find(|place| inside(place.rect, x, y))
    {
        return Some(Hit::Picture(place.index));
    }
    let rendered = v.rendered.as_deref()?;
    let row = usize::from(y - rest.y) + v.scroll;
    let row = u16::try_from(row).ok()?;
    rendered
        .links
        .iter()
        .find(|span| {
            span.row == row && x >= rest.x + span.col && x < rest.x + span.col + span.width
        })
        .map(|span| Hit::Link(span.number))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::markdown::{layout, parse};
    use crate::ui::theme::tests_support::theme;

    fn rendered(t: &Theme, width: u16) -> Arc<Rendered> {
        let doc = parse::parse(crate::ui::markdown::FIXTURE_MD);
        Arc::new(layout::layout(
            &doc,
            &layout::LayoutCtx {
                theme: t,
                width,
                pictures: None,
            },
        ))
    }

    fn view<'a>(t: &'a Theme, r: Option<Arc<Rendered>>) -> View<'a> {
        View {
            theme: t,
            focused: true,
            folded: false,
            title: "Why the borrow checker says no: a field guide to lifetimes",
            byline: Some("Jane Example \u{b7} example.org \u{b7} 20 Sep 2026 \u{b7} 6 min"),
            kind: EntryKind::Article,
            status: ArticleStatus::Extracted,
            error: None,
            rendered: r,
            scroll: 0,
            reading_width: 80,
            open: true,
            loading: false,
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
    fn the_text_is_the_reading_width_until_the_panel_is_narrower() {
        assert_eq!(text_cols(120, 80), 80);
        assert_eq!(text_cols(60, 80), 58);
        assert_eq!(text_cols(2, 80), 0);
        assert_eq!(text_cols(0, 80), 0);
    }

    #[test]
    fn the_text_is_centred_in_the_body() {
        let body = Rect::new(0, 0, 100, 20);
        let r = text_rect(body, 80);
        assert_eq!(r.width, 80);
        assert_eq!(r.x, 10);
        assert_eq!(r.x + r.width + 10, body.width);
    }

    /// A click on a link answers the number the article printed beside it,
    /// at every scroll position and at both widths.
    #[test]
    fn hit_agrees_with_render() {
        let t = theme("terminal");
        for (w, h) in [(100u16, 30u16), (60, 21)] {
            let area = Rect::new(0, 0, w, h);
            let body = frame::body(area, &words(ModuleId::Reader));
            let text = text_rect(body, 80);
            let r = rendered(&t, text.width);
            for scroll in [0usize, 4, 9] {
                let v = View {
                    scroll,
                    ..view(&t, Some(Arc::clone(&r)))
                };
                let mut buf = Buffer::empty(area);
                render(area, &mut buf, &v, &mut Bars::new());
                let head_rows = u16::try_from(head(&v, text.width).0.len()).unwrap();
                for span in &r.links {
                    let Some(y) = (usize::from(span.row))
                        .checked_sub(scroll)
                        .map(|d| text.y + head_rows + u16::try_from(d).unwrap())
                    else {
                        continue;
                    };
                    if y >= body.y + body.height {
                        continue;
                    }
                    assert_eq!(
                        hit(area, &v, text.x + span.col, y),
                        Some(Hit::Link(span.number)),
                        "{w}x{h} scroll {scroll} link {}",
                        span.number
                    );
                }
            }
        }
    }

    /// An article with one picture in it, six rows tall, laid out at the
    /// width the reader will draw at.
    fn with_a_picture(t: &Theme, width: u16) -> Arc<Rendered> {
        use crate::ui::markdown::layout::{PictureKnown, PictureSizes};
        let mut known = std::collections::HashMap::new();
        // 160 by 96 pixels at a cell of eight by sixteen: twenty columns
        // and six rows.
        known.insert(
            "https://e.org/hero.png".to_string(),
            PictureKnown::Natural(160, 96),
        );
        let sizes = PictureSizes {
            cap_rows: 9,
            cell: (8, 16),
            known: &known,
        };
        let doc = parse::parse(concat!(
            "Some words before it.\n\n",
            "![a diagram](https://e.org/hero.png)\n\n",
            "And some words after it.\n",
        ));
        Arc::new(layout::layout(
            &doc,
            &layout::LayoutCtx {
                theme: t,
                width,
                pictures: Some(&sizes),
            },
        ))
    }

    /// The rectangle a picture is drawn in is the one a click on it is
    /// answered from, at every scroll position -- and the cells under it
    /// are the blank ones the layout reserved.
    #[test]
    fn picture_rects_agree_with_render_and_a_click_finds_them() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 30);
        let body = frame::body(area, &words(ModuleId::Reader));
        let text = text_rect(body, 80);
        let r = with_a_picture(&t, text.width);
        assert_eq!(r.pictures.len(), 1, "{:?}", r.plain);
        assert_eq!((r.pictures[0].cols, r.pictures[0].rows), (20, 6));

        for scroll in [0usize, 4, 9] {
            let v = View {
                scroll,
                ..view(&t, Some(Arc::clone(&r)))
            };
            let mut buf = Buffer::empty(area);
            render(area, &mut buf, &v, &mut Bars::new());

            let places = picture_rects(area, &v);
            let Some(place) = places.first().copied() else {
                continue;
            };
            assert_eq!(place.index, 0);
            assert!(place.rect.height <= 6 && place.rect.width <= 20);
            assert!(
                place.rect.y >= body.y && place.rect.y + place.rect.height <= body.y + body.height,
                "scroll {scroll}: {:?} is outside {body:?}",
                place.rect
            );

            // Every cell it covers is one the text left blank, which is
            // what the reserved rows are for.
            for y in place.rect.y..place.rect.y + place.rect.height {
                for x in place.rect.x..place.rect.x + place.rect.width {
                    assert_eq!(
                        buf[(x, y)].symbol().trim(),
                        "",
                        "scroll {scroll}: something was drawn at {x},{y}"
                    );
                }
            }

            // And a click anywhere on it answers for it rather than for a
            // link on a row it covers.
            for (x, y) in [
                (place.rect.x, place.rect.y),
                (
                    place.rect.x + place.rect.width - 1,
                    place.rect.y + place.rect.height - 1,
                ),
            ] {
                assert_eq!(
                    hit(area, &v, x, y),
                    Some(Hit::Picture(0)),
                    "scroll {scroll} at {x},{y}"
                );
            }
        }
    }

    /// Scrolled half off the top, the rectangle starts at the first drawn
    /// row and the rows taken away are counted -- they are cut out of the
    /// picture rather than out of the rectangle, because a rectangle cannot
    /// start above the panel.
    #[test]
    fn a_picture_scrolled_off_the_top_is_cut_rather_than_moved() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 30);
        let body = frame::body(area, &words(ModuleId::Reader));
        let text = text_rect(body, 80);
        let r = with_a_picture(&t, text.width);
        let slot = r.pictures[0].clone();

        let v = View {
            scroll: usize::from(slot.row) + 2,
            ..view(&t, Some(Arc::clone(&r)))
        };
        let (_, rest) = text_area(area, &v).expect("an open article");
        let place = picture_rects(area, &v)[0];
        assert_eq!(place.cut_top, 2, "two of its six rows are above the view");
        assert_eq!(place.rect.height, 4);
        assert_eq!(place.rect.y, rest.y, "at the first drawn row");
        assert!(place.clipped);

        // All six above it: nothing to draw at all.
        let v = View {
            scroll: usize::from(slot.row) + usize::from(slot.rows),
            ..view(&t, Some(r))
        };
        assert!(picture_rects(area, &v).is_empty());
    }

    /// With nothing open, or nothing laid out, there is nothing to place.
    #[test]
    fn there_are_no_pictures_to_place_when_there_is_no_article() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 30);
        let closed = View {
            open: false,
            ..view(&t, None)
        };
        assert!(picture_rects(area, &closed).is_empty());
        assert!(picture_rects(area, &view(&t, None)).is_empty());
    }

    #[test]
    fn a_pending_article_says_it_is_being_fetched() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 20);
        let v = View {
            status: ArticleStatus::Pending,
            ..view(&t, None)
        };
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &v, &mut Bars::new());
        let text = dump(&buf, area);
        assert!(text.contains("extracting\u{2026}"), "{text}");
    }

    #[test]
    fn a_failed_article_says_why_and_offers_the_browser() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 20);
        let v = View {
            status: ArticleStatus::Failed,
            error: Some("paywall"),
            ..view(&t, None)
        };
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &v, &mut Bars::new());
        let text = dump(&buf, area);
        assert!(text.contains("paywall"), "{text}");
        assert!(text.contains("o opens it in the browser"), "{text}");
    }

    #[test]
    fn a_video_offers_the_player_under_its_description() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 24);
        let body = frame::body(area, &words(ModuleId::Reader));
        let width = text_cols(body.width, 80);
        let doc = parse::parse("A short description of the video.");
        let r = Arc::new(layout::layout(
            &doc,
            &layout::LayoutCtx {
                theme: &t,
                width,
                pictures: None,
            },
        ));
        let v = View {
            kind: EntryKind::Video,
            status: ArticleStatus::NotApplicable,
            ..view(&t, Some(r))
        };
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &v, &mut Bars::new());
        let text = dump(&buf, area);
        assert!(text.contains("A short description"), "{text}");
        assert!(text.contains("v plays it"), "{text}");
        assert!(text.contains("video"), "the badge: {text}");
    }

    #[test]
    fn nothing_open_says_so_and_has_no_links_to_click() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 20);
        let v = View {
            open: false,
            ..view(&t, None)
        };
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &v, &mut Bars::new());
        assert!(dump(&buf, area).contains("nothing open"));
        assert_eq!(hit(area, &v, 10, 10), None);
    }
}
