//! A [`Doc`], a width and a theme in; the rows to draw out.
//!
//! The second stage of the reader. Every block is turned into
//! `(String, Style)` runs, handed to `starkit::wrap::wrap_runs` and read
//! back as `Line`s of `Span`s -- the same pipeline STAR/CORD's message list
//! uses, and for the same reason: wrapping and drawing have to agree about
//! where a cluster ended, and the only way to be sure they do is for one
//! piece of code to decide it.
//!
//! What comes back is everything the panel needs and nothing it has to work
//! out again: the lines, the [`LinkSpan`]s a click lands on, the height a
//! scrollbar measures, and the article as plain text.
//!
//! ## The block treatments
//!
//! | block | how it is drawn |
//! |---|---|
//! | H1, H2 | `heading_fg`, bold, a blank line above |
//! | H3–H6 | `heading_fg` |
//! | paragraph | the inline modifiers; code on `code_bg`; a link underlined in `link_fg` and followed by ` [n]` |
//! | quote | a `▎ ` gutter in `quote_fg`, its blocks laid out at `width - 2` |
//! | code | a `▏ ` gutter, no reflow, every row painted `code_bg`, the language right-aligned on the first row |
//! | list | `• ` or `n. `, the item's blocks indented under it, two columns per level to four levels |
//! | rule | `─` across the width, in `rule_fg` |
//! | table | aligned columns when they fit, else one `header: cell` line per cell |
//! | image | the picture itself, where there is one; `[image: alt]` in `image_fg` otherwise |
//!
//! A blank line separates one block from the next. A code block is the one
//! thing that is never reflowed: a line of code broken at a space is a line
//! of code that no longer says what it said, so it is cut at the panel's
//! edge instead and the reader widens with `>` if that matters.
//!
//! ## Pictures are rows, and nothing more
//!
//! This module never sees a picture. It is told how big each one turned out
//! to be -- through [`PictureSizes`], which the panel builds from what the
//! core has fetched -- and it reserves that many blank rows and records a
//! [`PictureSlot`] saying where they are. A second pass over the drawn frame
//! puts the pixels there. Keeping it that way is what lets the whole of the
//! reader's arithmetic be tested by reading numbers rather than by looking
//! at a terminal, and it is why a laid-out article can be cached: the rows
//! do not move when the bytes arrive, unless the size they arrive at says
//! they should.
//!
//! A picture nothing knows the size of yet reserves the *whole* box -- the
//! full width, the full cap -- because most article pictures fill it anyway,
//! so the common case moves no rows at all when the bytes land. One that
//! turns out to be small shrinks its box once; one that will never arrive
//! collapses to the `[image: alt]` line once.

use std::collections::HashMap;

use starkit::ratatui::style::{Modifier, Style};
use starkit::ratatui::text::{Line, Span};
use starkit::theme::color::Rgb;
use starkit::wrap::{width_of, wrap_runs};

use super::parse::{Align, Block, Doc, Inline, MAX_LIST_DEPTH};
use crate::ui::panels::rgb;
use crate::ui::theme::Theme;

/// What to lay out against.
pub struct LayoutCtx<'a> {
    pub theme: &'a Theme,
    /// Columns of text. The panel works this out as
    /// `min([reading] width, body - 2)` and centres the result.
    pub width: u16,
    /// What is known about this article's pictures. `None` is every reason
    /// there might be no pictures at all -- `[articles] images` off,
    /// `[ui] graphics = off`, a terminal with no protocol -- and lays every
    /// image out as the `[image: alt]` line 0.0.1 drew.
    pub pictures: Option<&'a PictureSizes<'a>>,
}

/// What the layout is told about this article's pictures.
pub struct PictureSizes<'a> {
    /// The most rows one picture may take: `[reading] image_rows`, or a
    /// third of the reader's body where that is zero.
    pub cap_rows: u16,
    /// How many pixels a terminal cell is, width then height. `(8, 16)` is
    /// the stand-in where nothing has measured one, which is a common font
    /// at a common size and wrong by a little rather than by a lot.
    pub cell: (u16, u16),
    /// What the core has to say about each URL. A URL that is not in here
    /// has not been asked for yet, which is laid out the same way as one
    /// that has been asked for and not arrived.
    pub known: &'a HashMap<String, PictureKnown>,
}

/// How far the core has got with one picture, as far as the layout cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureKnown {
    Loading,
    Failed,
    /// The size it turned out to be, in pixels.
    Natural(u32, u32),
}

/// A picture's rows, and which picture they are for.
///
/// `row` is an index into [`Rendered::lines`] and `col` is a column inside
/// the text, exactly like [`LinkSpan`]: the panel adds its own header rows
/// and scroll before drawing or before answering a click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureSlot {
    pub row: u16,
    pub col: u16,
    pub cols: u16,
    pub rows: u16,
    pub url: String,
    pub alt: String,
}

/// How many cells a picture of `natural` pixels is drawn in.
///
/// One image pixel per terminal pixel where it fits -- `ceil(px / cell)` in
/// each direction, inside both the room available and the cap -- and scaled
/// down by the tighter of the two ratios where it does not. Never up: a
/// 160-pixel logo blown across a third of the reader is not a service.
///
/// `(0, 0)` means "no picture here": no cap, no room, or nothing to draw.
pub fn box_for(natural: (u32, u32), room: u16, cap_rows: u16, cell: (u16, u16)) -> (u16, u16) {
    let (w, h) = natural;
    let (cw, ch) = cell;
    if cap_rows == 0 || room == 0 || w == 0 || h == 0 || cw == 0 || ch == 0 {
        return (0, 0);
    }
    let ceil = |px: u32, cell: u16| -> u32 { px.div_ceil(u32::from(cell)).max(1) };
    let cols = ceil(w, cw);
    let rows = ceil(h, ch);
    if cols <= u32::from(room) && rows <= u32::from(cap_rows) {
        return (cols as u16, rows as u16);
    }
    let scale = (f64::from(room) / cols as f64).min(f64::from(cap_rows) / rows as f64);
    // Floored rather than rounded, and clamped again afterwards: a picture
    // one column wider than the text is a picture that wraps, and a
    // rectangle a row taller than the cap is the cap not meaning anything.
    let fit = |n: u32, limit: u16| -> u16 {
        (((n as f64) * scale).floor().max(1.0) as u32).min(u32::from(limit)) as u16
    };
    (fit(cols, room), fit(rows, cap_rows))
}

