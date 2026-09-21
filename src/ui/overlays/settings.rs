//! The settings worth reaching for, listed and changed in place.
//!
//! Every row here is also something a key does, or something whose key would
//! be a waste of the alphabet. That is the split STAR/CORD's own settings
//! overlay draws: the overlay is for *finding* a setting, the key is for
//! using it once you know it exists, and a setting only one of the two can
//! reach is either undiscoverable or tedious.
//!
//! Changing a row does two things -- it changes the running program, and it
//! writes the key through [`starkit::config::edit`], which rewrites one line
//! of `config.toml` and leaves every comment in the file alone. A settings
//! overlay that serialised the whole struct back would silently delete the
//! commentary the template was written to carry.
//!
//! ## What is not here
//!
//! `[player] video` -- the argv a video is handed to -- has no row, and that
//! is a limit rather than a decision: it is an array of strings and
//! `starkit::config::edit::Value` writes scalars. A row that changed the
//! player for this run and forgot it on the next would be worse than no row,
//! so the player is edited in the file, where `docs/configuration.md` says
//! it is.

use starkit::chrome::settings::{self, SettingsView};
use starkit::config::edit::Value;
use starkit::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::widgets::Widget;

use crate::config::Config;
use crate::ui::theme::Theme;

/// The widths `<` and `>` step through, and the ones the overlay offers.
///
/// Sixty is a narrow column, eighty is what a century of typography and
/// every one of these articles was written for, and a hundred and twenty is
/// for somebody who wants the terminal filled.
pub const WIDTHS: &[u16] = &[60, 72, 80, 100, 120];

/// Minutes between background refreshes, and `off`, which the core reads as
/// a zero.
pub const REFRESHES: &[u32] = &[15, 30, 60, 120, 0];

/// What the graphics setting can say. `auto` is what everybody should leave
/// it on; the rest are for a terminal that lies about itself.
pub const GRAPHICS: &[&str] = &["auto", "kitty", "blocks", "off"];

/// One thing the overlay can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Theme,
    ReadingWidth,
    Byline,
    MarkReadOnOpen,
    Extract,
    RefreshEvery,
    Graphics,
}

impl Setting {
    /// The table. The order is the order the rows are drawn in, and each
    /// label is what the file calls the key, so somebody who reads one can
    /// find the other.
    pub const ALL: &'static [Setting] = &[
        Setting::Theme,
        Setting::ReadingWidth,
        Setting::Byline,
        Setting::MarkReadOnOpen,
        Setting::Extract,
        Setting::RefreshEvery,
        Setting::Graphics,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Setting::Theme => "theme",
            Setting::ReadingWidth => "reading width",
            Setting::Byline => "byline",
            Setting::MarkReadOnOpen => "mark read on open",
            Setting::Extract => "extract articles",
            Setting::RefreshEvery => "refresh every",
            Setting::Graphics => "graphics",
        }
    }

    /// Where it lives in `config.toml`.
    pub fn where_written(self) -> (&'static str, &'static str) {
        match self {
            Setting::Theme => ("ui", "theme"),
            Setting::ReadingWidth => ("reading", "width"),
            Setting::Byline => ("reading", "show_byline"),
            Setting::MarkReadOnOpen => ("reading", "mark_read_on_open"),
            Setting::Extract => ("articles", "extract"),
            Setting::RefreshEvery => ("fetch", "refresh_minutes"),
            Setting::Graphics => ("ui", "graphics"),
        }
    }

    /// What the row shows right now.
    pub fn value(self, cfg: &Config) -> String {
        match self {
            Setting::Theme => cfg.ui.theme.clone(),
            Setting::ReadingWidth => cfg.reading.width.to_string(),
            Setting::Byline => on_off(cfg.reading.show_byline).into(),
            Setting::MarkReadOnOpen => on_off(cfg.reading.mark_read_on_open).into(),
            Setting::Extract => on_off(cfg.articles.extract).into(),
            Setting::RefreshEvery => match cfg.fetch.refresh_minutes {
                0 => "off".into(),
                n => format!("{n} min"),
            },
            Setting::Graphics => cfg.ui.graphics.clone(),
        }
    }

    /// Step this setting in `cfg` and hand back the line to write.
    ///
    /// The mutation and the value are one call rather than two so a row can
    /// never be applied without being saved, or saved as something other
    /// than what was applied.
    pub fn step(self, cfg: &mut Config, forward: bool, themes: &[String]) -> Value {
        match self {
            Setting::Theme => {
                let next = cycle_by(themes, &cfg.ui.theme, forward)
                    .cloned()
                    .unwrap_or_else(|| cfg.ui.theme.clone());
                cfg.ui.theme = next.clone();
                Value::Str(next)
            }
            Setting::ReadingWidth => {
                let next = cycle_by(WIDTHS, &cfg.reading.width, forward)
                    .copied()
                    .unwrap_or(cfg.reading.width);
                cfg.reading.width = next;
                Value::Int(i64::from(next))
            }
            Setting::Byline => {
                cfg.reading.show_byline = !cfg.reading.show_byline;
                Value::Bool(cfg.reading.show_byline)
            }
            Setting::MarkReadOnOpen => {
                cfg.reading.mark_read_on_open = !cfg.reading.mark_read_on_open;
                Value::Bool(cfg.reading.mark_read_on_open)
            }
            Setting::Extract => {
                cfg.articles.extract = !cfg.articles.extract;
                Value::Bool(cfg.articles.extract)
            }
            Setting::RefreshEvery => {
                let next = cycle_by(REFRESHES, &cfg.fetch.refresh_minutes, forward)
                    .copied()
                    .unwrap_or(cfg.fetch.refresh_minutes);
                cfg.fetch.refresh_minutes = next;
                Value::Int(i64::from(next))
            }
            Setting::Graphics => {
                let current = cfg.ui.graphics.as_str();
                let next = cycle_by(GRAPHICS, &current, forward)
                    .copied()
                    .unwrap_or("auto")
                    .to_string();
                cfg.ui.graphics = next.clone();
                Value::Str(next)
            }
        }
    }
}

