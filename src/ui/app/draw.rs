//! Drawing one frame, and the small view structs the panels render from.
//!
//! The order is fixed: the background, the three modules top to bottom, the
//! status row, then whatever overlay is open -- last, because it is drawn
//! over everything and STAR/KIT's own `chrome::overlay` clears the cells
//! under it first.
//!
//! Nothing here reads `State`. Everything a panel draws has already been
//! copied into [`super::ViewData`] by `refresh`, which is the only place the
//! read lock is taken; a draw that reached back for the truth would hold
//! that lock across a render.

use std::time::Instant;

use starkit::chrome::header;
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::{Modifier, Style};

use crate::ui::layout;
use crate::ui::panels::{self, entries, reader, rgb, sources, ModuleId};
use crate::ui::status;
use crate::ui::theme::Theme;
use crate::wire::feed::{ArticleStatus, EntryKind};

use super::App;

impl App {
    pub fn draw(&mut self, area: Rect, buf: &mut Buffer) {
        let bg = Style::default()
            .bg(rgb(self.theme.bg))
            .fg(rgb(self.theme.fg));
        buf.set_style(area, bg);

        // Taken out for the length of the draw rather than borrowed: every
        // panel's own view builder takes `&self`, which would otherwise make
        // `&mut self.bars` conflict with it for as long as the view lives.
        let mut bars = std::mem::take(&mut self.bars);
        bars.begin_frame();

        let padding = (self.cfg.ui.padding_x, self.cfg.ui.padding_y);
        let reader_open = self.open_article().is_some();
        let Some(regions) = self.layout.regions(area, padding, reader_open).cloned() else {
            too_small(area, buf, &self.theme);
            self.bars = bars;
            return;
        };

        {
            let v = self.sources_view();
            sources::render(regions.rect_of(ModuleId::Sources), buf, &v, &mut bars);
        }
        {
            let v = self.entries_view();
            entries::render(regions.rect_of(ModuleId::Entries), buf, &v, &mut bars);
        }

        // The reader's own text is laid out for the width it is about to be
        // drawn at, through the cache, and the link count it hands back is
        // what the `o` chord counts against.
        let reader_rect = regions.rect_of(ModuleId::Reader);
        let body = header::body(reader_rect);
        let cols = reader::text_cols(body.width, self.cfg.reading.width);
        let rendered = self.rendered(cols);
        self.links = rendered
            .as_ref()
            .and_then(|r| r.links.iter().map(|l| l.number).max())
            .unwrap_or(0);
        {
            let v = self.reader_view(rendered);
            reader::render(reader_rect, buf, &v, &mut bars);
        }

        status::render(regions.status, buf, &self.status_view(Instant::now()));

        // Overlays are modal: while one is open only its own bar may be
        // pressed, not whatever the three modules just drew behind it. A
        // second `begin_frame` clears those out; a grab already held
        // survives it, which is the whole point of a grab outliving a frame.
        if self.overlays.is_open() {
            bars.begin_frame();
        }
        let cursor = {
            let theme = self.theme.clone();
            let cfg = self.cfg.clone();
            self.overlays
                .render(regions.area, buf, &theme, &cfg, &mut bars)
        };
        if let Some((x, y)) = cursor {
            reverse_cell(buf, regions.area, x, y);
        }

        self.bars = bars;
    }

    // -- the views ----------------------------------------------------------

