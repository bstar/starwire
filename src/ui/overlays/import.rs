//! The first thing a new reader sees: "you already have a feed list --
//! shall I take it?"
//!
//! The core decides whether the offer stands (`State::import_offer`, set by
//! `Handle::spawn` only on an empty list that has never refused one). What
//! it does not do is count anything: the two numbers in the question --
//! how many feeds and how many of them are YouTube channels -- are read off
//! the file here by [`probe`], because they exist to help somebody decide
//! and not to drive anything.
//!
//! ## The three stages
//!
//! `Ask` is the question. Saying yes sends `ImportNewsboat` and moves to
//! `Waiting`, which is where the overlay sits until `State::last_import`
//! appears -- the import itself takes milliseconds, and the stage exists so
//! that the frame in between says something rather than flickering. `Done`
//! is the report, with the skipped lines and why.
//!
//! `Opml` is the fourth, and it is not part of that sequence: `alt+i` on a
//! machine with no newsboat installation opens a path field instead, because
//! the other way a feed list arrives is an OPML export from whatever the
//! reader used before.

use std::path::{Path, PathBuf};

use starkit::chrome::overlay::{self, Anchor};
use starkit::crossterm::event::{KeyCode, KeyEvent};
use starkit::input::{Edit, TextInput};
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;
use starkit::ratatui::style::Style;

use crate::ui::panels::rgb;
use crate::ui::theme::Theme;
use crate::wire::import::{newsboat, ImportReport};

/// A newsboat list worth offering, and what is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub path: PathBuf,
    pub cache: Option<PathBuf>,
    /// Lines that name a feed: comments, blanks and `!`-hidden lines are not
    /// among them.
    pub feeds: usize,
    /// How many of those are YouTube channels, which is the number that
    /// surprises people about their own list.
    pub videos: usize,
}

/// Look under `home` for a newsboat list, and count what is in it.
///
/// The paths are the core's own ([`newsboat::default_urls_paths`]), so this
/// and `Handle::probe_newsboat` can never be looking at different files. The
/// parsing is the core's too -- a `urls` file is a decade of hand edits, and
/// a second, laxer reading of one here would report numbers the import then
/// disagreed with.
pub fn probe(home: &Path) -> Option<Probe> {
    let path = newsboat::default_urls_paths(home)
        .into_iter()
        .find(|p| p.is_file())?;
    let cache = newsboat::default_cache_paths(home)
        .into_iter()
        .find(|p| p.is_file());
    let text = std::fs::read_to_string(&path).ok()?;
    let mut feeds = 0usize;
    let mut videos = 0usize;
    for line in newsboat::parse_urls(&text) {
        if let newsboat::UrlsLine::Feed(feed) = line {
            feeds += 1;
            let url = feed.url.to_ascii_lowercase();
            if url.contains("youtube.com") || url.contains("youtu.be") {
                videos += 1;
            }
        }
    }
    Some(Probe {
        path,
        cache,
        feeds,
        videos,
    })
}

/// Where the overlay is in the sequence.
#[derive(Debug)]
pub enum Stage {
    Ask(Probe),
    /// Sent, and waiting for `State::last_import`.
    Waiting,
    Done(ImportReport),
    /// No newsboat list here: a path to an OPML file instead.
    Opml {
        input: TextInput,
        error: Option<&'static str>,
    },
}

#[derive(Debug)]
pub struct Import {
    pub stage: Stage,
    /// The report's own scroll, for a list of skipped lines longer than the
    /// box.
    pub scroll: usize,
}

/// What a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Taken,
    Close,
    /// `y`: go ahead with the newsboat import.
    Yes,
    /// `n`: and never ask again.
    No,
    /// A path typed into the OPML field.
    Opml(PathBuf),
}

impl Import {
    pub fn ask(probe: Probe) -> Self {
        Self {
            stage: Stage::Ask(probe),
            scroll: 0,
        }
    }

