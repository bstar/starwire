//! The window: the loop, and everything that holds state between frames.
//!
//! Synchronous, like STAR/FOLD's and STAR/CORD's: draw, poll for a frame's
//! worth of time, act, repeat. The core is on its own threads and this never
//! sees a future; it drains events once a frame and reads the truth out of
//! [`crate::wire::State`] behind a read lock it drops before drawing.
//!
//! ## The order of a frame
//!
//! 1. [`App::tick`]: drain events, fold a fresh note into place, tell the
//!    core what the stack is now looking at, then [`App::refresh`] if the
//!    version or the stack moved;
//! 2. [`LayoutState::regions`] once, kept in `layout.last`;
//! 3. [`App::draw`];
//! 4. poll for [`FRAME`], and dispatch whatever arrived.
//!
//! ## The stack is the window's, and the core holds one of each
//!
//! The core has no notion of where the reader is looking: it holds *one*
//! open selection and *one* open article. So whenever the active ENTRIES
//! frame changes -- a push, a pop, an `alt+up` -- this sends `OpenFeed`, and
//! whenever the active ARTICLE changes it sends `OpenEntry`.
//! [`App::sync_core`] is the one place that happens, guarded by what was
//! last sent, so a frame where nothing moved sends nothing.
//!
//! ## Key dispatch, outermost first
//!
//! overlay → the `/` filter's text entry → an `o` waiting for a link number
//! → a `g` waiting for its second key → the focused module's own bindings →
//! the global table. See `ui/keymap.rs`'s module doc, which this mirrors
//! exactly: the state that decides which layer a key lands in lives here,
//! and every function in `keymap.rs` is a pure function of one key.
//!
//! ## Graphics before the terminal
//!
//! [`Graphics::probe_if_tty`] writes a capability query and reads the answer
//! off stdin, which only works before raw mode is on. So it happens in
//! [`App::run`], before `term::init`, never inside [`App::new`].

mod draw;
mod keys;
mod mouse;
mod refresh;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use starkit::crossterm::event::{self, Event as TermEvent, KeyEventKind};
use starkit::graphics::{Graphics, Mode};
use starkit::input::TextInput;
use starkit::mouse::ClickTracker;
use starkit::term::{self, Tui};

use super::keymap::Action;
use super::layout::LayoutState;
use super::markdown;
use super::overlays::{self, Overlays};
use super::panels::ModuleId;
use super::stack::{Level, Stack};
use super::theme::{self, Theme};
use super::Bars;
use crate::config::Config;
use crate::session;
use crate::wire::feed::{EntryId, EntryKind, Selection};
use crate::wire::{Command, Event, Handle, NoteLevel, RefreshScope, Setting};

pub use refresh::{SourceKey, ViewData};

/// One frame. Thirty per second, the same ceiling the rest of the family
/// settled on.
pub const FRAME: Duration = Duration::from_millis(33);

/// How many events one frame will take before it draws anyway. Events carry
/// no state (see `wire/handle.rs`'s module doc), so whatever is left in the
/// channel is still true next frame.
pub const DRAIN_CAP: usize = 500;

/// How far `<` and `>` step the reading width, and the range they stay in.
pub const WIDTH_STEP: u16 = 8;
pub const WIDTH_MIN: u16 = 40;
pub const WIDTH_MAX: u16 = 160;

/// Words a minute, for the `6 min` on a byline. Two hundred and twenty is
/// the middle of every study anybody cites and is only ever a rough guide to
/// whether an article is a coffee or an evening.
const WORDS_PER_MINUTE: usize = 220;

pub struct App {
    core: Handle,
    cfg: Config,
    cfg_path: PathBuf,
    session_path: Option<PathBuf>,
    session: session::Session,
    theme: Theme,
    /// The name last resolved through the registry, kept beside the resolved
    /// theme because that carries no name of its own.
    theme_name: String,
    /// Bumped whenever the theme changes, so every laid-out article in the
    /// render cache is a miss at once -- see `markdown::cache::Key`.
    theme_gen: u64,
    graphics: Graphics,
    layout: LayoutState,
    stack: Stack,
    overlays: Overlays,
    /// Some while `/` is being typed.
    filter: Option<TextInput>,
    g_pending: bool,
    /// The digits typed since an `o`, while the chord is open.
    o_pending: Option<String>,
    note: Option<(String, NoteLevel, Instant)>,
    view: ViewData,
    seen_version: u64,
    seen_stack: (usize, usize),
    cache: markdown::cache::Cache,
    /// Where each article was left, keyed by entry so `n` and `p` back and
    /// forth keep every position they have learnt. Seeded from the session
    /// and written back to it on the way out.
    reader_scroll: HashMap<EntryId, usize>,
    bars: Bars,
    clicks: ClickTracker,
    /// What was last sent to the core, so a frame where nothing moved sends
    /// nothing. See the module doc.
    last_open_feed: Option<(Selection, bool)>,
    last_open_entry: Option<EntryId>,
    /// The entry an extraction was last asked for, so a page that never
    /// arrives is asked for once rather than every frame.
    last_extract_for: Option<EntryId>,
    /// Set when `n`/`N` ran off the end of a list and a fresh selection was
    /// sent: the first unread of whatever comes back is what to open.
    pending_open: bool,
    spinner: usize,
    started: Instant,
    /// How many links the article on screen numbers, which is what the `o`
    /// chord counts against. Set by `draw`, read by the next key press.
    links: u16,
    /// Which article was last laid out, and how tall it came to at the
    /// width it was drawn at -- for clamping its scroll and for `G`.
    ///
    /// The entry is carried with the height because `n` swaps the article
    /// under the reader a frame before the next draw measures the new one:
    /// clamping the article that is now open against the height of the one
    /// that was would throw away the position `p` is about to come back to.
    last_render: Option<(EntryId, usize)>,
    /// Three lines derived once a frame in `tick`, so `draw` and the mouse's
    /// own status hit test read the same strings.
    byline_text: Option<String>,
    /// The headline drawn above the article -- see [`head_title`].
    head_title: String,
    progress_line: Option<String>,
    right_line: String,
    /// Throw away what the diff believes is on the screen next frame -- an
    /// overlay that closes leaves cells behind it that nothing else will
    /// think to redraw.
    repaint: bool,
    quit: bool,
    tz: jiff::tz::TimeZone,
    /// Pinned by the frame snapshots so an age reads the same on every
    /// machine; `None` everywhere else, which is always outside a test.
    now_override: Option<jiff::Timestamp>,
}

impl App {
    /// Build the window. Never touches the terminal -- see [`Self::run`],
    /// which is the only thing that does.
    pub fn new(
        core: Handle,
        cfg: Config,
        cfg_path: PathBuf,
        session_path: Option<PathBuf>,
        graphics: Graphics,
    ) -> Self {
        let (theme, _reason) = theme::registry().resolve_named(&cfg.ui.theme);
        let theme_name = cfg.ui.theme.clone();
        let layout = LayoutState::new(cfg.ui.list_rows);
        let session = session_path
            .as_deref()
            .map(session::load)
            .unwrap_or_default();

        let mut app = Self {
            core,
            cfg,
            cfg_path,
            session_path,
            theme,
            theme_name,
            theme_gen: 0,
            graphics,
            layout,
            stack: Stack::new(),
            overlays: Overlays::new(),
            filter: None,
            g_pending: false,
            o_pending: None,
            note: None,
            view: ViewData::default(),
            // Never equal to a fresh `State`'s version, so the first
            // `refresh` always copies rather than seeing "nothing changed".
            seen_version: u64::MAX,
            seen_stack: (usize::MAX, usize::MAX),
            cache: markdown::cache::Cache::new(),
            reader_scroll: HashMap::new(),
            bars: Bars::new(),
            clicks: ClickTracker::new(),
            last_open_feed: None,
            last_open_entry: None,
            last_extract_for: None,
            pending_open: false,
            spinner: 0,
            started: Instant::now(),
            links: 0,
            last_render: None,
            byline_text: None,
            head_title: String::new(),
            progress_line: None,
            right_line: String::new(),
            repaint: true,
            quit: false,
            tz: jiff::tz::TimeZone::system(),
            now_override: None,
            session: session::Session::default(),
        };
        app.restore(session);
        app.refresh();
        app.point_at_restored_source();
        app.open_import_offer();
        app.sync_core();
        app
    }

