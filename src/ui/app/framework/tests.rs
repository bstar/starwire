use super::*;
use starkit::ui::{command::Registry, Runtime, Viewport};

fn fixture() -> (Framework, crate::ui::fake::Fake) {
    let cfg = crate::config::Config::default();
    let (core, mut fake) = crate::ui::fake::handle(&cfg.core());
    let app = App::new(
        core,
        cfg,
        std::path::PathBuf::from("/unused/config.toml"),
        None,
        starkit::graphics::Graphics::disabled(),
    );
    fake.pump();
    let mut app = Framework::new(app);
    app.tick();
    (app, fake)
}
fn runtime(app: &Framework) -> Runtime {
    let mut ui = Runtime::new(
        app.view(),
        Registry::new(app.commands()).unwrap(),
        app.theme().unwrap(),
    )
    .unwrap();
    if let Some(id) = app.focus_request() {
        ui.request_focus(id).unwrap();
    }
    ui
}
fn frame(app: &mut Framework, ui: &mut Runtime) {
    ui.update(app.view(), Registry::new(app.commands()).unwrap())
        .unwrap();
    if let Some(id) = app.focus_request() {
        ui.request_focus(id).unwrap();
    }
    let f = ui.frame(Viewport::cells(100, 40)).unwrap();
    app.layout(f);
}
fn key(app: &mut Framework, ui: &mut Runtime, code: KeyCode) {
    let key = KeyEvent::new(code, KeyModifiers::NONE);
    if !app.intercept_key(key) {
        for event in ui.key(key) {
            app.event(event);
        }
    }
    frame(app, ui);
}
#[test]
fn shared_lists_open_the_real_source_and_article_without_changing_the_database_contract() {
    let (mut app, mut fake) = fixture();
    let mut ui = runtime(&app);
    frame(&mut app, &mut ui);
    assert_eq!(ui.current_frame().unwrap().focus, Some("sources".into()));
    key(&mut app, &mut ui, KeyCode::Enter);
    app.tick();
    fake.pump();
    app.tick();
    frame(&mut app, &mut ui);
    assert_eq!(app.app.layout.focus(), ModuleId::Entries);
    assert!(!app.app.view.entry_ids.is_empty());
    assert_eq!(ui.current_frame().unwrap().focus, Some("entries".into()));
    key(&mut app, &mut ui, KeyCode::Down);
    let chosen = app.app.view.entry_ids[app.app.cursor_of(ModuleId::Entries)];
    key(&mut app, &mut ui, KeyCode::Enter);
    app.tick();
    fake.pump();
    app.tick();
    frame(&mut app, &mut ui);
    assert_eq!(app.app.open_article(), Some(chosen));
    assert_eq!(ui.current_frame().unwrap().focus, Some("reader".into()));
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 12));
    assert!(app.cells(&"reader".into(), &mut buffer));
    let surface = app.native(&"reader".into(), [640, 288]).unwrap();
    surface.validate().unwrap();
    let copy = keymap::BINDINGS
        .iter()
        .position(|binding| binding.action == Action::CopyLink)
        .unwrap();
    app.handle_command(&format!("action-{copy}"));
    // The second fixture deliberately has no URL: never copy another article.
    assert!(app.effects().is_empty());
    let (index, expected) = {
        let state = app.app.core.state();
        app.app
            .view
            .entry_ids
            .iter()
            .enumerate()
            .find_map(|(index, id)| state.page.row(*id)?.url.clone().map(|url| (index, url)))
            .expect("a fixture entry with a link")
    };
    app.app.focus(ModuleId::Entries);
    app.app.set_cursor(ModuleId::Entries, index);
    app.handle_command(&format!("action-{copy}"));
    assert!(
        matches!(app.effects().as_slice(),[starkit::terminal_graphics::protocol::ServerMessage::Clipboard {text}] if *text == expected)
    );
    assert!(
        app.effects().is_empty(),
        "clipboard effect drains exactly once"
    );
}
#[test]
fn shared_fields_preserve_search_submission_and_confirmation_defaults() {
    let (mut app, _fake) = fixture();
    app.app.act(Action::Search);
    let mut ui = runtime(&app);
    frame(&mut app, &mut ui);
    assert_eq!(
        ui.current_frame().unwrap().focus,
        Some("dialog-input".into())
    );
    for event in ui.paste("fixture query") {
        app.event(event);
    }
    frame(&mut app, &mut ui);
    key(&mut app, &mut ui, KeyCode::Enter);
    assert!(!app.app.overlays.is_open());
    assert!(
        matches!(app.app.active_source(),Some((crate::wire::feed::Selection::Search(query),_)) if query=="fixture query")
    );
    app.app
        .overlays
        .open_confirm(overlays::confirm::Confirm::mark_all_read(
            crate::wire::feed::Selection::All,
            "All",
            42,
        ));
    frame(&mut app, &mut ui);
    assert_eq!(
        ui.current_frame().unwrap().focus,
        Some("dialog-cancel".into())
    );
    key(&mut app, &mut ui, KeyCode::Enter);
    assert!(!app.app.overlays.is_open());
}

