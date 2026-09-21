//! What is drawn over the column: the help, the import, a feed to add, the
//! settings, a confirmation, a search.
//!
//! One rule holds the six together, STAR/FOLD's and STAR/CORD's before it:
//! **only one is ever open**, and while one is, it takes every key -- a
//! dialogue drawn over a panel that lets a key through to the panel
//! underneath is a dialogue you can type through, which is the bug keeping
//! them behind [`Overlays`] makes impossible to write. `esc` always closes
//! whichever one is open rather than doing anything panel-specific, and
//! `ctrl+c` quits from inside any of them, because a modal box is not a
//! place the one true way out should stop working.
//!
//! [`Overlays::handle`] and [`Overlays::click`] are the only entry points a
//! caller needs: the app's key and mouse dispatch check
//! [`Overlays::is_open`] first and, while it answers true, hand every key
//! and click here instead of to the focused module.
//!
//! Closing an overlay can change what is behind it -- an import that just
//! added forty-one feeds, a theme that changed every colour -- so the caller
//! repaints the whole frame after any [`Answer`] that is not
//! [`Answer::Consumed`].
//!
//! The two text fields take a bracketed paste as well as keys, which is
//! STAR/CORD's [`Overlays::paste`]: a feed URL is the single most pasted
//! string in this program, and a paste that arrived as forty keypresses
//! would be forty chances for one of them to be read as a command.

pub mod add_feed;
pub mod confirm;
pub mod import;
pub mod search;
pub mod settings;

use std::path::PathBuf;

use starkit::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use starkit::keymap::{help_rect, HelpView};
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::widgets::Widget;

use crate::config::Config;
use crate::ui::keymap::{BINDINGS, MOUSE};
use crate::ui::theme::Theme;
use crate::ui::Bars;
use crate::wire::feed::{FeedId, Selection};

/// What a [`confirm::Confirm`] is asking about, carried through unopened so
/// the caller learns it again only once the answer is yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    RemoveFeed(FeedId),
    MarkAllRead(Selection),
}

/// The one overlay that may be open, and what it needs to keep drawing
/// itself between frames.
#[derive(Debug)]
pub enum Overlay {
    Help { scroll: u16 },
    Import(import::Import),
    AddFeed(add_feed::AddFeed),
    Settings(settings::Settings),
    Confirm(confirm::Confirm),
    Search(search::Search),
}

/// Modal things drawn over everything else. See the module doc for the one
/// rule they share.
#[derive(Debug, Default)]
pub struct Overlays {
    current: Option<Overlay>,
}

/// What handling a key or a click did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The overlay took the key or click; nothing else happened.
    Consumed,
    /// It closed without deciding anything -- `n`, `esc`, a click outside.
    Closed,
    /// `y`/`enter` on a [`confirm::Confirm`].
    Confirmed(Pending),
    /// A line typed into the add-feed field.
    AddFeed(String),
    /// A query typed into the search field.
    Search(String),
    /// `y` on the import offer.
    ImportNewsboat {
        urls: PathBuf,
        cache: Option<PathBuf>,
    },
    ImportOpml(PathBuf),
    /// `n` on the import offer: no, and never ask again.
    DismissImport,
    /// A settings row was stepped; `true` is forward.
    Setting(settings::Setting, bool),
    /// `ctrl+c`, which quits from inside an overlay the same as everywhere
    /// else.
    Quit,
}

impl Overlays {
    pub fn new() -> Self {
        Self { current: None }
    }

    /// Checked first by both the key and the mouse dispatch, so a key or a
    /// click reaches an open overlay rather than the panel under it.
    pub fn is_open(&self) -> bool {
        self.current.is_some()
    }

    pub fn current(&self) -> Option<&Overlay> {
        self.current.as_ref()
    }

    /// The same overlay, mutably -- for the app's tick to hand the import
    /// its report, and for a dragged scrollbar to move a scroll straight
    /// rather than through [`Answer`].
    pub fn current_mut(&mut self) -> Option<&mut Overlay> {
        self.current.as_mut()
    }