    /// Put the stack and the reading positions back where the last run left
    /// them. A session naming a feed that has since been removed simply
    /// opens on nothing, which is what the core answers for it anyway.
    fn restore(&mut self, session: session::Session) {
        for (id, row) in &session.positions {
            self.reader_scroll.insert(EntryId(*id), *row);
        }
        if let Some(source) = session.source() {
            self.stack.push(Level::Entries {
                source,
                unread_only: session.unread_only.unwrap_or(false),
            });
            if let Some(entry) = session.last_entry {
                self.stack.push(Level::Article {
                    entry: EntryId(entry),
                });
            }
        }
        self.layout.focus_set(self.stack.active().level.module());
        self.session = session;
    }

    /// Put the SOURCES cursor on the row the restored selection names, when
    /// there is one at the root.
    ///
    /// Without this a restored session draws `▸ sources › All` over a list
    /// of somebody else's feed, which is true -- the cursor really is on
    /// `All` -- and reads as wrong.
    fn point_at_restored_source(&mut self) {
        let Some((source, _)) = self.active_source() else {
            return;
        };
        let at = self
            .view
            .source_keys
            .iter()
            .position(|key| key.selection().as_ref() == Some(&source));
        if let Some(at) = at {
            self.set_cursor(ModuleId::Sources, at);
        }
    }

    /// Offer the import, if the core says there is one to offer.
    fn open_import_offer(&mut self) {
        let Some(offer) = self.view.import_offer.clone() else {
            return;
        };
        let home = std::env::home_dir()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        // The core found the files; the counts are this side's, read off the
        // same `urls` file with the same parser.
        let probe = overlays::import::probe(&home).unwrap_or(overlays::import::Probe {
            path: offer.urls.clone(),
            cache: offer.cache.clone(),
            feeds: 0,
            videos: 0,
        });
        self.overlays
            .open_import(overlays::import::Import::ask(probe));
    }

    /// Take over the terminal and run until something says to stop.
    ///
    /// The probe is before `term::init` and the restore is before the result
    /// is returned, so a failure inside the loop still leaves a usable
    /// terminal behind.
    pub fn run(
        core: Handle,
        cfg: Config,
        cfg_path: PathBuf,
        session_path: Option<PathBuf>,
    ) -> Result<()> {
        // A terminal, before anything else. `term::init` on a pipe fails
        // with the operating system's own words for it -- "No such device
        // or address" -- which says nothing about what to do; every other
        // subcommand works in a pipe, and this is the one that cannot.
        if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            anyhow::bail!(
                "STAR/WIRE's window needs a terminal. \
                 `starwire list`, `fetch` and `show` all work in a pipe; \
                 `starwire --help` has them."
            );
        }

        let graphics = Graphics::probe_if_tty(Mode::parse(&cfg.ui.graphics));
        graphics.log_capabilities();
        let mut app = App::new(core, cfg, cfg_path, session_path, graphics);

        let mut term = term::init()?;
        let result = app.event_loop(&mut term);
        term::restore()?;