    /// What a list's crumb row says about the filter on it: the field's own
    /// text while it is open, else whatever `enter` kept.
    fn filter_of(&self, m: ModuleId) -> Option<panels::Filter<'_>> {
        if let Some(text) = self.filter_field(m) {
            return Some(panels::Filter { text, typing: true });
        }
        let kept = self.filter_text(m);
        (!kept.is_empty()).then_some(panels::Filter {
            text: kept,
            typing: false,
        })
    }

    pub(super) fn sources_view(&self) -> sources::View<'_> {
        sources::View {
            theme: &self.theme,
            focused: self.layout.focus() == ModuleId::Sources,
            folded: !self.layout.is_open(ModuleId::Sources),
            crumbs: &self.view.source_crumbs,
            rows: &self.view.source_rows,
            cursor: self.cursor_of(ModuleId::Sources),
            scroll: self.scroll_of(ModuleId::Sources),
            summary: self.view.source_summary.clone(),
            filter: self.filter_of(ModuleId::Sources),
            loading: false,
        }
    }

    pub(super) fn entries_view(&self) -> entries::View<'_> {
        let open = self.active_source().is_some();
        let cursor = self.cursor_of(ModuleId::Entries);
        let len = self.view.entry_rows.len();
        let crumb = if !open {
            "\u{25b8} choose a source".to_string()
        } else if len == 0 {
            format!("\u{25b8} {}", self.view.entry_source)
        } else {
            let name = self
                .view
                .entry_rows
                .get(cursor)
                .map(|r| r.title.as_str())
                .unwrap_or("");
            format!(
                "\u{25b8} {} \u{203a} {} of {} \u{b7} {name}",
                self.view.entry_source,
                cursor + 1,
                len
            )
        };
        entries::View {
            theme: &self.theme,
            focused: self.layout.focus() == ModuleId::Entries,
            folded: !self.layout.is_open(ModuleId::Entries),
            rows: if open { &self.view.entry_rows } else { &[] },
            cursor,
            scroll: self.scroll_of(ModuleId::Entries),
            source: if open { &self.view.entry_source } else { "" },
            aggregate: self.view.entry_aggregate,
            badge: open.then(|| self.view.entry_badge.clone()).flatten(),
            crumb,
            loading: open && self.view.entries_loading,
            filter: self.filter_of(ModuleId::Entries),
            empty: if open {
                "nothing here"
            } else {
                "choose a source"
            },
        }
    }

    pub(super) fn reader_view(
        &self,
        rendered: Option<std::sync::Arc<crate::ui::markdown::layout::Rendered>>,
    ) -> reader::View<'_> {
        let open = self.open_article();
        let article = self.view.article.as_ref().filter(|a| Some(a.entry) == open);
        let kind = open
            .and_then(|id| self.entry_kind(id))
            .unwrap_or(EntryKind::Article);
        reader::View {
            theme: &self.theme,
            focused: self.layout.focus() == ModuleId::Reader,
            folded: !self.layout.is_open(ModuleId::Reader),
            title: self.head_title.as_str(),
            byline: self.byline_text.as_deref(),
            kind,
            status: article.map(|a| a.status).unwrap_or(ArticleStatus::Pending),
            error: article.and_then(|a| a.error.as_deref()),
            rendered,
            scroll: open.map(|id| self.reader_scroll_of(id)).unwrap_or(0),
            reading_width: self.cfg.reading.width,
            open: open.is_some(),
            loading: open.is_some() && article.is_none(),
        }
    }

    pub(super) fn status_view(&self, now: Instant) -> status::View<'_> {
        status::View {
            theme: &self.theme,
            note: self.note.as_ref(),
            now,
            progress: self.progress_line.as_deref(),
            hints: self.hints(),
            right: &self.right_line,
            graphics: self.graphics.name(),
        }
    }

    /// The key hints for whichever module has the keyboard -- and, while
    /// the `/` field is open, how to leave it instead.
    ///
    /// The way out goes first, because `status::render_hints` gives up the
    /// tail of the line when the field is narrow: at the sixty-column floor
    /// only the first pair or two are drawn, and the one worth keeping is
    /// the one that says how to stop typing.
    pub(super) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.filter.is_some() {
            return vec![
                ("enter", "keep"),
                ("esc", "clear"),
                ("alt+\u{2026}", "still work"),
            ];
        }
        let focus = self.layout.focus();
        let mut hints: Vec<(&'static str, &'static str)> = match focus {
            ModuleId::Sources => vec![
                ("enter", "open"),
                ("l", "into"),
                ("a", "add"),
                ("R", "refresh"),
                ("/", "filter"),
            ],
            ModuleId::Entries => vec![
                ("enter", "read"),
                ("m", "read"),
                ("s", "star"),
                ("o", "browser"),
                ("v", "play"),
                ("n", "next unread"),
            ],
            ModuleId::Reader => vec![
                ("space", "page"),
                ("n", "next"),
                ("m", "read"),
                ("s", "star"),
                ("o", "browser"),
                ("y", "copy"),
            ],
        };
        // A filter `enter` kept is invisible in the keys -- the list simply
        // has fewer rows in it -- so the way out of it leads the line for
        // as long as it is on.
        if !self.filter_text(focus).is_empty() {
            hints.insert(0, ("esc", "clear filter"));
        }
        hints
    }

    /// `⠋ refreshing  12 of 41`, while one is.
    pub(super) fn build_progress_line(&self) -> Option<String> {
        if !self.view.refresh.running {
            return None;
        }
        let frame = super::refresh::SPINNER[self.spinner % super::refresh::SPINNER.len()];
        let what = self.view.refresh.current.as_deref().unwrap_or("refreshing");
        Some(format!(
            "{frame} {what}  {} of {}",
            self.view.refresh.done, self.view.refresh.total
        ))
    }

    /// `Hacker News · 1/40 · 3%`: the source, where the cursor is in it, and
    /// how much of what is loaded has been read. A half-typed chord takes
    /// the field over, because a chord waiting for a key is the most urgent
    /// thing on screen.
    pub(super) fn build_right_line(&self) -> String {
        if let Some(chord) = self.pending_chord() {
            return format!("{chord}\u{2026}");
        }
        if self.active_source().is_none() {
            return String::new();
        }
        let len = self.view.entry_rows.len();
        if len == 0 {
            return self.view.entry_source.clone();
        }
        let read = self.view.entry_read.iter().filter(|r| **r).count();
        let pct = read * 100 / len;
        format!(
            "{} \u{b7} {}/{len} \u{b7} {pct}%",
            self.view.entry_source,
            self.cursor_of(ModuleId::Entries) + 1
        )
    }
}

