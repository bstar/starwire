//! One table describing every action, its keys and its help text.
//!
//! Copied from STAR/FOLD's `ui/keymap.rs`, which took it from STAR/CORD's:
//! the same three-layer read -- the focused module's own bindings, then the
//! global table, then nothing -- over three modules, with three more layers
//! above it that this module supplies the primitives for but does not run
//! itself.
//!
//! ## Dispatch order
//!
//! A key press reaches its action through six layers, tried in order: an
//! open overlay takes every key while it is up; the `/` filter's text entry
//! takes typing next ([`filter_eats`]); an `o` waiting for a link number
//! comes next ([`o_prefix`]); a `g` waiting for its second key comes after
//! that ([`g_prefix`]); the focused module's own bindings are offered the
//! key ([`module`]); and the global table ([`resolve`]) catches whatever
//! nothing above wanted. The first four layers are stateful -- is an overlay
//! open, does the filter have focus, is a chord half-typed -- and that state
//! lives in `ui/app/keys.rs`, not here: everything in this module is a pure
//! function of one `KeyEvent` and whatever the caller has already collected.
//!
//! ## The two chords
//!
//! `gg` (to the top of the list) is dispatched by [`g_prefix`], and `o<n>`
//! (open link *n* in the article) by [`o_prefix`]. Neither spelling is one
//! key, so `starkit::keymap::KeySpec::parse` refuses both -- which is what
//! keeps them out of [`GLOBAL`] and [`MODULES`] while leaving them in
//! [`BINDINGS`] for the help overlay to print. `Keymap::from_table` drops an
//! alternative it cannot parse rather than panicking on it, so the chord
//! spellings contribute no dispatch entry of their own and the two functions
//! above are the only way to reach [`Action::Home`] through `gg` and
//! [`Action::OpenLink`] at all.
//!
//! `o<n>` is the one that needed thought. The reader numbers the links in an
//! article `[1]`, `[2]`, … in document order, and typing `o` then `1` then
//! `2` has to mean link twelve where there are twelve links and link one
//! where there are three -- so the chord closes as soon as the number it has
//! *cannot grow*, and only waits when another digit could still make a
//! number that exists. That is [`OChord`], and it is why `o_prefix` is given
//! the digits so far and the link count rather than keeping either itself.
//!
//! ## Three module groups, and one shadow
//!
//! `sources`, `entries` and `reader` are each scoped to one module, and the
//! reader deliberately reuses one global key: `esc` closes the article where
//! the global `esc` merely cancels. [`SHADOWS`] has that one entry, with
//! why, and a test asserts nothing else in any module shadows a global
//! binding by accident.
//!
//! `A` is the one shifted letter that is not "the big step": it marks a
//! source read in SOURCES and everything read in ENTRIES, because `a` --
//! next to it, and unshifted -- adds a feed, and the pair reads as one
//! thought. Both are confirmed before anything happens.
//!
//! ## Invariants
//!
//! Carried over from STAR/FOLD, each one a test at the bottom of this file:
//! a bare arrow moves one and a shifted one moves ten; every key in the
//! table can be spelled in the table, chords included; `hjkl` navigates and
//! never adjusts a value; `esc` never quits; a label is at most 19
//! characters and a key spelling at most 13; every group appears in one run
//! of the table; no module key shadows a global one except the one in
//! [`SHADOWS`].

use std::sync::LazyLock;

use starkit::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use starkit::keymap::{Binding as KitBinding, Keymap, MouseHelp};

pub type Binding = KitBinding<Action>;

/// Everything the window can be asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    // -- moving about --
    CursorUp,
    CursorDown,
    CursorUpBig,
    CursorDownBig,
    PageUp,
    PageDown,
    /// `gg`/`home`: to the top of the list, or of the article.
    Home,
    End,
    /// `enter`: open what the cursor is on. A no-op in the reader, which
    /// says so -- `o` is what opens the article somewhere else.
    Activate,
    /// `esc`: put the filter away, abandon a chord. The reader shadows it --
    /// see [`SHADOWS`].
    Back,

    // -- focus --
    FocusNext,
    FocusPrev,
    FocusSources,
    FocusEntries,
    FocusReader,

    // -- the stack --
    /// `l`/`right`: push a level.
    Enter,
    /// `h`/`left`/`bs`: pop one, discarding it.
    Pop,
    JumpUp,
    JumpDown,

    // -- finding --
    /// `alt+f`: full-text search over every article ever extracted. Its
    /// results are a level of the stack like any other.
    Search,
    /// `/`: narrow the rows already on screen.
    Filter,

    // -- sources --
    RefreshSource,
    RemoveFeed,
    MarkSourceRead,

    // -- an entry, from the list or from the reader --
    ToggleRead,
    ToggleStar,
    OpenBrowser,
    CopyLink,
    PlayVideo,
    NextUnread,
    PrevUnread,
    UnreadOnly,
    MarkAllRead,

    // -- the reader --
    NextEntry,
    PrevEntry,
    /// `o<n>`: the numbered link. Reached only through [`o_prefix`], which
    /// says *which* link; this is the entry the help overlay prints.
    OpenLink,
    Extract,
    Narrower,
    Wider,
    CloseArticle,

    // -- feeds --
    AddFeed,
    RefreshAll,
    Import,

    // -- appearance --
    NextTheme,
    PrevTheme,
    Settings,

    // -- application --
    Help,
    /// `q`/`ctrl+c`: quit. `ctrl+c` is bound here rather than to anything
    /// gentler because that is the terminal habit everywhere else.
    Quit,
    Redraw,
}

