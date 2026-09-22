//! CommonMark in, a [`Doc`] out. Pure, and it has never heard of a theme.
//!
//! The core stores an extracted article as markdown and nothing else, so
//! this is the first half of the reader: the events `pulldown-cmark` yields,
//! walked once into a tree of [`Block`]s the layout can measure. Nothing
//! here knows a width or a colour, which is what makes it the half that can
//! be tested by reading its output rather than by looking at a terminal.
//!
//! ## What is kept and what is thrown away
//!
//! Three extensions are on: tables, strikethrough and footnotes. They are
//! on because `wire::extract` emits all three -- a table in a page becomes a
//! table, and the footnotes are how a long-form article's references
//! survive. Raw HTML is dropped outright: it arrives here only when the
//! extractor could not turn a tag into markdown, and a terminal that printed
//! `<div class="foo">` at the reader would be worse than one that printed
//! nothing.
//!
//! A soft break is a space, because a feed's own line breaks are an artefact
//! of whatever editor wrote it and the reader is rewrapping anyway. A hard
//! break -- two spaces or a backslash, which somebody meant -- is a `\n`,
//! and the wrapper honours it.
//!
//! ## Link numbers
//!
//! Every link gets a number in document order, and the number is what `o<n>`
//! opens. Document order rather than reading order is the point: the number
//! beside a link must not change when the panel is resized, or the number a
//! reader has just seen and is about to type would be somebody else's by the
//! time they typed it. A link whose text *is* its URL still gets one, so
//! that `o3` works on it too.
//!
//! ## Pictures
//!
//! An image becomes a [`Block::Image`] carrying **both** its alt text and
//! the address it is at, and an image in the middle of a paragraph splits
//! that paragraph in two rather than being reordered around it. Inside a
//! heading or a table cell, where a block cannot go, it becomes the
//! `[image: alt]` text inline and the address is dropped -- there is no row
//! to put a picture on there.
//!
//! An image is never a numbered link, even where the markdown wraps one in a
//! link: the numbers are for things `o<n>` opens in a browser, and a picture
//! is opened by clicking it.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A parsed article.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doc {
    pub blocks: Vec<Block>,
    /// Every link's destination, in document order. A link's number is its
    /// index here plus one.
    pub links: Vec<String>,
}

/// How a table column is aligned. British spelling, as everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Centre,
    Right,
}

/// One block of an article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        /// 1 to 6.
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    Quote(Vec<Block>),
    Code {
        lang: Option<String>,
        text: String,
    },
    List {
        /// `Some(n)` for an ordered list starting at n, `None` for a bulleted
        /// one.
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Rule,
    Table {
        align: Vec<Align>,
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    /// A picture: what it is of, and where it is. See the module doc.
    Image {
        alt: String,
        url: String,
    },
}

/// A run of text and how it is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub style: InlineStyle,
}

/// What a run of text is. Nothing here is a colour: `code` and `link` say
/// *what* the run is, and `markdown::layout` decides what that looks like in
/// the theme it was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// The link's number, counting from one. See the module doc.
    pub link: Option<u16>,
}

/// The extensions the extractor emits, and no others.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_FOOTNOTES
}

/// Parse an article.
///
/// Never fails and never panics: the input is a page somebody else
/// published, reduced to markdown by a converter that is allowed to be
/// imperfect. Anything this cannot make sense of comes out as text.
pub fn parse(text: &str) -> Doc {
    let mut b = Builder::new();
    for event in Parser::new_ext(text, options()) {
        b.event(event);
    }
    b.finish()
}

/// How deep a nested list is allowed to indent. Four levels is eight
/// columns, which is as much as a forty-column reader can give up before the
/// list is narrower than the words in it.
pub const MAX_LIST_DEPTH: usize = 4;

/// The open list at each level, while one is being built.
#[derive(Debug)]
struct ListFrame {
    start: Option<u64>,
    items: Vec<Vec<Block>>,
}

