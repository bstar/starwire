//! Bounded article content; pane and dialog chrome belongs to KIT.
use super::*;

impl Content for Framework {
    fn cells(&mut self, id: &Id, buffer: &mut Buffer) -> bool {
        if id.0 != "reader" {
            return false;
        }
        let width = buffer.area.width.min(self.app.cfg.reading.width).max(1);
        let left = (buffer.area.width - width) / 2;
        let Some(rendered) = self.app.rendered(width, 0) else {
            return false;
        };
        self.app.links = rendered
            .links
            .iter()
            .map(|link| link.number)
            .max()
            .unwrap_or(0);
        let mut head = 0;
        for row in starkit::wrap::wrap(&self.app.head_title, width)
            .iter()
            .take(2)
        {
            if head >= buffer.area.height {
                break;
            }
            buffer.set_stringn(
                left,
                head,
                row.drawn(&self.app.head_title),
                usize::from(width),
                starkit::ratatui::style::Style::default()
                    .fg(crate::ui::panels::rgb(self.app.theme.wire.heading_fg))
                    .add_modifier(Modifier::BOLD),
            );
            head += 1;
        }
        if let Some(byline) = &self.app.byline_text {
            if head < buffer.area.height {
                buffer.set_stringn(
                    left,
                    head,
                    byline,
                    usize::from(width),
                    starkit::ratatui::style::Style::default()
                        .fg(crate::ui::panels::rgb(self.app.theme.wire.byline_fg)),
                );
                head += 1;
            }
        }
        head = (head + 1).min(buffer.area.height);
        let rows = usize::from(buffer.area.height - head);
        self.page_rows[ModuleId::Reader.index()] = rows.max(1);
        let scroll = self
            .app
            .open_article()
            .map(|id| self.app.reader_scroll_of(id))
            .unwrap_or(0)
            .min(rendered.lines.len().saturating_sub(rows.max(1)));
        if let Some(id) = self.app.open_article() {
            self.app.reader_scroll.insert(id, scroll);
        }
        for (row, line) in rendered.lines.iter().skip(scroll).take(rows).enumerate() {
            line.clone()
                .render(Rect::new(left, head + row as u16, width, 1), buffer);
        }
        self.reader_hit = Some(ReaderHit {
            rendered,
            left,
            head,
            scroll,
            scale: [1, 1],
        });
        true
    }
    fn native(&mut self, id: &Id, size: [u16; 2]) -> Option<Surface> {
        let mut buffer =
            Buffer::empty(Rect::new(0, 0, (size[0] / 8).max(1), (size[1] / 24).max(1)));
        if !self.cells(id, &mut buffer) {
            return None;
        }
        if let Some(hit) = &mut self.reader_hit {
            hit.scale = [8, 24];
        }
        let style = NativeStyle::new(Palette::for_chrome(&self.app.theme));
        let mut surface = style.surface(size[0], size[1]).with_ui_text();
        surface.cell_size = Some([8, 24]);
        for y in 0..buffer.area.height {
            let mut x = 0;
            while x < buffer.area.width {
                let cell = &buffer[(x, y)];
                let ink = match cell.fg {
                    starkit::ratatui::style::Color::Rgb(r, g, b) => hex([r, g, b]),
                    _ => style.colors.ink.clone(),
                };
                let mut text = String::new();
                let start = x;
                let modifiers = cell.modifier;
                while x < buffer.area.width
                    && buffer[(x, y)].fg == cell.fg
                    && buffer[(x, y)].modifier == modifiers
                {
                    let glyph = buffer[(x, y)].symbol();
                    text.push_str(glyph);
                    x += starkit::wrap::width_of(glyph).max(1);
                }
                surface.text(
                    PixelRect::new(
                        start * 8,
                        y * 24,
                        ((x - start) * 8).min(size[0].saturating_sub(start * 8)),
                        24.min(size[1] - y * 24),
                    ),
                    text,
                    &ink,
                    12,
                    modifiers.contains(Modifier::BOLD),
                );
            }
        }
        Some(surface)
    }
}