fn on_off(yes: bool) -> &'static str {
    if yes {
        "on"
    } else {
        "off"
    }
}

/// The next value round the ring, or the first when what is there now is not
/// in it -- which is what a hand-edited `width = 77` looks like.
fn cycle_by<'a, T: PartialEq>(ring: &'a [T], current: &T, forward: bool) -> Option<&'a T> {
    if ring.is_empty() {
        return None;
    }
    let Some(i) = ring.iter().position(|v| v == current) else {
        return ring.first();
    };
    let n = ring.len();
    Some(
        &ring[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }],
    )
}

/// The open overlay.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub cursor: usize,
    pub scroll: usize,
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn selected(&self) -> Setting {
        Setting::ALL[self.cursor.min(Setting::ALL.len() - 1)]
    }

    pub fn handle(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') => Action::Quit,
                _ => Action::Taken,
            };
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(',') => Action::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.step(-1);
                Action::Taken
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.step(1);
                Action::Taken
            }
            // Left and right both change it; no row here has more than five
            // values, so cycling one way reaches them all and the other way
            // is the shortcut back.
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                Action::Change(self.selected(), true)
            }
            KeyCode::Left | KeyCode::Char('h') => Action::Change(self.selected(), false),
            _ => Action::Taken,
        }
    }

    fn step(&mut self, delta: isize) {
        let n = Setting::ALL.len() as isize;
        self.cursor = ((self.cursor as isize + delta).rem_euclid(n)) as usize;
        self.scroll = settings::clamp_scroll(self.cursor, self.scroll, Setting::ALL.len());
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.step(delta.signum() as isize);
    }

    /// What a click landed on, through STAR/KIT's own hit test so a row that
    /// scrolled out of sight is not a row anything can click. A click on a
    /// row steps it, exactly as `enter` on it would; a click off the list
    /// closes, which is what clicking outside a dialogue has always meant
    /// here.
    pub fn click(&mut self, area: Rect, x: u16, y: u16) -> Action {
        match settings::hit(area, Setting::ALL.len(), self.scroll, x, y) {
            Some(index) => {
                self.cursor = index;
                Action::Change(self.selected(), true)
            }
            None => Action::Close,
        }
    }

    pub fn rows(&self, cfg: &Config) -> Vec<settings::Row> {
        Setting::ALL
            .iter()
            .map(|s| settings::Row::setting(s.label(), s.value(cfg)))
            .collect()
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme, cfg: &Config) {
        let rows = self.rows(cfg);
        let core: &starkit::theme::Theme = theme;
        SettingsView {
            theme: core,
            heading: "settings",
            title: "the window",
            rows: &rows,
            cursor: self.cursor,
            scroll: self.scroll,
            footer: "enter change \u{b7} esc close",
        }
        .render(area, buf);
    }
}

