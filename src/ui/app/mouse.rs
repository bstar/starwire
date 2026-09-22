//! Where a click, a drag and a wheel land.
//!
//! The order matters and is the same one STAR/FOLD settled on: every
//! scrollbar's own grab first (a press on a bar is never anything else's to
//! answer), then an open overlay, then the status row, then the module the
//! pointer is over -- its header words, its fold, its rows.
//!
//! Nothing here recomputes a rectangle. The geometry is whatever
//! `LayoutState::regions` decided at the top of the last draw, and each
//! panel's own `hit` answers against the same split its `render` drew from,
//! which is why a click cannot land on a row that was not there.

use starkit::chrome::header;
use starkit::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::ui::keymap::Action;
use crate::ui::layout::Regions;
use crate::ui::panels::{self, entries, reader, sources, ModuleId, Word};
use crate::ui::status;
use crate::ui::Bar;
use crate::wire::{open, Command, OpenKind};

use super::App;

impl App {
    pub fn mouse(&mut self, m: MouseEvent) {
        let Some(regions) = self.layout.last.clone() else {
            return;
        };

        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((bar, above)) = self.bars.press(m.column, m.row) {
                    self.scroll_bar_to(bar, above);
                    return;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((bar, above)) = self.bars.drag(m.row) {
                    self.scroll_bar_to(bar, above);
                    return;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.bars.release();
            }
            _ => {}
        }

        if self.overlays.is_open() {
            match m.kind {
                MouseEventKind::ScrollDown => self.overlays.scroll(false),
                MouseEventKind::ScrollUp => self.overlays.scroll(true),
                MouseEventKind::Down(MouseButton::Left) => {
                    let cfg = self.cfg.clone();
                    let answer = self.overlays.click(m.column, m.row, regions.area, &cfg);
                    self.after_overlay_answer(answer);
                }
                _ => {}
            }
            return;
        }

        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(&regions, m.column, m.row),
            MouseEventKind::Down(MouseButton::Right) => self.right_click(&regions, m.column, m.row),
            MouseEventKind::ScrollDown => self.scroll_at(&regions, m.column, m.row, 3),
            MouseEventKind::ScrollUp => self.scroll_at(&regions, m.column, m.row, -3),
            _ => {}
        }
        self.clamp_scrolls();
    }

    /// Apply a press's or a drag's `above` to whichever scroll position the
    /// bar answers for -- the one rule `Scrollbars` asks of a caller: what
    /// is applied here is what the next draw for that bar must report back,
    /// or the thumb snaps to wherever the untouched value still is.
    fn scroll_bar_to(&mut self, bar: Bar, above: u32) {
        let above = above as usize;
        match bar {
            Bar::Sources | Bar::Entries => {
                let m = if bar == Bar::Sources {
                    ModuleId::Sources
                } else {
                    ModuleId::Entries
                };
                let Some(i) = self.stack.top_of(m) else {
                    return;
                };
                let height = self.bars.track_of(bar).map(|r| r.height).unwrap_or(0);
                let len = self.rows_of(m);
                let cursor = starkit::list::cursor_into_view(
                    self.stack.frames()[i].cursor,
                    above,
                    usize::from(height),
                    len,
                );
                {
                    let frame = self.stack.frames_mut().nth(i).expect("the frame is there");
                    frame.scroll = above;
                }
                self.set_cursor(m, cursor);
            }
            Bar::Reader => {
                if let Some(id) = self.open_article() {
                    self.reader_scroll.insert(id, above);
                }
            }
            // The overlays hold their own scroll and are moved through
            // `Overlays::scroll`; neither records a bar this frame, so
            // `press` can never hand one of these back.
            Bar::Help | Bar::Import => {}
        }
    }

    fn click(&mut self, regions: &Regions, x: u16, y: u16) {
        if let Some(hit) = status::hit(
            regions.status,
            &self.status_view(std::time::Instant::now()),
            x,
            y,
        ) {
            match hit {
                status::Hit::Help => {
                    self.overlays.open_help();
                    self.repaint = true;
                }
                status::Hit::Count => self.focus(ModuleId::Sources),
                status::Hit::Progress => self.focus(ModuleId::Entries),
            }
            return;
        }

        let Some(module) = regions.hit(x, y) else {
            return;
        };
        let rect = regions.rect_of(module);
        let words = panels::words(module);
        if let Some(word) = header::hit(rect, &words, x, y) {
            self.word_click(module, word);
            return;
        }

        let double = self.clicks.click(x, y);
        // A click on a folded module opens it and nothing else: the row
        // under the pointer after it opens is not the row that was clicked.
        if !self.layout.is_open(module) {
            self.focus(module);
            return;
        }
        let was = self.layout.focus();
        if was != module {
            self.focus(module);
        }

        match module {
            ModuleId::Sources => {
                let v = self.sources_view();
                match sources::hit(rect, &v, x, y) {
                    Some(sources::Hit::Crumb(i)) => {
                        self.stack.jump_to(i);
                        self.sync_focus();
                    }
                    Some(sources::Hit::Row(i)) => {
                        self.set_cursor(ModuleId::Sources, i);
                        if double {
                            self.act(Action::Activate);
                        }
                    }
                    None => {}
                }
            }
            ModuleId::Entries => {
                let v = self.entries_view();
                match entries::hit(rect, &v, x, y) {
                    Some(entries::Hit::Crumb) => self.focus(ModuleId::Sources),
                    Some(entries::Hit::Row(i)) => {
                        self.set_cursor(ModuleId::Entries, i);
                        if double {
                            self.act(Action::Activate);
                        }
                    }
                    None => {}
                }
            }
            ModuleId::Reader => {
                let body = header::body(rect);
                let cols = reader::text_cols(body.width, self.cfg.reading.width);
                let rendered = self.rendered(cols);
                let v = self.reader_view(rendered);
                if let Some(reader::Hit::Link(n)) = reader::hit(rect, &v, x, y) {
                    self.open_link_at(n);
                }
            }
        }
    }

    /// Right-click stars whatever entry it landed on, which is the one
    /// gesture in this program that changes something without a key.
    fn right_click(&mut self, regions: &Regions, x: u16, y: u16) {
        if regions.hit(x, y) != Some(ModuleId::Entries) || !self.layout.is_open(ModuleId::Entries) {
            return;
        }
        let rect = regions.rect_of(ModuleId::Entries);
        let v = self.entries_view();
        if let Some(entries::Hit::Row(i)) = entries::hit(rect, &v, x, y) {
            self.set_cursor(ModuleId::Entries, i);
            self.focus(ModuleId::Entries);
            self.act(Action::ToggleStar);
        }
    }

    fn scroll_at(&mut self, regions: &Regions, x: u16, y: u16, delta: i32) {
        let Some(module) = regions.hit(x, y) else {
            return;
        };
        match module {
            ModuleId::Reader => {
                if let Some(id) = self.open_article() {
                    let at = self.reader_scroll_of(id);
                    self.reader_scroll
                        .insert(id, (at as i64 + i64::from(delta)).max(0) as usize);
                }
            }
            m => {
                let Some(i) = self.stack.top_of(m) else {
                    return;
                };
                let at = self.stack.frames()[i].scroll;
                let to = (at as i64 + i64::from(delta)).max(0) as usize;
                let frame = self.stack.frames_mut().nth(i).expect("the frame is there");
                frame.scroll = to;
            }
        }
    }

    /// A header word does what it says.
    fn word_click(&mut self, module: ModuleId, word: Word) {
        match word {
            Word::Back => {
                self.focus(module);
                self.act(Action::Pop);
            }
            Word::Add => self.act(Action::AddFeed),
            Word::Refresh => self.act(Action::RefreshAll),
            Word::Unread => {
                self.focus(ModuleId::Entries);
                self.act(Action::UnreadOnly);
            }
            // The word is on the ENTRIES header, so it filters the entries
            // whatever had the keyboard -- `/` filters the focused list, and
            // clicking a word is not the same as pressing it.
            Word::Filter => {
                self.focus(ModuleId::Entries);
                self.act(Action::Filter);
            }
            Word::Star => {
                self.focus(module);
                self.act(Action::ToggleStar);
            }
            Word::Browser => {
                self.focus(module);
                self.act(Action::OpenBrowser);
            }
        }
    }

    /// The same as the `o<n>` chord's, for a click straight on a link.
    fn open_link_at(&mut self, n: u16) {
        let Some(article) = self.view.article.as_ref() else {
            return;
        };
        let doc = crate::ui::markdown::parse::parse(&article.markdown);
        let Some(url) = doc.links.get(usize::from(n).saturating_sub(1)).cloned() else {
            return;
        };
        let kind = match open::kind_of(&url) {
            open::Target::Video(_) => OpenKind::Video,
            // `kind_of` answers for a link, and a link is one of the two.
            _ => OpenKind::Browser,
        };
        self.core.send(Command::OpenUrl { url, kind });
        self.say(format!("opening link {n}"));
    }
}