/// Which module a key is offered to first.
///
/// Mirrors [`ModuleId`](super::panels::ModuleId) and is its own type so this
/// module does not depend on the panels, which depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Module {
    Sources,
    Entries,
    Reader,
}

/// Where a group of bindings applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Modules(&'static [Module]),
}

/// Every group in [`BINDINGS`], and where it applies.
pub const GROUPS: &[(&str, Scope)] = &[
    ("navigation", Scope::Global),
    ("sources", Scope::Modules(&[Module::Sources])),
    ("entries", Scope::Modules(&[Module::Entries])),
    ("reader", Scope::Modules(&[Module::Reader])),
    ("feeds", Scope::Global),
    ("appearance", Scope::Global),
    ("application", Scope::Global),
];

/// Every key, in the order the help overlay prints them.
pub const BINDINGS: &[Binding] = &[
    // -- navigation ---------------------------------------------------------
    Binding {
        action: Action::FocusNext,
        keys: "tab",
        label: "next module",
        group: "navigation",
    },
    Binding {
        action: Action::FocusPrev,
        keys: "shift+tab",
        label: "previous module",
        group: "navigation",
    },
    Binding {
        action: Action::FocusSources,
        keys: "alt+1",
        label: "the sources",
        group: "navigation",
    },
    Binding {
        action: Action::FocusEntries,
        keys: "alt+2",
        label: "the entries",
        group: "navigation",
    },
    Binding {
        action: Action::FocusReader,
        keys: "alt+3",
        label: "the reader",
        group: "navigation",
    },
    Binding {
        action: Action::CursorUp,
        keys: "up/k",
        label: "up one",
        group: "navigation",
    },
    Binding {
        action: Action::CursorDown,
        keys: "down/j",
        label: "down one",
        group: "navigation",
    },
    Binding {
        action: Action::CursorUpBig,
        keys: "shift+up/K",
        label: "up ten",
        group: "navigation",
    },
    Binding {
        action: Action::CursorDownBig,
        keys: "shift+down/J",
        label: "down ten",
        group: "navigation",
    },
    Binding {
        action: Action::PageUp,
        keys: "pgup",
        label: "page up",
        group: "navigation",
    },
    Binding {
        action: Action::PageDown,
        keys: "pgdn",
        label: "page down",
        group: "navigation",
    },
    Binding {
        action: Action::Home,
        keys: "home/gg",
        label: "to the top",
        group: "navigation",
    },
    Binding {
        action: Action::End,
        keys: "end/G",
        label: "to the bottom",
        group: "navigation",
    },
    Binding {
        action: Action::Activate,
        keys: "enter",
        label: "open it",
        group: "navigation",
    },
    Binding {
        action: Action::Back,
        keys: "esc",
        label: "cancel",
        group: "navigation",
    },
    Binding {
        action: Action::Enter,
        keys: "l/right",
        label: "drill in",
        group: "navigation",
    },
    Binding {
        action: Action::Pop,
        keys: "h/left/bs",
        label: "back one level",
        group: "navigation",
    },
    Binding {
        action: Action::JumpUp,
        keys: "alt+up",
        label: "jump to the parent",
        group: "navigation",
    },
    Binding {
        action: Action::JumpDown,
        keys: "alt+down",
        label: "jump back down",
        group: "navigation",
    },
    Binding {
        action: Action::Search,
        keys: "alt+f",
        label: "search everything",
        group: "navigation",
    },
    Binding {
        action: Action::Filter,
        keys: "/",
        label: "filter this list",
        group: "navigation",
    },
    // -- sources ------------------------------------------------------------
    Binding {
        action: Action::RefreshSource,
        keys: "r",
        label: "refresh this source",
        group: "sources",
    },
    Binding {
        action: Action::RemoveFeed,
        keys: "d/delete",
        label: "remove the feed",
        group: "sources",
    },
    Binding {
        action: Action::MarkSourceRead,
        keys: "A",
        label: "mark source read",
        group: "sources",
    },
    // -- entries ------------------------------------------------------------
    Binding {
        action: Action::ToggleRead,
        keys: "m",
        label: "read, unread",
        group: "entries",
    },
    Binding {
        action: Action::ToggleStar,
        keys: "s",
        label: "star, unstar",
        group: "entries",
    },
    Binding {
        action: Action::OpenBrowser,
        keys: "o",
        label: "in the browser",
        group: "entries",
    },
    Binding {
        action: Action::CopyLink,
        keys: "y",
        label: "copy the link",
        group: "entries",
    },
    Binding {
        action: Action::PlayVideo,
        keys: "v",
        label: "play the video",
        group: "entries",
    },
    Binding {
        action: Action::NextUnread,
        keys: "n",
        label: "next unread",
        group: "entries",
    },
    Binding {
        action: Action::PrevUnread,
        keys: "p",
        label: "previous unread",
        group: "entries",
    },
    Binding {
        action: Action::UnreadOnly,
        keys: "u",
        label: "unread only",
        group: "entries",
    },
    Binding {
        action: Action::MarkAllRead,
        keys: "A",
        label: "mark all read",
        group: "entries",
    },
    // -- reader --------------------------------------------------------------
    Binding {
        action: Action::PageDown,
        keys: "space",
        label: "page down",
        group: "reader",
    },
    Binding {
        action: Action::PageUp,
        keys: "b",
        label: "page up",
        group: "reader",
    },
    Binding {
        action: Action::NextEntry,
        keys: "n",
        label: "next entry",
        group: "reader",
    },
    Binding {
        action: Action::PrevEntry,
        keys: "p",
        label: "previous entry",
        group: "reader",
    },
    Binding {
        action: Action::NextUnread,
        keys: "N",
        label: "next unread",
        group: "reader",
    },
    Binding {
        action: Action::ToggleRead,
        keys: "m",
        label: "read, unread",
        group: "reader",
    },
    Binding {
        action: Action::ToggleStar,
        keys: "s",
        label: "star, unstar",
        group: "reader",
    },
    Binding {
        action: Action::OpenBrowser,
        keys: "o",
        label: "in the browser",
        group: "reader",
    },
    Binding {
        action: Action::CopyLink,
        keys: "y",
        label: "copy the link",
        group: "reader",
    },
    Binding {
        action: Action::PlayVideo,
        keys: "v",
        label: "play the video",
        group: "reader",
    },
    // A chord, spelled but not parsed as one key -- see the module doc.
    Binding {
        action: Action::OpenLink,
        keys: "o<n>",
        label: "open link n",
        group: "reader",
    },
    Binding {
        action: Action::Extract,
        keys: "e",
        label: "extract again",
        group: "reader",
    },
    Binding {
        action: Action::Narrower,
        keys: "<",
        label: "narrower",
        group: "reader",
    },
    Binding {
        action: Action::Wider,
        keys: ">",
        label: "wider",
        group: "reader",
    },
    Binding {
        action: Action::CloseArticle,
        keys: "esc",
        label: "close the article",
        group: "reader",
    },
    // -- feeds ----------------------------------------------------------------
    Binding {
        action: Action::AddFeed,
        keys: "a",
        label: "add a feed",
        group: "feeds",
    },
    Binding {
        action: Action::RefreshAll,
        keys: "R/F5",
        label: "refresh everything",
        group: "feeds",
    },
    Binding {
        action: Action::Import,
        keys: "alt+i",
        label: "import feeds",
        group: "feeds",
    },
    // -- appearance -----------------------------------------------------------
    Binding {
        action: Action::NextTheme,
        keys: "t",
        label: "next theme",
        group: "appearance",
    },
    Binding {
        action: Action::PrevTheme,
        keys: "T",
        label: "previous theme",
        group: "appearance",
    },
    Binding {
        action: Action::Settings,
        keys: ",",
        label: "settings",
        group: "appearance",
    },
    // -- application ----------------------------------------------------------
    Binding {
        action: Action::Help,
        keys: "?/F1",
        label: "this list",
        group: "application",
    },
    Binding {
        action: Action::Redraw,
        keys: "ctrl+l",
        label: "redraw the screen",
        group: "application",
    },
    Binding {
        action: Action::Quit,
        keys: "q/ctrl+c",
        label: "quit",
        group: "application",
    },
];

