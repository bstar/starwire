//! Responsive shared controls and modal trees.
use super::*;

impl Framework {
    pub(super) fn list_id(module: ModuleId) -> &'static str {
        match module {
            ModuleId::Sources => "sources",
            ModuleId::Entries => "entries",
            ModuleId::Reader => "reader",
        }
    }
    pub(super) fn source_id(key: &super::SourceKey) -> Id {
        format!("source:{}", key.stable().trim_start_matches('\0')).into()
    }
    pub(super) fn sources(&self) -> Node {
        let items = self
            .app
            .view
            .source_rows
            .iter()
            .zip(&self.app.view.source_keys)
            .map(|(row, key)| {
                let mut item = Item::new(
                    Self::source_id(key),
                    format!(
                        "{} {}{}",
                        row.glyph,
                        row.name,
                        row.extra
                            .as_ref()
                            .map(|extra| format!(" · {extra}"))
                            .unwrap_or_default()
                    ),
                );
                item.columns
                    .push(row.count.map(|n| n.to_string()).unwrap_or_default());
                item
            })
            .collect();
        Node::table_with_columns(
            "sources",
            vec![
                Column::new("Source"),
                Column::new("Unread")
                    .width(Size::fixed(7))
                    .align(Align::Right),
            ],
            items,
            self.app
                .view
                .source_keys
                .get(self.app.cursor_of(ModuleId::Sources))
                .map(Self::source_id),
        )
    }
    pub(super) fn entries(&self) -> Node {
        let feed = self.wide && self.app.view.entry_aggregate;
        let mut columns = vec![Column::new("Article")];
        if feed {
            columns.push(Column::new("Feed").width(Size::bounded(8, 18, 24)));
        }
        columns.push(Column::new("Age").width(Size::fixed(5)).align(Align::Right));
        let items = self
            .app
            .view
            .entry_rows
            .iter()
            .zip(&self.app.view.entry_ids)
            .map(|(row, id)| {
                let mut item = Item::new(
                    format!("entry:{}", id.0),
                    format!(
                        "{}{} {}",
                        if row.unread { "●" } else { " " },
                        if row.starred { "★" } else { " " },
                        row.title
                    ),
                );
                if feed {
                    item.columns.push(row.feed.clone());
                }
                item.columns.push(row.age.clone());
                item
            })
            .collect();
        Node::table_with_columns(
            "entries",
            columns,
            items,
            self.app
                .view
                .entry_ids
                .get(self.app.cursor_of(ModuleId::Entries))
                .map(|id| format!("entry:{}", id.0).into()),
        )
    }
    pub(super) fn pane(&self, module: ModuleId) -> Node {
        let active = self.app.layout.focus() == module;
        let reader_open = self.app.open_article().is_some();
        let peeking = reader_open && module == ModuleId::Reader && !active;
        let id = Self::list_id(module);
        let content = if active || peeking {
            match module {
                ModuleId::Sources => self.sources(),
                ModuleId::Entries => self.entries(),
                ModuleId::Reader => Node::new(
                    "reader",
                    starkit::ui::Widget::Custom {
                        fallback: "Choose an article to read".into(),
                        revision: self.stamp(),
                    },
                ),
            }
        } else {
            Node::button(format!("{id}-peek"), format!("focus-{id}"))
                .enabled(self.app.stack.top_of(module).is_some())
        };
        let detail = match module {
            ModuleId::Sources => self.app.view.source_summary.clone(),
            ModuleId::Entries => self.app.view.entry_source.clone(),
            ModuleId::Reader => self.app.head_title.clone(),
        };
        Node::pane(
            format!("{id}-pane"),
            Heading::new(module.title()).status(detail),
            content,
        )
        .height(if active && reader_open && module != ModuleId::Reader {
            Size::fixed(self.app.cfg.ui.list_rows.max(8))
        } else if active {
            Size::fill(8)
        } else if peeking {
            Size::fill(4)
        } else {
            Size::fixed(3)
        })
        .overflow(if active {
            starkit::ui::Overflow::Keep
        } else {
            starkit::ui::Overflow::Hide
        })
        .priority(if active { 100 } else { 1 })
    }
    pub(super) fn dialog(&self) -> Option<Node> {
        let overlay = self.app.overlays.current()?;
        let close = || Node::button("dialog-cancel", "dialog-cancel").height(Size::fixed(1));
        let (title, content, height) = match overlay {
            Overlay::Help { scroll } => (
                "Help".to_string(),
                vec![
                    Node::table_with_columns(
                        "help-table",
                        vec![
                            Column::new("Keys").width(Size::fixed(18)),
                            Column::new("Action"),
                        ],
                        keymap::BINDINGS
                            .iter()
                            .enumerate()
                            .map(|(i, b)| {
                                let mut row = Item::new(format!("help-{i}"), b.keys);
                                row.columns.push(b.label.into());
                                row.enabled = Self::supports_action(b.action);
                                row
                            })
                            .collect(),
                        Some(format!("help-{scroll}").into()),
                    ),
                    close(),
                ],
                22,
            ),
            Overlay::AddFeed(add) => (
                "Add a feed".into(),
                vec![
                    Node::field("dialog-input", add.input.text()),
                    Node::label(
                        "dialog-error",
                        add.error.unwrap_or("URL, @handle or channel ID"),
                    ),
                    Node::button("dialog-submit", "dialog-submit"),
                    close(),
                ],
                9,
            ),
            Overlay::Search(search) => (
                "Search articles".into(),
                vec![
                    Node::field("dialog-input", search.input.text()),
                    Node::button("dialog-submit", "dialog-submit"),
                    close(),
                ],
                8,
            ),
            Overlay::Confirm(confirm) => (
                confirm.title.clone(),
                vec![
                    Node::label("question", confirm.body.join("\n"))
                        .height(Size::fixed(confirm.body.len().max(1) as u16)),
                    Node::button("dialog-confirm", "dialog-confirm"),
                    close(),
                ],
                10,
            ),
            Overlay::Settings(settings) => (
                "Settings".into(),
                vec![
                    Node::table_with_columns(
                        "settings-table",
                        vec![Column::new("Setting"), Column::new("Value")],
                        overlays::settings::Setting::ALL
                            .iter()
                            .enumerate()
                            .map(|(i, s)| {
                                let mut item = Item::new(format!("setting-{i}"), s.label());
                                item.columns.push(s.value(&self.app.cfg));
                                item
                            })
                            .collect(),
                        Some(format!("setting-{}", settings.cursor).into()),
                    ),
                    Node::button("setting-change", "setting-change"),
                    close(),
                ],
                20,
            ),
            Overlay::Import(import) => match &import.stage {
                overlays::import::Stage::Ask(probe) => (
                    "Import feeds".into(),
                    vec![
                        Node::label(
                            "import-question",
                            format!(
                                "Import {} feeds ({} video channels) from {}?",
                                probe.feeds,
                                probe.videos,
                                probe.path.display()
                            ),
                        ),
                        Node::button("import-yes", "import-yes"),
                        Node::button("import-no", "import-no"),
                        close(),
                    ],
                    12,
                ),
                overlays::import::Stage::Waiting => (
                    "Importing feeds".into(),
                    vec![Node::label("import-wait", "Import in progress…"), close()],
                    7,
                ),
                overlays::import::Stage::Opml { input, error } => (
                    "Import OPML".into(),
                    vec![
                        Node::field("dialog-input", input.text()),
                        Node::label("dialog-error", error.unwrap_or("Path to an OPML file")),
                        Node::button("dialog-submit", "dialog-submit"),
                        close(),
                    ],
                    9,
                ),
                overlays::import::Stage::Done(report) => (
                    "Import complete".into(),
                    vec![
                        Node::label(
                            "import-summary",
                            format!(
                                "{} added · {} existing · {} skipped",
                                report.added,
                                report.already_there,
                                report.skipped.len()
                            ),
                        ),
                        Node::list(
                            "import-report",
                            report
                                .skipped
                                .iter()
                                .enumerate()
                                .map(|(i, (url, reason))| {
                                    Item::new(format!("skip-{i}"), format!("{url}: {reason}"))
                                })
                                .collect(),
                            (!report.skipped.is_empty()).then(|| {
                                format!("skip-{}", import.scroll.min(report.skipped.len() - 1))
                                    .into()
                            }),
                        ),
                        close(),
                    ],
                    18,
                ),
            },
            Overlay::Picture(picture) => (
                picture.current().alt.clone(),
                vec![
                    Node::new(
                        "picture",
                        starkit::ui::Widget::Custom {
                            fallback: picture.current().url.clone(),
                            revision: self.app.pictures_gen,
                        },
                    ),
                    close(),
                ],
                22,
            ),
        };
        Some(
            Node::pane(
                "dialog",
                Heading::new(title),
                Node::column("dialog-content", content),
            )
            .width(Size::fixed(64))
            .height(Size::fixed(height)),
        )
    }
    pub(super) fn build_view(&self) -> View {
        let toolbar = Node::row(
            "toolbar",
            vec![
                Node::button("add-button", "add"),
                Node::button("refresh-button", "refresh"),
                Node::button("settings-button", "settings"),
                Node::button("help-button", "kit.help"),
                Node::button("quit-button", "kit.quit"),
            ],
        )
        .height(Size::fixed(1))
        .wrap(Wrap::AtMinimum);
        let mut children = vec![toolbar];
        if let Some(filter) = &self.app.filter {
            children.push(Node::field("filter", filter.input.text()));
        }
        children.extend([
            self.pane(ModuleId::Sources),
            self.pane(ModuleId::Entries),
            self.pane(ModuleId::Reader),
        ]);
        let note = self
            .app
            .note
            .as_ref()
            .filter(|(_, _, at)| at.elapsed().as_secs() < 5)
            .map(|(text, _, _)| text.as_str())
            .unwrap_or("");
        children.push(Node::new(
            "status",
            starkit::ui::Widget::Status(format!(
                "{}  {}  {}",
                self.app.progress_line.as_deref().unwrap_or(note),
                self.app.right_line,
                "F1 Help · F9 Cells / Graphics"
            )),
        ));
        let mut view = View::new(Node::column("application", children));
        view.modal = self.dialog();
        view
    }
}
