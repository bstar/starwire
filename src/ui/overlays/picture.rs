//! One picture, as large as the window allows.
//!
//! What a click on a picture opens when there is nowhere else to open it --
//! over ssh, or on a bare tty, where handing the file to `xdg-open` reaches
//! the far machine's desktop and nobody is sitting at it. The reader's own
//! pictures are capped at a third of the panel, and this is the answer to
//! "let me see that properly" on a session that has no other one.
//!
//! ## Fitting includes growing
//!
//! `Resize::Fit` never upsizes -- its own documentation says so -- so a
//! picture smaller than the box would sit in the middle of it at whatever
//! size it happened to be, which is most of what a news article carries and
//! is the one thing somebody who opened this was asking for. So the
//! rectangle is measured in *pixels*, through
//! [`Graphics::cell_size`](starkit::graphics::Graphics::cell_size): the
//! pixels are scaled to exactly the size of the rectangle they will be
//! placed over, and the footer says `\u{d7}2.4` when they were. This is
//! STAR/CORD's media viewer, and deliberately the same arithmetic, so the
//! two applications agree about what a picture looks like.
//!
//! A terminal that never measured a cell has no honest pixel count to work
//! from and gets the picture at the size the reader already had.
//!
//! ## One scaled copy
//!
//! The grown pixels are this window's own, so they carry their own identity
//! -- [`ImageId::of_arc`] of the copy rather than of the picture it was made
//! from. Exactly one is kept, because exactly one picture is ever grown; the
//! identity the copy it replaced went under is handed back so the caller can
//! make the terminal forget the protocol built from it before the allocator
//! hands that address to something else.

use std::collections::HashMap;
use std::sync::Arc;

use starkit::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use starkit::graphics::ImageId;
use starkit::image::RgbaImage;
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::{Modifier, Style};
use starkit::ratatui::text::{Line, Span};
use starkit::ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::ui::panels::{fit, rgb};
use crate::ui::pictures;
use crate::ui::theme::Theme;
use crate::wire::PictureState;

/// The filter a picture is grown with.
///
/// Catmull-Rom rather than Lanczos3, which is the choice STAR/FOLD's preview
/// made and STAR/CORD's viewer after it: both are sharp enough at these
/// sizes, and Lanczos3's negative lobes ring on a hard edge. An article's
/// pictures are as often a chart or a screenshot of a terminal as a
/// photograph, and a halo around every letter is the worse of the two
/// failures.
const SMOOTH: starkit::image::imageops::FilterType =
    starkit::image::imageops::FilterType::CatmullRom;

/// One of the article's pictures, as the overlay knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub url: String,
    pub alt: String,
}

/// The open overlay.
#[derive(Debug)]
pub struct Picture {
    /// Every picture in the article, in the order it reads them, so `n` and
    /// `p` walk them without closing and reopening.
    pub items: Vec<Shown>,
    pub index: usize,
    grown: Option<Grown>,
}

/// The one scaled copy, and what it was made from.
#[derive(Debug)]
struct Grown {
    url: String,
    pixels: (u32, u32),
    id: ImageId,
    image: Arc<RgbaImage>,
}

/// What a key asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Taken,
    Close,
    /// `o`: hand it to whatever shows pictures.
    Open(String),
    /// `y`: the address, for the clipboard.
    Copy(String),
}

/// What one frame placed, for the caller's own drawing pass.
#[derive(Debug)]
pub struct Placed {
    pub rect: Rect,
    pub id: ImageId,
    pub image: Arc<RgbaImage>,
    /// The identity a copy this frame replaced went under, which now names
    /// nothing: `ImageId::of_arc` is an address, and the next picture along
    /// may be handed the one just freed.
    pub stale: Option<ImageId>,
}

impl Picture {
    pub fn new(items: Vec<Shown>, index: usize) -> Option<Self> {
        if items.is_empty() || index >= items.len() {
            return None;
        }
        Some(Self {
            items,
            index,
            grown: None,
        })
    }

