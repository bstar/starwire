//! Where a key press lands.
//!
//! Six layers, tried outermost first, exactly as `ui/keymap.rs`'s module doc
//! describes them: an open overlay takes every key while it is up; the `/`
//! filter's text entry takes typing next; an `o` waiting for a link number
//! comes next; a `g` waiting for its second key after that; then the focused
//! module's own bindings; then the global table.
//!
//! The state that decides which layer a key lands in -- is an overlay open,
//! is the filter being typed, is a chord half-typed -- lives here. Every
//! function in `keymap.rs` itself is a pure function of one key, which is
//! what makes the table testable without an `App` at all.

use starkit::crossterm::event::{KeyCode, KeyEvent};

use crate::ui::keymap::{self, OChord};
use crate::ui::markdown;
use crate::ui::panels::ModuleId;
use crate::wire::{open, Command, OpenKind};

use super::App;

impl App {
    pub fn key(&mut self, k: KeyEvent) {
        if self.overlays.is_open() {
            let answer = self.overlays.handle(k);
            self.after_overlay_answer(answer);
            return;
        }

        if self.filter.is_some() && self.filter_key(k) {
            return;
        }

        if let Some(typed) = self.o_pending.clone() {
            if self.o_chord(k, &typed) {
                return;
            }
        }

        if self.g_pending {
            self.g_pending = false;
            if let Some(action) = keymap::g_prefix(k) {
                self.act(action);
            }
            return;
        }
        if k.modifiers.is_empty() && k.code == KeyCode::Char('g') {
            self.g_pending = true;
            return;
        }

        // `o` in the reader opens a chord rather than an action: a digit
        // after it names a link, and a second `o` is the article itself.
        // Everywhere else `o` is the plain "open in the browser".
        if self.layout.focus() == ModuleId::Reader
            && k.modifiers.is_empty()
            && k.code == KeyCode::Char('o')
        {
            self.o_pending = Some(String::new());
            return;
        }

        let module = self.layout.focus().module();
        if let Some(action) = keymap::module(module, k).or_else(|| keymap::resolve(k)) {
            self.act(action);
        }
    }

    /// The `/` field, while it has focus. `true` when the key was typing
    /// rather than a command.
    fn filter_key(&mut self, k: KeyEvent) -> bool {
        let input = self.filter.as_mut().expect("checked by the caller");
        if keymap::filter_eats(k) {
            let before = input.text().to_string();
            input.handle(k);
            if input.text() != before {
                let text = input.text().to_string();
                self.core.send(Command::Filter(text));
            }
            return true;
        }
        match k.code {
            // Enter keeps the filter on the list and puts the field away;
            // esc throws the filter away as well. Both are the way out.
            KeyCode::Enter => {
                self.filter = None;
                true
            }
            KeyCode::Esc => {
                self.filter = None;
                self.core.send(Command::ClearFilter);
                true
            }
            _ => false,
        }
    }

    /// An `o` chord, mid-number. `true` when the key belonged to it.
    ///
    /// `OChord::Ignore` falls through deliberately, and the chord stays
    /// pending: changing focus or cycling the theme has to keep working
    /// with a half-typed number, the same carve-out the filter makes.
    fn o_chord(&mut self, k: KeyEvent, typed: &str) -> bool {
        match keymap::o_prefix(k, typed, self.links) {
            OChord::Digit => {
                if let KeyCode::Char(c) = k.code {
                    let mut grown = typed.to_string();
                    grown.push(c);
                    self.o_pending = Some(grown);
                }
                true
            }
            OChord::Open(n) => {
                self.o_pending = None;
                self.open_link(n);
                true
            }
            OChord::OpenArticle => {
                self.o_pending = None;
                self.act(keymap::Action::OpenBrowser);
                true
            }
            OChord::Cancel => {
                self.o_pending = None;
                if self.links == 0 {
                    self.say("no links in this one");
                }
                true
            }
            OChord::Ignore => false,
        }
    }

    /// Open the article's *n*th link.
    ///
    /// The URLs live on the parsed document rather than on the laid-out one
    /// -- `Rendered` records where a link was drawn, `Doc` records what it
    /// points at -- so this parses again. A parse is microseconds and this
    /// runs once per keystroke that opens something.
    fn open_link(&mut self, n: u16) {
        let Some(article) = self.view.article.as_ref() else {
            return;
        };
        let doc = markdown::parse::parse(&article.markdown);
        let Some(url) = doc.links.get(usize::from(n).saturating_sub(1)).cloned() else {
            self.say(format!("there is no link {n}"));
            return;
        };
        let kind = match open::kind_of(&url) {
            open::Target::Video(_) => OpenKind::Video,
            open::Target::Browser(_) => OpenKind::Browser,
        };
        self.core.send(Command::OpenUrl { url, kind });
        self.say(format!("opening link {n}"));
    }

    /// A bracketed paste. The overlays' two text fields take one, and so
    /// does the `/` filter; everywhere else a paste is not an input.
    pub fn paste(&mut self, text: &str) {
        if self.overlays.takes_paste() {
            self.overlays.paste(text);
            self.repaint = true;
            return;
        }
        if let Some(input) = self.filter.as_mut() {
            input.paste(text);
            let text = input.text().to_string();
            self.core.send(Command::Filter(text));
        }
    }

    /// Whether a chord or a field is half-finished -- what the status row
    /// shows instead of the hints while one is.
    pub(super) fn pending_chord(&self) -> Option<String> {
        if let Some(typed) = &self.o_pending {
            return Some(format!("o{typed}"));
        }
        self.g_pending.then(|| "g".to_string())
    }
}