/// A run of cells that is a link, and which link it is.
///
/// `row` is an index into [`Rendered::lines`], so the panel adds its own
/// scroll and header rows before comparing it with a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkSpan {
    pub row: u16,
    pub col: u16,
    pub width: u16,
    /// The link's number, counting from one -- an index into
    /// [`Doc::links`](super::parse::Doc::links), and what `o<n>` types.
    pub number: u16,
}

/// One article, measured and styled.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<LinkSpan>,
    /// Where each picture's rows are. Empty when the article has none, and
    /// when pictures are off.
    pub pictures: Vec<PictureSlot>,
    /// `lines.len()`, saturating. What the scrollbar measures and what
    /// `clamp_scrolls` holds the reading position under.
    pub height: u16,
    /// The article as words, with none of the decoration: no gutters, no
    /// link numbers, no padding.
    pub plain: String,
}

impl Rendered {
    /// A rough byte cost, for the cache's budget.
    pub fn weight(&self) -> usize {
        let text: usize = self
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.len() + 24).sum::<usize>())
            .sum();
        let pictures: usize = self
            .pictures
            .iter()
            .map(|p| p.url.len() + p.alt.len() + 48)
            .sum();
        text + self.plain.len() + pictures + 96
    }
}

/// The quote gutter, and the code gutter. Two columns each, which is what
/// every width below is `- 2` for.
const QUOTE_BAR: &str = "\u{258e} ";
const CODE_BAR: &str = "\u{258f} ";
const GUTTER: u16 = 2;

/// Lay out an article.
pub fn layout(doc: &Doc, ctx: &LayoutCtx<'_>) -> Rendered {
    let mut w = Writer::new(ctx.theme, ctx.width, ctx.theme.row_fg);
    w.pictures = ctx.pictures;
    w.blocks(&doc.blocks, 0);
    w.finish()
}

/// A run of text, how it is drawn, and which link it belongs to.
type Run = (String, Style, Option<u16>);

struct Writer<'a> {
    theme: &'a Theme,
    /// Columns available to this writer. A quote's inner writer gets two
    /// fewer, a list item's gets its marker's worth fewer.
    width: u16,
    /// What plain text is drawn in here. The root's is `row_fg`; a quote's
    /// inner writer's is `quote_fg`.
    base_fg: Rgb,
    out: Rendered,
    /// Set once anything has been written, so the blank line a block wants
    /// above it is not the first row of the article.
    started: bool,
    /// Set on the writer laying out one list item's blocks. A list nested
    /// under an item's own text is a tight list -- `- a` with `- b` indented
    /// under it is one thought, and a blank row between the two would read
    /// as two.
    tight: bool,
    /// What is known about the pictures, passed down to every sub-writer so
    /// a picture inside a quote or a list item is still a picture.
    pictures: Option<&'a PictureSizes<'a>>,
}

impl<'a> Writer<'a> {
    fn new(theme: &'a Theme, width: u16, base_fg: Rgb) -> Self {
        Self {
            theme,
            // Zero would make `wrap_runs` divide a row into nothing at all.
            width: width.max(1),
            base_fg,
            out: Rendered::default(),
            started: false,
            tight: false,
            pictures: None,
        }
    }

    fn finish(mut self) -> Rendered {
        // A picture's rows are blank, because the pixels go over them in a
        // second pass -- so the trailing-blank trim has to stop where the
        // last picture ends or an article ending in one loses it.
        let keep = self
            .out
            .pictures
            .iter()
            .map(|p| usize::from(p.row) + usize::from(p.rows))
            .max()
            .unwrap_or(0);
        while self.out.lines.len() > keep
            && self
                .out
                .lines
                .last()
                .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        {
            self.out.lines.pop();
        }
        self.out.height = self.out.lines.len().min(usize::from(u16::MAX)) as u16;
        self.out.plain = self.out.plain.trim_end().to_string();
        self.out
    }

    fn row(&self) -> u16 {
        self.out.lines.len().min(usize::from(u16::MAX)) as u16
    }

    fn base(&self) -> Style {
        Style::default().fg(rgb(self.base_fg))
    }

    fn blank(&mut self) {
        self.out.lines.push(Line::default());
    }

    fn say(&mut self, text: &str) {
        self.out.plain.push_str(text);
        self.out.plain.push('\n');
    }

    /// Lay out a run of blocks, separated by a blank line.
    fn blocks(&mut self, blocks: &[Block], depth: usize) {
        for block in blocks {
            let nested_list = self.tight && matches!(block, Block::List { .. });
            if self.started && !nested_list {
                self.blank();
            }
            self.block(block, depth);
            self.started = true;
        }
    }