    pub fn current(&self) -> &Shown {
        &self.items[self.index.min(self.items.len() - 1)]
    }

    /// Step to another of the article's pictures, wrapping.
    pub fn step(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        self.index = ((self.index as isize + delta).rem_euclid(n)) as usize;
    }

    pub fn handle(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Taken;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Action::Close,
            KeyCode::Char('n') | KeyCode::Right | KeyCode::Char('l') => {
                self.step(1);
                Action::Taken
            }
            KeyCode::Char('p') | KeyCode::Left | KeyCode::Char('h') => {
                self.step(-1);
                Action::Taken
            }
            KeyCode::Char('o') => Action::Open(self.current().url.clone()),
            KeyCode::Char('y') => Action::Copy(self.current().url.clone()),
            _ => Action::Taken,
        }
    }

    /// The pixels to draw, scaled to `pixels` if the copy already held is
    /// not that one.
    ///
    /// The old copy is dropped only once the new one has been allocated, so
    /// the two cannot share an address.
    fn scale(&mut self, url: &str, source: &Arc<RgbaImage>, pixels: (u32, u32)) -> Placed {
        if let Some(held) = self
            .grown
            .as_ref()
            .filter(|g| g.url == url && g.pixels == pixels)
        {
            return Placed {
                rect: Rect::default(),
                id: held.id,
                image: Arc::clone(&held.image),
                stale: None,
            };
        }
        let started = std::time::Instant::now();
        let image = Arc::new(starkit::image::imageops::resize(
            &**source, pixels.0, pixels.1, SMOOTH,
        ));
        tracing::debug!(
            "grew a picture to {}x{} in {:?}",
            pixels.0,
            pixels.1,
            started.elapsed()
        );
        let id = ImageId::of_arc(&image);
        let stale = self
            .grown
            .replace(Grown {
                url: url.to_string(),
                pixels,
                id,
                image: Arc::clone(&image),
            })
            .map(|old| old.id)
            .filter(|old| *old != id);
        Placed {
            rect: Rect::default(),
            id,
            image,
            stale,
        }
    }
}

/// Where the box lands: most of the window, with a margin so the article
/// behind it is still visible at the edges.
pub fn rect(area: Rect) -> Rect {
    let w = area.width.saturating_sub(4).max(8);
    let h = area.height.saturating_sub(2).max(5);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// How much bigger a picture has to be made to fill a box of pixels, and the
/// size to make it. `None` when it already fills the box or overflows it.
///
/// Pure, so that the factor the footer prints and the pixels the drawing
/// pass makes cannot disagree.
pub fn fill_up(picture: (u32, u32), room: (u32, u32)) -> Option<Grow> {
    let ((w, h), (room_w, room_h)) = (picture, room);
    if w == 0 || h == 0 || room_w == 0 || room_h == 0 {
        return None;
    }
    // The rectangle already has the picture's own shape, so the two ratios
    // are within a rounding of each other; the smaller is taken, so the
    // result cannot spill past the rectangle and be shrunk straight back by
    // the encoder.
    let factor = (f64::from(room_w) / f64::from(w)).min(f64::from(room_h) / f64::from(h));
    // Under a tenth there is nothing to see. A rectangle is a whole number
    // of cells and a picture is not, so one that already fills its box lands
    // just above one every time, and a resize pass for three per cent costs
    // the frame it happens on and shows nobody anything -- it would print as
    // `x1.0`, which is a way of saying "not really".
    if !factor.is_finite() || (factor * 10.0).round() < 11.0 {
        return None;
    }
    Some(Grow {
        pixels: (
            ((f64::from(w) * factor).round() as u32).clamp(1, room_w),
            ((f64::from(h) * factor).round() as u32).clamp(1, room_h),
        ),
        factor: factor as f32,
    })
}

/// What [`fill_up`] decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grow {
    pub pixels: (u32, u32),
    pub factor: f32,
}