    /// Opening any overlay replaces whatever was open; see the module doc
    /// for why there is never more than one.
    pub fn open_help(&mut self) {
        self.current = Some(Overlay::Help { scroll: 0 });
    }

    pub fn open_import(&mut self, i: import::Import) {
        self.current = Some(Overlay::Import(i));
    }

    pub fn open_add_feed(&mut self) {
        self.current = Some(Overlay::AddFeed(add_feed::AddFeed::new()));
    }

    pub fn open_settings(&mut self) {
        self.current = Some(Overlay::Settings(settings::Settings::new()));
    }

    pub fn open_confirm(&mut self, c: confirm::Confirm) {
        self.current = Some(Overlay::Confirm(c));
    }

    pub fn open_search(&mut self, query: &str) {
        self.current = Some(Overlay::Search(search::Search::with(query)));
    }

    pub fn close(&mut self) {
        self.current = None;
    }

    /// Whether the open overlay is a text field waiting for a paste -- what
    /// the app's `Event::Paste` arm asks before handing one over.
    pub fn takes_paste(&self) -> bool {
        matches!(
            self.current,
            Some(Overlay::AddFeed(_)) | Some(Overlay::Search(_))
        ) || matches!(
            self.current,
            Some(Overlay::Import(import::Import {
                stage: import::Stage::Opml { .. },
                ..
            }))
        )
    }

    /// Put pasted text into whichever field is open. A no-op otherwise: a
    /// paste into a confirmation is not an answer to it.
    pub fn paste(&mut self, text: &str) {
        match self.current.as_mut() {
            Some(Overlay::AddFeed(a)) => a.paste(text),
            Some(Overlay::Search(s)) => s.paste(text),
            Some(Overlay::Import(i)) => i.paste(text),
            _ => {}
        }
    }