    fn block(&mut self, block: &Block, depth: usize) {
        match block {
            Block::Heading { level, inlines } => self.heading(*level, inlines),
            Block::Paragraph(inlines) => {
                let runs = self.runs_of(inlines, self.base());
                self.emit(&runs, self.width, None);
                self.say(&plain_of(inlines));
            }
            Block::Quote(inner) => self.quote(inner, depth),
            Block::Code { lang, text } => self.code(lang.as_deref(), text),
            Block::List { start, items } => self.list(*start, items, depth),
            Block::Rule => {
                let width = usize::from(self.width);
                let style = Style::default().fg(rgb(self.theme.wire.rule_fg));
                self.out
                    .lines
                    .push(Line::from(Span::styled("\u{2500}".repeat(width), style)));
            }
            Block::Table {
                align,
                header,
                rows,
            } => self.table(align, header, rows),
            Block::Image { alt, url } => self.picture(alt, url),
        }
    }

    /// A picture: its rows and a slot, or the line that stands in for one.
    ///
    /// The plain text says `[image: alt]` either way. `y` copies an article
    /// as words, and a picture is not words however it was drawn.
    fn picture(&mut self, alt: &str, url: &str) {
        let text = if alt.trim().is_empty() {
            // A picture with no alt text at all is commoner in a real
            // article than one with any -- a decorative header image,
            // usually -- and `[image: ]` reads as a fault where `[image]`
            // reads as a picture nobody described.
            "[image]".to_string()
        } else {
            format!("[image: {alt}]")
        };
        let (cols, rows) = self.picture_box(url);
        if cols == 0 || rows == 0 {
            let style = Style::default().fg(rgb(self.theme.wire.image_fg));
            self.emit(&[(text.clone(), style, None)], self.width, None);
            self.say(&text);
            return;
        }
        self.out.pictures.push(PictureSlot {
            row: self.row(),
            col: 0,
            cols,
            rows,
            url: url.to_string(),
            alt: alt.to_string(),
        });
        for _ in 0..rows {
            self.blank();
        }
        self.say(&text);
    }

    /// How many cells this picture gets: its own shape where that is known,
    /// the whole box while it is on its way, and nothing at all where it
    /// failed or where there are no pictures.
    fn picture_box(&self, url: &str) -> (u16, u16) {
        let Some(sizes) = self.pictures else {
            return (0, 0);
        };
        if url.is_empty() {
            return (0, 0);
        }
        let room = self.width;
        match sizes.known.get(url) {
            Some(PictureKnown::Failed) => (0, 0),
            Some(PictureKnown::Natural(w, h)) => {
                box_for((*w, *h), room, sizes.cap_rows, sizes.cell)
            }
            // On its way, or not asked for yet. The full box, because most
            // article pictures fill it and the rows then do not move when
            // the bytes land.
            _ => (room, sizes.cap_rows),
        }
    }

    fn heading(&mut self, level: u8, inlines: &[Inline]) {
        // A blank above the two loudest headings, on top of the one every
        // block already gets: a section break is worth a row of its own.
        if level <= 2 && self.started {
            self.blank();
        }
        let mut style = Style::default().fg(rgb(self.theme.wire.heading_fg));
        if level <= 2 {
            style = style.add_modifier(Modifier::BOLD);
        }
        let runs = self.runs_of(inlines, style);
        self.emit(&runs, self.width, None);
        self.say(&plain_of(inlines));
    }

    /// A quote is its own little article, two columns narrower, with a bar
    /// down the left of every row it produced. Laying it out separately is
    /// what lets a quote hold a list or a code block without this function
    /// knowing about either.
    fn quote(&mut self, inner: &[Block], depth: usize) {
        let width = self.width.saturating_sub(GUTTER).max(1);
        let mut sub = Writer::new(self.theme, width, self.theme.wire.quote_fg);
        sub.pictures = self.pictures;
        sub.blocks(inner, depth);
        let rendered = sub.finish();
        let bar = Style::default().fg(rgb(self.theme.wire.quote_fg));
        self.splice(rendered, Span::styled(QUOTE_BAR, bar), GUTTER);
    }

    /// Fenced or indented code. Never reflowed -- see the module doc -- and
    /// every row painted to the full width so the block reads as one shape
    /// rather than as a ragged edge.
    fn code(&mut self, lang: Option<&str>, text: &str) {
        let width = self.width.saturating_sub(GUTTER).max(1);
        let bar = Style::default().fg(rgb(self.theme.dim));
        let body = Style::default()
            .fg(rgb(self.theme.row_fg))
            .bg(rgb(self.theme.wire.code_bg));
        let dim_on_code = Style::default()
            .fg(rgb(self.theme.dim))
            .bg(rgb(self.theme.wire.code_bg));

        for (i, raw) in text.lines().enumerate() {
            let cut = clip(raw, width);
            let mut spans = vec![Span::styled(CODE_BAR, bar)];
            // The language sits on the first row, right-aligned, where the
            // padding would otherwise be. It is dropped rather than
            // overlapping when the code reaches that far.
            let tag = lang.filter(|_| i == 0).unwrap_or("");
            let used = width_of(&cut);
            let tag_width = width_of(tag);
            if !tag.is_empty() && used + tag_width < width {
                let pad = width - used - tag_width;
                spans.push(Span::styled(cut, body));
                spans.push(Span::styled(" ".repeat(usize::from(pad)), body));
                spans.push(Span::styled(tag.to_string(), dim_on_code));
            } else {
                let pad = width.saturating_sub(used);
                spans.push(Span::styled(
                    format!("{cut}{}", " ".repeat(usize::from(pad))),
                    body,
                ));
            }
            self.out.lines.push(Line::from(spans));
            self.say(raw);
        }
    }

