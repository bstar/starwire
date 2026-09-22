//! Putting an article's pictures on the screen.
//!
//! The window's half. The core fetched and decoded; this hands the pixels to
//! whatever the terminal can do with them, which is one of three things and
//! is decided per picture rather than once at startup.
//!
//! The order is STAR/CORD's, and the reasons are in `starkit`'s
//! `docs/graphics.md`:
//!
//! - `[ui] graphics = off` draws no picture at all. Off means off, not
//!   coarse -- somebody who turned pictures off did not ask for a worse
//!   picture.
//! - `blocks` draws half blocks, which work in every terminal.
//! - A rectangle the viewport has cut is half blocks unless the protocol
//!   tolerates a clipped placement. kitty does, because it takes a
//!   placement and crops it; sixel and iTerm2 draw from the top left of
//!   wherever they land whatever was asked for, so a picture half off the
//!   top would be drawn over the status row and left there.
//! - Otherwise a protocol, and [`starkit::graphics::mend_unit_placeholder`]
//!   afterwards for the one-cell case the kitty renderer gets wrong.
//!
//! Cropping is this module's own, and it is why `cut_top` exists. A picture
//! scrolled half off the top of the reader cannot be placed above the panel,
//! so the rows that have gone are cut out of the *pixels* before anything is
//! encoded, and the rectangle starts at the first row that is still drawn.

use std::sync::Arc;

use starkit::graphics::{self, Graphics, ImageId, Mode};
use starkit::image::RgbaImage;
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::Style;
use starkit::ratatui::widgets::Widget;
use starkit::ratatui_image::Image;

/// What a cell is taken to be when the terminal never said.
///
/// Eight by sixteen is a common font at a common size. It is used to decide
/// how many rows a picture is given, so being wrong by a little means a
/// picture slightly the wrong shape, and being wrong by a lot would mean a
/// picture that takes the whole reader -- which is why the cap is in rows
/// and not in pixels.
pub const DEFAULT_CELL: (u16, u16) = (8, 16);

/// Draw one picture into one rectangle.
///
/// True when a protocol was built, which is what the caller collects for
/// [`Graphics::forget_unused`]: half blocks are cells like any other and
/// cost the terminal nothing to forget.
pub fn draw_one(
    graphics: &mut Graphics,
    id: ImageId,
    img: &Arc<RgbaImage>,
    rect: Rect,
    clipped: bool,
    buf: &mut Buffer,
) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    match graphics.mode() {
        Mode::Off => return false,
        Mode::Blocks => {
            graphics::halfblocks(img, rect, buf);
            return false;
        }
        _ => {}
    }
    if clipped && graphics.name() != "kitty" {
        graphics::halfblocks(img, rect, buf);
        return false;
    }
    match graphics.protocol(id, img, rect) {
        Some(protocol) => {
            Image::new(protocol).render(rect, buf);
            // A one-cell image leaves the cursor a row low; see the note on
            // `mend_unit_placeholder`. Called for every size, because it
            // only rewrites the exact tail that is wrong.
            graphics::mend_unit_placeholder(buf, rect.x, rect.y);
            true
        }
        // No protocol, or the encoder refused. Half blocks are a worse
        // picture rather than no picture.
        None => {
            graphics::halfblocks(img, rect, buf);
            false
        }
    }
}

/// The part of a picture that is still on screen, when the scroll has taken
/// rows off the top or the bottom of it.
///
/// Measured in whole cells of the terminal's own height, because that is
/// what the reserved rows are: row three of a six-row picture is three cells
/// down whatever the picture's own aspect came to. The original `Arc` comes
/// straight back when nothing is cut, so the common case allocates nothing
/// and keeps the identity the cache already holds.
pub fn crop_rows(
    img: &Arc<RgbaImage>,
    cut_top: u16,
    rows: u16,
    cell_height: u16,
) -> Arc<RgbaImage> {
    let height = img.height();
    if cell_height == 0 || height == 0 || rows == 0 {
        return Arc::clone(img);
    }
    let top = u32::from(cut_top) * u32::from(cell_height);
    let keep = u32::from(rows) * u32::from(cell_height);
    if top == 0 && keep >= height {
        return Arc::clone(img);
    }
    if top >= height {
        return Arc::clone(img);
    }
    let keep = keep.min(height - top).max(1);
    Arc::new(starkit::image::imageops::crop_imm(&**img, 0, top, img.width(), keep).to_image())
}