        app.save_session();
        app.core.send(Command::Shutdown);
        result
    }

    fn save_session(&mut self) {
        let Some(path) = self.session_path.clone() else {
            return;
        };
        self.remember_position();
        let mut positions: Vec<(i64, usize)> = self
            .session
            .positions
            .iter()
            .copied()
            .filter(|(id, _)| !self.reader_scroll.contains_key(&EntryId(*id)))
            .collect();
        positions.extend(
            self.reader_scroll
                .iter()
                .map(|(id, row)| (id.0, *row))
                .collect::<Vec<_>>(),
        );
        let mut session = session::Session {
            last_source: self
                .active_source()
                .map(|(sel, _)| session::encode_source(&sel)),
            last_entry: self.open_article().map(|id| id.0),
            unread_only: self.active_source().map(|(_, unread_only)| unread_only),
            positions: Vec::new(),
        };
        session.set_positions(positions);
        if let Err(e) = session.save(&path) {
            tracing::warn!("could not save the session: {e}");
        }
    }

    fn event_loop(&mut self, term: &mut Tui) -> Result<()> {
        while !self.quit {
            self.tick();

            if std::mem::take(&mut self.repaint) {
                term.clear()?;
            }
            term.draw(|f| {
                self.draw(f.area(), f.buffer_mut());
            })?;

            if event::poll(FRAME)? {
                match event::read()? {
                    TermEvent::Key(k) if k.kind == KeyEventKind::Press => self.key(k),
                    TermEvent::Mouse(m) => self.mouse(m),
                    // A font zoom arrives as a resize, and it changes the
                    // cell size any built protocol was sized for.
                    TermEvent::Resize(..) => {
                        self.graphics.remeasure();
                        self.repaint = true;
                    }
                    TermEvent::Paste(text) => self.paste(&text),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Everything a frame does before it draws.
    pub fn tick(&mut self) {
        // A tenth of a second a frame, which is the rate the rest of the
        // family spins at and slow enough to read.
        self.spinner = self.started.elapsed().as_millis() as usize / 100;

        let batch: Vec<Event> = self.core.drain().take(DRAIN_CAP).collect();
        for e in batch {
            if let Event::Note(note) = e {
                self.note = Some((note.text, note.level, Instant::now()));
            }
            // Every other event, `Refresh` included, means only "go re-read
            // the truth" -- which `refresh` below does anyway, guarded by
            // the version it already has to check.
        }
        self.sync_core();
        self.refresh();
        self.after_refresh();
        self.head_title = match (self.open_article(), self.view.article.as_ref()) {
            (Some(id), Some(a)) if a.entry == id => head_title(&a.markdown, &a.title),
            _ => String::new(),
        };
        self.byline_text = self.byline();
        self.progress_line = self.build_progress_line();
        self.right_line = self.build_right_line();
    }

    /// Tell the core what the stack is now looking at. The one place
    /// `OpenFeed` and `OpenEntry` are sent -- see the module doc.
    fn sync_core(&mut self) {
        if let Some((source, unread_only)) = self.active_source() {
            let want = (source.clone(), unread_only);
            if self.last_open_feed.as_ref() != Some(&want) {
                // The flag first: `OpenFeed` builds its query out of it, and
                // sending it the other way round would page the list twice.
                if self
                    .last_open_feed
                    .as_ref()
                    .is_none_or(|(_, was)| *was != unread_only)
                {
                    self.core.send(Command::SetUnreadOnly(unread_only));
                }
                self.core.send(Command::OpenFeed(source));
                self.last_open_feed = Some(want);
            }
        }

        let open = self.open_article();
        if self.last_open_entry != open {
            match open {
                Some(id) => self.core.send(Command::OpenEntry(id)),
                None => self.core.send(Command::CloseEntry),
            }
            self.last_open_entry = open;
            self.last_extract_for = None;
        }
    }

    /// The two things a fresh `ViewData` can imply.
    fn after_refresh(&mut self) {
        // The import overlay is waiting for its report.
        if let Some(report) = self.view.last_import.clone() {
            if let Some(overlays::Overlay::Import(i)) = self.overlays.current_mut() {
                if i.is_waiting() {
                    i.done(report);
                    self.repaint = true;
                }
            }
        }

        // A list that has just arrived after `n` ran off the end of the last
        // one: open its first unread.
        if self.pending_open && !self.view.entries_loading {
            self.pending_open = false;
            if let Some(i) = self.view.entry_read.iter().position(|read| !*read) {
                let id = self.view.entry_ids[i];
                self.set_cursor(ModuleId::Entries, i);
                self.open_entry(id);
            } else {
                self.say("nothing unread");
            }
        }

        // An article whose page never arrived: ask once, and only when the
        // core has nothing of its own in flight -- which is what makes a
        // dropped job cost a frame rather than an article.
        if let (Some(id), Some(article)) = (self.open_article(), self.view.article.as_ref()) {
            let stalled = article.status.is_pending()
                && self.view.extract_on
                && !self.view.extract_busy
                && self.last_extract_for != Some(id);
            if stalled && article.entry == id {
                self.last_extract_for = Some(id);
                self.core.send(Command::Extract(id));
            }
        }
    }

    // -- where things are ---------------------------------------------------

    /// The selection the active ENTRIES frame is looking at, if there is
    /// one.
    pub(super) fn active_source(&self) -> Option<(Selection, bool)> {
        let i = self.stack.top_of(ModuleId::Entries)?;
        match &self.stack.frames()[i].level {
            Level::Entries {
                source,
                unread_only,
            } => Some((source.clone(), *unread_only)),
            _ => None,
        }
    }

    /// The entry the active ARTICLE frame is looking at.
    pub(super) fn open_article(&self) -> Option<EntryId> {
        let i = self.stack.top_of(ModuleId::Reader)?;
        match &self.stack.frames()[i].level {
            Level::Article { entry } => Some(*entry),
            _ => None,
        }
    }

    /// The frame a module draws, if it has one.
    fn frame_of(&self, m: ModuleId) -> Option<&super::stack::Frame> {
        self.stack.top_of(m).map(|i| &self.stack.frames()[i])
    }

    fn cursor_of(&self, m: ModuleId) -> usize {
        self.frame_of(m).map(|f| f.cursor).unwrap_or(0)
    }

    fn scroll_of(&self, m: ModuleId) -> usize {
        self.frame_of(m).map(|f| f.scroll).unwrap_or(0)
    }

    fn rows_of(&self, m: ModuleId) -> usize {
        match m {
            ModuleId::Sources => self.view.source_rows.len(),
            ModuleId::Entries => self.view.entry_rows.len(),
            ModuleId::Reader => 0,
        }
    }

    fn set_cursor(&mut self, m: ModuleId, to: usize) {
        let key = match m {
            ModuleId::Sources => self.view.source_keys.get(to).map(|k| k.stable()),
            ModuleId::Entries => self.view.entry_ids.get(to).map(|id| id.0.to_string()),
            ModuleId::Reader => None,
        };
        let Some(i) = self.stack.top_of(m) else {
            return;
        };
        let at = self.stack.active_index();
        self.stack.jump_to(i);
        let frame = self.stack.active_mut();
        frame.cursor = to;
        frame.cursor_key = key;
        self.stack.jump_to(at);
    }

    /// The entry a key like `m`, `s` or `o` applies to: the one under the
    /// cursor in ENTRIES, or the open article when the reader has the
    /// keyboard.
    fn target_entry(&self) -> Option<EntryId> {
        match self.layout.focus() {
            ModuleId::Reader => self.open_article(),
            _ => self
                .view
                .entry_ids
                .get(self.cursor_of(ModuleId::Entries))
                .copied(),
        }
    }

    fn entry_kind(&self, id: EntryId) -> Option<EntryKind> {
        let i = self.view.entry_ids.iter().position(|e| *e == id)?;
        Some(self.view.entry_rows[i].kind)
    }

    /// How tall the open article came to, if that is what was last laid
    /// out. `None` before the first draw, and between `n` and the draw that
    /// measures what it opened.
    pub(super) fn rendered_height(&self) -> Option<usize> {
        let open = self.open_article()?;
        match self.last_render {
            Some((entry, height)) if entry == open => Some(height),
            _ => None,
        }
    }

    pub(super) fn now(&self) -> jiff::Timestamp {
        self.now_override.unwrap_or_else(jiff::Timestamp::now)
    }

    pub(super) fn say(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), NoteLevel::Info, Instant::now()));
    }

    fn warn(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), NoteLevel::Warning, Instant::now()));
    }

    // -- moving about -------------------------------------------------------

    /// Put the keyboard on `m` by jumping the stack to whatever frame `m`
    /// draws. Focus and the stack agree by construction; a module holding no
    /// frame says so rather than silently doing nothing.
    pub fn focus(&mut self, m: ModuleId) {
        match self.stack.top_of(m) {
            Some(i) => {
                self.stack.jump_to(i);
                self.layout.focus_set(m);
            }
            None => {
                let what = match m {
                    ModuleId::Sources => "the sources are always there",
                    ModuleId::Entries => "choose a source first",
                    ModuleId::Reader => "nothing open",
                };
                self.say(what);
            }
        }
    }

    fn focus_step(&mut self, forward: bool) {
        let order = super::panels::COLUMN;
        let here = self.layout.focus().index();
        for step in 1..=order.len() {
            let i = if forward {
                (here + step) % order.len()
            } else {
                (here + order.len() - step) % order.len()
            };
            if self.stack.top_of(order[i]).is_some() {
                self.focus(order[i]);
                return;
            }
        }
    }

    /// Keep the focused module's own frame pointing where the stack now is.
    fn sync_focus(&mut self) {
        let m = self.stack.active().level.module();
        self.layout.focus_set(m);
    }

    fn move_cursor(&mut self, delta: i64) {
        let m = self.layout.focus();
        if m == ModuleId::Reader {
            self.scroll_reader(delta);
            return;
        }
        let len = self.rows_of(m);
        if len == 0 {
            return;
        }
        let next = (self.cursor_of(m) as i64 + delta).clamp(0, len as i64 - 1) as usize;
        self.set_cursor(m, next);
        self.clamp_scrolls();
    }

    fn move_to_edge(&mut self, top: bool) {
        let m = self.layout.focus();
        if m == ModuleId::Reader {
            let to = if top {
                0
            } else {
                self.rendered_height().unwrap_or(0)
            };
            self.set_reader_scroll(to);
            return;
        }
        let len = self.rows_of(m);
        if len == 0 {
            return;
        }
        self.set_cursor(m, if top { 0 } else { len - 1 });
        self.clamp_scrolls();
    }

    fn scroll_reader(&mut self, delta: i64) {
        let at = self.open_article().map(|id| self.reader_scroll_of(id));
        let Some(at) = at else { return };
        self.set_reader_scroll((at as i64 + delta).max(0) as usize);
    }

    fn reader_scroll_of(&self, id: EntryId) -> usize {
        self.reader_scroll.get(&id).copied().unwrap_or(0)
    }

    fn set_reader_scroll(&mut self, to: usize) {
        let Some(id) = self.open_article() else {
            return;
        };
        self.reader_scroll.insert(id, to);
        self.clamp_scrolls();
    }

    /// Where the open article was left, written back into the session's own
    /// list.
    fn remember_position(&mut self) {
        if let Some(id) = self.open_article() {
            let at = self.reader_scroll_of(id);
            self.session.remember(id, at);
        }
    }

    // -- opening things -----------------------------------------------------

    /// `enter` or a double-click on whatever the focused module is showing.
    fn activate(&mut self) {
        match self.layout.focus() {
            ModuleId::Sources => self.open_source(false),
            ModuleId::Entries => {
                if let Some(id) = self
                    .view
                    .entry_ids
                    .get(self.cursor_of(ModuleId::Entries))
                    .copied()
                {
                    self.open_entry(id);
                }
            }
            // `o` is what opens an article somewhere else; `enter` in the
            // reader has nothing left to do, and says so rather than
            // pretending.
            ModuleId::Reader => self.say("o opens it in the browser"),
        }
    }

    /// `l`/`right`: drill in. The only place it differs from `enter` is a
    /// folder -- `enter` reads the whole folder, `l` opens its feeds.
    fn drill(&mut self) {
        match self.layout.focus() {
            ModuleId::Sources => self.open_source(true),
            _ => self.activate(),
        }
    }

    fn open_source(&mut self, into_folder: bool) {
        let cursor = self.cursor_of(ModuleId::Sources);
        let Some(key) = self.view.source_keys.get(cursor).cloned() else {
            return;
        };
        if key == SourceKey::Search {
            self.open_search();
            return;
        }
        if into_folder {
            if let SourceKey::Folder(id) = key {
                self.focus(ModuleId::Sources);
                self.stack.push(Level::Sources { folder: Some(id) });
                self.sync_focus();
                return;
            }
        }
        let Some(source) = key.selection() else {
            return;
        };
        let unread_only = key == SourceKey::Unread;
        self.focus(ModuleId::Sources);
        self.stack.push(Level::Entries {
            source,
            unread_only,
        });
        self.sync_focus();
    }

    /// Open an entry in the reader.
    ///
    /// Replacing rather than pushing when an article is already open is what
    /// keeps `FrameId`s from churning under `n`/`p`, which is what keeps the
    /// per-entry reading positions -- see `ui/stack.rs`'s module doc.
    pub fn open_entry(&mut self, id: EntryId) {
        self.remember_position();
        let at_reader = self
            .stack
            .top_of(ModuleId::Reader)
            .is_some_and(|i| i == self.stack.len() - 1);
        if at_reader {
            self.stack.jump_to(self.stack.len() - 1);
            self.stack.replace_active(Level::Article { entry: id });
        } else {
            // From wherever the cursor is: the entries frame is what an
            // article hangs off.
            if let Some(i) = self.stack.top_of(ModuleId::Entries) {
                self.stack.jump_to(i);
            }
            self.stack.push(Level::Article { entry: id });
        }
        self.sync_focus();

        // Where it was left, if this run or the last one knows.
        if !self.reader_scroll.contains_key(&id) {
            if let Some(row) = self.session.position(id) {
                self.reader_scroll.insert(id, row);
            }
        }

        if self.cfg.reading.mark_read_on_open {
            self.core.send(Command::SetRead {
                entries: vec![id],
                read: true,
            });
        }
    }

    /// `n`/`p` in the reader, and `n`/`p` in the list.
    ///
    /// `unread_only` is the `N` case: when it runs off the end of the loaded
    /// rows it crosses into the next source that has something unread in it,
    /// which is a fresh `OpenFeed` and therefore a frame or two away --
    /// [`App::pending_open`] is what remembers to open the first unread of
    /// whatever comes back.
    pub fn next_entry(&mut self, delta: i64, unread_only: bool) {
        let ids = self.view.entry_ids.clone();
        if ids.is_empty() {
            return;
        }
        let here = match self.open_article() {
            Some(id) => ids.iter().position(|e| *e == id),
            None => Some(self.cursor_of(ModuleId::Entries)),
        };
        let mut i = here.unwrap_or(0) as i64;
        loop {
            i += delta;
            if i < 0 || i >= ids.len() as i64 {
                if unread_only {
                    self.cross_sources();
                } else {
                    self.say(if delta > 0 {
                        "the last one"
                    } else {
                        "the first one"
                    });
                }
                return;
            }
            let at = i as usize;
            if !unread_only || !self.view.entry_read[at] {
                self.set_cursor(ModuleId::Entries, at);
                if self.open_article().is_some() {
                    self.open_entry(ids[at]);
                }
                self.clamp_scrolls();
                return;
            }
        }
    }

    /// The next feed with something unread in it, wrapping -- what `N` does
    /// once the list it is in has nothing left.
    fn cross_sources(&mut self) {
        let state = self.core.state();
        let feeds: Vec<(crate::wire::feed::FeedId, i64)> =
            state.feeds.iter().map(|f| (f.id, f.unread)).collect();
        drop(state);

        let here = match self.active_source() {
            Some((Selection::Feed(id), _)) => feeds.iter().position(|(f, _)| *f == id),
            _ => None,
        };
        let start = here.map(|i| i + 1).unwrap_or(0);
        let n = feeds.len();
        for step in 0..n {
            let (id, unread) = feeds[(start + step) % n];
            if unread > 0 {
                self.focus(ModuleId::Entries);
                self.stack.replace_active(Level::Entries {
                    source: Selection::Feed(id),
                    unread_only: true,
                });
                self.sync_focus();
                self.pending_open = true;
                return;
            }
        }
        self.say("nothing unread anywhere");
    }

    // -- one action ---------------------------------------------------------

    /// The single place a key or a click becomes a change.
    pub fn act(&mut self, a: Action) {
        match a {
            Action::CursorUp => self.move_cursor(-1),
            Action::CursorDown => self.move_cursor(1),
            Action::CursorUpBig => self.move_cursor(-10),
            Action::CursorDownBig => self.move_cursor(10),
            Action::PageUp => {
                let n = self.page_size();
                self.move_cursor(-n);
            }
            Action::PageDown => {
                let n = self.page_size();
                self.move_cursor(n);
            }
            Action::Home => self.move_to_edge(true),
            Action::End => self.move_to_edge(false),
            Action::Activate => self.activate(),
            Action::Back => {
                if self.filter.is_some() {
                    self.filter = None;
                    self.core.send(Command::ClearFilter);
                }
                self.o_pending = None;
                self.g_pending = false;
            }

            Action::FocusNext => self.focus_step(true),
            Action::FocusPrev => self.focus_step(false),
            Action::FocusSources => self.focus(ModuleId::Sources),
            Action::FocusEntries => self.focus(ModuleId::Entries),
            Action::FocusReader => self.focus(ModuleId::Reader),

            Action::Enter => self.drill(),
            Action::Pop => self.pop(),
            Action::JumpUp => {
                let at = self.stack.active_index();
                if at > 0 {
                    self.stack.jump_to(at - 1);
                    self.sync_focus();
                }
            }
            Action::JumpDown => {
                if self.stack.forward() {
                    self.sync_focus();
                }
            }

            Action::Search => self.open_search(),
            Action::Filter => {
                self.filter = Some(TextInput::single().with_text(self.view.filter.clone()));
                self.focus(ModuleId::Entries);
            }

            Action::RefreshSource => self.refresh_source(),
            Action::RemoveFeed => self.confirm_remove_feed(),
            Action::MarkSourceRead | Action::MarkAllRead => self.confirm_mark_read(),

            Action::ToggleRead => self.toggle_read(),
            Action::ToggleStar => self.toggle_star(),
            Action::OpenBrowser => self.open_external(),
            Action::CopyLink => self.copy_link(),
            Action::PlayVideo => self.play_video(),
            Action::NextUnread => self.next_entry(1, true),
            Action::PrevUnread => self.next_entry(-1, true),
            Action::UnreadOnly => self.toggle_unread_only(),

            Action::NextEntry => self.next_entry(1, false),
            Action::PrevEntry => self.next_entry(-1, false),
            // Reached through `o_prefix`, never through the table.
            Action::OpenLink => {}
            Action::Extract => {
                if let Some(id) = self.open_article() {
                    self.last_extract_for = Some(id);
                    self.core.send(Command::Extract(id));
                    self.say("pulling the page again");
                }
            }
            Action::Narrower => self.step_width(-(WIDTH_STEP as i32)),
            Action::Wider => self.step_width(WIDTH_STEP as i32),
            Action::CloseArticle => {
                if self.open_article().is_some() {
                    self.focus(ModuleId::Reader);
                    self.pop();
                }
            }

            Action::AddFeed => {
                self.overlays.open_add_feed();
                self.repaint = true;
            }
            Action::RefreshAll => {
                self.core.send(Command::Refresh(RefreshScope::All));
                self.say("refreshing");
            }
            Action::Import => self.open_import(),

            Action::NextTheme => self.cycle_theme(true),
            Action::PrevTheme => self.cycle_theme(false),
            Action::Settings => {
                self.overlays.open_settings();
                self.repaint = true;
            }

            Action::Help => {
                self.overlays.open_help();
                self.repaint = true;
            }
            Action::Quit => self.quit = true,
            Action::Redraw => self.repaint = true,
        }
        self.clamp_scrolls();
    }

    fn pop(&mut self) {
        self.remember_position();
        if self.stack.pop() {
            self.sync_focus();
        }
    }

    fn page_size(&self) -> i64 {
        let Some(regions) = &self.layout.last else {
            return 10;
        };
        let m = self.layout.focus();
        let rect = regions.rect_of(m);
        let rows = match m {
            ModuleId::Sources => {
                super::panels::sources::visible_rows(rect, !self.layout.is_open(m))
            }
            ModuleId::Entries => {
                super::panels::entries::visible_rows(rect, !self.layout.is_open(m))
            }
            ModuleId::Reader => usize::from(starkit::chrome::header::body(rect).height),
        };
        i64::try_from(rows).unwrap_or(10).max(1)
    }

    fn open_search(&mut self) {
        let query = match self.active_source() {
            Some((Selection::Search(q), _)) => q,
            _ => String::new(),
        };
        self.overlays.open_search(&query);
        self.repaint = true;
    }

    fn open_import(&mut self) {
        let home = std::env::home_dir()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let overlay = match overlays::import::probe(&home) {
            Some(probe) => overlays::import::Import::ask(probe),
            // No newsboat here, so the other way a list arrives: a path to
            // an OPML file.
            None => overlays::import::Import::opml(),
        };
        self.overlays.open_import(overlay);
        self.repaint = true;
    }

    fn refresh_source(&mut self) {
        let scope = match self.layout.focus() {
            ModuleId::Sources => match self.view.source_keys.get(self.cursor_of(ModuleId::Sources))
            {
                Some(SourceKey::Feed(id)) => RefreshScope::Feed(*id),
                Some(SourceKey::Folder(id)) => RefreshScope::Folder(*id),
                _ => RefreshScope::All,
            },
            _ => match self.active_source() {
                Some((Selection::Feed(id), _)) => RefreshScope::Feed(id),
                Some((Selection::Folder(id), _)) => RefreshScope::Folder(id),
                _ => RefreshScope::All,
            },
        };
        self.core.send(Command::Refresh(scope));
        self.say("refreshing");
    }

    fn confirm_remove_feed(&mut self) {
        let cursor = self.cursor_of(ModuleId::Sources);
        let Some(SourceKey::Feed(id)) = self.view.source_keys.get(cursor).cloned() else {
            self.warn("that is not a feed");
            return;
        };
        let name = self.view.source_rows[cursor].name.clone();
        self.overlays
            .open_confirm(overlays::confirm::Confirm::remove_feed(id, &name));
        self.repaint = true;
    }

    fn confirm_mark_read(&mut self) {
        let (sel, name) = match self.layout.focus() {
            ModuleId::Sources => {
                let cursor = self.cursor_of(ModuleId::Sources);
                let Some(key) = self.view.source_keys.get(cursor).cloned() else {
                    return;
                };
                let Some(sel) = key.selection() else { return };
                (sel, self.view.source_rows[cursor].name.clone())
            }
            _ => match self.active_source() {
                Some((sel, _)) => (sel, self.view.entry_source.clone()),
                None => return,
            },
        };
        let n = self.unread_in(&sel);
        if n == 0 {
            self.say("nothing unread there");
            return;
        }
        self.overlays
            .open_confirm(overlays::confirm::Confirm::mark_all_read(sel, &name, n));
        self.repaint = true;
    }

    fn unread_in(&self, sel: &Selection) -> i64 {
        let state = self.core.state();
        match sel {
            Selection::Feed(id) => state.feed(*id).map(|f| f.unread).unwrap_or(0),
            Selection::Folder(id) => state
                .feeds
                .iter()
                .filter(|f| f.folder == Some(*id))
                .map(|f| f.unread)
                .sum(),
            Selection::All => state.unread_total(),
            // Starred, Videos and a search have no count of their own in
            // `State`; the rows on screen are what there is to go on.
            _ => state.page.rows.iter().filter(|r| !r.read).count() as i64,
        }
    }

    fn toggle_read(&mut self) {
        let Some(id) = self.target_entry() else {
            return;
        };
        let read = self
            .view
            .entry_ids
            .iter()
            .position(|e| *e == id)
            .map(|i| self.view.entry_read[i])
            .unwrap_or(false);
        self.core.send(Command::SetRead {
            entries: vec![id],
            read: !read,
        });
    }

    fn toggle_star(&mut self) {
        let Some(id) = self.target_entry() else {
            return;
        };
        let on = self
            .view
            .entry_ids
            .iter()
            .position(|e| *e == id)
            .map(|i| !self.view.entry_rows[i].starred)
            .unwrap_or(true);
        self.core.send(Command::SetStarred { entry: id, on });
    }

    fn open_external(&mut self) {
        match self.target_entry() {
            Some(id) => {
                self.core.send(Command::OpenExternal(id));
                self.say("opening");
            }
            None => self.warn("nothing to open"),
        }
    }

    fn play_video(&mut self) {
        let Some(id) = self.target_entry() else {
            self.warn("nothing to play");
            return;
        };
        if self.entry_kind(id) != Some(EntryKind::Video) {
            self.warn("that is not a video");
            return;
        }
        self.core.send(Command::OpenExternal(id));
        self.say("playing");
    }

    fn copy_link(&mut self) {
        let Some(id) = self.target_entry() else {
            return;
        };
        let url = {
            let state = self.core.state();
            state
                .page
                .row(id)
                .and_then(|r| r.url.clone())
                .or_else(|| state.article.as_ref().and_then(|a| a.url.clone()))
        };
        match url {
            Some(url) => match super::clipboard::copy(&url) {
                Ok(()) => self.say("link copied"),
                Err(e) => self.warn(format!("no clipboard: {e}")),
            },
            None => self.warn("that one has no link"),
        }
    }

    fn toggle_unread_only(&mut self) {
        let Some((source, unread_only)) = self.active_source() else {
            self.say("choose a source first");
            return;
        };
        self.focus(ModuleId::Entries);
        self.stack.replace_active(Level::Entries {
            source,
            unread_only: !unread_only,
        });
        self.sync_focus();
        self.say(if unread_only {
            "everything"
        } else {
            "unread only"
        });
    }

    fn step_width(&mut self, delta: i32) {
        let next = (i32::from(self.cfg.reading.width) + delta)
            .clamp(i32::from(WIDTH_MIN), i32::from(WIDTH_MAX)) as u16;
        if next == self.cfg.reading.width {
            return;
        }
        self.cfg.reading.width = next;
        self.write_setting(
            "reading",
            "width",
            starkit::config::edit::Value::Int(i64::from(next)),
        );
        self.say(format!("{next} columns"));
    }

    /// Rewrite one line of `config.toml`, leaving every comment in it alone.
    pub(super) fn write_setting(
        &mut self,
        section: &str,
        key: &str,
        value: starkit::config::edit::Value,
    ) {
        if let Err(e) = starkit::config::edit::set(&self.cfg_path, section, key, &value) {
            tracing::warn!("could not save {section}.{key}: {e}");
            self.warn(format!("could not save {key}"));
        }
    }

    pub fn cycle_theme(&mut self, forward: bool) {
        let ids = theme::registry().selectable();
        if ids.is_empty() {
            return;
        }
        let current = ids
            .iter()
            .position(|id| *id == self.theme_name)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % ids.len()
        } else {
            (current + ids.len() - 1) % ids.len()
        };
        let name = ids[next].clone();
        self.set_theme(&name);
        self.write_setting(
            "ui",
            "theme",
            starkit::config::edit::Value::Str(name.clone()),
        );
        self.say(format!("theme: {name}"));
    }

    pub(super) fn set_theme(&mut self, name: &str) {
        let (theme, _reason) = theme::registry().resolve_named(name);
        self.theme = theme;
        self.theme_name = name.to_string();
        self.cfg.ui.theme = name.to_string();
        // Every laid-out article in the cache was laid out in the old
        // colours; bumping the generation makes each of them a miss.
        self.theme_gen += 1;
        self.repaint = true;
    }

    /// Apply whatever a settings row just changed, now that `cfg` holds it.
    pub(super) fn apply_setting(&mut self, setting: overlays::settings::Setting) {
        use overlays::settings::Setting as S;
        match setting {
            S::Theme => {
                let name = self.cfg.ui.theme.clone();
                self.set_theme(&name);
            }
            S::ReadingWidth | S::Byline => self.repaint = true,
            S::MarkReadOnOpen => {}
            S::Extract => self.core.send(Command::SetSetting(Setting::Extract(
                self.cfg.articles.extract,
            ))),
            S::RefreshEvery => self.core.send(Command::SetSetting(Setting::RefreshMinutes(
                self.cfg.fetch.refresh_minutes,
            ))),
            S::Graphics => {
                let mode = Mode::parse(&self.cfg.ui.graphics);
                if !self.graphics.set_mode(mode) {
                    self.say("graphics: on restart");
                }
                self.repaint = true;
            }
        }
    }

    /// What an overlay answered.
    pub(super) fn after_overlay_answer(&mut self, answer: overlays::Answer) {
        use overlays::Answer as A;
        match answer {
            A::Consumed | A::Closed => {}
            A::Confirmed(pending) => match pending {
                overlays::Pending::RemoveFeed(id) => {
                    self.core.send(Command::RemoveFeed(id));
                    self.say("removed");
                }
                overlays::Pending::MarkAllRead(sel) => {
                    self.core.send(Command::MarkAllRead(sel));
                }
            },
            A::AddFeed(url) => {
                self.core.send(Command::AddFeed {
                    url,
                    title: None,
                    folder: None,
                });
            }
            A::Search(q) => {
                self.focus(ModuleId::Sources);
                self.stack.push(Level::Entries {
                    source: Selection::Search(q),
                    unread_only: false,
                });
                self.sync_focus();
            }
            A::ImportNewsboat { urls, cache } => {
                self.core.send(Command::ImportNewsboat { urls, cache });
            }
            A::ImportOpml(path) => {
                self.core.send(Command::ImportOpml(path));
                self.say("importing");
            }
            A::DismissImport => self.core.send(Command::DismissImportOffer),
            A::Setting(setting, forward) => {
                let themes = theme::registry().selectable();
                let value = setting.step(&mut self.cfg, forward, &themes);
                let (section, key) = setting.where_written();
                self.write_setting(section, key, value);
                self.apply_setting(setting);
            }
            A::Quit => self.quit = true,
        }
        // Closing an overlay can uncover a panel that draws differently from
        // what is on screen, and a half-typed field repaints on every
        // keystroke -- cheap next to the cost of a frame that is wrong.
        self.repaint = true;
    }

    // -- the reader's text --------------------------------------------------

    /// The open article, laid out for the width the reader is drawing at.
    ///
    /// Through the cache, keyed by the entry, when its text was last
    /// written, the width and which theme is up -- so `n` and `p` back and
    /// forth over a handful of articles, and a `<` that changes the width,
    /// all stay warm.
    pub(super) fn rendered(
        &mut self,
        width: u16,
    ) -> Option<std::sync::Arc<markdown::layout::Rendered>> {
        let article = self.view.article.clone()?;
        if width == 0 {
            return None;
        }
        let key = markdown::cache::Key {
            entry: article.entry,
            extracted_at: article.extracted_at,
            width,
            theme_gen: self.theme_gen,
        };
        let theme = self.theme.clone();
        let text = article.markdown.clone();
        let title = article.title.clone();
        let rendered = self.cache.get_or_insert_with(key, || {
            let mut doc = markdown::parse::parse(&text);
            drop_repeated_title(&mut doc, &title);
            markdown::layout::layout(
                &doc,
                &markdown::layout::LayoutCtx {
                    theme: &theme,
                    width,
                },
            )
        });
        self.last_render = Some((article.entry, rendered.lines.len()));
        Some(rendered)
    }

    /// `author · site · date · 6 min`, as much of it as there is.
    pub(super) fn byline(&self) -> Option<String> {
        if !self.cfg.reading.show_byline {
            return None;
        }
        let a = self.view.article.as_ref()?;
        let mut parts: Vec<String> = Vec::new();
        if let Some(author) = &a.byline {
            parts.push(author.clone());
        }
        if let Some(site) = &a.site_name {
            parts.push(site.clone());
        }
        if let Some(at) = self.view.open_published {
            let zoned = at.to_zoned(self.tz.clone());
            let date = zoned.strftime("%-d %b %Y").to_string();
            // Readability's byline is whatever the page put under the
            // headline, and on a news site that is usually "by Somebody, 20
            // September 2026". Printing our own date after it says the same
            // thing twice in two formats, so the year is what decides: a
            // byline that already carries this article's year has a date in
            // it.
            let year = zoned.strftime("%Y").to_string();
            if !a.byline.as_deref().is_some_and(|b| b.contains(&year)) {
                parts.push(date);
            }
        }
        let words = a.markdown.split_whitespace().count();
        if words > 0 {
            parts.push(format!("{} min", (words / WORDS_PER_MINUTE).max(1)));
        }
        (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
    }

    // -- frame snapshots ----------------------------------------------------
    //
    // Four small hooks `ui/frames.rs` needs and nothing else does: a frame
    // has to be pinned to a fixed clock and zone to be deterministic, and it
    // needs to read `ViewData` to find a row by name.

    #[cfg(test)]
    pub(crate) fn view(&self) -> &ViewData {
        &self.view
    }

    #[cfg(test)]
    pub(crate) fn stack(&self) -> &Stack {
        &self.stack
    }

    #[cfg(test)]
    pub(crate) fn set_tz(&mut self, tz: jiff::tz::TimeZone) {
        self.tz = tz;
        self.seen_version = u64::MAX;
        self.refresh();
    }

    #[cfg(test)]
    pub(crate) fn set_now(&mut self, now: jiff::Timestamp) {
        self.now_override = Some(now);
        self.seen_version = u64::MAX;
        self.refresh();
    }

    /// Open the import overlay on a probe of the caller's own, so a frame
    /// does not depend on whether the machine running it has newsboat.
    #[cfg(test)]
    pub(crate) fn open_import_for_tests(&mut self, probe: overlays::import::Probe) {
        self.overlays
            .open_import(overlays::import::Import::ask(probe));
    }

    /// Hand the waiting import overlay its report, which a real run gets
    /// from `State::last_import` a millisecond later.
    #[cfg(test)]
    pub(crate) fn import_report_for_tests(&mut self, report: crate::wire::import::ImportReport) {
        if let Some(overlays::Overlay::Import(i)) = self.overlays.current_mut() {
            i.done(report);
        }
    }

    #[cfg(test)]
    pub(crate) fn note_text(&self) -> Option<&str> {
        self.note.as_ref().map(|(t, _, _)| t.as_str())
    }
}