    pub fn opml() -> Self {
        Self {
            stage: Stage::Opml {
                input: TextInput::single(),
                error: None,
            },
            scroll: 0,
        }
    }

    /// The import has been asked for; wait for the report.
    pub fn waiting(&mut self) {
        self.stage = Stage::Waiting;
        self.scroll = 0;
    }

    pub fn done(&mut self, report: ImportReport) {
        self.stage = Stage::Done(report);
        self.scroll = 0;
    }

    /// Whether the overlay is sitting on `State::last_import` -- what the
    /// app's tick checks before handing one over.
    pub fn is_waiting(&self) -> bool {
        matches!(self.stage, Stage::Waiting)
    }

    pub fn handle(&mut self, key: KeyEvent) -> Action {
        match &mut self.stage {
            Stage::Ask(_) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Action::Yes,
                KeyCode::Char('n') | KeyCode::Char('N') => Action::No,
                _ => Action::Taken,
            },
            // Nothing to answer while it runs, and nothing to answer once it
            // has: the report is read and dismissed.
            Stage::Waiting => Action::Taken,
            Stage::Done(_) => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.scroll = self.scroll.saturating_add(1);
                    Action::Taken
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.scroll = self.scroll.saturating_sub(1);
                    Action::Taken
                }
                _ => Action::Close,
            },
            Stage::Opml { input, error } => match input.handle(key) {
                Edit::Submit => {
                    let text = input.text().trim().to_string();
                    let path = PathBuf::from(shellexpand(&text));
                    if text.is_empty() {
                        *error = Some("type the path to an OPML file");
                        Action::Taken
                    } else if !path.is_file() {
                        *error = Some("there is no file there");
                        Action::Taken
                    } else {
                        Action::Opml(path)
                    }
                }
                Edit::Cancel => Action::Close,
                Edit::Consumed => {
                    *error = None;
                    Action::Taken
                }
                Edit::Ignored => Action::Taken,
            },
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let Stage::Opml { input, error } = &mut self.stage {
            input.paste(text);
            *error = None;
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = (self.scroll as i64 + i64::from(delta)).max(0) as usize;
    }

    /// The rows the box draws, in order. One function so [`render`] and the
    /// height [`rect`] asks for cannot disagree.
    fn rows(&self) -> Vec<(String, Row)> {
        match &self.stage {
            Stage::Ask(p) => {
                let noun = if p.feeds == 1 { "feed" } else { "feeds" };
                let mut out = vec![
                    (
                        format!("Import {} {noun} from newsboat?", p.feeds),
                        Row::Body,
                    ),
                    (p.path.display().to_string(), Row::Dim),
                ];
                if p.videos > 0 {
                    let rest = p.feeds.saturating_sub(p.videos);
                    let plural = if p.videos == 1 { "is a" } else { "are" };
                    out.push((
                        format!(
                            "{} {plural} YouTube channel{}; the other {rest} are feeds.",
                            p.videos,
                            if p.videos == 1 { "" } else { "s" }
                        ),
                        Row::Dim,
                    ));
                }
                if p.cache.is_some() {
                    out.push((
                        "The newsboat cache is here too, so what you have already read stays read."
                            .into(),
                        Row::Dim,
                    ));
                }
                out
            }
            Stage::Waiting => vec![("importing\u{2026}".into(), Row::Dim)],
            Stage::Done(r) => {
                let mut out = vec![(
                    format!("{} imported \u{b7} {} skipped", r.added, r.skipped.len()),
                    Row::Body,
                )];
                if r.already_there > 0 {
                    out.push((
                        format!("{} were already on the list.", r.already_there),
                        Row::Dim,
                    ));
                }
                if r.videos > 0 {
                    out.push((format!("{} YouTube channels.", r.videos), Row::Dim));
                }
                if r.read_marks > 0 {
                    out.push((
                        format!("{} read marks carried across.", r.read_marks),
                        Row::Dim,
                    ));
                }
                if !r.skipped.is_empty() {
                    out.push((String::new(), Row::Dim));
                    for (url, why) in r.skipped.iter().skip(self.scroll) {
                        out.push((format!("{url} \u{2014} {why}"), Row::Error));
                    }
                }
                out
            }
            Stage::Opml { input: _, error } => {
                let mut out = vec![(String::new(), Row::Field)];
                match error {
                    Some(e) => out.push(((*e).to_string(), Row::Error)),
                    None => out.push((
                        "the path to an OPML file exported from another reader".into(),
                        Row::Dim,
                    )),
                }
                out
            }
        }
    }

    fn title(&self) -> &'static str {
        match self.stage {
            Stage::Ask(_) | Stage::Waiting => "import from newsboat",
            Stage::Done(_) => "imported",
            Stage::Opml { .. } => "import feeds",
        }
    }

    fn footer(&self) -> &'static str {
        match self.stage {
            Stage::Ask(_) => "y yes \u{b7} n no",
            Stage::Waiting => "\u{2026}",
            Stage::Done(_) => "any key closes",
            Stage::Opml { .. } => "enter import \u{b7} esc cancel",
        }
    }
}