    fn list(&mut self, start: Option<u64>, items: &[Vec<Block>], depth: usize) {
        // Two columns a level, four levels deep. Past that a list is
        // narrower than the words in it, so it stops indenting and keeps
        // drawing its markers.
        let indenting = depth < MAX_LIST_DEPTH;
        let marker_style = Style::default().fg(rgb(self.base_fg));

        for (i, item) in items.iter().enumerate() {
            let marker = match start {
                Some(n) => format!("{}. ", n + i as u64),
                None => "\u{2022} ".to_string(),
            };
            let indent = if indenting { width_of(&marker) } else { 0 };
            let width = self.width.saturating_sub(indent).max(1);

            let mut sub = Writer::new(self.theme, width, self.base_fg);
            sub.pictures = self.pictures;
            sub.tight = true;
            sub.blocks(item, depth + 1);
            let rendered = sub.finish();

            let lead = Span::styled(marker.clone(), marker_style);
            let pad = Span::raw(" ".repeat(usize::from(indent)));
            self.splice_first(rendered, lead, pad, indent);
        }
    }

    fn table(&mut self, align: &[Align], header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>]) {
        let cols = header
            .len()
            .max(rows.iter().map(Vec::len).max().unwrap_or(0));
        if cols == 0 {
            return;
        }

        // The natural width of each column, and whether they and the two
        // spaces between each pair fit.
        let mut natural = vec![0u16; cols];
        for (c, widest) in natural.iter_mut().enumerate() {
            *widest = header.get(c).map(|i| cell_width(i)).unwrap_or(0);
            for row in rows {
                *widest = (*widest).max(row.get(c).map(|i| cell_width(i)).unwrap_or(0));
            }
        }
        let total: u32 = natural.iter().map(|w| u32::from(*w)).sum::<u32>() + 2 * (cols as u32 - 1);

        if total <= u32::from(self.width) {
            self.aligned_table(align, header, rows, &natural, cols);
        } else {
            self.listed_table(header, rows, cols);
        }
    }

    fn aligned_table(
        &mut self,
        align: &[Align],
        header: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
        natural: &[u16],
        cols: usize,
    ) {
        let head_style = self.base().add_modifier(Modifier::BOLD);
        self.table_row(header, natural, align, cols, head_style);
        self.say(&row_plain(header));

        let rule = natural.iter().map(|w| usize::from(*w)).sum::<usize>() + 2 * (cols - 1);
        self.out.lines.push(Line::from(Span::styled(
            "\u{2500}".repeat(rule),
            Style::default().fg(rgb(self.theme.wire.rule_fg)),
        )));

        for row in rows {
            let base = self.base();
            self.table_row(row, natural, align, cols, base);
            self.say(&row_plain(row));
        }
    }

    fn table_row(
        &mut self,
        cells: &[Vec<Inline>],
        natural: &[u16],
        align: &[Align],
        cols: usize,
        base: Style,
    ) {
        let empty: Vec<Inline> = Vec::new();
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut col = 0u16;
        let row = self.row();
        for (c, want) in natural.iter().copied().enumerate().take(cols) {
            if c > 0 {
                spans.push(Span::raw("  "));
                col += 2;
            }
            let inlines = cells.get(c).unwrap_or(&empty);
            let runs = self.runs_of(inlines, base);
            let have = cell_width(inlines).min(want);
            let (before, after) = match align.get(c).copied().unwrap_or(Align::Left) {
                Align::Left => (0, want - have),
                Align::Right => (want - have, 0),
                Align::Centre => {
                    let gap = want - have;
                    (gap / 2, gap - gap / 2)
                }
            };
            if before > 0 {
                spans.push(Span::raw(" ".repeat(usize::from(before))));
                col += before;
            }
            self.clip_runs(&runs, want, row, &mut col, &mut spans);
            if after > 0 {
                spans.push(Span::raw(" ".repeat(usize::from(after))));
                col += after;
            }
        }
        self.out.lines.push(Line::from(spans));
    }

    /// A table too wide for the panel, one cell per line: `header: cell`.
    /// Not a truncated table -- a column cut to four characters is a column
    /// nobody can read -- and not a horizontally scrolled one, which would
    /// be a second kind of scrolling in a reader that has one.
    fn listed_table(&mut self, header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>], cols: usize) {
        let empty: Vec<Inline> = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            if r > 0 {
                self.blank();
            }
            for c in 0..cols {
                let head = header.get(c).unwrap_or(&empty);
                let cell = row.get(c).unwrap_or(&empty);
                let mut runs: Vec<Run> = Vec::new();
                if !head.is_empty() {
                    runs.push((
                        format!("{}: ", plain_of(head)),
                        self.base().add_modifier(Modifier::BOLD),
                        None,
                    ));
                }
                runs.extend(self.runs_of(cell, self.base()));
                self.emit(&runs, self.width, None);
                self.say(&format!("{}: {}", plain_of(head), plain_of(cell)));
            }
        }
    }

    /// Turn styled runs into rows, recording a [`LinkSpan`] for every row a
    /// link lands on. `lead` is a gutter drawn at the start of every row,
    /// already counted out of `width` by the caller.
    fn emit(&mut self, runs: &[Run], width: u16, lead: Option<Span<'static>>) {
        let width = width.max(1);
        let strs: Vec<&str> = runs.iter().map(|(t, _, _)| t.as_str()).collect();
        if strs.iter().all(|s| s.is_empty()) {
            return;
        }
        for row in wrap_runs(&strs, width) {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut col = 0u16;
            if let Some(lead) = &lead {
                col += width_of(&lead.content);
                spans.push(lead.clone());
            }
            let at = self.row();
            for piece in row.drawn(&strs) {
                let text = piece.slice(&strs);
                if text.is_empty() {
                    continue;
                }
                let (_, style, link) = &runs[piece.run];
                let w = width_of(text);
                if let Some(number) = link {
                    self.out.links.push(LinkSpan {
                        row: at,
                        col,
                        width: w,
                        number: *number,
                    });
                }
                spans.push(Span::styled(text.to_string(), *style));
                col += w;
            }
            self.out.lines.push(Line::from(spans));
        }
    }

    /// Draw runs on one row, stopping at `limit` columns. For a table cell,
    /// which is measured rather than wrapped.
    fn clip_runs(
        &mut self,
        runs: &[Run],
        limit: u16,
        row: u16,
        col: &mut u16,
        spans: &mut Vec<Span<'static>>,
    ) {
        let mut used = 0u16;
        for (text, style, link) in runs {
            if used >= limit {
                break;
            }
            let cut = clip(text, limit - used);
            if cut.is_empty() {
                continue;
            }
            let w = width_of(&cut);
            if let Some(number) = link {
                self.out.links.push(LinkSpan {
                    row,
                    col: *col,
                    width: w,
                    number: *number,
                });
            }
            spans.push(Span::styled(cut, *style));
            used += w;
            *col += w;
        }
    }

    /// Take a sub-writer's rows, put `lead` in front of each one, and add
    /// them to this writer's -- shifting the link spans it recorded by the
    /// rows above them and by the gutter's width.
    fn splice(&mut self, rendered: Rendered, lead: Span<'static>, indent: u16) {
        let offset = self.row();
        for line in rendered.lines {
            let mut spans = vec![lead.clone()];
            spans.extend(line.spans);
            self.out.lines.push(Line::from(spans));
        }
        for mut span in rendered.links {
            span.row += offset;
            span.col += indent;
            self.out.links.push(span);
        }
        for mut slot in rendered.pictures {
            slot.row += offset;
            slot.col += indent;
            self.out.pictures.push(slot);
        }
        self.out.plain.push_str(&rendered.plain);
        self.out.plain.push('\n');
    }

    /// The same, with a different lead on the first row: a list marker, then
    /// spaces under it.
    fn splice_first(
        &mut self,
        rendered: Rendered,
        first: Span<'static>,
        rest: Span<'static>,
        indent: u16,
    ) {
        let offset = self.row();
        for (i, line) in rendered.lines.into_iter().enumerate() {
            let mut spans = vec![if i == 0 { first.clone() } else { rest.clone() }];
            spans.extend(line.spans);
            self.out.lines.push(Line::from(spans));
        }
        for mut span in rendered.links {
            span.row += offset;
            span.col += indent;
            self.out.links.push(span);
        }
        for mut slot in rendered.pictures {
            slot.row += offset;
            slot.col += indent;
            self.out.pictures.push(slot);
        }
        self.out.plain.push_str(&rendered.plain);
        self.out.plain.push('\n');
    }

    /// The styled runs one sequence of inlines makes, with a link's number
    /// appended after it -- ` [3]` -- so that a reader can see what to type.
    fn runs_of(&self, inlines: &[Inline], base: Style) -> Vec<Run> {
        let mut out: Vec<Run> = Vec::new();
        let mut open: Option<u16> = None;
        for inline in inlines {
            if open.is_some() && open != inline.style.link {
                self.close_link(&mut out, open);
            }
            open = inline.style.link;
            let mut style = base;
            if inline.style.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if inline.style.italic {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if inline.style.strike {
                style = style.add_modifier(Modifier::CROSSED_OUT);
            }
            if inline.style.code {
                style = style
                    .fg(rgb(self.theme.row_fg))
                    .bg(rgb(self.theme.wire.code_bg));
            }
            if inline.style.link.is_some() {
                style = style
                    .fg(rgb(self.theme.wire.link_fg))
                    .add_modifier(Modifier::UNDERLINED);
            }
            out.push((inline.text.clone(), style, inline.style.link));
        }
        self.close_link(&mut out, open);
        out
    }

    fn close_link(&self, out: &mut Vec<Run>, open: Option<u16>) {
        if let Some(n) = open {
            out.push((
                format!(" [{n}]"),
                Style::default().fg(rgb(self.theme.wire.link_fg)),
                None,
            ));
        }
    }
}