/// Take the article's own opening heading off when it says what the panel
/// has already written above it.
///
/// Readability keeps the `<h1>` in the body *and* reports it as the title,
/// so a real extraction carries the headline twice -- and the two spellings
/// are often not identical: the stored title is frequently the `<title>`
/// tag, which is the headline without its subtitle. So [`same_headline`]
/// accepts a prefix either way, and the reader draws whichever of the two is
/// the fuller ([`head_title`]).
///
/// Only the *first* block, and only a heading at the top two levels: a page
/// that puts its own name in the `h1` leaves the headline as an `h2`, which
/// is what `testdata/pages/article.html` does and what the first real run
/// against `blog.rust-lang.org` did. A heading further down is part of the
/// article whatever it says.
fn drop_repeated_title(doc: &mut markdown::parse::Doc, title: &str) {
    let Some(markdown::parse::Block::Heading { level, inlines }) = doc.blocks.first() else {
        return;
    };
    if *level > 2 {
        return;
    }
    let heading: String = inlines.iter().map(|i| i.text.as_str()).collect();
    if same_headline(&heading, title) {
        doc.blocks.remove(0);
    }
}

/// The headline to draw above the article: the article's own opening `h1`
/// when it is the fuller spelling of the stored title, and the stored title
/// otherwise.
///
/// A line scan rather than a parse: this runs every frame, and the answer
/// only ever depends on the first non-blank line.
pub(super) fn head_title(markdown: &str, title: &str) -> String {
    let Some(first) = markdown.lines().find(|l| !l.trim().is_empty()) else {
        return title.to_string();
    };
    let trimmed = first.trim();
    let Some(heading) = trimmed
        .strip_prefix("# ")
        .or_else(|| trimmed.strip_prefix("## "))
    else {
        return title.to_string();
    };
    let heading = heading.trim();
    if same_headline(heading, title) && squash(heading).len() > squash(title).len() {
        return heading.to_string();
    }
    title.to_string()
}