#[test]
fn reader_links_use_frontend_requests_and_keep_chords_and_detach_cleanup() {
    use starkit::terminal_graphics::{
        frontend_actions::{Error, ToClient, ToHost},
        protocol::ServerMessage,
    };
    let (mut app, mut fake) = fixture();
    app.app.open_entry(fake.entry("borrow checker"));
    app.tick();
    fake.pump();
    app.tick();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
    assert!(app.cells(&"reader".into(), &mut buffer));
    let expected =
        crate::ui::markdown::parse::parse(&app.app.view.article.as_ref().unwrap().markdown).links
            [0]
        .clone();
    assert!(!app.supports_action(Action::OpenBrowser));
    app.urls.set_available(true);
    let mut ui = runtime(&app);
    frame(&mut app, &mut ui);
    key(&mut app, &mut ui, KeyCode::Char('o'));
    key(&mut app, &mut ui, KeyCode::Char('1'));
    if app.app.o_pending.is_some() {
        key(&mut app, &mut ui, KeyCode::Enter);
    }
    let effects = app.effects();
    let [ServerMessage::FrontendAction {
        message: ToClient::OpenUrl { id, url },
    }] = effects.as_slice()
    else {
        panic!("Numbered link did not use the frontend: {effects:?}");
    };
    assert_eq!(
        *url,
        starkit::terminal_graphics::frontend_actions::web_url(&expected).unwrap()
    );
    assert!(!app.frontend_action(ToHost::Result {
        id: id + 1,
        error: None
    }));
    assert!(app.frontend_action(ToHost::Result {
        id: *id,
        error: Some(Error::LaunchFailed)
    }));
    assert!(app.app.note.as_ref().unwrap().0.contains("Could not open"));
    key(&mut app, &mut ui, KeyCode::Char('o'));
    key(&mut app, &mut ui, KeyCode::Char('o'));
    app.detached();
    assert!(
        app.effects().is_empty(),
        "queued browser requests must not replay on reattach"
    );
    assert!(!app.urls.available());
    assert!(!app.supports_action(Action::PlayVideo));
    assert!(!app.frontend_action(ToHost::Result {
        id: id + 1,
        error: None
    }));
}

#[test]
fn a_linkless_selection_cannot_borrow_the_open_articles_url() {
    let (mut app, mut fake) = fixture();
    let article = fake.entry("borrow checker");
    let no_link = fake.entry("not arrived yet");
    app.app.open_entry(article);
    app.tick();
    fake.pump();
    app.tick();
    assert!(app.app.entry_url(article).is_some());
    assert!(app.app.entry_url(no_link).is_none());
    app.urls.set_available(true);
    assert!(app.urls.open_url("file:///etc/passwd").is_err());
    assert!(app.effects().is_empty());
}
#[test]
fn shared_shell_and_all_modal_shapes_stay_bounded_across_presentations() {
    for action in [
        Action::Help,
        Action::Settings,
        Action::AddFeed,
        Action::Search,
        Action::Import,
    ] {
        let (mut app, _fake) = fixture();
        app.app.act(action);
        let mut ui = runtime(&app);
        for (width, height) in [(100, 40), (60, 21), (20, 8)] {
            ui.render_cells_with(Viewport::cells(width, height), &mut app)
                .unwrap();
            ui.render_scene_with(
                starkit::terminal_graphics::Viewport {
                    columns: width,
                    rows: height,
                    width: u32::from(width) * 8,
                    height: u32::from(height) * 24,
                    ..Default::default()
                },
                false,
                &mut app,
            )
            .unwrap();
        }
    }
}