/// The rectangle a picture of `size` pixels is drawn in, inside `box_`.
///
/// `cell` is the terminal's own, where it has measured one; without it the
/// aspect is taken as two rows to the column, which is the usual shape of a
/// cell and the assumption everything else in the family makes.
pub fn picture_rect(box_: Rect, size: (u32, u32), cell: Option<(u16, u16)>) -> Rect {
    let (w, h) = size;
    if w == 0 || h == 0 || box_.width == 0 || box_.height == 0 {
        return box_;
    }
    let aspect = match cell {
        Some((cw, ch)) if cw > 0 && ch > 0 => f32::from(ch) / f32::from(cw),
        _ => 2.0,
    };
    let by_width = (
        box_.width,
        ((f32::from(box_.width) * (h as f32 / w as f32) / aspect).round() as u16).max(1),
    );
    let (cols, rows) = if by_width.1 <= box_.height {
        by_width
    } else {
        (
            ((f32::from(box_.height) * aspect * (w as f32 / h as f32)).round() as u16).max(1),
            box_.height,
        )
    };
    let cols = cols.clamp(1, box_.width);
    let rows = rows.clamp(1, box_.height);
    Rect {
        x: box_.x + (box_.width - cols) / 2,
        y: box_.y + (box_.height - rows) / 2,
        width: cols,
        height: rows,
    }
}