/// Whether two spellings of a headline are the same headline: equal, or one
/// the beginning of the other.
fn same_headline(a: &str, b: &str) -> bool {
    let (a, b) = (squash(a), squash(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a.starts_with(&b) || b.starts_with(&a)
}

/// Lower-cased with every run of whitespace reduced to one space, which is
/// the only difference worth ignoring between a title and its heading.
fn squash(text: &str) -> String {
    text.split_whitespace()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::fake;
    use crate::ui::overlays::Overlay;
    use starkit::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use starkit::ratatui::buffer::Buffer;
    use starkit::ratatui::layout::Rect;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    /// An app over the fixture, in a fresh directory of its own so a test
    /// that writes a setting writes it there and nowhere near a real config.
    fn app() -> (App, fake::Fake, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cfg = Config::default();
        let (core, mut fk) = fake::handle(&cfg.core());
        let mut app = App::new(
            core,
            cfg,
            dir.path().join("config.toml"),
            None,
            Graphics::disabled(),
        );
        app.set_now(crate::wire::testing::now());
        settle(&mut app, &mut fk);
        (app, fk, dir)
    }

    fn settle(app: &mut App, fk: &mut fake::Fake) {
        for _ in 0..3 {
            app.tick();
            fk.pump();
        }
        app.tick();
    }

    /// Draw once, so `layout.last` exists and the panels' geometry is known
    /// -- a click and a page key both read it.
    fn draw(app: &mut App) -> String {
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        app.draw(area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn source_row(app: &App, needle: &str) -> usize {
        app.view
            .source_rows
            .iter()
            .position(|r| r.name.contains(needle))
            .unwrap_or_else(|| panic!("no source row holding {needle}"))
    }

    fn goto_source(app: &mut App, fk: &mut fake::Fake, needle: &str) {
        app.key(code(KeyCode::Home));
        settle(app, fk);
        for _ in 0..source_row(app, needle) {
            app.key(key('j'));
        }
        settle(app, fk);
    }

    fn into_feed(app: &mut App, fk: &mut fake::Fake, folder: &str, feed: &str) {
        goto_source(app, fk, folder);
        app.key(key('l'));
        settle(app, fk);
        goto_source(app, fk, feed);
        app.key(code(KeyCode::Enter));
        settle(app, fk);
    }

    #[test]
    fn a_new_app_draws_the_heading_the_fixture_and_the_help_hint() {
        let (mut app, _fk, _dir) = app();
        let text = draw(&mut app);
        assert!(text.contains("S T A R / W I R E"), "{text}");
        assert!(
            text.contains("Hacker News") || text.contains("Tech"),
            "{text}"
        );
        assert!(text.contains("? help"), "{text}");
    }

    /// The column only ever goes down, and focus follows the stack.
    #[test]
    fn drilling_into_a_folder_then_a_feed_walks_the_column_down() {
        let (mut app, mut fk, _dir) = app();
        assert_eq!(app.layout.focus(), ModuleId::Sources);
        goto_source(&mut app, &mut fk, "Tech");
        app.key(key('l'));
        settle(&mut app, &mut fk);
        assert_eq!(app.layout.focus(), ModuleId::Sources, "a folder is sources");
        assert_eq!(app.stack.len(), 2);

        goto_source(&mut app, &mut fk, "Hacker News");
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        assert_eq!(app.layout.focus(), ModuleId::Entries);
        assert!(matches!(
            app.active_source(),
            Some((Selection::Feed(_), false))
        ));
        assert_eq!(app.view.entry_rows.len(), 3, "the fixture's HN entries");
    }

    /// `enter` on a folder reads the whole folder; `l` opens its feeds. Two
    /// keys, two answers, and `docs/the-stack.md` says which is which.
    #[test]
    fn enter_on_a_folder_reads_it_and_l_opens_it() {
        let (mut app, mut fk, _dir) = app();
        goto_source(&mut app, &mut fk, "Tech");
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        assert!(matches!(
            app.active_source(),
            Some((Selection::Folder(_), _))
        ));
    }

    #[test]
    fn opening_an_entry_marks_it_read_and_esc_closes_it() {
        let (mut app, mut fk, _dir) = app();
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        assert!(app.view.entry_read.iter().all(|r| !*r));

        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        assert_eq!(app.layout.focus(), ModuleId::Reader);
        assert!(app.open_article().is_some());
        assert!(app.view.entry_read[0], "opening it marked it read");

        app.key(code(KeyCode::Esc));
        settle(&mut app, &mut fk);
        assert!(app.open_article().is_none(), "esc closed the article");
        assert_eq!(app.layout.focus(), ModuleId::Entries);
    }

    /// `mark_read_on_open = false` leaves it alone, which is the whole point
    /// of the key.
    #[test]
    fn opening_an_entry_leaves_it_unread_when_the_setting_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.reading.mark_read_on_open = false;
        let (core, mut fk) = fake::handle(&cfg.core());
        let mut app = App::new(
            core,
            cfg,
            dir.path().join("config.toml"),
            None,
            Graphics::disabled(),
        );
        settle(&mut app, &mut fk);
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        assert!(!app.view.entry_read[0]);
    }

    /// `n` replaces the article in place rather than pushing a level, which
    /// is what keeps the frame ids -- and so the reading positions -- alive.
    #[test]
    fn n_and_p_replace_the_article_without_growing_the_stack() {
        let (mut app, mut fk, _dir) = app();
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        let depth = app.stack.len();
        let first = app.open_article().expect("an article");
        let frame = app.stack.active().id;

        app.key(key('n'));
        settle(&mut app, &mut fk);
        assert_eq!(app.stack.len(), depth, "the stack did not grow");
        assert_eq!(app.stack.active().id, frame, "the frame id did not churn");
        assert_ne!(app.open_article(), Some(first));

        app.key(key('p'));
        settle(&mut app, &mut fk);
        assert_eq!(app.open_article(), Some(first));
    }

    /// The reading position is per entry and survives going away and back.
    #[test]
    fn a_reading_position_is_kept_per_entry() {
        let (mut app, mut fk, _dir) = app();
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        draw(&mut app);
        let first = app.open_article().expect("an article");

        for _ in 0..3 {
            app.key(key('j'));
        }
        draw(&mut app);
        let at = app.reader_scroll_of(first);
        assert!(at > 0, "the reader scrolled");

        app.key(key('n'));
        settle(&mut app, &mut fk);
        draw(&mut app);
        app.key(key('p'));
        settle(&mut app, &mut fk);
        draw(&mut app);
        assert_eq!(
            app.reader_scroll_of(first),
            at,
            "it came back to where it was"
        );
    }

    /// `alt+up` keeps the article's frame, which is what makes the peek a
    /// peek rather than a close.
    #[test]
    fn jumping_up_keeps_the_article_open_behind_the_list() {
        let (mut app, mut fk, _dir) = app();
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        let open = app.open_article();

        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
        settle(&mut app, &mut fk);
        assert_eq!(app.layout.focus(), ModuleId::Entries);
        assert_eq!(app.open_article(), open, "the article is still there");
        assert!(app.stack.has_forward());

        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT));
        settle(&mut app, &mut fk);
        assert_eq!(app.layout.focus(), ModuleId::Reader);
    }

    /// `alt+3` reaches the reader even when its frame is ahead of the active
    /// one, which is what `Stack::top_of` looking past `active` is for.
    #[test]
    fn alt_3_reaches_the_reader_from_the_forward_trail() {
        let (mut app, mut fk, _dir) = app();
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
        settle(&mut app, &mut fk);
        app.key(alt('3'));
        assert_eq!(app.layout.focus(), ModuleId::Reader);
    }

    #[test]
    fn a_module_with_no_frame_says_so_rather_than_doing_nothing() {
        let (mut app, _fk, _dir) = app();
        app.key(alt('3'));
        assert_eq!(app.layout.focus(), ModuleId::Sources);
        assert_eq!(app.note_text(), Some("nothing open"));
    }

    /// `u` replaces the level rather than pushing one, and the core is told
    /// about it exactly once.
    #[test]
    fn u_toggles_unread_only_in_place() {
        let (mut app, mut fk, _dir) = app();
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        let depth = app.stack.len();
        app.key(key('u'));
        settle(&mut app, &mut fk);
        assert_eq!(app.stack.len(), depth);
        assert!(matches!(app.active_source(), Some((_, true))));
        assert!(app.core.state().unread_only);
    }

    /// The `/` field takes typing and leaves `alt+…` alone, which is
    /// `keymap::filter_eats`' rule seen from the outside.
    #[test]
    fn the_filter_takes_typing_and_lets_alt_through() {
        let (mut app, mut fk, _dir) = app();
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        app.key(key('/'));
        for c in "paywall".chars() {
            app.key(key(c));
        }
        settle(&mut app, &mut fk);
        assert_eq!(app.view.filter, "paywall");
        assert_eq!(app.view.entry_rows.len(), 1, "the filter narrowed the list");

        // `alt+1` still works with a filter half-typed.
        app.key(alt('1'));
        assert_eq!(app.layout.focus(), ModuleId::Sources);

        app.key(code(KeyCode::Esc));
        settle(&mut app, &mut fk);
        assert_eq!(app.view.filter, "");
    }

    /// The `o` chord: a digit that cannot grow opens at once, `oo` is the
    /// article itself, and `esc` gives up.
    #[test]
    fn the_o_chord_waits_only_while_the_number_can_grow() {
        let (mut app, mut fk, _dir) = app();
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        draw(&mut app);
        assert!(
            app.links >= 2,
            "the fixture article has links: {}",
            app.links
        );

        app.key(key('o'));
        assert_eq!(app.pending_chord().as_deref(), Some("o"));
        app.key(key('1'));
        assert!(app.pending_chord().is_none(), "one link cannot grow past 1");
        assert_eq!(app.note_text(), Some("opening link 1"));

        app.key(key('o'));
        app.key(code(KeyCode::Esc));
        assert!(app.pending_chord().is_none());
    }

    /// `<` and `>` step the width, clamp, and write the line.
    #[test]
    fn the_reading_width_steps_clamps_and_is_saved() {
        let (mut app, _fk, dir) = app();
        assert_eq!(app.cfg.reading.width, 80);
        app.act(Action::Narrower);
        assert_eq!(app.cfg.reading.width, 72);
        for _ in 0..20 {
            app.act(Action::Narrower);
        }
        assert_eq!(app.cfg.reading.width, WIDTH_MIN);
        for _ in 0..40 {
            app.act(Action::Wider);
        }
        assert_eq!(app.cfg.reading.width, WIDTH_MAX);

        let written = std::fs::read_to_string(dir.path().join("config.toml")).expect("a config");
        assert!(
            written.contains(&format!("width = {WIDTH_MAX}")),
            "{written}"
        );
        let read: Config = toml::from_str(&written).expect("it parses");
        assert_eq!(read.reading.width, WIDTH_MAX);
    }

    /// Cycling the theme bumps the generation, which is what makes every
    /// laid-out article in the cache a miss.
    #[test]
    fn cycling_the_theme_bumps_the_generation_and_saves_the_name() {
        let (mut app, _fk, dir) = app();
        let before = (app.theme_gen, app.theme_name.clone());
        app.act(Action::NextTheme);
        assert_ne!(app.theme_name, before.1);
        assert_eq!(app.theme_gen, before.0 + 1);
        let written = std::fs::read_to_string(dir.path().join("config.toml")).expect("a config");
        assert!(written.contains(&app.theme_name), "{written}");
    }

    /// A settings row changes the running program and the file in one step.
    #[test]
    fn a_settings_row_writes_and_applies() {
        let (mut app, _fk, dir) = app();
        app.key(key(','));
        assert!(app.overlays.is_open());
        // Down to `extract articles` and change it.
        for _ in 0..4 {
            app.key(code(KeyCode::Down));
        }
        app.key(code(KeyCode::Enter));
        assert!(!app.cfg.articles.extract);
        assert!(app.overlays.is_open(), "the box stays open");
        let written = std::fs::read_to_string(dir.path().join("config.toml")).expect("a config");
        assert!(written.contains("extract = false"), "{written}");
        assert!(!app.core.state().settings.extract, "and the core was told");
    }

    /// A search pushes a level and sends it, which is the whole of what a
    /// search is here.
    #[test]
    fn a_search_becomes_a_level_of_the_stack() {
        let (mut app, mut fk, _dir) = app();
        app.key(alt('f'));
        for c in "borrow".chars() {
            app.key(key(c));
        }
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        assert!(matches!(
            app.active_source(),
            Some((Selection::Search(q), _)) if q == "borrow"
        ));
        assert_eq!(app.view.entry_rows.len(), 1, "{:?}", app.view.entry_rows);
    }

    /// `d` asks first, and saying yes is what removes the feed.
    #[test]
    fn removing_a_feed_is_confirmed_before_anything_happens() {
        let (mut app, mut fk, _dir) = app();
        goto_source(&mut app, &mut fk, "Phoronix");
        app.key(key('d'));
        assert!(matches!(app.overlays.current(), Some(Overlay::Confirm(_))));
        let before = app.view.feeds_len;

        app.key(key('n'));
        settle(&mut app, &mut fk);
        assert_eq!(app.view.feeds_len, before, "no meant no");

        app.key(key('d'));
        app.key(key('y'));
        settle(&mut app, &mut fk);
        assert_eq!(app.view.feeds_len, before - 1);
    }

    /// A click moves the cursor onto the row the renderer drew there.
    #[test]
    fn a_click_lands_on_the_row_under_the_pointer() {
        let (mut app, mut fk, _dir) = app();
        draw(&mut app);
        let regions = app.layout.last.clone().expect("a layout");
        let rect = regions.rect_of(ModuleId::Sources);
        let body = starkit::chrome::frame::body(rect, &crate::ui::panels::words(ModuleId::Sources));
        // The crumb row is the first; the list starts under it.
        let y = body.y + 1 + 3;
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(starkit::crossterm::event::MouseButton::Left),
            column: body.x + 3,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        settle(&mut app, &mut fk);
        assert_eq!(app.cursor_of(ModuleId::Sources), 3);
        assert_eq!(app.view.source_rows[3].name, "Videos");
    }

    /// A headline the article repeats is drawn once, and the fuller of the
    /// two spellings is the one drawn.
    #[test]
    fn a_repeated_headline_is_drawn_once() {
        assert_eq!(
            head_title("## A title: and its subtitle\n\nbody\n", "A title"),
            "A title: and its subtitle"
        );
        assert_eq!(head_title("# A title\n\nbody\n", "A title"), "A title");
        assert_eq!(
            head_title("# Something else\n\nbody\n", "A title"),
            "A title",
            "a heading that is not the headline is left alone"
        );
        assert_eq!(head_title("body with no heading\n", "A title"), "A title");

        let mut doc = markdown::parse::parse("## A title: and its subtitle\n\nbody\n");
        drop_repeated_title(&mut doc, "A title");
        assert_eq!(doc.blocks.len(), 1, "{doc:?}");

        let mut doc = markdown::parse::parse("### A title\n\nbody\n");
        drop_repeated_title(&mut doc, "A title");
        assert_eq!(doc.blocks.len(), 2, "a deeper heading is the article's own");
    }

    /// The window sends `OpenFeed` when the active ENTRIES frame changes and
    /// not otherwise -- a frame where nothing moved sends nothing.
    #[test]
    fn the_core_is_told_once_per_change_and_not_once_per_frame() {
        let (mut app, mut fk, _dir) = app();
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        let sent = app.last_open_feed.clone();
        let version = app.core.state().version;
        for _ in 0..5 {
            app.tick();
            fk.pump();
        }
        assert_eq!(app.last_open_feed, sent);
        assert_eq!(app.core.state().version, version, "nothing was re-sent");
    }

    /// Quitting writes the source, the entry and the position, and the next
    /// start puts them back.
    #[test]
    fn the_session_is_written_on_the_way_out_and_read_on_the_way_in() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("session.toml");
        let cfg = Config::default();
        let (core, mut fk) = fake::handle(&cfg.core());
        let mut app = App::new(
            core,
            cfg.clone(),
            dir.path().join("config.toml"),
            Some(path.clone()),
            Graphics::disabled(),
        );
        settle(&mut app, &mut fk);
        into_feed(&mut app, &mut fk, "Tech", "Hacker News");
        app.key(code(KeyCode::Enter));
        settle(&mut app, &mut fk);
        draw(&mut app);
        for _ in 0..4 {
            app.key(key('j'));
        }
        draw(&mut app);

        let entry = app.open_article().expect("an article");
        let source = app.active_source().expect("a source").0;
        let at = app.reader_scroll_of(entry);
        app.save_session();

        let read = crate::session::load(&path);
        assert_eq!(read.source(), Some(source.clone()));
        assert_eq!(read.last_entry, Some(entry.0));
        assert_eq!(read.position(entry), Some(at));

        // And a fresh window over the same session opens where it left off.
        let (core, mut fk) = fake::handle(&cfg.core());
        let mut next = App::new(
            core,
            cfg,
            dir.path().join("config.toml"),
            Some(path),
            Graphics::disabled(),
        );
        settle(&mut next, &mut fk);
        assert_eq!(next.active_source().map(|(s, _)| s), Some(source));
        assert_eq!(next.open_article(), Some(entry));
        assert_eq!(next.reader_scroll_of(entry), at);
    }
}
