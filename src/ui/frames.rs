//! Snapshots of whole frames.
//!
//! A layout regression is a diff of a drawn screen -- the one thing a pile
//! of assertions about rectangles cannot show: a column clipped mid-word, a
//! crumb line running into a count, an article wrapped at the panel's width
//! instead of the reader's. Every one of those is found by looking at a
//! frame, which is what `insta` lets a person, and a review, do without
//! running the program.
//!
//! Two sizes, because they exercise different code: a hundred by thirty is
//! the column with room to spend, and sixty by twenty-one is the floor,
//! where every module is at its minimum and nothing is left over. Two
//! themes, because the `[wire]` roles are resolved per theme and only a
//! drawn frame shows what they come to: `terminal`, the sixteen-colour one,
//! and `catppuccin-mocha`, the default, which has a full base16 palette.
//!
//! ## Determinism
//!
//! Every app here is built with [`starkit::graphics::Graphics::disabled`],
//! UTC, and the clock pinned to [`crate::wire::testing::now`] -- the same
//! instant the fixture's own publication dates are measured from, so an age
//! reads the same string whatever day this is run on. `App::set_tz` and
//! `App::set_now` are the `#[cfg(test)]` hooks for that; `App::view` and
//! `App::stack` hand back what a test needs to find a row by name.
//! [`fake::Fake::state_mut`] is the fourth hook, for the one thing no real
//! run leaves sitting still long enough to draw: a refresh stopped at `12 of
//! 41`.

use std::path::PathBuf;

use starkit::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use starkit::graphics::Graphics;
use starkit::ratatui::buffer::Buffer;
use starkit::ratatui::layout::Rect;

use super::app::App;
use super::fake;
use crate::config::{Config, Ui};
use crate::wire::testing;

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn code(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
}

fn alt_code(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::ALT)
}