    /// Route a key to the open overlay, if any. `ctrl+c` is handled once,
    /// here, ahead of every overlay's own keys; `esc` is too, except where
    /// an overlay's own text field wants it as "cancel the field", which is
    /// the same answer.
    pub fn handle(&mut self, k: KeyEvent) -> Answer {
        if self.current.is_none() {
            return Answer::Closed;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.current = None;
            return Answer::Quit;
        }
        if k.code == KeyCode::Esc {
            self.current = None;
            return Answer::Closed;
        }

        let overlay = self.current.as_mut().expect("checked above");
        let (close, answer) = match overlay {
            Overlay::Help { scroll } => match k.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    *scroll = scroll.saturating_add(1);
                    (false, Answer::Consumed)
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    *scroll = scroll.saturating_sub(1);
                    (false, Answer::Consumed)
                }
                KeyCode::PageDown => {
                    *scroll = scroll.saturating_add(10);
                    (false, Answer::Consumed)
                }
                KeyCode::PageUp => {
                    *scroll = scroll.saturating_sub(10);
                    (false, Answer::Consumed)
                }
                // Anything else closes it, `?`/`F1` included -- the help has
                // no other use for a key.
                _ => (true, Answer::Closed),
            },
            Overlay::Import(i) => match i.handle(k) {
                import::Action::Taken => (false, Answer::Consumed),
                import::Action::Close => (true, Answer::Closed),
                import::Action::Yes => match &i.stage {
                    import::Stage::Ask(p) => {
                        let answer = Answer::ImportNewsboat {
                            urls: p.path.clone(),
                            cache: p.cache.clone(),
                        };
                        // Stays open: the report lands in it.
                        i.waiting();
                        (false, answer)
                    }
                    _ => (false, Answer::Consumed),
                },
                import::Action::No => (true, Answer::DismissImport),
                import::Action::Opml(path) => (true, Answer::ImportOpml(path)),
            },
            Overlay::AddFeed(a) => match a.handle(k) {
                add_feed::Action::Taken => (false, Answer::Consumed),
                add_feed::Action::Close => (true, Answer::Closed),
                add_feed::Action::Submit(url) => (true, Answer::AddFeed(url)),
            },
            Overlay::Settings(s) => match s.handle(k) {
                settings::Action::Taken => (false, Answer::Consumed),
                settings::Action::Close => (true, Answer::Closed),
                // Stays open: changing three settings should not be three
                // trips through the key that opened it.
                settings::Action::Change(setting, forward) => {
                    (false, Answer::Setting(setting, forward))
                }
                settings::Action::Quit => (true, Answer::Quit),
            },
            Overlay::Confirm(c) => match starkit::chrome::confirm::answer(k) {
                starkit::chrome::confirm::Answer::Yes => {
                    (true, Answer::Confirmed(c.pending.clone()))
                }
                starkit::chrome::confirm::Answer::No => (true, Answer::Closed),
                // `ctrl+c` is caught above, so this arm is unreached in
                // practice; kept exhaustive rather than assumed away.
                starkit::chrome::confirm::Answer::Quit => (true, Answer::Quit),
                starkit::chrome::confirm::Answer::Waiting => (false, Answer::Consumed),
            },
            Overlay::Search(s) => match s.handle(k) {
                search::Action::Taken => (false, Answer::Consumed),
                search::Action::Close => (true, Answer::Closed),
                search::Action::Submit(q) => (true, Answer::Search(q)),
            },
        };
        if close {
            self.current = None;
        }
        answer
    }

    /// A click, while something is open. Outside the box it closes,
    /// whichever overlay it is; inside, each answers for itself.
    pub fn click(&mut self, x: u16, y: u16, area: Rect, cfg: &Config) -> Answer {
        if self.current.is_none() {
            return Answer::Closed;
        }
        let overlay = self.current.as_mut().expect("checked above");
        let (close, answer) = match overlay {
            Overlay::Help { .. } => {
                if inside(help_rect(area), x, y) {
                    (false, Answer::Consumed)
                } else {
                    (true, Answer::Closed)
                }
            }
            Overlay::Import(i) => {
                if inside(import::rect(area, i), x, y) {
                    (false, Answer::Consumed)
                } else {
                    (true, Answer::Closed)
                }
            }
            Overlay::AddFeed(_) => {
                if inside(add_feed::rect(area), x, y) {
                    (false, Answer::Consumed)
                } else {
                    (true, Answer::Closed)
                }
            }
            Overlay::Settings(s) => {
                let rows = s.rows(cfg).len();
                if !inside(starkit::chrome::settings::rect(area, rows), x, y) {
                    (true, Answer::Closed)
                } else {
                    match s.click(area, x, y) {
                        settings::Action::Change(setting, forward) => {
                            (false, Answer::Setting(setting, forward))
                        }
                        settings::Action::Close => (false, Answer::Consumed),
                        _ => (false, Answer::Consumed),
                    }
                }
            }
            Overlay::Confirm(c) => match confirm::layout(area, c) {
                Some(l) if !inside(l.rect, x, y) => (true, Answer::Closed),
                Some(l) if in_word(l.yes, l.footer_y, x, y) => {
                    (true, Answer::Confirmed(c.pending.clone()))
                }
                Some(l) if in_word(l.no, l.footer_y, x, y) => (true, Answer::Closed),
                Some(_) => (false, Answer::Consumed),
                None => (true, Answer::Closed),
            },
            Overlay::Search(_) => {
                if inside(search::rect(area), x, y) {
                    (false, Answer::Consumed)
                } else {
                    (true, Answer::Closed)
                }
            }
        };
        if close {
            self.current = None;
        }
        answer
    }

    /// The wheel, while something is open. Only the three that hold a list
    /// long enough to scroll have anything to do with it.
    pub fn scroll(&mut self, up: bool) {
        let delta = if up { -3 } else { 3 };
        match self.current.as_mut() {
            Some(Overlay::Help { scroll }) => {
                *scroll = if up {
                    scroll.saturating_sub(3)
                } else {
                    scroll.saturating_add(3)
                };
            }
            Some(Overlay::Import(i)) => i.scroll_by(delta),
            Some(Overlay::Settings(s)) => s.scroll_by(delta),
            _ => {}
        }
    }

    /// Draw whatever is open. Returns where the terminal's own cursor
    /// belongs -- only the three text fields ever want it somewhere.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        cfg: &Config,
        _bars: &mut Bars,
    ) -> Option<(u16, u16)> {
        match self.current.as_mut()? {
            Overlay::Help { scroll } => {
                // `HelpView` takes STAR/KIT's own `Theme`, which this crate's
                // wrapper derefs to; a struct literal is not a coercion site,
                // so the target type is spelled out to reach it.
                let core: &starkit::theme::Theme = theme;
                HelpView {
                    theme: core,
                    bindings: BINDINGS,
                    mouse: MOUSE,
                    scroll: *scroll,
                    title: "keys",
                }
                .render(area, buf);
                None
            }
            Overlay::Import(i) => import::render(area, buf, theme, i),
            Overlay::AddFeed(a) => add_feed::render(area, buf, theme, a),
            Overlay::Settings(s) => {
                s.render(area, buf, theme, cfg);
                None
            }
            Overlay::Confirm(c) => {
                confirm::render(area, buf, theme, c);
                None
            }
            Overlay::Search(s) => search::render(area, buf, theme, s),
        }
    }
}