/// What the mouse does.
pub const MOUSE: &[MouseHelp] = &[
    MouseHelp {
        gesture: "click",
        label: "move the cursor",
        group: "lists",
    },
    MouseHelp {
        gesture: "double-click",
        label: "open it, read it",
        group: "lists",
    },
    MouseHelp {
        gesture: "right-click",
        label: "star it",
        group: "entries",
    },
    MouseHelp {
        gesture: "click a crumb",
        label: "jump there",
        group: "lists",
    },
    MouseHelp {
        gesture: "click a link",
        label: "open it",
        group: "reader",
    },
    MouseHelp {
        gesture: "wheel",
        label: "scroll three rows",
        group: "lists",
    },
    MouseHelp {
        gesture: "click a fold",
        label: "open the module",
        group: "modules",
    },
    MouseHelp {
        gesture: "click a word",
        label: "what it says",
        group: "modules",
    },
    MouseHelp {
        gesture: "drag the bar",
        label: "scroll it",
        group: "modules",
    },
    MouseHelp {
        gesture: "click the count",
        label: "to the sources",
        group: "status",
    },
];

/// The global half of the table, keyed.
///
/// `Keymap::from_table` parses each binding's alternatives with
/// `KeySpec::parse` and silently drops whatever does not parse as one key,
/// which is what lets the `gg` and `o<n>` spellings sit in [`BINDINGS`] for
/// the help overlay without ever producing an entry here.
static GLOBAL: LazyLock<Keymap<Action>> = LazyLock::new(|| Keymap::from_table(&global_bindings()));

/// A module's own half, one map each, built once.
static MODULES: LazyLock<Vec<(Module, Keymap<Action>)>> = LazyLock::new(|| {
    ALL_MODULES
        .iter()
        .map(|&m| (m, Keymap::from_table(&module_bindings(m))))
        .collect()
});

