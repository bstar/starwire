//! Domain commands/events around KIT-owned editing and navigation.
use super::*;

impl Framework {
    pub(super) fn supports_action(action: Action) -> bool {
        !matches!(
            action,
            Action::OpenBrowser | Action::PlayVideo | Action::OpenLink
        )
    }
    fn copy_link(&mut self) {
        let Some(id) = self.app.target_entry() else {
            return;
        };
        let url = {
            let state = self.app.core.state();
            state
                .page
                .row(id)
                .and_then(|row| row.url.clone())
                .or_else(|| {
                    state
                        .article
                        .as_ref()
                        .filter(|article| article.entry == id)
                        .and_then(|article| article.url.clone())
                })
        };
        if let Some(url) = url.filter(|url| url.len() <= 8192) {
            if self.effects.len() < 8 {
                self.effects.push(
                    starkit::terminal_graphics::protocol::ServerMessage::Clipboard { text: url },
                );
                self.app.say("Copy link requested");
            }
        } else {
            self.app.warn("No copyable link");
        }
    }
    pub(super) fn handle_command(&mut self, id: &str) {
        let action = match id {
            "kit.quit" => Some(Action::Quit),
            "kit.help" => Some(Action::Help),
            "kit.theme" => Some(Action::NextTheme),
            "focus-sources" => Some(Action::FocusSources),
            "focus-entries" => Some(Action::FocusEntries),
            "focus-reader" => Some(Action::FocusReader),
            "add" => Some(Action::AddFeed),
            "refresh" => Some(Action::RefreshAll),
            "settings" => Some(Action::Settings),
            _ => None,
        };
        if let Some(action) = action {
            self.app.act(action);
            return;
        }
        match id {
            "dialog-cancel" => {
                self.app.overlays.close();
            }
            "dialog-submit" => {
                let answer = self
                    .app
                    .overlays
                    .handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                self.app.after_overlay_answer(answer);
            }
            "dialog-confirm" => {
                if let Some(Overlay::Confirm(confirm)) = self.app.overlays.current() {
                    self.app
                        .after_overlay_answer(overlays::Answer::Confirmed(confirm.pending.clone()));
                }
            }
            "setting-change" => {
                if let Some(Overlay::Settings(settings)) = self.app.overlays.current() {
                    self.app
                        .after_overlay_answer(overlays::Answer::Setting(settings.selected(), true));
                }
            }
            "import-yes" | "import-no" => {
                let answer = self.app.overlays.handle(KeyEvent::new(
                    KeyCode::Char(if id == "import-yes" { 'y' } else { 'n' }),
                    KeyModifiers::NONE,
                ));
                self.app.after_overlay_answer(answer);
            }
            _ => {
                if let Some(index) = id
                    .strip_prefix("action-")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    if let Some(binding) = keymap::BINDINGS.get(index) {
                        match binding.action {
                            Action::PageUp | Action::PageDown => {
                                let rows = self.page_rows[self.app.layout.focus().index()] as i64;
                                self.app.move_cursor(if binding.action == Action::PageUp {
                                    -rows
                                } else {
                                    rows
                                });
                            }
                            Action::CopyLink => self.copy_link(),
                            action if !Self::supports_action(action) => self
                                .app
                                .say("Opening links and videos is unavailable in this preview"),
                            action => self.app.act(action),
                        }
                    }
                }
            }
        }
    }
    pub(super) fn build_commands(&self) -> Vec<UiCommand> {
        let mut commands = starkit::ui::command::standard();
        commands.extend(
            [
                ("focus-sources", "Sources"),
                ("focus-entries", "Entries"),
                ("focus-reader", "Reader"),
                ("add", "Add feed"),
                ("refresh", "Refresh"),
                ("settings", "Settings"),
            ]
            .into_iter()
            .map(|(id, label)| UiCommand::new(id, label)),
        );
        let modal_scope = Scope::View("dialog".into());
        commands.extend(
            [
                ("dialog-cancel", "Cancel"),
                ("dialog-submit", "Submit"),
                ("dialog-confirm", "Confirm"),
                ("setting-change", "Change"),
                ("import-yes", "Import"),
                ("import-no", "Don't ask again"),
            ]
            .into_iter()
            .map(|(id, label)| {
                let label = match (id, self.app.overlays.current()) {
                    ("dialog-confirm", Some(Overlay::Confirm(confirm))) => confirm.yes,
                    ("dialog-cancel", Some(Overlay::Confirm(confirm))) => confirm.no,
                    ("dialog-submit", Some(Overlay::Search(_))) => "Search",
                    ("dialog-submit", Some(Overlay::AddFeed(_))) => "Add feed",
                    (
                        "dialog-cancel",
                        Some(
                            Overlay::Help { .. }
                            | Overlay::Settings(_)
                            | Overlay::Picture(_)
                            | Overlay::Import(_),
                        ),
                    ) => "Close",
                    _ => label,
                };
                UiCommand::new(id, label).scope(modal_scope.clone())
            }),
        );
        // Resolve the existing WIRE table first, so intentional reader shadows
        // are retained while the shared registry rejects accidental conflicts.
        let mut keys: Vec<_> = commands
            .iter()
            .flat_map(|command| command.shortcuts.iter())
            .filter_map(|s| starkit::keymap::KeySpec::parse(s))
            .collect();
        for (index, binding) in keymap::BINDINGS.iter().enumerate() {
            let mut command = UiCommand::new(format!("action-{index}"), binding.label)
                .group(binding.group)
                .enabled(Self::supports_action(binding.action));
            for spelling in starkit::keymap::alternatives(binding.keys) {
                let Some(spec) = starkit::keymap::KeySpec::parse(spelling) else {
                    continue;
                };
                if keys.contains(&spec) {
                    continue;
                }
                {
                    let key = KeyEvent::new(spec.code, spec.mods);
                    let resolved = keymap::module(self.app.layout.focus().module(), key)
                        .or_else(|| keymap::resolve(key));
                    if resolved != Some(binding.action) {
                        continue;
                    }
                    keys.push(spec);
                    let standard = match binding.action {
                        Action::Help => Some("kit.help"),
                        Action::Quit => Some("kit.quit"),
                        Action::Redraw => Some("kit.repaint"),
                        Action::NextTheme => Some("kit.theme"),
                        _ => None,
                    };
                    if let Some(id) = standard {
                        if let Some(command) =
                            commands.iter_mut().find(|command| command.id.0 == id)
                        {
                            command.shortcuts.push(spelling.into());
                        }
                        continue;
                    }
                    command = command.shortcut(spelling);
                }
            }
            commands.push(command);
        }
        commands
    }
    pub(super) fn handle_event(&mut self, event: Event) {
        let context = self.context();
        let activated = matches!(event, Event::Activated { .. });
        match event {
            Event::Command(id) => self.handle_command(&id.0),
            Event::Focused(id) => {
                self.focused = Some(id.clone());
                for module in crate::ui::panels::COLUMN {
                    if id.0 == Self::list_id(module)
                        || id.0 == format!("{}-peek", Self::list_id(module))
                    {
                        self.app.focus(module);
                        self.focused = Some(Self::list_id(module).into());
                    }
                }
            }
            Event::Selected { id, item } | Event::Activated { id, item } => {
                if id.0 == "sources" {
                    if let Some(index) = self
                        .app
                        .view
                        .source_keys
                        .iter()
                        .position(|key| Self::source_id(key) == item)
                    {
                        self.app.set_cursor(ModuleId::Sources, index);
                    }
                } else if id.0 == "entries" {
                    if let Some(index) = self
                        .app
                        .view
                        .entry_ids
                        .iter()
                        .position(|entry| item.0 == format!("entry:{}", entry.0))
                    {
                        self.app.set_cursor(ModuleId::Entries, index);
                    }
                } else if id.0 == "settings-table" {
                    if let Some(index) =
                        item.0.strip_prefix("setting-").and_then(|n| n.parse().ok())
                    {
                        if let Some(Overlay::Settings(settings)) = self.app.overlays.current_mut() {
                            settings.cursor = index;
                        }
                    }
                } else if id.0 == "help-table" {
                    if let Some(index) = item.0.strip_prefix("help-").and_then(|n| n.parse().ok()) {
                        if let Some(Overlay::Help { scroll }) = self.app.overlays.current_mut() {
                            *scroll = index;
                        }
                    }
                } else if id.0 == "import-report" {
                    if let Some(index) = item.0.strip_prefix("skip-").and_then(|n| n.parse().ok()) {
                        if let Some(Overlay::Import(import)) = self.app.overlays.current_mut() {
                            import.scroll = index;
                        }
                    }
                }
                if activated {
                    if id.0 == "sources" || id.0 == "entries" {
                        self.app.activate();
                    }
                    if id.0 == "settings-table" {
                        self.handle_command("setting-change");
                    }
                }
            }
            Event::Submitted { id, .. } if id.0 == "dialog-input" => {
                self.handle_command("dialog-submit")
            }
            Event::Submitted { id, .. } if id.0 == "filter" => {
                self.app.filter = None;
            }
            Event::Cancelled { id } if id.0 == "filter" => self.app.act(Action::Back),
            Event::Changed { id, value } if id.0 == "filter" => {
                if let Some(filter) = self.app.filter.as_mut() {
                    let module = filter.module;
                    filter.input.set_text(value.clone());
                    self.app.set_filter(module, value);
                }
            }
            Event::Changed { id, value } if id.0 == "dialog-input" => {
                match self.app.overlays.current_mut() {
                    Some(Overlay::AddFeed(add)) => add.input.set_text(value),
                    Some(Overlay::Search(search)) => search.input.set_text(value),
                    Some(Overlay::Import(import)) => {
                        if let overlays::import::Stage::Opml { input, .. } = &mut import.stage {
                            input.set_text(value);
                        }
                    }
                    _ => {}
                }
            }
            Event::CancelModal => self.app.overlays.close(),
            Event::CustomScroll { id, lines, .. } if id.0 == "reader" => {
                self.app.scroll_reader(i64::from(lines))
            }
            Event::CustomPointer {
                id,
                x,
                y,
                pressed: false,
            } if id.0 == "reader" => {
                let number = self.reader_hit.as_ref().and_then(|hit| {
                    let x = x / hit.scale[0];
                    let y = y / hit.scale[1];
                    if x < hit.left || y < hit.head {
                        return None;
                    }
                    let row = usize::from(y - hit.head) + hit.scroll;
                    hit.rendered
                        .links
                        .iter()
                        .find(|link| {
                            usize::from(link.row) == row
                                && x - hit.left >= link.col
                                && x - hit.left < link.col + link.width
                        })
                        .map(|link| link.number)
                });
                if number.is_some() {
                    self.app.say("Opening links is unavailable in this preview");
                }
            }
            Event::CustomKey { id, key } if id.0 == "reader" => self.app.key(key),
            _ => {}
        }
        if self.context() != context {
            self.focused = None;
        }
    }
    pub(super) fn desired_focus(&self) -> Option<Id> {
        if let Some(id) = &self.focused {
            return Some(id.clone());
        }
        if let Some(overlay) = self.app.overlays.current() {
            return Some(
                match overlay {
                    Overlay::AddFeed(_)
                    | Overlay::Search(_)
                    | Overlay::Import(overlays::import::Import {
                        stage: overlays::import::Stage::Opml { .. },
                        ..
                    }) => "dialog-input",
                    Overlay::Help { .. } => "help-table",
                    Overlay::Settings(_) => "settings-table",
                    _ => "dialog-cancel",
                }
                .into(),
            );
        }
        Some(
            if self.app.filter.is_some() {
                "filter"
            } else {
                Self::list_id(self.app.layout.focus())
            }
            .into(),
        )
    }
    pub(super) fn key_chord(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.app.quit = true;
            return true;
        }
        if self.app.overlays.is_open() {
            return false;
        }
        if self.app.filter.is_some() || key.code == KeyCode::F(9) {
            return false;
        }
        if keymap::module(self.app.layout.focus().module(), key)
            .or_else(|| keymap::resolve(key))
            .is_some_and(|action| !Self::supports_action(action))
        {
            self.app
                .say("Opening links and videos is unavailable in this preview");
            return true;
        }
        if self.app.g_pending
            || self.app.o_pending.is_some()
            || (key.modifiers.is_empty()
                && (key.code == KeyCode::Char('g')
                    || (key.code == KeyCode::Char('o')
                        && self.app.layout.focus() == ModuleId::Reader)))
        {
            self.app.key(key);
            return true;
        }
        false
    }
}