/// A table, while it is being built.
#[derive(Debug, Default)]
struct TableFrame {
    align: Vec<Align>,
    header: Vec<Vec<Inline>>,
    rows: Vec<Vec<Vec<Inline>>>,
    row: Vec<Vec<Inline>>,
    in_head: bool,
}

/// The one mutable thing in this module.
struct Builder {
    /// A stack of block collectors: the root, then one per open quote, list
    /// item or footnote definition. Never empty.
    blocks: Vec<Vec<Block>>,
    lists: Vec<ListFrame>,
    table: Option<TableFrame>,
    /// The paragraph, heading or table cell being collected.
    inlines: Vec<Inline>,
    heading: Option<u8>,
    /// `Some` while a fenced or indented code block is open.
    code: Option<(Option<String>, String)>,
    /// `Some` while an image is open: where it is, and the alt text being
    /// collected for it.
    image: Option<(String, String)>,
    /// The label of the footnote definition being collected, if any.
    footnote: Vec<String>,
    bold: u32,
    italic: u32,
    strike: u32,
    inline_code: bool,
    link: Option<u16>,
    links: Vec<String>,
}

impl Builder {
    fn new() -> Self {
        Self {
            blocks: vec![Vec::new()],
            lists: Vec::new(),
            table: None,
            inlines: Vec::new(),
            heading: None,
            code: None,
            image: None,
            footnote: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            inline_code: false,
            link: None,
            links: Vec::new(),
        }
    }

    fn finish(mut self) -> Doc {
        // An unbalanced document -- which the parser will not produce, but
        // which a future extension might -- leaves frames open. Close them
        // rather than losing what is in them.
        self.flush_paragraph();
        while self.blocks.len() > 1 {
            let inner = self.blocks.pop().expect("checked");
            self.push_block(Block::Quote(inner));
        }
        Doc {
            blocks: self.blocks.pop().unwrap_or_default(),
            links: self.links,
        }
    }

    fn style(&self) -> InlineStyle {
        InlineStyle {
            bold: self.bold > 0,
            italic: self.italic > 0,
            code: self.inline_code,
            strike: self.strike > 0,
            link: self.link,
        }
    }

    /// Where a finished block goes: the innermost open collector.
    fn push_block(&mut self, block: Block) {
        self.blocks
            .last_mut()
            .expect("the root collector is never popped")
            .push(block);
    }

