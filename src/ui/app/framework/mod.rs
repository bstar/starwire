//! KIT-owned application shell. The reader's stack and workers remain in WIRE.
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use starkit::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use starkit::native_surface::{
    style::{hex, Palette, Style as NativeStyle},
    PixelRect, Surface,
};
use starkit::ratatui::{buffer::Buffer, layout::Rect, style::Modifier, widgets::Widget};
use starkit::ui::{
    application::{Application, Host},
    command::{Command as UiCommand, Scope},
    content::Content,
    Align, Column, Event, Frame, Heading, Id, Item, Node, Presentation, Size, View, Wrap,
};

use super::{App, SourceKey};
use crate::ui::{
    keymap::{self, Action},
    overlays::{self, Overlay},
    panels::ModuleId,
};

pub struct Framework {
    app: App,
    page_rows: [usize; 3],
    wide: bool,
    layout_changed: bool,
    focused: Option<Id>,
    reader_hit: Option<ReaderHit>,
    effects: Vec<starkit::terminal_graphics::protocol::ServerMessage>,
}

struct ReaderHit {
    rendered: std::sync::Arc<crate::ui::markdown::layout::Rendered>,
    left: u16,
    head: u16,
    scroll: usize,
    scale: [u16; 2],
}

mod content;
mod input;
#[cfg(test)]
mod tests;
mod view;

impl Framework {
    pub fn new(app: App) -> Self {
        Self {
            app,
            page_rows: [10; 3],
            wide: true,
            layout_changed: false,
            focused: None,
            reader_hit: None,
            effects: vec![],
        }
    }
    fn context(&self) -> (ModuleId, Option<std::mem::Discriminant<Overlay>>, bool) {
        (
            self.app.layout.focus(),
            self.app.overlays.current().map(std::mem::discriminant),
            self.app.filter.is_some(),
        )
    }
    fn stamp(&self) -> u64 {
        let mut hash = DefaultHasher::new();
        (
            self.app.seen_version,
            self.app.seen_stack,
            self.app.theme_gen,
            self.app.pictures_gen,
            &self.app.progress_line,
            &self.app.right_line,
            &self.app.head_title,
        )
            .hash(&mut hash);
        self.app
            .note
            .as_ref()
            .map(|(text, _, at)| (text, at.elapsed().as_secs() < 5))
            .hash(&mut hash);
        self.app
            .open_article()
            .map(|id| (id.0, self.app.reader_scroll_of(id)))
            .hash(&mut hash);
        (self.app.cfg.reading.width, self.app.cfg.reading.show_byline).hash(&mut hash);
        hash.finish()
    }
}

impl Application for Framework {
    fn view(&self) -> View {
        self.build_view()
    }
    fn commands(&self) -> Vec<UiCommand> {
        self.build_commands()
    }
    fn event(&mut self, event: Event) {
        self.handle_event(event);
    }
    fn focus_request(&self) -> Option<Id> {
        self.desired_focus()
    }
    fn intercept_key(&mut self, key: KeyEvent) -> bool {
        self.key_chord(key)
    }
    fn theme(&self) -> Option<starkit::theme::Theme> {
        Some((*self.app.theme).clone())
    }
    fn tick(&mut self) -> bool {
        let before = self.stamp();
        let context = self.context();
        self.app.tick();
        if self.context() != context {
            self.focused = None;
        }
        let layout_changed = std::mem::take(&mut self.layout_changed);
        self.stamp() != before || layout_changed
    }
    fn layout(&mut self, frame: &Frame) {
        let scale = if frame.viewport.presentation == Presentation::Graphics {
            24
        } else {
            1
        };
        for module in crate::ui::panels::COLUMN {
            if module == ModuleId::Reader {
                continue;
            }
            if let Some(p) = frame.get(&Self::list_id(module).into()) {
                self.page_rows[module.index()] = usize::from(p.body.height / scale)
                    .saturating_sub(usize::from(module != ModuleId::Reader))
                    .max(1);
            }
        }
        let width = if frame.viewport.presentation == Presentation::Graphics {
            frame.viewport.width / 8
        } else {
            frame.viewport.width
        };
        let wide = width >= 80;
        if wide != self.wide {
            self.wide = wide;
            self.layout_changed = true;
        }
    }
    fn closed(&self) -> bool {
        self.app.quit
    }
    fn effects(&mut self) -> Vec<starkit::terminal_graphics::protocol::ServerMessage> {
        std::mem::take(&mut self.effects)
    }
    fn shutdown(&mut self) {
        self.app.save_session();
        self.app.core.send(crate::wire::Command::Shutdown);
    }
}

pub fn run() -> anyhow::Result<()> {
    use starkit::terminal_graphics::{
        application::Application as Session, client::PresentationPreference,
    };
    let cells = std::env::args().any(|arg| arg == "--cells");
    Session::new("starwire")?
        .with_presentation(PresentationPreference {
            cells,
            path: crate::paths::PATHS.config_dir()?.join("presentation.toml"),
        })
        .run(|args| {
            use clap::Parser;
            let cli = crate::cli::Cli::try_parse_from(
                std::iter::once("starwire").chain(args.iter().map(String::as_str)),
            )?;
            anyhow::ensure!(
                cli.command.is_none(),
                "Graphical sessions require the reader window"
            );
            let cfg_path = crate::paths::PATHS.config_file()?;
            if let Err(error) = crate::config::Config::write_template(&cfg_path) {
                tracing::warn!(%error,"Could not write starting configuration");
            }
            let cfg = crate::config::Config::load(&cfg_path)?;
            let http = crate::build_http(&cfg.core(), cli.replay.as_deref(), false)?;
            let core = crate::wire::Handle::spawn_with(
                cfg.core(),
                crate::paths::db_file()?,
                crate::wire::Handle::probe_newsboat(),
                std::sync::Arc::from(http),
            )?;
            let app = App::new(
                core,
                cfg,
                cfg_path,
                crate::paths::PATHS.session_file().ok(),
                starkit::graphics::Graphics::disabled(),
            );
            let theme = (*app.theme).clone();
            Host::new(Framework::new(app), theme)
        })
}