/// The quiet block of `░` that stands where a picture is going to be.
///
/// Drawn rather than left blank so the rows read as a picture on its way
/// rather than as a hole in the article -- and so that the layout moving
/// nothing when the bytes land is visible as the thing it is.
pub fn placeholder(rect: Rect, buf: &mut Buffer, style: Style) {
    graphics::placeholder(rect, buf, style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use starkit::image::Rgba;

    fn picture(w: u32, h: u32) -> Arc<RgbaImage> {
        Arc::new(RgbaImage::from_fn(w, h, |_, y| {
            // A gradient down the picture, so a crop can be told from an
            // uncropped one by reading a pixel.
            Rgba([(y % 256) as u8, 0, 0, 255])
        }))
    }

    fn blocks() -> Graphics {
        let mut g = Graphics::disabled();
        g.set_mode(Mode::Blocks);
        g
    }

    #[test]
    fn cropping_takes_whole_cells_off_the_top() {
        let img = picture(80, 96); // six rows of sixteen pixels
        let whole = crop_rows(&img, 0, 6, 16);
        assert!(Arc::ptr_eq(&whole, &img), "nothing cut, nothing copied");

        let cut = crop_rows(&img, 2, 4, 16);
        assert_eq!(cut.dimensions(), (80, 64));
        assert_eq!(
            cut.get_pixel(0, 0)[0],
            img.get_pixel(0, 32)[0],
            "it starts two rows down"
        );

        // Cut at the bottom as well: four of six rows, from the top.
        let short = crop_rows(&img, 0, 4, 16);
        assert_eq!(short.dimensions(), (80, 64));
        assert_eq!(short.get_pixel(0, 0)[0], img.get_pixel(0, 0)[0]);

        // Nonsense asks come back unharmed rather than panicking.
        assert_eq!(crop_rows(&img, 99, 4, 16).dimensions(), (80, 96));
        assert_eq!(crop_rows(&img, 1, 0, 16).dimensions(), (80, 96));
        assert_eq!(crop_rows(&img, 1, 4, 0).dimensions(), (80, 96));
    }

    /// Off draws nothing, blocks draws cells, and neither builds a protocol
    /// for the terminal to remember.
    #[test]
    fn off_draws_nothing_and_blocks_draws_cells() {
        let img = picture(16, 16);
        let rect = Rect::new(0, 0, 2, 1);

        let mut buf = Buffer::empty(rect);
        let before = buf.clone();
        let mut g = Graphics::disabled();
        assert!(!draw_one(
            &mut g,
            ImageId::of_arc(&img),
            &img,
            rect,
            false,
            &mut buf
        ));
        assert_eq!(buf, before, "off means no picture, not a coarse one");

        let mut g = blocks();
        assert!(!draw_one(
            &mut g,
            ImageId::of_arc(&img),
            &img,
            rect,
            false,
            &mut buf
        ));
        assert_eq!(buf[(0, 0)].symbol(), "\u{2580}");
        assert_ne!(buf, before);

        // A clipped rectangle is half blocks too, in everything but kitty.
        let mut buf = Buffer::empty(rect);
        assert!(!draw_one(
            &mut g,
            ImageId::of_arc(&img),
            &img,
            rect,
            true,
            &mut buf
        ));
        assert_eq!(buf[(0, 0)].symbol(), "\u{2580}");

        // And an empty rectangle is nothing at all.
        let empty = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(rect);
        assert!(!draw_one(
            &mut g,
            ImageId::of_arc(&img),
            &img,
            empty,
            false,
            &mut buf
        ));
    }

    #[test]
    fn the_placeholder_fills_its_rectangle() {
        let rect = Rect::new(0, 0, 3, 2);
        let mut buf = Buffer::empty(rect);
        placeholder(rect, &mut buf, Style::default());
        for y in 0..rect.height {
            for x in 0..rect.width {
                assert_eq!(buf[(x, y)].symbol(), "\u{2591}");
            }
        }
    }
}