pub fn render(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    overlay: &mut Picture,
    pictures: &HashMap<String, PictureState>,
    cell: Option<(u16, u16)>,
) -> Option<Placed> {
    let r = rect(area);
    if r.width < 8 || r.height < 5 {
        return None;
    }
    Clear.render(r, buf);

    let item = overlay.current().clone();
    let counter = format!("{}/{}", overlay.index + 1, overlay.items.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(rgb(theme.border_focused)))
        .title(Span::styled(
            format!(
                "{}PICTURE \u{b7} {counter} ",
                starkit::chrome::frame::TITLE_LEAD
            ),
            Style::default()
                .fg(rgb(theme.header_fg))
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                " n p walk \u{b7} o open \u{b7} y copy \u{b7} esc close ",
                Style::default().fg(rgb(theme.dim)),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(rgb(theme.panel_bg)));
    let inner = block.inner(r);
    block.render(r, buf);
    if inner.width == 0 || inner.height < 2 {
        return None;
    }
    let box_ = Rect {
        height: inner.height - 1,
        ..inner
    };
    let footer_y = inner.y + inner.height - 1;

    let (placed, caption) = match pictures.get(&item.url) {
        Some(PictureState::Ready(picture)) => {
            let size = picture.image.dimensions();
            let rect = picture_rect(box_, size, cell);
            let room = cell.map(|(cw, ch)| {
                (
                    u32::from(rect.width) * u32::from(cw),
                    u32::from(rect.height) * u32::from(ch),
                )
            });
            let grow = room.and_then(|room| fill_up(size, room));
            let mut placed = match grow {
                Some(grow) => overlay.scale(&item.url, &picture.image, grow.pixels),
                None => Placed {
                    rect,
                    id: ImageId::of_arc(&picture.image),
                    image: Arc::clone(&picture.image),
                    stale: None,
                },
            };
            placed.rect = rect;
            let (nw, nh) = picture.natural;
            let caption = match grow {
                Some(grow) => format!("{nw}\u{d7}{nh} \u{b7} \u{d7}{:.1}", grow.factor),
                None => format!("{nw}\u{d7}{nh}"),
            };
            (Some(placed), caption)
        }
        Some(PictureState::Failed(why)) => {
            centred(box_, buf, why, Style::default().fg(rgb(theme.error)));
            (None, "it could not be fetched".to_string())
        }
        // On its way, or about to be asked for: the same box of `\u{2591}`
        // the reader draws, so the overlay does not flash empty.
        _ => {
            pictures::placeholder(box_, buf, Style::default().fg(rgb(theme.dim)));
            (None, "fetching\u{2026}".to_string())
        }
    };

    let alt = if item.alt.trim().is_empty() {
        item.url.clone()
    } else {
        item.alt.clone()
    };
    buf.set_string(
        inner.x,
        footer_y,
        fit(&format!("{alt} \u{b7} {caption}"), inner.width),
        Style::default().fg(rgb(theme.dim)),
    );
    placed
}

fn centred(area: Rect, buf: &mut Buffer, text: &str, style: Style) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let fitted = fit(text, area.width);
    let trimmed = fitted.trim_end().to_string();
    let width = u16::try_from(trimmed.chars().count()).unwrap_or(area.width);
    let x = area.x + area.width.saturating_sub(width) / 2;
    buf.set_string(x, area.y + area.height / 2, trimmed, style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::tests_support::theme;
    use crate::wire::pictures::Picture as Decoded;

    fn items() -> Vec<Shown> {
        vec![
            Shown {
                url: "https://e.org/one.png".into(),
                alt: "a diagram".into(),
            },
            Shown {
                url: "https://e.org/two.png".into(),
                alt: String::new(),
            },
        ]
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ready(w: u32, h: u32) -> PictureState {
        PictureState::Ready(Arc::new(Decoded {
            image: Arc::new(RgbaImage::from_pixel(
                w,
                h,
                starkit::image::Rgba([10, 20, 30, 255]),
            )),
            natural: (w, h),
            path: None,
        }))
    }

    fn store(url: &str, state: PictureState) -> HashMap<String, PictureState> {
        let mut map = HashMap::new();
        map.insert(url.to_string(), state);
        map
    }

    #[test]
    fn it_opens_on_the_picture_that_was_clicked_and_walks_the_article() {
        let mut p = Picture::new(items(), 1).expect("two pictures");
        assert_eq!(p.current().url, "https://e.org/two.png");
        assert_eq!(p.handle(key('n')), Action::Taken);
        assert_eq!(p.current().url, "https://e.org/one.png", "it wrapped");
        p.handle(key('p'));
        assert_eq!(p.current().url, "https://e.org/two.png");

        assert_eq!(
            p.handle(key('o')),
            Action::Open("https://e.org/two.png".into())
        );
        assert_eq!(
            p.handle(key('y')),
            Action::Copy("https://e.org/two.png".into())
        );
        assert_eq!(p.handle(key('q')), Action::Close);
        assert_eq!(
            p.handle(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Action::Close
        );

        assert!(Picture::new(Vec::new(), 0).is_none());
        assert!(Picture::new(items(), 9).is_none());
    }

    /// The whole of the growing rule, which the footer's number comes from.
    #[test]
    fn a_picture_smaller_than_its_box_is_grown_to_fill_it() {
        let grow = fill_up((100, 50), (240, 120)).expect("it fits twice over");
        assert_eq!(grow.pixels, (240, 120));
        assert!((grow.factor - 2.4).abs() < 0.01, "{}", grow.factor);

        // Already filling it, or overflowing it: nothing to do.
        assert_eq!(fill_up((240, 120), (240, 120)), None);
        assert_eq!(fill_up((500, 500), (240, 120)), None);
        // Three per cent is not worth a resize pass.
        assert_eq!(fill_up((100, 100), (103, 103)), None);
        // And nothing absurd panics.
        assert_eq!(fill_up((0, 0), (10, 10)), None);
        assert_eq!(fill_up((10, 10), (0, 0)), None);
    }

    /// The rectangle keeps the picture's shape, allowing for a cell being
    /// taller than it is wide.
    #[test]
    fn the_rectangle_keeps_the_pictures_shape() {
        let box_ = Rect::new(0, 0, 80, 20);
        // Square, at two rows to the column: twenty columns is ten rows, so
        // the height binds and it comes out forty by twenty.
        let r = picture_rect(box_, (100, 100), Some((8, 16)));
        assert_eq!((r.width, r.height), (40, 20));
        assert!(r.x > box_.x, "and it is centred: {r:?}");

        // Wide: the width binds.
        let r = picture_rect(box_, (1000, 100), Some((8, 16)));
        assert_eq!(r.width, 80);
        assert!(r.height < 20, "{r:?}");

        // With no measured cell the assumption is two, which is the same
        // answer here.
        assert_eq!(picture_rect(box_, (100, 100), None).width, 40);
        // And nothing to draw is the whole box.
        assert_eq!(picture_rect(box_, (0, 0), None), box_);
    }

    /// One copy is kept, and replacing it hands back the identity the old
    /// one went under so the terminal can be told to forget it.
    #[test]
    fn one_scaled_copy_is_kept_and_the_old_identity_comes_back() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        let mut p = Picture::new(items(), 0).expect("a picture");
        let pictures = store("https://e.org/one.png", ready(40, 20));

        let first =
            render(area, &mut buf, &t, &mut p, &pictures, Some((8, 16))).expect("it was placed");
        assert!(first.stale.is_none());
        assert!(
            first.image.width() > 40,
            "it was grown: {}",
            first.image.width()
        );

        // The same frame again: the copy already held, and no new identity.
        let again =
            render(area, &mut buf, &t, &mut p, &pictures, Some((8, 16))).expect("it was placed");
        assert_eq!(again.id, first.id);
        assert!(again.stale.is_none());

        // A different box: a new copy, and the old identity to forget.
        let smaller = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(smaller);
        let third =
            render(smaller, &mut buf, &t, &mut p, &pictures, Some((8, 16))).expect("it was placed");
        assert_ne!(third.id, first.id);
        assert_eq!(third.stale, Some(first.id));
    }

    /// With no measured cell there is no honest pixel count, so nothing is
    /// grown and the footer says only how big the picture is.
    #[test]
    fn an_unmeasured_cell_grows_nothing() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        let mut p = Picture::new(items(), 0).expect("a picture");
        let pictures = store("https://e.org/one.png", ready(40, 20));

        let placed = render(area, &mut buf, &t, &mut p, &pictures, None).expect("placed");
        assert_eq!(placed.image.dimensions(), (40, 20));
        assert!(
            dump(&buf, area).contains("40\u{d7}20"),
            "{}",
            dump(&buf, area)
        );
        assert!(
            !dump(&buf, area).contains("\u{d7}1."),
            "nothing claimed to be grown"
        );
    }

    /// The three states the footer has something different to say about.
    #[test]
    fn it_says_what_is_there_and_what_is_not() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 30);
        let url = "https://e.org/one.png";

        let mut p = Picture::new(items(), 0).expect("a picture");
        let mut buf = Buffer::empty(area);
        render(
            area,
            &mut buf,
            &t,
            &mut p,
            &store(url, ready(40, 20)),
            Some((8, 16)),
        );
        let text = dump(&buf, area);
        assert!(text.contains("PICTURE \u{b7} 1/2"), "{text}");
        assert!(
            text.contains("a diagram \u{b7} 40\u{d7}20 \u{b7} \u{d7}"),
            "{text}"
        );
        assert!(text.contains("esc close"), "{text}");

        let mut buf = Buffer::empty(area);
        assert!(render(
            area,
            &mut buf,
            &t,
            &mut p,
            &store(url, PictureState::Loading),
            Some((8, 16))
        )
        .is_none());
        assert!(dump(&buf, area).contains('\u{2591}'));
        assert!(dump(&buf, area).contains("fetching"));

        let mut buf = Buffer::empty(area);
        assert!(render(
            area,
            &mut buf,
            &t,
            &mut p,
            &store(url, PictureState::Failed("403".into())),
            Some((8, 16))
        )
        .is_none());
        let text = dump(&buf, area);
        assert!(text.contains("403"), "{text}");
        assert!(text.contains("could not be fetched"), "{text}");

        // And a picture with no alt text says where it came from instead.
        p.step(1);
        let mut buf = Buffer::empty(area);
        render(
            area,
            &mut buf,
            &t,
            &mut p,
            &store("https://e.org/two.png", ready(40, 20)),
            Some((8, 16)),
        );
        assert!(dump(&buf, area).contains("two.png"));
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
}