/// What a key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Taken,
    Close,
    /// Change this setting; `true` steps forward.
    Change(Setting, bool),
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::tests_support::theme;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn themes() -> Vec<String> {
        vec![
            "system".to_string(),
            "terminal".to_string(),
            "catppuccin-mocha".to_string(),
        ]
    }

    /// Every setting names a real place in the file. A row that wrote to a
    /// key nothing reads would change the program until it was restarted and
    /// then quietly stop.
    #[test]
    fn every_row_writes_somewhere_the_config_reads() {
        let cfg = Config::default();
        let toml = toml::to_string(&cfg).expect("the config serialises");
        for s in Setting::ALL {
            let (section, key) = s.where_written();
            assert!(
                toml.contains(&format!("[{section}]")),
                "{section} is not a table in the config"
            );
            assert!(
                toml.contains(&format!("{key} =")),
                "{key} is not a key in the config"
            );
            assert!(!s.value(&cfg).is_empty(), "{s:?} has no value to show");
        }
    }

    /// And the line it writes is the value it just applied, read back off
    /// the file the same way the next start would read it.
    #[test]
    fn what_a_row_writes_is_what_it_applied() {
        for s in Setting::ALL {
            let mut cfg = Config::default();
            let value = s.step(&mut cfg, true, &themes());
            let (section, key) = s.where_written();
            let text = starkit::config::edit::apply("", section, key, &value);
            let read: Config =
                toml::from_str(&text).unwrap_or_else(|e| panic!("{s:?}: {e}\n{text}"));
            assert_eq!(
                s.value(&read),
                s.value(&cfg),
                "{s:?} wrote {value:?} but the file reads back differently"
            );
        }
    }

    #[test]
    fn a_ring_wraps_both_ways_and_a_hand_edited_value_joins_it() {
        let mut cfg = Config::default();
        assert_eq!(cfg.reading.width, 80);
        Setting::ReadingWidth.step(&mut cfg, true, &themes());
        assert_eq!(cfg.reading.width, 100);
        Setting::ReadingWidth.step(&mut cfg, false, &themes());
        assert_eq!(cfg.reading.width, 80);

        cfg.reading.width = 77;
        Setting::ReadingWidth.step(&mut cfg, true, &themes());
        assert_eq!(
            cfg.reading.width, WIDTHS[0],
            "an unknown width joins the ring"
        );
    }

    #[test]
    fn refreshing_can_be_turned_off_and_the_core_reads_that_as_a_zero() {
        let mut cfg = Config::default();
        for _ in 0..4 {
            Setting::RefreshEvery.step(&mut cfg, true, &themes());
        }
        assert_eq!(cfg.fetch.refresh_minutes, 0);
        assert_eq!(Setting::RefreshEvery.value(&cfg), "off");
        assert_eq!(cfg.core().fetch.refresh_minutes, 0);
    }

    #[test]
    fn the_cursor_wraps_and_enter_changes_the_row_it_is_on() {
        let mut s = Settings::new();
        assert_eq!(s.selected(), Setting::Theme);
        s.handle(key(KeyCode::Up));
        assert_eq!(s.selected(), *Setting::ALL.last().unwrap());
        s.handle(key(KeyCode::Down));
        assert_eq!(
            s.handle(key(KeyCode::Enter)),
            Action::Change(Setting::Theme, true)
        );
        assert_eq!(
            s.handle(key(KeyCode::Left)),
            Action::Change(Setting::Theme, false)
        );
        assert_eq!(s.handle(key(KeyCode::Esc)), Action::Close);
    }

    #[test]
    fn a_click_steps_the_row_it_landed_on() {
        let area = Rect::new(0, 0, 60, 20);
        let mut s = Settings::new();
        let rows = Setting::ALL.len();
        let mut found = None;
        for y in area.y..area.y + area.height {
            if settings::hit(area, rows, 0, area.x + area.width / 2, y) == Some(rows - 1) {
                found = Some(y);
                break;
            }
        }
        let y = found.expect("the widget draws the last row somewhere");
        assert_eq!(
            s.click(area, area.x + area.width / 2, y),
            Action::Change(Setting::ALL[rows - 1], true)
        );
        assert_eq!(s.cursor, rows - 1, "the cursor followed the pointer");
        assert_eq!(s.click(area, area.x, area.y), Action::Close);
    }

    #[test]
    fn it_draws_the_rows_and_their_values() {
        let t = theme("terminal");
        let cfg = Config::default();
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        Settings::new().render(area, &mut buf, &t, &cfg);
        let text: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("reading width"), "{text}");
        assert!(text.contains("80"), "{text}");
        assert!(text.contains("refresh every"), "{text}");
        assert!(text.contains("15 min"), "{text}");
    }
}