fn reverse_cell(buf: &mut Buffer, area: Rect, x: u16, y: u16) {
    if x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height {
        buf[(x, y)].modifier |= Modifier::REVERSED;
    }
}

/// The one line a terminal below the floor gets.
pub(super) fn too_small(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let text = format!(
        "STAR/WIRE needs at least {}\u{d7}{}",
        layout::MIN_COLS,
        layout::MIN_ROWS
    );
    let text: String = text.chars().take(usize::from(area.width)).collect();
    let x = area.x + area.width.saturating_sub(text.chars().count() as u16) / 2;
    let y = area.y + area.height / 2;
    buf.set_string(x, y, text, Style::default().fg(rgb(theme.error)));
}

/// Where each module's list can scroll to, given what the last layout drew.
impl App {
    pub(super) fn clamp_scrolls(&mut self) {
        let Some(regions) = self.layout.last.clone() else {
            return;
        };
        for m in panels::COLUMN {
            let Some(i) = self.stack.top_of(m) else {
                continue;
            };
            let rect = regions.rect_of(m);
            let folded = !self.layout.is_open(m);
            match m {
                ModuleId::Sources => {
                    let rows = sources::visible_rows(rect, folded);
                    let cursor = self.stack.frames()[i].cursor;
                    let at = self.stack.frames()[i].scroll;
                    let to = starkit::list::clamp_scroll(cursor, at, rows);
                    self.stack
                        .frames_mut()
                        .nth(i)
                        .expect("the frame is there")
                        .scroll = to;
                }
                ModuleId::Entries => {
                    let rows = entries::visible_rows(rect, folded);
                    let cursor = self.stack.frames()[i].cursor;
                    let at = self.stack.frames()[i].scroll;
                    let to = starkit::list::clamp_scroll(cursor, at, rows);
                    self.stack
                        .frames_mut()
                        .nth(i)
                        .expect("the frame is there")
                        .scroll = to;
                }
                ModuleId::Reader => {
                    let (Some(id), Some(total)) = (self.open_article(), self.rendered_height())
                    else {
                        continue;
                    };
                    let body = header::body(rect);
                    let height = usize::from(body.height).max(1);
                    let max = total.saturating_sub(height);
                    let at = self.reader_scroll_of(id).min(max);
                    self.reader_scroll.insert(id, at);
                }
            }
        }
    }
}