/// Cut to `width` columns, and no padding.
///
/// Not `starkit::text::fit`, which pads to exactly the width it is given:
/// that is right for a whole row and wrong for a piece of one, and a padded
/// piece is how a table cell comes out eighteen columns wider than the
/// column it was measured for.
fn clip(text: &str, width: u16) -> String {
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

/// A cell's width, link numbers included: what is measured is what is drawn.
fn cell_width(inlines: &[Inline]) -> u16 {
    let mut w = width_of(&plain_of(inlines));
    let mut open: Option<u16> = None;
    for inline in inlines {
        if let Some(n) = open {
            if open != inline.style.link {
                w += width_of(&format!(" [{n}]"));
            }
        }
        open = inline.style.link;
    }
    if let Some(n) = open {
        w += width_of(&format!(" [{n}]"));
    }
    w
}

fn plain_of(inlines: &[Inline]) -> String {
    inlines.iter().map(|i| i.text.as_str()).collect()
}

fn row_plain(cells: &[Vec<Inline>]) -> String {
    cells
        .iter()
        .map(|c| plain_of(c))
        .collect::<Vec<_>>()
        .join("  ")
}

#[cfg(test)]
mod tests {
    use super::super::parse::parse;
    use super::super::FIXTURE_MD;
    use super::*;
    use crate::ui::theme::tests_support::theme;
    use proptest::prelude::*;

    fn render(md: &str, width: u16) -> Rendered {
        let theme = theme("catppuccin-mocha");
        let doc = parse(md);
        layout(
            &doc,
            &LayoutCtx {
                theme: &theme,
                width,
                pictures: None,
            },
        )
    }

    fn drawn(r: &Rendered) -> Vec<String> {
        r.lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn row_width(line: &Line<'_>) -> u16 {
        line.spans.iter().map(|s| width_of(&s.content)).sum()
    }

    /// The property the panel depends on: nothing ever overruns the width it
    /// was laid out for, at any width, for any block in the fixture.
    #[test]
    fn wrapping_at_40_and_80_never_exceeds_the_width() {
        for width in [40u16, 60, 80, 100] {
            let r = render(FIXTURE_MD, width);
            for (i, line) in r.lines.iter().enumerate() {
                assert!(
                    row_width(line) <= width,
                    "row {i} is {} columns at width {width}: {:?}",
                    row_width(line),
                    drawn(&r)[i]
                );
            }
        }
    }

    #[test]
    fn the_height_is_the_number_of_lines() {
        let r = render(FIXTURE_MD, 80);
        assert_eq!(usize::from(r.height), r.lines.len());
        assert!(r.height > 20, "the fixture is longer than that");
    }

    #[test]
    fn a_heading_is_bold_at_the_top_two_levels_and_coloured_at_all_of_them() {
        let t = theme("catppuccin-mocha");
        let r = render("# one\n\n### three\n", 40);
        let rows = drawn(&r);
        let h1 = rows.iter().position(|l| l.contains("one")).unwrap();
        let h3 = rows.iter().position(|l| l.contains("three")).unwrap();
        let s1 = r.lines[h1].spans[0].style;
        let s3 = r.lines[h3].spans[0].style;
        assert!(s1.add_modifier.contains(Modifier::BOLD));
        assert!(!s3.add_modifier.contains(Modifier::BOLD));
        assert_eq!(s1.fg, Some(rgb(t.wire.heading_fg)));
        assert_eq!(s3.fg, Some(rgb(t.wire.heading_fg)));
    }

    #[test]
    fn a_link_is_underlined_and_followed_by_its_number() {
        let t = theme("catppuccin-mocha");
        let r = render("see [the docs](https://example.org/) now", 60);
        let rows = drawn(&r);
        assert!(rows[0].contains("the docs [1]"), "{rows:?}");
        let span = r.lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("the docs"))
            .unwrap();
        assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(span.style.fg, Some(rgb(t.wire.link_fg)));
        assert_eq!(r.links.len(), 1);
        assert_eq!(r.links[0].number, 1);
    }

    /// A link that wraps is one span per row it lands on, each covering only
    /// the cells that row drew: a click on the second half of a wrapped link
    /// still opens it, and a click past its end does not.
    #[test]
    fn a_link_that_wraps_records_one_span_per_row() {
        let md = "[a very long link label that will certainly not fit on one \
                  row of a narrow reader](https://example.org/)";
        let r = render(md, 24);
        assert!(r.links.len() >= 2, "{:?}", r.links);
        let rows: Vec<u16> = r.links.iter().map(|l| l.row).collect();
        let mut sorted = rows.clone();
        sorted.dedup();
        assert_eq!(rows, sorted, "two spans landed on one row");
        for span in &r.links {
            assert_eq!(span.number, 1);
            let line = &r.lines[usize::from(span.row)];
            assert!(
                span.col + span.width <= row_width(line),
                "{span:?} runs past its row"
            );
        }
    }

    #[test]
    fn inline_code_is_drawn_on_the_code_background() {
        let t = theme("catppuccin-mocha");
        let r = render("a `fn main()` b", 40);
        let span = r.lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("fn main"))
            .unwrap();
        assert_eq!(span.style.bg, Some(rgb(t.wire.code_bg)));
    }

    /// A line of code broken at a space is a line of code that no longer
    /// says what it said. It is cut instead.
    #[test]
    fn code_blocks_are_not_reflowed() {
        let long = "let x = some_function(with, several, arguments, that, go, on);";
        let md = format!("```rust\n{long}\n```\n");
        let r = render(&md, 30);
        let rows = drawn(&r);
        assert_eq!(rows.len(), 1, "one source line is one row: {rows:?}");
        assert!(rows[0].starts_with(CODE_BAR));
        assert_eq!(row_width(&r.lines[0]), 30, "the row is painted to the edge");
        assert!(!rows[0].contains("on);"), "the tail was cut, not wrapped");
    }

    #[test]
    fn a_fenced_block_says_its_language_on_the_first_row() {
        let r = render("```rust\nlet x = 1;\n```\n", 40);
        let rows = drawn(&r);
        assert!(rows[0].trim_end().ends_with("rust"), "{rows:?}");
    }

    #[test]
    fn a_quote_has_a_bar_down_its_left() {
        let r = render("> quoted words here\n", 40);
        for row in drawn(&r) {
            assert!(row.starts_with(QUOTE_BAR), "{row:?}");
        }
    }

    /// A quote is laid out as its own article, so a list inside one works
    /// without this knowing there is a list inside one.
    #[test]
    fn a_quote_can_hold_a_list() {
        let r = render("> - one\n> - two\n", 40);
        let rows = drawn(&r);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            rows[0].starts_with(&format!("{QUOTE_BAR}\u{2022} ")),
            "{rows:?}"
        );
    }

    #[test]
    fn a_bulleted_list_is_marked_and_an_ordered_one_is_numbered() {
        let r = render("- one\n- two\n", 40);
        let rows = drawn(&r);
        assert!(rows[0].starts_with("\u{2022} one"), "{rows:?}");

        let r = render("3. three\n4. four\n", 40);
        let rows = drawn(&r);
        assert!(rows[0].starts_with("3. three"), "{rows:?}");
        assert!(rows.iter().any(|r| r.starts_with("4. four")), "{rows:?}");
    }

    #[test]
    fn a_nested_list_indents_under_its_parent() {
        let r = render("- one\n  - inner\n", 40);
        let rows = drawn(&r);
        let inner = rows.iter().find(|l| l.contains("inner")).unwrap();
        assert!(inner.starts_with("  \u{2022} inner"), "{inner:?}");
    }

    /// Past four levels the indent stops rather than squeezing the words out
    /// of the panel.
    #[test]
    fn nesting_stops_indenting_at_four_levels() {
        let md = "- a\n  - b\n    - c\n      - d\n        - e\n          - f\n";
        let r = render(md, 40);
        let rows = drawn(&r);
        let at = |needle: &str| {
            rows.iter()
                .find(|l| l.contains(needle))
                .map(|l| l.len() - l.trim_start().len())
                .unwrap()
        };
        assert_eq!(at("d"), 6);
        assert_eq!(at("e"), 8, "the fourth level is the last that indents");
        assert_eq!(at("f"), 8, "and the fifth sits where the fourth does");
    }

    #[test]
    fn a_rule_is_the_full_width() {
        let r = render("---\n", 33);
        assert_eq!(row_width(&r.lines[0]), 33);
        assert!(drawn(&r)[0].chars().all(|c| c == '\u{2500}'));
    }

    #[test]
    fn a_table_that_fits_is_drawn_as_columns() {
        let md = "| a | b |\n| --- | ---: |\n| one | two |\n";
        let r = render(md, 40);
        let rows = drawn(&r);
        assert!(rows[0].starts_with("a "), "{rows:?}");
        assert!(rows[1].chars().all(|c| c == '\u{2500}'), "{rows:?}");
        assert!(rows[2].contains("one"), "{rows:?}");
        assert!(rows[2].ends_with("two"), "right-aligned: {rows:?}");
    }

    #[test]
    fn a_table_that_does_not_fit_becomes_one_line_per_cell() {
        let md = "| heading one | heading two |\n| --- | --- |\n\
                  | a rather long value | another rather long value |\n";
        let r = render(md, 24);
        let rows = drawn(&r);
        assert!(
            rows.iter().any(|l| l.starts_with("heading one: ")),
            "{rows:?}"
        );
        for line in &r.lines {
            assert!(row_width(line) <= 24);
        }
    }

    #[test]
    fn an_image_with_no_alt_text_is_just_a_picture() {
        let r = render("![](https://example.org/header.png)\n", 40);
        assert_eq!(drawn(&r)[0], "[image]");
    }

    #[test]
    fn an_image_is_its_alt_text_in_the_image_colour() {
        let t = theme("catppuccin-mocha");
        let r = render("![a diagram](x.png)\n", 40);
        assert_eq!(drawn(&r)[0], "[image: a diagram]");
        assert_eq!(r.lines[0].spans[0].style.fg, Some(rgb(t.wire.image_fg)));
    }

    // ------------------------------------------------------ pictures ----

    fn sizes<'a>(known: &'a HashMap<String, PictureKnown>, cap_rows: u16) -> PictureSizes<'a> {
        PictureSizes {
            cap_rows,
            cell: (8, 16),
            known,
        }
    }

    fn with_pictures(md: &str, width: u16, sizes: &PictureSizes<'_>) -> Rendered {
        let theme = theme("catppuccin-mocha");
        layout(
            &parse(md),
            &LayoutCtx {
                theme: &theme,
                width,
                pictures: Some(sizes),
            },
        )
    }

    /// The whole of the sizing rule, at a cell of eight by sixteen -- which
    /// is a common font at a common size, and the stand-in used where a
    /// terminal never measured one.
    #[test]
    fn a_picture_takes_its_own_cells_until_it_cannot() {
        let cell = (8, 16);
        // 64x32 pixels is exactly eight columns and two rows.
        assert_eq!(box_for((64, 32), 80, 12, cell), (8, 2));
        // A pixel over is another cell: the rectangle has to cover it.
        assert_eq!(box_for((65, 33), 80, 12, cell), (9, 3));

        // A banner, wider than the text: fitted to the width, and the rows
        // follow it down.
        assert_eq!(box_for((1600, 100), 80, 12, cell), (80, 2));
        // A tall one: the cap binds, and the columns follow it in.
        assert_eq!(box_for((800, 1600), 80, 12, cell), (12, 12));
        // A photograph wide enough to want more rows than the cap allows:
        // the cap is what binds, not the width.
        assert_eq!(box_for((1600, 800), 80, 12, cell), (48, 12));
        // Whichever binds, neither bound is ever exceeded.
        for natural in [(4000, 4000), (9000, 40), (40, 9000), (1, 1)] {
            let (cols, rows) = box_for(natural, 80, 12, cell);
            assert!(cols <= 80 && rows <= 12, "{natural:?}: {cols}x{rows}");
            assert!(cols >= 1 && rows >= 1, "{natural:?}: {cols}x{rows}");
        }

        // Never up. A small picture keeps its own size however much room
        // there is.
        assert_eq!(box_for((16, 16), 200, 40, cell), (2, 1));

        // And nothing at all where there is nowhere to put one.
        assert_eq!(box_for((64, 32), 80, 0, cell), (0, 0), "no cap");
        assert_eq!(box_for((64, 32), 0, 12, cell), (0, 0), "no room");
        assert_eq!(box_for((0, 0), 80, 12, cell), (0, 0), "no picture");
        assert_eq!(box_for((64, 32), 80, 12, (0, 0)), (0, 0), "no cell");
    }

    /// A picture whose size is known gets exactly those rows, and the slot
    /// says where they are. The plain text still says `[image: alt]`,
    /// because `y` copies words and a picture is not words.
    #[test]
    fn a_known_picture_reserves_its_own_rows() {
        let mut known = HashMap::new();
        known.insert("p.png".to_string(), PictureKnown::Natural(64, 32));
        let r = with_pictures(
            "one

![a diagram](p.png)

two
",
            40,
            &sizes(&known, 12),
        );

        assert_eq!(r.pictures.len(), 1);
        let slot = &r.pictures[0];
        assert_eq!((slot.cols, slot.rows), (8, 2));
        assert_eq!(slot.col, 0, "at the left edge of the text");
        assert_eq!(slot.url, "p.png");
        assert_eq!(slot.alt, "a diagram");
        assert_eq!(drawn(&r)[slot.row as usize], "", "its rows are blank");
        assert_eq!(
            drawn(&r)[usize::from(slot.row) + usize::from(slot.rows) - 1],
            ""
        );
        assert_eq!(
            r.plain,
            "one
[image: a diagram]
two"
        );
    }

    /// Before anything is known a picture takes the whole box, so the rows
    /// do not move when the bytes land at the size most of them land at.
    #[test]
    fn a_loading_picture_reserves_the_whole_box() {
        let mut known = HashMap::new();
        let r = with_pictures(
            "![a](p.png)
",
            40,
            &sizes(&known, 9),
        );
        assert_eq!(r.pictures.len(), 1);
        assert_eq!((r.pictures[0].cols, r.pictures[0].rows), (40, 9));
        assert_eq!(r.height, 9, "nine blank rows, and the trim left them");

        // Explicitly loading is the same answer as never asked for.
        known.insert("p.png".to_string(), PictureKnown::Loading);
        let again = with_pictures(
            "![a](p.png)
",
            40,
            &sizes(&known, 9),
        );
        assert_eq!(again.pictures, r.pictures);
    }

    /// One that will never arrive collapses to the line 0.0.1 drew, once.
    #[test]
    fn a_failed_picture_is_the_alt_line_again() {
        let mut known = HashMap::new();
        known.insert("p.png".to_string(), PictureKnown::Failed);
        let r = with_pictures(
            "![a diagram](p.png)
",
            40,
            &sizes(&known, 12),
        );
        assert!(r.pictures.is_empty());
        assert_eq!(drawn(&r)[0], "[image: a diagram]");
    }

    /// With no sizes at all -- pictures off, graphics off, a terminal with
    /// no protocol -- every image is the line it always was, and the
    /// existing snapshots are unchanged.
    #[test]
    fn no_sizes_is_the_line_this_release_started_with() {
        let r = render(
            "![a diagram](p.png)
",
            40,
        );
        assert!(r.pictures.is_empty());
        assert_eq!(drawn(&r)[0], "[image: a diagram]");
    }

    /// A picture inside a quote is spliced like the text around it: the
    /// gutter moves its rows down and its columns across, so the second
    /// pass draws it beside the bar rather than over it.
    #[test]
    fn a_picture_inside_a_quote_is_offset_by_the_gutter() {
        let mut known = HashMap::new();
        known.insert("p.png".to_string(), PictureKnown::Natural(64, 32));
        let r = with_pictures(
            "lead in\n\n> quoted\n>\n> ![a](p.png)\n",
            40,
            &sizes(&known, 12),
        );
        assert_eq!(r.pictures.len(), 1, "{:?}", drawn(&r));
        let slot = &r.pictures[0];
        assert_eq!(slot.col, GUTTER, "the quote bar is two columns");
        assert!(slot.row >= 2, "below the lead-in and the quote's first row");
        assert!(
            drawn(&r)[slot.row as usize].starts_with(QUOTE_BAR),
            "{:?}",
            drawn(&r)[slot.row as usize]
        );
    }

    /// A picture with no address cannot be fetched, so it is the line
    /// instead -- which is what an image inside a heading or a table cell
    /// has already become by the time this sees it.
    #[test]
    fn a_picture_with_no_address_is_the_line() {
        let known = HashMap::new();
        let r = with_pictures("![a](<>)\n", 40, &sizes(&known, 12));
        assert!(r.pictures.is_empty(), "{:?}", r.pictures);
    }

    #[test]
    fn the_plain_text_is_the_words_without_the_decoration() {
        let r = render("# Title\n\nSome **words** here.\n", 40);
        assert_eq!(r.plain, "Title\nSome words here.");
    }

    #[test]
    fn a_blank_line_separates_the_blocks() {
        let r = render("one\n\ntwo\n", 40);
        let rows = drawn(&r);
        assert_eq!(
            rows,
            vec!["one".to_string(), String::new(), "two".to_string()]
        );
    }

    /// Every block treatment at once, at the width the reader defaults to.
    #[test]
    fn the_fixture_lays_out_every_block_it_has() {
        let r = render(FIXTURE_MD, 80);
        let rows = drawn(&r);
        let has = |needle: &str| rows.iter().any(|l| l.contains(needle));
        assert!(has("Why the borrow checker"), "the heading");
        assert!(has("nomicon [1]"), "a numbered link");
        assert!(has("\u{2022} Every value"), "a bullet");
        assert!(has("1. Name the lifetime"), "an ordered item");
        assert!(has(CODE_BAR), "a code gutter");
        assert!(has(QUOTE_BAR), "a quote gutter");
        assert!(has("[image: a borrow checker diagram]"), "the image");
        assert!(has("[^1]"), "the footnote");
        assert_eq!(r.links.iter().map(|l| l.number).max(), Some(3));
    }

    proptest! {
        /// Whatever a page turns out to be, laying it out does not panic,
        /// never draws past the width, and never records a link the document
        /// does not have.
        #[test]
        fn any_document_lays_out_within_its_width(
            text in ".{0,1500}",
            width in 10u16..120,
        ) {
            let theme = theme("terminal");
            let doc = parse(&text);
            let r = layout(&doc, &LayoutCtx {
                    theme: &theme,
                    width,
                    pictures: None,
                });
            prop_assert_eq!(usize::from(r.height), r.lines.len());
            for line in &r.lines {
                prop_assert!(row_width(line) <= width);
            }
            for span in &r.links {
                prop_assert!(span.number >= 1);
                prop_assert!(usize::from(span.number) <= doc.links.len());
                prop_assert!(usize::from(span.row) < r.lines.len());
            }
        }
    }
}