    /// Add text to the run being built, merging it into the last run when
    /// that run is drawn the same way. Merging is not tidiness: a paragraph
    /// of a hundred one-character runs would be a hundred `Span`s and a
    /// hundred wrap pieces per row.
    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some((_, alt)) = self.image.as_mut() {
            alt.push_str(text);
            return;
        }
        if let Some((_, body)) = self.code.as_mut() {
            body.push_str(text);
            return;
        }
        let style = self.style();
        match self.inlines.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.inlines.push(Inline {
                text: text.to_string(),
                style,
            }),
        }
    }

    /// End the paragraph being collected, if there is anything in it.
    fn flush_paragraph(&mut self) {
        let inlines = take_meaningful(&mut self.inlines);
        if !inlines.is_empty() {
            self.push_block(Block::Paragraph(inlines));
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.push_text(&t),
            Event::Code(t) => {
                self.inline_code = true;
                self.push_text(&t);
                self.inline_code = false;
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.push_text("\n"),
            Event::Rule => {
                self.flush_paragraph();
                self.push_block(Block::Rule);
            }
            Event::FootnoteReference(label) => {
                let text = format!("[^{label}]");
                self.push_text(&text);
            }
            // Raw HTML, maths and task lists: see the module doc. The last
            // two are not even enabled; they are listed so that turning one
            // on is a change here rather than a surprise.
            Event::Html(_)
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::TaskListMarker(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.inlines.clear(),
            Tag::Heading { level, .. } => {
                self.inlines.clear();
                self.heading = Some(heading_level(level));
            }
            Tag::BlockQuote(_) => {
                self.flush_paragraph();
                self.blocks.push(Vec::new());
            }
            Tag::CodeBlock(kind) => {
                self.flush_paragraph();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) if !l.trim().is_empty() => Some(l.trim().to_string()),
                    _ => None,
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.flush_paragraph();
                self.lists.push(ListFrame {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => self.blocks.push(Vec::new()),
            Tag::FootnoteDefinition(label) => {
                self.flush_paragraph();
                self.footnote.push(label.to_string());
                self.blocks.push(Vec::new());
            }
            Tag::Table(aligns) => {
                self.flush_paragraph();
                self.table = Some(TableFrame {
                    align: aligns.iter().map(alignment).collect(),
                    ..TableFrame::default()
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.row.clear();
                }
            }
            Tag::TableCell => self.inlines.clear(),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                self.links.push(dest_url.to_string());
                // Past sixty-five thousand links in one article the numbers
                // stop being typeable anyway; the link is still drawn, just
                // without one.
                self.link = u16::try_from(self.links.len()).ok();
            }
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.to_string(), String::new())),
            // Not enabled, and listed so that enabling one is a change here.
            Tag::HtmlBlock
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_paragraph(),
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(1);
                let inlines = take_meaningful(&mut self.inlines);
                if !inlines.is_empty() {
                    self.push_block(Block::Heading { level, inlines });
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_paragraph();
                let inner = self.pop_collector();
                if !inner.is_empty() {
                    self.push_block(Block::Quote(inner));
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, text)) = self.code.take() {
                    self.push_block(Block::Code {
                        lang,
                        text: text.trim_end_matches('\n').to_string(),
                    });
                }
            }
            TagEnd::List(_) => {
                if let Some(frame) = self.lists.pop() {
                    if !frame.items.is_empty() {
                        self.push_block(Block::List {
                            start: frame.start,
                            items: frame.items,
                        });
                    }
                }
            }
            TagEnd::Item => {
                self.flush_paragraph();
                let inner = self.pop_collector();
                match self.lists.last_mut() {
                    Some(list) => list.items.push(inner),
                    // An item outside a list: the parser does not produce
                    // one, but losing its text if it ever did would be the
                    // wrong trade.
                    None => {
                        for block in inner {
                            self.push_block(block);
                        }
                    }
                }
            }
            TagEnd::FootnoteDefinition => {
                self.flush_paragraph();
                let label = self.footnote.pop().unwrap_or_default();
                let mut inner = self.pop_collector();
                // The definition reads as prose at the foot of the article,
                // led by the same `[^n]` the reference in the text showed,
                // so the two can be matched up by eye.
                let lead = Inline {
                    text: format!("[^{label}]: "),
                    style: InlineStyle::default(),
                };
                match inner.first_mut() {
                    Some(Block::Paragraph(inlines)) => inlines.insert(0, lead),
                    _ => inner.insert(0, Block::Paragraph(vec![lead])),
                }
                for block in inner {
                    self.push_block(block);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.push_block(Block::Table {
                        align: t.align,
                        header: t.header,
                        rows: t.rows,
                    });
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                let cell = take_meaningful(&mut self.inlines);
                if let Some(t) = self.table.as_mut() {
                    if t.in_head {
                        t.header.push(cell);
                    } else {
                        t.row.push(cell);
                    }
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.link = None,
            TagEnd::Image => {
                let (url, alt) = self.image.take().unwrap_or_default();
                let alt = alt.trim().to_string();
                if self.heading.is_some() || self.table.is_some() {
                    // No room for a block here, so the placeholder is text.
                    let text = format!("[image: {alt}]");
                    self.push_text(&text);
                } else {
                    self.flush_paragraph();
                    self.push_block(Block::Image {
                        alt,
                        url: url.trim().to_string(),
                    });
                }
            }
            TagEnd::HtmlBlock
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    /// Close the innermost collector, never the root.
    fn pop_collector(&mut self) -> Vec<Block> {
        if self.blocks.len() > 1 {
            self.blocks.pop().expect("checked")
        } else {
            Vec::new()
        }
    }
}

/// Take the collected runs, dropping the ones that are only whitespace at
/// either end. A paragraph of nothing but a soft break is not a paragraph.
fn take_meaningful(inlines: &mut Vec<Inline>) -> Vec<Inline> {
    let mut out = std::mem::take(inlines);
    while out.first().is_some_and(|i| i.text.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|i| i.text.trim().is_empty()) {
        out.pop();
    }
    out
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn alignment(a: &pulldown_cmark::Alignment) -> Align {
    match a {
        pulldown_cmark::Alignment::Right => Align::Right,
        pulldown_cmark::Alignment::Center => Align::Centre,
        // `None` is the column nobody aligned, which reads as left.
        pulldown_cmark::Alignment::None | pulldown_cmark::Alignment::Left => Align::Left,
    }
}

#[cfg(test)]
mod tests {
    use super::super::FIXTURE_MD;
    use super::*;
    use proptest::prelude::*;

    fn doc() -> Doc {
        parse(FIXTURE_MD)
    }

    fn text_of(inlines: &[Inline]) -> String {
        inlines.iter().map(|i| i.text.as_str()).collect()
    }

    fn find(doc: &Doc, f: impl Fn(&Block) -> bool) -> &Block {
        doc.blocks
            .iter()
            .find(|b| f(b))
            .unwrap_or_else(|| panic!("the fixture has no such block"))
    }

    #[test]
    fn the_heading_is_the_first_block() {
        let d = doc();
        let Block::Heading { level, inlines } = &d.blocks[0] else {
            panic!("{:?}", d.blocks[0]);
        };
        assert_eq!(*level, 1);
        assert_eq!(
            text_of(inlines),
            "Why the borrow checker says no: a field guide to lifetimes"
        );
    }

    #[test]
    fn a_paragraph_carries_its_bold_italic_code_and_two_links() {
        let d = doc();
        let Block::Paragraph(inlines) = &d.blocks[1] else {
            panic!("{:?}", d.blocks[1]);
        };
        let bold = inlines.iter().find(|i| i.style.bold).expect("bold");
        assert_eq!(bold.text, "not a wall");
        let italic = inlines.iter().find(|i| i.style.italic).expect("italic");
        assert_eq!(italic.text, "patient");
        let code = inlines.iter().find(|i| i.style.code).expect("code");
        assert_eq!(code.text, "&'a str");
        let links: Vec<u16> = inlines.iter().filter_map(|i| i.style.link).collect();
        assert_eq!(links, vec![1, 2], "two links, numbered in order");
        assert_eq!(
            d.links[0..2],
            [
                "https://doc.rust-lang.org/nomicon/".to_string(),
                "https://example.org/lifetimes".to_string()
            ]
        );
    }

    /// The numbers are assigned by where a link is in the document and by
    /// nothing else, so the number beside a link does not change when the
    /// panel is resized. Parsing twice gives the same list.
    #[test]
    fn link_numbers_are_document_order_and_stable() {
        let a = doc();
        let b = doc();
        assert_eq!(a.links, b.links);
        assert!(a.links.len() >= 3);

        // And a link whose text is its URL is numbered like any other.
        let plain = parse("see <https://example.org/> and [x](https://example.com/)");
        assert_eq!(
            plain.links,
            vec![
                "https://example.org/".to_string(),
                "https://example.com/".to_string()
            ]
        );
    }

    #[test]
    fn a_nested_bullet_list_keeps_its_nesting() {
        let d = doc();
        let Block::List { start, items } = find(
            &d,
            |b| matches!(b, Block::List { start: None, items } if items.len() == 3),
        ) else {
            unreachable!()
        };
        assert_eq!(*start, None);
        assert_eq!(items.len(), 3);
        let nested = items[1]
            .iter()
            .find(|b| matches!(b, Block::List { .. }))
            .expect("the second bullet has a list under it");
        let Block::List { items: inner, .. } = nested else {
            unreachable!()
        };
        assert_eq!(inner.len(), 2);
    }

    #[test]
    fn an_ordered_list_keeps_where_it_starts() {
        let d = doc();
        let Block::List { start, items } =
            find(&d, |b| matches!(b, Block::List { start: Some(_), .. }))
        else {
            unreachable!()
        };
        assert_eq!(*start, Some(1));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn a_fenced_block_keeps_its_language_and_its_line_breaks() {
        let d = doc();
        let Block::Code { lang, text } = find(&d, |b| matches!(b, Block::Code { .. })) else {
            unreachable!()
        };
        assert_eq!(lang.as_deref(), Some("rust"));
        assert_eq!(text.lines().count(), 3);
        assert!(text.starts_with("fn longest<'a>"));
        assert!(!text.ends_with('\n'), "the trailing newline is the fence's");
    }

    #[test]
    fn a_quote_is_a_block_of_blocks() {
        let d = doc();
        let Block::Quote(inner) = find(&d, |b| matches!(b, Block::Quote(_))) else {
            unreachable!()
        };
        assert_eq!(inner.len(), 1);
        let Block::Paragraph(inlines) = &inner[0] else {
            panic!("{:?}", inner[0]);
        };
        assert!(text_of(inlines).starts_with("Lifetimes are not about"));
    }

    #[test]
    fn a_rule_is_its_own_block() {
        let d = doc();
        assert_eq!(
            d.blocks.iter().filter(|b| **b == Block::Rule).count(),
            1,
            "one thematic break"
        );
    }

    #[test]
    fn a_table_keeps_its_alignment_its_header_and_its_rows() {
        let d = doc();
        let Block::Table {
            align,
            header,
            rows,
        } = find(&d, |b| matches!(b, Block::Table { .. }))
        else {
            unreachable!()
        };
        assert_eq!(align.len(), 3);
        assert_eq!(align[2], Align::Right, "the third column is right-aligned");
        assert_eq!(header.len(), 3);
        assert_eq!(text_of(&header[0]), "Rule");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 3);
    }

    #[test]
    fn an_image_is_a_block_carrying_its_alt_text() {
        let d = doc();
        let Block::Image { alt, .. } = find(&d, |b| matches!(b, Block::Image { .. })) else {
            unreachable!()
        };
        assert_eq!(alt, "a borrow checker diagram");
    }

    /// The address is what the reader fetches, so it has to survive the
    /// parse -- and a picture inside a link is still a picture rather than
    /// link number three.
    #[test]
    fn an_image_keeps_its_url_and_takes_no_link_number() {
        let d = parse("![a diagram](https://e.org/pictures/one.png)");
        assert_eq!(
            d.blocks,
            vec![Block::Image {
                alt: "a diagram".into(),
                url: "https://e.org/pictures/one.png".into(),
            }]
        );

        // Wrapped in a link, as a thumbnail on a news site is. The picture
        // carries no number: `o<n>` is for what a browser opens, and this
        // one is opened by clicking it.
        let d = parse("[![a diagram](https://e.org/one.png)](https://e.org/full)");
        assert_eq!(
            d.blocks,
            vec![Block::Image {
                alt: "a diagram".into(),
                url: "https://e.org/one.png".into(),
            }]
        );
        assert!(
            !d.blocks.iter().any(|b| matches!(
                b,
                Block::Paragraph(i) if i.iter().any(|r| r.style.link.is_some())
            )),
            "{:?}",
            d.blocks
        );

        // And one with nothing to say about itself still has an address.
        let d = parse("![](https://e.org/decorative.png)");
        assert_eq!(
            d.blocks,
            vec![Block::Image {
                alt: String::new(),
                url: "https://e.org/decorative.png".into(),
            }]
        );
    }

    /// An image in the middle of a paragraph splits it rather than being
    /// moved to the end of it: what was said before the picture stays before
    /// it.
    #[test]
    fn an_image_inside_a_paragraph_splits_it() {
        let d = parse("before ![alt](p.png) after");
        assert_eq!(d.blocks.len(), 3);
        let Block::Paragraph(first) = &d.blocks[0] else {
            panic!("{:?}", d.blocks[0]);
        };
        assert_eq!(text_of(first), "before ");
        assert_eq!(
            d.blocks[1],
            Block::Image {
                alt: "alt".into(),
                url: "p.png".into(),
            }
        );
        let Block::Paragraph(last) = &d.blocks[2] else {
            panic!("{:?}", d.blocks[2]);
        };
        assert_eq!(text_of(last), " after");
    }

    #[test]
    fn a_footnote_reference_is_text_and_its_definition_is_led_by_the_label() {
        let d = doc();
        let referenced = d.blocks.iter().any(|b| match b {
            Block::Paragraph(i) => text_of(i).contains("[^1]"),
            _ => false,
        });
        assert!(referenced, "the reference is in the prose");

        let defined = d.blocks.iter().any(|b| match b {
            Block::Paragraph(i) => text_of(i).starts_with("[^1]: "),
            _ => false,
        });
        assert!(defined, "the definition leads with its label");
    }

    /// A soft break is an artefact of whoever wrapped the source; a hard
    /// break is something somebody meant.
    #[test]
    fn a_soft_break_is_a_space_and_a_hard_break_is_a_newline() {
        let d = parse("one\ntwo");
        let Block::Paragraph(i) = &d.blocks[0] else {
            unreachable!()
        };
        assert_eq!(text_of(i), "one two");

        let d = parse("one  \ntwo");
        let Block::Paragraph(i) = &d.blocks[0] else {
            unreachable!()
        };
        assert_eq!(text_of(i), "one\ntwo");
    }

    #[test]
    fn raw_html_is_dropped() {
        let d = parse("<div class=\"ad\">buy</div>\n\nreal text");
        let text: String = d
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(i) => Some(text_of(i)),
                _ => None,
            })
            .collect();
        assert!(!text.contains("<div"), "{text:?}");
        assert!(text.contains("real text"));
    }

    /// Runs drawn the same way are one run. A paragraph of a hundred
    /// one-character runs is a hundred spans and a hundred wrap pieces per
    /// row, which is the cost this avoids.
    #[test]
    fn adjacent_runs_of_one_style_are_merged() {
        let d = parse("plain **bold** plain");
        let Block::Paragraph(i) = &d.blocks[0] else {
            unreachable!()
        };
        assert_eq!(i.len(), 3, "{i:?}");
        assert_eq!(i[1].text, "bold");
    }

    #[test]
    fn strikethrough_is_a_style_and_not_two_tildes() {
        let d = parse("~~gone~~ here");
        let Block::Paragraph(i) = &d.blocks[0] else {
            unreachable!()
        };
        assert!(i[0].style.strike);
        assert_eq!(i[0].text, "gone");
    }

    proptest! {
        /// This reads a page somebody else published. Whatever arrives, it
        /// comes back as a `Doc` -- no panic, and every link number really
        /// indexes a link, which is what `o<n>` will dereference.
        #[test]
        fn any_input_parses_and_every_link_number_is_in_range(text in ".{0,2000}") {
            let d = parse(&text);
            let mut stack: Vec<&Block> = d.blocks.iter().collect();
            while let Some(block) = stack.pop() {
                let inline_sets: Vec<&Vec<Inline>> = match block {
                    Block::Paragraph(i) | Block::Heading { inlines: i, .. } => vec![i],
                    Block::Quote(inner) => {
                        stack.extend(inner.iter());
                        vec![]
                    }
                    Block::List { items, .. } => {
                        for item in items {
                            stack.extend(item.iter());
                        }
                        vec![]
                    }
                    Block::Table { header, rows, .. } => {
                        let mut all: Vec<&Vec<Inline>> = header.iter().collect();
                        for row in rows {
                            all.extend(row.iter());
                        }
                        all
                    }
                    Block::Code { .. } | Block::Rule | Block::Image { .. } => vec![],
                };
                for inlines in inline_sets {
                    for i in inlines {
                        if let Some(n) = i.style.link {
                            prop_assert!(n >= 1);
                            prop_assert!(usize::from(n) <= d.links.len(), "link {n} of {}", d.links.len());
                        }
                    }
                }
            }
        }
    }
}