/// A frame as text, one row per line, trailing spaces trimmed. Styles are
/// asserted by the theme legibility test and by each panel's own tests; what
/// is asserted here is the shape.
fn render(app: &mut App, w: u16, h: u16) -> String {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    app.draw(area, &mut buf);
    (0..h)
        .map(|y| {
            let row: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
            row.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run whatever a key produced and fold the result back into `App`, twice:
/// the window sends `OpenFeed` from `tick` *after* `refresh` notices the
/// stack moved, which is after the `pump` that would have answered it, so
/// one round leaves a freshly-opened level one query short. A second round
/// picks up exactly that query; a caller that moved nothing finds nothing to
/// do on it, so this is never wrong to call.
fn settle(app: &mut App, fk: &mut fake::Fake) {
    for _ in 0..3 {
        app.tick();
        fk.pump();
    }
    app.tick();
}

/// An app over the fixture, pinned to `theme`, UTC and the fixture's clock.
fn build(theme: &str) -> (App, fake::Fake) {
    let cfg = Config {
        ui: Ui {
            theme: theme.into(),
            ..Ui::default()
        },
        ..Config::default()
    };
    let (core, mut fk) = fake::handle(&cfg.core());
    let mut app = App::new(
        core,
        cfg,
        PathBuf::from("/nonexistent/config.toml"),
        None,
        Graphics::disabled(),
    );
    app.set_tz(jiff::tz::TimeZone::UTC);
    app.set_now(testing::now());
    settle(&mut app, &mut fk);
    (app, fk)
}

/// Move the cursor in the focused list onto the row whose drawn name holds
/// `needle`: `home` first, so this lands on it wherever the cursor already
/// was, then `j` at a time.
fn cursor_to(app: &mut App, fk: &mut fake::Fake, needle: &str) {
    app.key(code(KeyCode::Home));
    settle(app, fk);
    let idx = row_index(app, needle);
    for _ in 0..idx {
        app.key(key('j'));
    }
    settle(app, fk);
}

fn row_index(app: &App, needle: &str) -> usize {
    let v = app.view();
    if let Some(i) = v.source_rows.iter().position(|r| r.name.contains(needle)) {
        return i;
    }
    v.entry_rows
        .iter()
        .position(|r| r.title.contains(needle))
        .unwrap_or_else(|| {
            panic!(
                "{needle} is in neither list: {:?} / {:?}",
                v.source_rows.iter().map(|r| &r.name).collect::<Vec<_>>(),
                v.entry_rows.iter().map(|r| &r.title).collect::<Vec<_>>()
            )
        })
}

/// Into Hacker News and onto the article with every treatment in it.
fn into_hn(app: &mut App, fk: &mut fake::Fake) {
    // Hacker News is inside the Tech folder, so this is `l` into the folder
    // and then `enter` on the feed -- the two keys the plan's mocks show.
    cursor_to(app, fk, "Tech");
    app.key(key('l'));
    settle(app, fk);
    cursor_to(app, fk, "Hacker News");
    app.key(code(KeyCode::Enter));
    settle(app, fk);
}

fn open(app: &mut App, fk: &mut fake::Fake, needle: &str) {
    cursor_to(app, fk, needle);
    app.key(code(KeyCode::Enter));
    settle(app, fk);
}

// -- the column -----------------------------------------------------------

#[test]
fn the_sources_with_the_fixture_loaded() {
    let (mut app, _fk) = build("terminal");
    insta::assert_snapshot!("sources-terminal-100x30", render(&mut app, 100, 30));
    insta::assert_snapshot!("sources-terminal-60x21", render(&mut app, 60, 21));

    let (mut app, _fk) = build("catppuccin-mocha");
    insta::assert_snapshot!("sources-mocha-100x30", render(&mut app, 100, 30));
    insta::assert_snapshot!("sources-mocha-60x21", render(&mut app, 60, 21));
}

#[test]
fn one_feeds_entries() {
    let (mut app, mut fk) = build("terminal");
    into_hn(&mut app, &mut fk);
    insta::assert_snapshot!("entries-terminal-100x30", render(&mut app, 100, 30));
    insta::assert_snapshot!("entries-terminal-60x21", render(&mut app, 60, 21));

    let (mut app, mut fk) = build("catppuccin-mocha");
    into_hn(&mut app, &mut fk);
    insta::assert_snapshot!("entries-mocha-100x30", render(&mut app, 100, 30));
    insta::assert_snapshot!("entries-mocha-60x21", render(&mut app, 60, 21));
}

/// The refresh badge and the status bar, stopped where no real run holds
/// still: twelve of forty-one.
#[test]
fn entries_while_a_refresh_runs() {
    let (mut app, mut fk) = build("terminal");
    into_hn(&mut app, &mut fk);
    {
        let mut state = fk.state_mut();
        state.refresh.running = true;
        state.refresh.done = 12;
        state.refresh.total = 41;
        state.refresh.current = Some("refreshing".into());
        state.version += 1;
    }
    app.tick();
    insta::assert_snapshot!("entries-refreshing", render(&mut app, 100, 30));
}

// -- the reader -----------------------------------------------------------

#[test]
fn the_article_with_every_treatment_in_it() {
    let (mut app, mut fk) = build("terminal");
    into_hn(&mut app, &mut fk);
    open(&mut app, &mut fk, "borrow checker");
    insta::assert_snapshot!("article-terminal-100x30", render(&mut app, 100, 30));
    insta::assert_snapshot!("article-terminal-60x21", render(&mut app, 60, 21));

    let (mut app, mut fk) = build("catppuccin-mocha");
    into_hn(&mut app, &mut fk);
    open(&mut app, &mut fk, "borrow checker");
    insta::assert_snapshot!("article-mocha-100x30", render(&mut app, 100, 30));
}

#[test]
fn the_three_articles_that_are_not_text() {
    let (mut app, mut fk) = build("terminal");
    into_hn(&mut app, &mut fk);
    open(&mut app, &mut fk, "not arrived");
    insta::assert_snapshot!("article-pending", render(&mut app, 100, 30));

    let (mut app, mut fk) = build("terminal");
    into_hn(&mut app, &mut fk);
    open(&mut app, &mut fk, "paywall");
    insta::assert_snapshot!("article-failed", render(&mut app, 100, 30));

    let (mut app, mut fk) = build("terminal");
    cursor_to(&mut app, &mut fk, "Videos");
    app.key(code(KeyCode::Enter));
    settle(&mut app, &mut fk);
    open(&mut app, &mut fk, "transistor");
    insta::assert_snapshot!("article-video", render(&mut app, 100, 30));
}

/// The peek: an article open, the cursor jumped back up to the entries, and
/// READER keeping the height it had.
#[test]
fn the_peek_with_an_article_open_behind_the_list() {
    let (mut app, mut fk) = build("terminal");
    into_hn(&mut app, &mut fk);
    open(&mut app, &mut fk, "borrow checker");
    app.key(alt_code(KeyCode::Up));
    settle(&mut app, &mut fk);
    insta::assert_snapshot!("peek", render(&mut app, 100, 30));
}

/// A search is a level of the stack like any other.
#[test]
fn a_search_is_a_level() {
    let (mut app, mut fk) = build("terminal");
    app.key(alt('f'));
    for c in "borrow".chars() {
        app.key(key(c));
    }
    app.key(code(KeyCode::Enter));
    settle(&mut app, &mut fk);
    insta::assert_snapshot!("search-level", render(&mut app, 100, 30));
}

// -- the overlays ---------------------------------------------------------

#[test]
fn the_import_offer_and_its_report() {
    let (mut app, _fk) = build("terminal");
    app.open_import_for_tests(fake_probe());
    insta::assert_snapshot!("import-100x30", render(&mut app, 100, 30));
    insta::assert_snapshot!("import-60x21", render(&mut app, 60, 21));

    let (mut app, _fk) = build("terminal");
    app.open_import_for_tests(fake_probe());
    app.key(key('y'));
    app.import_report_for_tests(crate::wire::import::ImportReport {
        added: 39,
        already_there: 0,
        skipped: vec![
            ("not a url".into(), "no scheme".into()),
            ("gopher://old.example/".into(), "unsupported scheme".into()),
        ],
        folders: 0,
        videos: 26,
        read_marks: 812,
    });
    insta::assert_snapshot!("import-done", render(&mut app, 100, 30));
}

fn fake_probe() -> super::overlays::import::Probe {
    super::overlays::import::Probe {
        path: PathBuf::from("/home/somebody/.config/newsboat/urls"),
        cache: Some(PathBuf::from(
            "/home/somebody/.local/share/newsboat/cache.db",
        )),
        feeds: 41,
        videos: 26,
    }
}

#[test]
fn the_help_the_settings_the_field_and_the_question() {
    let (mut app, _fk) = build("terminal");
    app.key(key('?'));
    insta::assert_snapshot!("help", render(&mut app, 100, 30));

    let (mut app, _fk) = build("terminal");
    app.key(key(','));
    insta::assert_snapshot!("settings", render(&mut app, 100, 30));

    let (mut app, _fk) = build("terminal");
    app.key(key('a'));
    insta::assert_snapshot!("add-feed", render(&mut app, 100, 30));

    let (mut app, mut fk) = build("terminal");
    cursor_to(&mut app, &mut fk, "Phoronix");
    app.key(key('d'));
    insta::assert_snapshot!("confirm-remove", render(&mut app, 100, 30));
}

// -- below the floor ------------------------------------------------------

/// The two ways below the floor, each of which draws one line and nothing
/// else: a column short, and a row short.
#[test]
fn a_terminal_below_the_floor_draws_the_size_message() {
    let (mut app, _fk) = build("terminal");
    insta::assert_snapshot!("floor-59x21", render(&mut app, 59, 21));
    insta::assert_snapshot!("floor-60x20", render(&mut app, 60, 20));
}