const ALL_MODULES: &[Module] = &[Module::Sources, Module::Entries, Module::Reader];

/// One module binding that intentionally claims a key the global table
/// already uses, and why. Every other repeat between a module's bindings and
/// the global table is a mistake rather than a decision, which is what the
/// invariant test below is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shadow {
    pub module: Module,
    /// The key spelling, exactly as [`starkit::keymap::KeySpec::parse`]
    /// accepts it.
    pub keys: &'static str,
    pub module_action: Action,
    pub global_action: Action,
    pub reason: &'static str,
}

/// The complete list of deliberate shadows -- one. See [`Shadow`].
pub const SHADOWS: &[Shadow] = &[Shadow {
    module: Module::Reader,
    keys: "esc",
    module_action: Action::CloseArticle,
    global_action: Action::Back,
    reason: "esc in the reader closes the article, which is the only thing \
             there is to back out of once the filter and the chord have had \
             their turn",
}];

fn scope_of(group: &str) -> Scope {
    GROUPS
        .iter()
        .find(|(name, _)| *name == group)
        .map(|(_, scope)| *scope)
        .unwrap_or(Scope::Global)
}

fn global_bindings() -> Vec<Binding> {
    BINDINGS
        .iter()
        .filter(|b| scope_of(b.group) == Scope::Global)
        .map(copy_binding)
        .collect()
}

fn module_bindings(m: Module) -> Vec<Binding> {
    BINDINGS
        .iter()
        .filter(|b| match scope_of(b.group) {
            Scope::Global => false,
            Scope::Modules(list) => list.contains(&m),
        })
        .map(copy_binding)
        .collect()
}

fn copy_binding(b: &Binding) -> Binding {
    Binding {
        action: b.action,
        keys: b.keys,
        label: b.label,
        group: b.group,
    }
}

/// The focused module's own bindings, tried first. `None` means the module
/// has no use for this key and the global table should have it.
pub fn module(m: Module, k: KeyEvent) -> Option<Action> {
    MODULES
        .iter()
        .find(|(id, _)| *id == m)
        .and_then(|(_, map)| map.resolve(k))
}

/// The global table, tried after the focused module has declined.
pub fn resolve(k: KeyEvent) -> Option<Action> {
    GLOBAL.resolve(k)
}

/// What the key after a `g` means, if `g` was the one before it.
///
/// One chord here, `gg`, where STAR/FOLD has three: a news reader has no
/// home directory and no root to go to. Pure and stateless -- whether a `g`
/// is pending is state `ui/app/keys.rs` owns, not this module's.
pub fn g_prefix(k: KeyEvent) -> Option<Action> {
    if k.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    match k.code {
        KeyCode::Char('g') => Some(Action::Home),
        _ => None,
    }
}

/// What an `o` chord does with the key that followed it.
///
/// See the module doc for why the number closes as soon as it cannot grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OChord {
    /// The digit was taken and the chord is still waiting: another digit
    /// could still name a link that exists.
    Digit,
    /// Open link *n* -- the number is complete, either because a further
    /// digit could not name a link or because `enter` closed it.
    Open(u16),
    /// `oo`: the article's own link, in the browser.
    OpenArticle,
    /// The chord is over and nothing happens: `esc`, or a number no link
    /// has.
    Cancel,
    /// Not the chord's key. The chord stays pending and the caller
    /// dispatches the key the ordinary way -- the same carve-out
    /// [`filter_eats`] makes, and for the same reason: cycling the theme or
    /// changing focus has to keep working with a chord half-typed.
    Ignore,
}

/// The most digits a link number is ever worth reading. Four is ten thousand
/// links in one article, which is two orders of magnitude past the worst
/// real page; the cap is here so a held-down digit key cannot grow a number
/// past what `u16` holds.
const MAX_LINK_DIGITS: usize = 4;

/// What `k` means, given an `o` chord is open, `typed` digits have been
/// collected so far and the article has `links` numbered links.
///
/// `typed` is the digits as they were typed, not a number, so that "nothing
/// yet" and "a typed zero" are different states -- `o0` names no link and is
/// refused, where an empty `typed` followed by `o` is the article itself.
pub fn o_prefix(k: KeyEvent, typed: &str, links: u16) -> OChord {
    if k.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return OChord::Ignore;
    }
    match k.code {
        KeyCode::Esc => OChord::Cancel,
        KeyCode::Enter => match typed.parse::<u16>() {
            Ok(n) if n >= 1 && n <= links => OChord::Open(n),
            _ => OChord::Cancel,
        },
        KeyCode::Char('o') if typed.is_empty() => OChord::OpenArticle,
        KeyCode::Char(c) if c.is_ascii_digit() => {
            if typed.len() >= MAX_LINK_DIGITS {
                return OChord::Cancel;
            }
            let mut grown = String::with_capacity(typed.len() + 1);
            grown.push_str(typed);
            grown.push(c);
            let Ok(n) = grown.parse::<u32>() else {
                return OChord::Cancel;
            };
            if n == 0 || n > u32::from(links) {
                return OChord::Cancel;
            }
            // Could another digit still name a link? If not, there is
            // nothing to wait for.
            if n * 10 > u32::from(links) {
                OChord::Open(n as u16)
            } else {
                OChord::Digit
            }
        }
        _ => OChord::Cancel,
    }
}