fn inside(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn in_word((start, end): (u16, u16), row_y: u16, x: u16, y: u16) -> bool {
    y == row_y && x >= start && x < end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::tests_support::theme;
    use crate::wire::import::ImportReport;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn a_probe() -> import::Probe {
        import::Probe {
            path: PathBuf::from("/home/somebody/.config/newsboat/urls"),
            cache: None,
            feeds: 41,
            videos: 26,
        }
    }

    fn every_overlay() -> Vec<fn(&mut Overlays)> {
        vec![
            |o: &mut Overlays| o.open_help(),
            |o: &mut Overlays| o.open_import(import::Import::ask(a_probe())),
            |o: &mut Overlays| o.open_add_feed(),
            |o: &mut Overlays| o.open_settings(),
            |o: &mut Overlays| o.open_confirm(confirm::Confirm::remove_feed(FeedId(1), "Lobsters")),
            |o: &mut Overlays| o.open_search("lifetimes"),
        ]
    }

    #[test]
    fn nothing_is_open_to_begin_with() {
        let o = Overlays::new();
        assert!(!o.is_open());
        assert!(o.current().is_none());
    }

    /// Escape closes every one of the six, and never quits -- the rule the
    /// key table holds everywhere else, held again here because an overlay
    /// is the place it is most tempting to break.
    #[test]
    fn escape_closes_every_overlay() {
        for open in every_overlay() {
            let mut o = Overlays::new();
            open(&mut o);
            assert_eq!(o.handle(code(KeyCode::Esc)), Answer::Closed);
            assert!(!o.is_open());
        }
    }

    #[test]
    fn ctrl_c_quits_from_inside_any_overlay() {
        for open in every_overlay() {
            let mut o = Overlays::new();
            open(&mut o);
            assert_eq!(
                o.handle(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
                Answer::Quit
            );
            assert!(!o.is_open());
        }
    }

    #[test]
    fn a_click_outside_the_box_closes_whatever_is_open() {
        let cfg = Config::default();
        // Large enough that even the help overlay -- capped at 80x38 --
        // leaves a margin outside itself for (0, 0) to land in.
        let area = Rect::new(0, 0, 120, 44);
        for open in every_overlay() {
            let mut o = Overlays::new();
            open(&mut o);
            assert_eq!(o.click(0, 0, area, &cfg), Answer::Closed, "at (0, 0)");
            assert!(!o.is_open());
        }
    }

    #[test]
    fn opening_one_overlay_replaces_whatever_was_open() {
        let mut o = Overlays::new();
        for open in every_overlay() {
            open(&mut o);
            assert!(o.is_open());
        }
        o.open_help();
        assert!(matches!(o.current(), Some(Overlay::Help { .. })));
    }

    /// The import's own sequence: the question, the command, the wait, the
    /// report -- and the overlay stays open across the middle of it.
    #[test]
    fn saying_yes_to_the_import_sends_it_and_waits_for_the_report() {
        let mut o = Overlays::new();
        o.open_import(import::Import::ask(a_probe()));
        assert_eq!(
            o.handle(key('y')),
            Answer::ImportNewsboat {
                urls: PathBuf::from("/home/somebody/.config/newsboat/urls"),
                cache: None,
            }
        );
        assert!(o.is_open(), "the report lands in it");
        match o.current_mut() {
            Some(Overlay::Import(i)) => {
                assert!(i.is_waiting());
                i.done(ImportReport {
                    added: 39,
                    ..ImportReport::default()
                });
            }
            _ => panic!("the import closed"),
        }
        assert_eq!(o.handle(key(' ')), Answer::Closed);
    }

    #[test]
    fn saying_no_to_the_import_dismisses_it_for_good() {
        let mut o = Overlays::new();
        o.open_import(import::Import::ask(a_probe()));
        assert_eq!(o.handle(key('n')), Answer::DismissImport);
        assert!(!o.is_open());
    }

    #[test]
    fn a_confirmation_answers_on_y_and_on_n() {
        let mut o = Overlays::new();
        o.open_confirm(confirm::Confirm::remove_feed(FeedId(5), "Phoronix"));
        assert_eq!(
            o.handle(key('y')),
            Answer::Confirmed(Pending::RemoveFeed(FeedId(5)))
        );
        assert!(!o.is_open());

        o.open_confirm(confirm::Confirm::remove_feed(FeedId(5), "Phoronix"));
        assert_eq!(o.handle(key('n')), Answer::Closed);
        assert!(!o.is_open());
    }

    #[test]
    fn a_confirmation_answers_on_a_click_of_either_word() {
        let cfg = Config::default();
        let area = Rect::new(0, 0, 60, 21);
        let mut o = Overlays::new();
        o.open_confirm(confirm::Confirm::remove_feed(FeedId(5), "Phoronix"));
        let l = match o.current() {
            Some(Overlay::Confirm(c)) => confirm::layout(area, c).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(
            o.click(l.yes.0, l.footer_y, area, &cfg),
            Answer::Confirmed(Pending::RemoveFeed(FeedId(5)))
        );
    }

    /// The settings box stays open while it is being used, which is what
    /// makes changing three of them one visit rather than three.
    #[test]
    fn a_settings_row_answers_without_closing_the_box() {
        let mut o = Overlays::new();
        o.open_settings();
        assert_eq!(
            o.handle(code(KeyCode::Enter)),
            Answer::Setting(settings::Setting::Theme, true)
        );
        assert!(o.is_open());
    }

    #[test]
    fn the_two_text_fields_take_a_paste_and_the_others_ignore_one() {
        let mut o = Overlays::new();
        o.open_add_feed();
        assert!(o.takes_paste());
        o.paste("https://example.org/feed.xml");
        match o.current() {
            Some(Overlay::AddFeed(a)) => assert_eq!(a.input.text(), "https://example.org/feed.xml"),
            _ => panic!("it closed"),
        }

        o.open_search("");
        assert!(o.takes_paste());
        o.paste("lifetimes");
        assert_eq!(
            o.handle(code(KeyCode::Enter)),
            Answer::Search("lifetimes".into())
        );

        o.open_confirm(confirm::Confirm::remove_feed(FeedId(1), "x"));
        assert!(!o.takes_paste());
        o.paste("nothing happens");
        assert!(o.is_open());
    }

    #[test]
    fn render_draws_each_overlays_title_and_does_not_panic() {
        let t = theme("terminal");
        let cfg = Config::default();
        for (open, title) in [
            (
                (|o: &mut Overlays| o.open_help()) as fn(&mut Overlays),
                "KEYS",
            ),
            (
                |o: &mut Overlays| o.open_import(import::Import::ask(a_probe())),
                "IMPORT FROM NEWSBOAT",
            ),
            (|o: &mut Overlays| o.open_add_feed(), "ADD A FEED"),
            (|o: &mut Overlays| o.open_settings(), "SETTINGS"),
            (
                |o: &mut Overlays| {
                    o.open_confirm(confirm::Confirm::remove_feed(FeedId(1), "Lobsters"))
                },
                "REMOVE THE FEED",
            ),
            (|o: &mut Overlays| o.open_search("x"), "SEARCH"),
        ] {
            for area in [Rect::new(0, 0, 60, 21), Rect::new(0, 0, 200, 60)] {
                let mut o = Overlays::new();
                open(&mut o);
                let mut buf = Buffer::empty(area);
                o.render(area, &mut buf, &t, &cfg, &mut Bars::new());
                let text: String = (0..area.height)
                    .map(|y| {
                        (0..area.width)
                            .map(|x| buf[(x, y)].symbol().to_string())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(text.contains(title), "{area:?}: {text}");
            }
        }
    }

    #[test]
    fn a_closed_overlay_renders_nothing_and_no_cursor() {
        let t = theme("terminal");
        let cfg = Config::default();
        let area = Rect::new(0, 0, 60, 21);
        let mut buf = Buffer::empty(area);
        let before = buf.clone();
        let cursor = Overlays::new().render(area, &mut buf, &t, &cfg, &mut Bars::new());
        assert_eq!(buf, before);
        assert_eq!(cursor, None);
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_key() -> impl Strategy<Value = KeyEvent> {
            let code = prop_oneof![
                Just(KeyCode::Esc),
                Just(KeyCode::Enter),
                Just(KeyCode::Up),
                Just(KeyCode::Down),
                Just(KeyCode::Left),
                Just(KeyCode::Right),
                Just(KeyCode::PageUp),
                Just(KeyCode::PageDown),
                Just(KeyCode::Home),
                Just(KeyCode::End),
                Just(KeyCode::Backspace),
                Just(KeyCode::Delete),
                Just(KeyCode::F(1)),
                prop_oneof![
                    Just('a'),
                    Just('y'),
                    Just('n'),
                    Just('o'),
                    Just('j'),
                    Just('k'),
                    Just('c'),
                    Just('?'),
                    Just(','),
                    Just('/'),
                    Just(' '),
                ]
                .prop_map(KeyCode::Char),
            ];
            let modifiers = prop_oneof![
                Just(KeyModifiers::NONE),
                Just(KeyModifiers::CONTROL),
                Just(KeyModifiers::SHIFT),
                Just(KeyModifiers::ALT),
            ];
            (code, modifiers).prop_map(|(code, modifiers)| KeyEvent::new(code, modifiers))
        }

        fn open_nth(o: &mut Overlays, n: usize) {
            every_overlay()[n % 6](o);
        }

        proptest! {
            #[test]
            fn random_keys_never_panic_whichever_overlay_is_open(
                opener in 0..6usize,
                keys in proptest::collection::vec(arb_key(), 0..60),
            ) {
                let mut o = Overlays::new();
                open_nth(&mut o, opener);
                for k in keys {
                    if !o.is_open() {
                        open_nth(&mut o, opener);
                    }
                    let _ = o.handle(k);
                }
            }

            /// And a click anywhere in a frame-sized area is either answered
            /// or closes the box -- never a panic and never a hang.
            #[test]
            fn random_clicks_never_panic(
                opener in 0..6usize,
                x in 0..100u16,
                y in 0..30u16,
            ) {
                let cfg = Config::default();
                let mut o = Overlays::new();
                open_nth(&mut o, opener);
                let _ = o.click(x, y, Rect::new(0, 0, 100, 30), &cfg);
            }
        }
    }
}