/// What a row of the box is, which is all that decides its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Body,
    Dim,
    Error,
    /// The OPML path field, drawn by the `TextInput` itself.
    Field,
}

/// Where a path beginning `~` actually is.
fn shellexpand(text: &str) -> String {
    let Some(rest) = text.strip_prefix('~') else {
        return text.to_string();
    };
    let home = std::env::home_dir()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    format!("{}{rest}", home.display())
}

/// Where the box lands, sized to its rows.
///
/// The plan's mock says fifty-two columns; the shared overlay's own clamp is
/// twenty-four to fifty-six against the area, which lands on fifty-two at a
/// hundred columns and narrows sensibly below that. One spelling of a box's
/// width is worth more than the exact number.
pub fn rect(area: Rect, i: &Import) -> Rect {
    let want = u16::try_from(i.rows().len()).unwrap_or(4) + 2;
    overlay::rect(area, (24, 56), want, 4, Anchor::Centre)
}

pub fn render(area: Rect, buf: &mut Buffer, theme: &Theme, i: &mut Import) -> Option<(u16, u16)> {
    let r = rect(area, i);
    if r.width < 10 || r.height < 4 {
        return None;
    }
    let rows = i.rows();
    let title = i.title();
    let footer = i.footer();
    let core: &starkit::theme::Theme = theme;
    let inner = overlay::render(
        r,
        buf,
        &overlay::Overlay {
            theme: core,
            title,
            detail: None,
            footer: Some(footer),
        },
    );
    if inner.width == 0 || inner.height == 0 {
        return None;
    }

    let width = inner.width.saturating_sub(1);
    let mut cursor = None;
    let mut y = inner.y;
    for (text, kind) in &rows {
        if y >= inner.y + inner.height {
            break;
        }
        if *kind == Row::Field {
            if let Stage::Opml { input, .. } = &mut i.stage {
                buf.set_string(
                    inner.x + 1,
                    y,
                    "\u{203a} ",
                    Style::default().fg(rgb(theme.dim)),
                );
                cursor = input.render(
                    Rect {
                        x: inner.x + 3,
                        y,
                        width: width.saturating_sub(2),
                        height: 1,
                    },
                    buf,
                    Style::default().fg(rgb(theme.fg)),
                );
            }
            y += 1;
            continue;
        }
        let colour = match kind {
            Row::Body => theme.fg,
            Row::Dim => theme.dim,
            Row::Error => theme.error,
            Row::Field => theme.fg,
        };
        for row in starkit::wrap::wrap(text, width) {
            if y >= inner.y + inner.height {
                break;
            }
            buf.set_string(
                inner.x + 1,
                y,
                row.drawn(text),
                Style::default().fg(rgb(colour)),
            );
            y += 1;
        }
    }
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::tests_support::theme;
    use starkit::crossterm::event::KeyModifiers;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn fixture_probe() -> Probe {
        Probe {
            path: PathBuf::from("/home/somebody/.config/newsboat/urls"),
            cache: Some(PathBuf::from(
                "/home/somebody/.local/share/newsboat/cache.db",
            )),
            feeds: 41,
            videos: 26,
        }
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

    /// The counts come off the real fixture `urls` file through the core's
    /// own parser -- the same file `starwire import newsboat --dry-run`
    /// reads, so the two agree by construction.
    #[test]
    fn probing_a_urls_file_counts_its_feeds_and_its_channels() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".config").join("newsboat");
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("import")
            .join("urls");
        std::fs::copy(&fixture, dir.join("urls")).unwrap();

        let p = probe(home.path()).expect("the file is there");
        assert_eq!(p.path, dir.join("urls"));
        assert!(p.feeds > 0, "{p:?}");
        assert!(p.videos > 0, "{p:?}");
        assert!(p.videos < p.feeds, "{p:?}");
        assert!(p.cache.is_none(), "no cache was written beside it");
    }

    #[test]
    fn nothing_to_import_is_no_probe_at_all() {
        let home = tempfile::tempdir().unwrap();
        assert!(probe(home.path()).is_none());
    }

    #[test]
    fn the_question_says_the_two_numbers_and_the_path() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        let mut i = Import::ask(fixture_probe());
        render(area, &mut buf, &t, &mut i);
        let text = dump(&buf, area);
        assert!(text.contains("IMPORT FROM NEWSBOAT"), "{text}");
        assert!(text.contains("Import 41 feeds from newsboat?"), "{text}");
        assert!(text.contains("26 are YouTube channels"), "{text}");
        assert!(text.contains("newsboat/urls"), "{text}");
        assert!(text.contains("y yes \u{b7} n no"), "{text}");
    }

    #[test]
    fn yes_and_no_are_the_only_two_answers() {
        let mut i = Import::ask(fixture_probe());
        assert_eq!(i.handle(key('y')), Action::Yes);
        assert_eq!(i.handle(key('n')), Action::No);
        assert_eq!(i.handle(key('q')), Action::Taken, "a modal takes every key");
        assert_eq!(
            i.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Yes
        );
    }

    #[test]
    fn the_report_says_what_was_imported_and_what_was_not() {
        let t = theme("terminal");
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        let mut i = Import::ask(fixture_probe());
        i.waiting();
        assert!(i.is_waiting());
        i.done(ImportReport {
            added: 39,
            already_there: 0,
            skipped: vec![("not a url".into(), "no scheme".into())],
            folders: 0,
            videos: 26,
            read_marks: 812,
        });
        render(area, &mut buf, &t, &mut i);
        let text = dump(&buf, area);
        assert!(text.contains("39 imported \u{b7} 1 skipped"), "{text}");
        assert!(text.contains("not a url \u{2014} no scheme"), "{text}");
        assert!(text.contains("812 read marks"), "{text}");
        // And any key puts it away.
        assert_eq!(i.handle(key(' ')), Action::Close);
    }

    #[test]
    fn the_opml_field_refuses_a_path_with_no_file_behind_it() {
        let mut i = Import::opml();
        for c in "/nowhere/at/all.opml".chars() {
            i.handle(key(c));
        }
        assert_eq!(
            i.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Taken
        );
        match &i.stage {
            Stage::Opml { error, .. } => assert_eq!(*error, Some("there is no file there")),
            _ => panic!("the stage changed"),
        }
    }

    #[test]
    fn the_opml_field_takes_a_path_that_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blogroll.opml");
        std::fs::write(&path, "<opml/>").unwrap();
        let mut i = Import::opml();
        for c in path.display().to_string().chars() {
            i.handle(key(c));
        }
        assert_eq!(
            i.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Opml(path)
        );
    }
}