/// Whether `k`, with no `ctrl`/`alt`, is a key someone typing text would
/// expect to work: a character, or a plain editing motion.
pub fn is_text_key(k: KeyEvent) -> bool {
    if k.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return false;
    }
    matches!(
        k.code,
        KeyCode::Char(_)
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
    )
}

/// Whether the `/` filter's text entry, having focus, takes this key as
/// typing rather than as a command.
///
/// STAR/FOLD's rule, unchanged. Every `alt+…` falls through, so changing
/// focus or the theme works with a filter half-typed; `ctrl+a`/`e`/`u`/`w`
/// are the line-editing motions the field implements and every other
/// `ctrl+…` falls through; `?` is the one plain character carved out,
/// because help has to stay reachable mid-search; `esc` and `enter` are the
/// ways out and neither is a text key to begin with.
pub fn filter_eats(k: KeyEvent) -> bool {
    if k.modifiers.contains(KeyModifiers::ALT) {
        return false;
    }
    if k.code == KeyCode::Char('?') {
        return false;
    }
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        return matches!(
            k.code,
            KeyCode::Char('u') | KeyCode::Char('w') | KeyCode::Char('a') | KeyCode::Char('e')
        );
    }
    is_text_key(k)
}

/// What `docs/keys-and-mouse.md` says before the tables.
const HEADER: &str = "\
# Keys and the mouse

Every key STAR/WIRE knows, in the order the `?` overlay prints them. This
file is generated from the table in `src/ui/keymap.rs`, and a test fails if
the two disagree.

A key reaches its action through six layers, tried in order: an open overlay
takes every key while it is up; the `/` filter's text entry takes typing
next, because a letter typed into it is a letter, not a command; an `o`
waiting for a link number comes next; a `g` waiting for its second key comes
after that; the focused module's own bindings are offered the key, so a
binding under a module heading works while that module has focus; and the
global table catches whatever nothing above wanted, which is what makes it
work from everywhere. While the filter has focus, every `alt+\u{2026}` falls
through it and so does `?`, so help and the appearance keys stay reachable
mid-search; `esc` and `enter` are always the way out.

In the reader `o` waits: `o` again opens the article itself in the browser,
and a digit opens the link the article numbers with it. The number closes as
soon as it cannot grow \u{2014} `o1` in an article with three links opens link
one at once, and in an article with twelve links it waits to see whether a `2`
follows.
";

/// The key table as `docs/keys-and-mouse.md`. Run with
/// `STARWIRE_UPDATE_DOCS=1` to rewrite the file the test below compares
/// against.
pub fn document() -> String {
    let mut out = String::new();
    out.push_str(HEADER);

    let mut group = "";
    for b in BINDINGS {
        if b.group != group {
            group = b.group;
            let scope = match scope_of(group) {
                Scope::Global => "everywhere".to_string(),
                Scope::Modules(list) => {
                    let names: Vec<&str> = list.iter().map(|m| module_name(*m)).collect();
                    format!("in {}", names.join(", "))
                }
            };
            out.push_str(&format!("\n## {group}\n\n_{scope}_\n\n"));
            out.push_str("| key | what it does |\n|---|---|\n");
        }
        let keys = format!("`{}`", b.keys);
        out.push_str(&format!(
            "| {keys:<width$} | {} |\n",
            b.label,
            width = starkit::keymap::KEYS_COLUMN
        ));
    }

    out.push_str("\n## The mouse\n\n| where | gesture | what it does |\n|---|---|---|\n");
    for m in MOUSE {
        out.push_str(&format!(
            "| {:<8} | {:<width$} | {} |\n",
            m.group,
            m.gesture,
            m.label,
            width = starkit::keymap::GESTURE_COLUMN
        ));
    }
    out
}

fn module_name(m: Module) -> &'static str {
    match m {
        Module::Sources => "the sources",
        Module::Entries => "the entries",
        Module::Reader => "the reader",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starkit::keymap::KeySpec;

    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn with(c: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(c, m)
    }

    fn specs(b: &Binding) -> Vec<KeySpec> {
        starkit::keymap::alternatives(b.keys)
            .filter_map(KeySpec::parse)
            .collect()
    }

    /// The second character of a `g<char>` chord spelling, or `None` if the
    /// alternative is not shaped like one. `KeySpec::parse` refuses two
    /// characters as one key, and `g_prefix` is how it dispatches instead.
    fn g_chord(alt: &str) -> Option<char> {
        let mut chars = alt.chars();
        let first = chars.next()?;
        let second = chars.next()?;
        if chars.next().is_some() || first != 'g' {
            return None;
        }
        Some(second)
    }

    #[test]
    fn every_group_in_the_table_declares_its_scope() {
        for b in BINDINGS {
            assert!(
                GROUPS.iter().any(|(name, _)| *name == b.group),
                "the group {:?} is in the table and not in GROUPS",
                b.group
            );
        }
    }

    #[test]
    fn every_binding_group_is_listed_once() {
        let mut seen: Vec<&str> = Vec::new();
        let mut last = "";
        for b in BINDINGS {
            if b.group != last {
                assert!(
                    !seen.contains(&b.group),
                    "{} appears in two places in the table",
                    b.group
                );
                seen.push(b.group);
                last = b.group;
            }
        }
    }

    #[test]
    fn nothing_in_the_table_overruns_its_column() {
        for b in BINDINGS {
            assert!(
                b.keys.chars().count() <= 13,
                "{:?} leaves no gap before {:?}",
                b.keys,
                b.label
            );
            assert!(b.label.chars().count() <= 19, "{:?} would wrap", b.label);
        }
        for m in MOUSE {
            assert!(m.gesture.chars().count() <= 19, "{:?}", m.gesture);
            assert!(m.label.chars().count() <= 19, "{:?} would wrap", m.label);
        }
    }

    /// Every key in [`BINDINGS`] can be pressed and reaches its action --
    /// through [`resolve`]/[`module`] for an ordinary spelling, through
    /// [`g_prefix`] for `gg`, and through [`o_prefix`] for `o<n>`. A
    /// spelling that is none of those is the "gg has no real key" mistake
    /// the chord rule exists to catch.
    #[test]
    fn every_binding_reaches_its_action() {
        for b in BINDINGS {
            let mut reachable = false;
            for alt in starkit::keymap::alternatives(b.keys) {
                if alt == "o<n>" {
                    // The one chord whose action is the chord itself: the
                    // number comes from `o_prefix`, not from a key table.
                    assert_eq!(b.action, Action::OpenLink, "{alt:?} is not the link chord");
                    assert_eq!(o_prefix(plain('1'), "", 3), OChord::Open(1));
                    reachable = true;
                    continue;
                }
                if let Some(c) = g_chord(alt) {
                    assert_eq!(
                        g_prefix(plain(c)),
                        Some(b.action),
                        "{:?} ({:?}) is a chord and g_prefix does not dispatch it",
                        b.keys,
                        b.action
                    );
                    reachable = true;
                    continue;
                }
                let Some(spec) = KeySpec::parse(alt) else {
                    continue;
                };
                let k = KeyEvent::new(spec.code, spec.mods);
                let hit = match scope_of(b.group) {
                    Scope::Global => resolve(k) == Some(b.action),
                    Scope::Modules(list) => list.iter().any(|&m| module(m, k) == Some(b.action)),
                };
                reachable |= hit;
            }
            assert!(
                reachable,
                "{:?} ({:?}) is in the help and dispatches to nothing",
                b.keys, b.action
            );
        }
    }

    #[test]
    fn shift_is_the_big_step() {
        assert_eq!(resolve(code(KeyCode::Up)), Some(Action::CursorUp));
        assert_eq!(
            resolve(with(KeyCode::Up, KeyModifiers::SHIFT)),
            Some(Action::CursorUpBig)
        );
        assert_eq!(
            resolve(with(KeyCode::Down, KeyModifiers::SHIFT)),
            Some(Action::CursorDownBig)
        );
    }

    /// `hjkl` navigates and never adjusts a value: `j`/`k` reach the global
    /// cursor and no module claims them, and `h`/`l` are the global stack
    /// keys and nothing else.
    #[test]
    fn hjkl_only_ever_moves() {
        for &m in ALL_MODULES {
            for c in ['h', 'j', 'k', 'l'] {
                assert_eq!(module(m, plain(c)), None, "{m:?} claims {c:?}");
            }
        }
        assert_eq!(resolve(plain('j')), Some(Action::CursorDown));
        assert_eq!(resolve(plain('k')), Some(Action::CursorUp));
        assert_eq!(resolve(plain('l')), Some(Action::Enter));
        assert_eq!(resolve(plain('h')), Some(Action::Pop));
    }

    #[test]
    fn esc_never_quits() {
        assert_ne!(resolve(code(KeyCode::Esc)), Some(Action::Quit));
        for &m in ALL_MODULES {
            assert_ne!(module(m, code(KeyCode::Esc)), Some(Action::Quit), "{m:?}");
        }
        for b in BINDINGS {
            if b.action == Action::Quit {
                assert!(!b.keys.contains("esc"), "{:?}", b.keys);
            }
        }
    }

    #[test]
    fn every_alt_binding_survives_the_filter() {
        for b in BINDINGS {
            if scope_of(b.group) != Scope::Global {
                continue;
            }
            for s in specs(b) {
                if !s.mods.contains(KeyModifiers::ALT) {
                    continue;
                }
                let k = KeyEvent::new(s.code, s.mods);
                assert!(
                    !filter_eats(k),
                    "the filter eats {:?}, so {:?} cannot be reached while typing",
                    b.keys,
                    b.action
                );
                assert_eq!(resolve(k), Some(b.action));
            }
        }
    }

    /// And the same for the `o` chord: an `alt+…` pressed with a half-typed
    /// link number falls through to the ordinary dispatch rather than
    /// cancelling it.
    #[test]
    fn every_alt_binding_survives_the_o_chord() {
        for b in BINDINGS {
            if scope_of(b.group) != Scope::Global {
                continue;
            }
            for s in specs(b) {
                if !s.mods.contains(KeyModifiers::ALT) {
                    continue;
                }
                let k = KeyEvent::new(s.code, s.mods);
                assert_eq!(o_prefix(k, "1", 40), OChord::Ignore, "{:?}", b.keys);
            }
        }
    }

    #[test]
    fn no_module_has_a_key_twice() {
        for &m in ALL_MODULES {
            let bindings = module_bindings(m);
            let mut seen: Vec<(KeySpec, Action)> = Vec::new();
            for b in &bindings {
                for s in specs(b) {
                    if let Some((_, other)) = seen.iter().find(|(k, _)| *k == s) {
                        assert_eq!(
                            *other, b.action,
                            "{m:?} binds {s:?} to two different actions"
                        );
                    }
                    seen.push((s, b.action));
                }
            }
        }
    }

    #[test]
    fn no_global_key_means_two_things() {
        let mut seen: Vec<(KeySpec, Action)> = Vec::new();
        for b in global_bindings() {
            for s in specs(&b) {
                if let Some((_, other)) = seen.iter().find(|(k, _)| *k == s) {
                    panic!("{s:?} is both {other:?} and {:?}", b.action);
                }
                seen.push((s, b.action));
            }
        }
    }

    #[test]
    fn the_g_prefix_reaches_the_top_of_the_list() {
        assert_eq!(g_prefix(plain('g')), Some(Action::Home));
        assert_eq!(g_prefix(plain('x')), None, "a mistake reaches nothing");
    }

    /// `ctrl`/`alt` on the second key is never part of the sequence -- it is
    /// some other binding's, not a mistyped chord.
    #[test]
    fn the_g_prefix_ignores_ctrl_and_alt() {
        assert_eq!(
            g_prefix(with(KeyCode::Char('g'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(g_prefix(with(KeyCode::Char('g'), KeyModifiers::ALT)), None);
    }

    /// `g` on its own -- with no chord in progress -- resolves to nothing in
    /// either table, or the first half of `gg` would do something before the
    /// second half arrived.
    #[test]
    fn g_on_its_own_resolves_to_nothing() {
        assert_eq!(resolve(plain('g')), None);
        for &m in ALL_MODULES {
            assert_eq!(module(m, plain('g')), None, "{m:?}");
        }
    }

    /// The rule the whole chord exists for: a number that no further digit
    /// could grow into a link opens at once, and one that could waits.
    #[test]
    fn the_o_prefix_opens_at_once_when_the_number_cannot_grow() {
        // Three links: `1` cannot become `1n` for any n <= 3, so it opens.
        assert_eq!(o_prefix(plain('1'), "", 3), OChord::Open(1));
        assert_eq!(o_prefix(plain('3'), "", 3), OChord::Open(3));
        // Twelve links: `1` could still become `10`, `11` or `12`.
        assert_eq!(o_prefix(plain('1'), "", 12), OChord::Digit);
        assert_eq!(o_prefix(plain('2'), "1", 12), OChord::Open(12));
        // `2` on its own cannot grow past twelve, so it opens.
        assert_eq!(o_prefix(plain('2'), "", 12), OChord::Open(2));
        // A number no link has is nothing at all.
        assert_eq!(o_prefix(plain('4'), "", 3), OChord::Cancel);
        assert_eq!(o_prefix(plain('0'), "", 3), OChord::Cancel);
        assert_eq!(o_prefix(plain('9'), "1", 12), OChord::Cancel);
        // An article with no links has no chord to close.
        assert_eq!(o_prefix(plain('1'), "", 0), OChord::Cancel);
        // `esc` abandons it, `enter` closes whatever has been typed.
        assert_eq!(o_prefix(code(KeyCode::Esc), "1", 12), OChord::Cancel);
        assert_eq!(o_prefix(code(KeyCode::Enter), "1", 12), OChord::Open(1));
        assert_eq!(o_prefix(code(KeyCode::Enter), "", 12), OChord::Cancel);
        // And a number too long to be one stops rather than overflowing.
        assert_eq!(o_prefix(plain('1'), "9999", 60000), OChord::Cancel);
    }

    /// `o` on its own waits for a number only in the reader. In the entries
    /// list it is not a chord at all: it opens the entry under the cursor
    /// the moment it is pressed.
    #[test]
    fn o_on_its_own_is_the_browser_only_in_the_reader() {
        assert_eq!(o_prefix(plain('o'), "", 3), OChord::OpenArticle);
        // A second `o` after a digit is not the article: the chord is
        // already committed to a number.
        assert_eq!(o_prefix(plain('o'), "1", 40), OChord::Cancel);
        assert_eq!(
            module(Module::Entries, plain('o')),
            Some(Action::OpenBrowser)
        );
        assert_eq!(module(Module::Sources, plain('o')), None);
        assert_eq!(resolve(plain('o')), None, "`o` is nobody's global key");
    }

    /// `o<n>` sits in [`BINDINGS`] for the help overlay, and a bare `o` in
    /// the reader keeps its own meaning: the chord costs nothing to the key
    /// it borrows from.
    #[test]
    fn the_chord_spellings_do_not_shadow_their_own_letters() {
        assert_eq!(
            module(Module::Reader, plain('o')),
            Some(Action::OpenBrowser)
        );
        assert_eq!(resolve(code(KeyCode::Home)), Some(Action::Home));
    }

    #[test]
    fn plain_letters_are_typing_in_the_filter_and_esc_and_enter_are_not() {
        for c in ('a'..='z').chain('A'..='Z').chain('0'..='9') {
            assert!(
                filter_eats(plain(c)),
                "{c:?} should be typed, not dispatched"
            );
        }
        assert!(!filter_eats(code(KeyCode::Esc)));
        assert!(!filter_eats(code(KeyCode::Enter)));
    }

    #[test]
    fn the_filter_does_not_eat_question_mark() {
        assert!(!filter_eats(plain('?')));
        assert_eq!(resolve(plain('?')), Some(Action::Help));
    }

    #[test]
    fn the_filter_does_not_eat_the_navigation_and_redraw_keys() {
        for k in [
            code(KeyCode::Up),
            code(KeyCode::Down),
            code(KeyCode::PageUp),
            code(KeyCode::PageDown),
            code(KeyCode::Tab),
            code(KeyCode::BackTab),
            with(KeyCode::Char('l'), KeyModifiers::CONTROL),
        ] {
            assert!(!filter_eats(k), "{k:?} should fall through the filter");
        }
    }

    #[test]
    fn quitting_and_help_work_from_every_module() {
        for &m in ALL_MODULES {
            for (k, want) in [
                (plain('q'), Action::Quit),
                (plain('?'), Action::Help),
                (
                    with(KeyCode::Char('c'), KeyModifiers::CONTROL),
                    Action::Quit,
                ),
            ] {
                let got = module(m, k).or_else(|| resolve(k));
                assert_eq!(got, Some(want), "{m:?} + {k:?}");
            }
        }
    }

    #[test]
    fn a_module_declines_what_it_does_not_want() {
        for &m in ALL_MODULES {
            for c in ['q', 't', 'T', '/', 'a'] {
                assert_eq!(
                    module(m, plain(c)),
                    None,
                    "{m:?} swallowed {c:?}, which is global"
                );
            }
        }
    }

    /// `a` adds a feed from anywhere and `A` marks read in the two lists.
    /// They are next to each other on the keyboard on purpose, and both of
    /// the `A`s are confirmed before anything happens -- see the module doc.
    #[test]
    fn the_shifted_a_marks_read_and_the_bare_one_adds_a_feed() {
        assert_eq!(resolve(plain('a')), Some(Action::AddFeed));
        assert_eq!(
            module(Module::Sources, plain('A')),
            Some(Action::MarkSourceRead)
        );
        assert_eq!(
            module(Module::Entries, plain('A')),
            Some(Action::MarkAllRead)
        );
        assert_eq!(resolve(plain('A')), None, "`A` is nobody's global key");
    }

    /// A module's own binding is allowed to claim a key the global table
    /// already uses only where [`SHADOWS`] says so.
    #[test]
    fn no_module_key_shadows_a_global_one_except_the_documented_shadows() {
        for &m in ALL_MODULES {
            for b in module_bindings(m) {
                for s in specs(&b) {
                    let k = KeyEvent::new(s.code, s.mods);
                    let Some(global_action) = resolve(k) else {
                        continue;
                    };
                    let documented = SHADOWS.iter().any(|sh| {
                        sh.module == m
                            && sh.module_action == b.action
                            && sh.global_action == global_action
                    });
                    assert!(
                        documented,
                        "{m:?} binds {s:?} to {:?}, shadowing the global {global_action:?}, \
                         and this is not in SHADOWS",
                        b.action
                    );
                }
            }
        }

        // And every declared shadow is a real overlap, not a stale entry.
        assert_eq!(SHADOWS.len(), 1, "there is one deliberate shadow");
        for sh in SHADOWS {
            let spec = KeySpec::parse(sh.keys).unwrap_or_else(|| panic!("{sh:?} does not parse"));
            let k = KeyEvent::new(spec.code, spec.mods);
            assert_eq!(module(sh.module, k), Some(sh.module_action), "{sh:?}");
            assert_eq!(resolve(k), Some(sh.global_action), "{sh:?}");
            assert!(!sh.reason.is_empty(), "{sh:?} has no reason");
        }
    }
}

#[cfg(test)]
mod doc_tests {
    use super::*;

    fn path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/keys-and-mouse.md")
    }

    #[test]
    fn the_document_is_the_table() {
        let want = document();
        let path = path();
        if std::env::var_os("STARWIRE_UPDATE_DOCS").is_some() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).expect("the docs directory");
            }
            std::fs::write(&path, &want).expect("writing the document");
            return;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            have, want,
            "docs/keys-and-mouse.md is out of step with the key table; \
             run STARWIRE_UPDATE_DOCS=1 cargo test to rewrite it"
        );
    }

    #[test]
    fn every_key_and_gesture_is_in_the_document() {
        let text = std::fs::read_to_string(path()).unwrap_or_default();
        for b in BINDINGS {
            assert!(text.contains(b.keys), "{:?} is not in the document", b.keys);
            assert!(
                text.contains(b.label),
                "{:?} is not in the document",
                b.label
            );
        }
        for m in MOUSE {
            assert!(
                text.contains(m.gesture),
                "{:?} is not in the document",
                m.gesture
            );
            assert!(
                text.contains(m.label),
                "{:?} is not in the document",
                m.label
            );
        }
    }
}
