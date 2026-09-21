//! Reading the truth, once a frame, under one lock.
//!
//! [`ViewData`] is everything a frame draws, copied out of
//! [`crate::wire::State`] while the read guard is up and read from there for
//! the rest of the frame. A worker takes the *write* lock to fold its own
//! result in, and a read guard held across a draw or a channel send would
//! stall it -- so the guard's whole life is [`App::refresh`], and nothing
//! inside it sends, draws or allocates a terminal.
//!
//! The panels' own row structs are built here too, rather than in `draw`,
//! because they are what the cursor is clamped against and what a click is
//! answered from: a click handled between two draws must see the same rows
//! the last draw did.

use std::collections::BTreeSet;

use starkit::chrome::frame::Tone;

use crate::ui::panels::ModuleId;
use crate::ui::panels::{entries, sources};
use crate::ui::stack::Level;
use crate::wire::feed::{ArticleView, EntryId, FeedId, FolderId, Selection};
use crate::wire::import::ImportReport;
use crate::wire::state::RefreshProgress;
use crate::wire::ImportOffer;

use super::App;

/// What a SOURCES row names, and therefore what opening it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKey {
    All,
    Unread,
    Starred,
    Videos,
    /// The `⌕ Search…` row: opens the overlay rather than a level.
    Search,
    Folder(FolderId),
    Feed(FeedId),
}

impl SourceKey {
    /// The selection this row opens as a list of entries. `None` for the
    /// search row, which has a question to ask first.
    pub fn selection(&self) -> Option<Selection> {
        Some(match self {
            SourceKey::All | SourceKey::Unread => Selection::All,
            SourceKey::Starred => Selection::Starred,
            SourceKey::Videos => Selection::Videos,
            SourceKey::Search => return None,
            SourceKey::Folder(id) => Selection::Folder(*id),
            SourceKey::Feed(id) => Selection::Feed(*id),
        })
    }

    /// A stable name for the row, so a refresh that inserted a feed above
    /// the cursor leaves the cursor on the same thing.
    pub fn stable(&self) -> String {
        match self {
            SourceKey::All => "\u{0}all".into(),
            SourceKey::Unread => "\u{0}unread".into(),
            SourceKey::Starred => "\u{0}starred".into(),
            SourceKey::Videos => "\u{0}videos".into(),
            SourceKey::Search => "\u{0}search".into(),
            SourceKey::Folder(id) => format!("\u{0}folder:{}", id.0),
            SourceKey::Feed(id) => format!("\u{0}feed:{}", id.0),
        }
    }
}

/// Everything one frame draws.
#[derive(Debug, Default)]
pub struct ViewData {
    // -- sources ----------------------------------------------------------
    pub source_rows: Vec<sources::SourceRow>,
    pub source_keys: Vec<SourceKey>,
    pub source_crumbs: Vec<sources::Crumb>,
    /// `41 feeds · 312 unread`.
    pub source_summary: String,

    // -- entries ----------------------------------------------------------
    pub entry_rows: Vec<entries::EntryRow>,
    pub entry_ids: Vec<EntryId>,
    /// Read straight off the rows, for `m`, `s` and the status percentage.
    pub entry_read: Vec<bool>,
    pub entry_source: String,
    pub entry_aggregate: bool,
    pub entry_badge: Option<(String, Tone)>,
    pub entries_loading: bool,
    pub page_total: i64,
    pub filter: String,

    // -- reader -----------------------------------------------------------
    pub article: Option<ArticleView>,
    pub open_entry: Option<EntryId>,
    /// When the open entry was published, which is the date the byline
    /// shows. The entry's own date rather than when the page was scraped:
    /// "20 Sep 2026" is a fact about the article, and `extracted_at` is a
    /// fact about this program.
    pub open_published: Option<jiff::Timestamp>,

    // -- the rest ---------------------------------------------------------
    pub refresh: RefreshProgress,
    pub fetching: BTreeSet<FeedId>,
    pub feeds_len: usize,
    pub unread_total: i64,
    pub import_offer: Option<ImportOffer>,
    pub last_import: Option<ImportReport>,
    pub extract_on: bool,
    pub extract_busy: bool,
}

impl App {
    /// Copy what the panels draw out of `State`, when the version has moved
    /// or the stack has.
    ///
    /// One read lock: everything below happens while `state` is held and the
    /// guard is dropped before anything else runs -- see the module doc.
    pub(super) fn refresh(&mut self) {
        let stack_now = self.stack_stamp();
        {
            let state = self.core.state();
            if state.version == self.seen_version && stack_now == self.seen_stack {
                return;
            }
            self.seen_version = state.version;
            self.seen_stack = stack_now;

            let (source_rows, source_keys) = build_source_rows(&state, self.sources_folder());
            let source_summary = summary(state.feeds.len(), state.unread_total());
            let source_crumbs = self.build_source_crumbs(&state);

            let (entry_rows, entry_ids, entry_read) = build_entry_rows(&state, self.now());
            let entry_source = selection_name(&state, &state.selection);
            let entry_aggregate = !matches!(state.selection, Selection::Feed(_));

            self.view = ViewData {
                source_rows,
                source_keys,
                source_crumbs,
                source_summary,
                entry_rows,
                entry_ids,
                entry_read,
                entry_source,
                entry_aggregate,
                entry_badge: None,
                entries_loading: state.page.loading,
                page_total: state.page.total,
                filter: state.page.filter.clone(),
                article: state.article.clone(),
                open_entry: state.open_entry,
                open_published: state
                    .open_entry
                    .and_then(|id| state.page.row(id))
                    .and_then(|row| row.published),
                refresh: state.refresh.clone(),
                fetching: state.fetching.clone(),
                feeds_len: state.feeds.len(),
                unread_total: state.unread_total(),
                import_offer: state.import_offer.clone(),
                last_import: state.last_import.clone(),
                extract_on: state.settings.extract,
                extract_busy: state.extract_inflight > 0 || !state.extract_queue.is_empty(),
            };
        }
        // The badge needs the spinner, which needs the clock, which is this
        // side of the lock.
        self.view.entry_badge = self.entries_badge();
        self.refind_cursors();
        self.clamp_scrolls();
    }

    /// Which folder the active SOURCES frame is looking at.
    pub(super) fn sources_folder(&self) -> Option<FolderId> {
        match self
            .stack
            .top_of(ModuleId::Sources)
            .map(|i| &self.stack.frames()[i].level)
        {
            Some(Level::Sources { folder }) => *folder,
            _ => None,
        }
    }

    /// One value that changes whenever the stack does, so `refresh` can tell
    /// an `alt+up` (which moves no version) from nothing having happened.
    fn stack_stamp(&self) -> (usize, usize) {
        (self.stack.active_index(), self.stack.len())
    }

    fn build_source_crumbs(&self, state: &crate::wire::State) -> Vec<sources::Crumb> {
        let top = self.stack.top_of(ModuleId::Sources).unwrap_or(0);
        self.stack.frames()[..=top]
            .iter()
            .filter_map(|f| match &f.level {
                Level::Sources { folder: None } => Some(sources::Crumb {
                    name: "sources".into(),
                    count: Some(state.feeds.len()),
                }),
                Level::Sources { folder: Some(id) } => {
                    let name = state
                        .folders
                        .iter()
                        .find(|f| f.id == *id)
                        .map(|f| f.name.clone())
                        .unwrap_or_else(|| "folder".into());
                    let count = state.feeds.iter().filter(|f| f.folder == Some(*id)).count();
                    Some(sources::Crumb {
                        name,
                        count: Some(count),
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// The ENTRIES badge: the refresh spinner while one runs, the unread
    /// count otherwise.
    fn entries_badge(&self) -> Option<(String, Tone)> {
        if self.view.refresh.running {
            let frame = SPINNER[self.spinner % SPINNER.len()];
            return Some((
                format!(
                    "{frame} {} of {}",
                    self.view.refresh.done, self.view.refresh.total
                ),
                Tone::Accent,
            ));
        }
        let unread = self.view.entry_read.iter().filter(|read| !**read).count();
        (unread > 0).then(|| (format!("{unread} unread"), Tone::Dim))
    }

    /// Put each frame's cursor back on the row it was on, by name, after
    /// the rows underneath it have been rebuilt.
    fn refind_cursors(&mut self) {
        let sources_top = self.stack.top_of(ModuleId::Sources);
        let entries_top = self.stack.top_of(ModuleId::Entries);
        let source_keys: Vec<String> = self.view.source_keys.iter().map(|k| k.stable()).collect();
        let entry_keys: Vec<String> = self
            .view
            .entry_ids
            .iter()
            .map(|id| id.0.to_string())
            .collect();

        for (i, frame) in self.stack.frames_mut().enumerate() {
            let keys = if Some(i) == sources_top {
                &source_keys
            } else if Some(i) == entries_top {
                &entry_keys
            } else {
                continue;
            };
            if keys.is_empty() {
                frame.cursor = 0;
                continue;
            }
            if let Some(key) = &frame.cursor_key {
                if let Some(at) = keys.iter().position(|k| k == key) {
                    frame.cursor = at;
                    continue;
                }
            }
            frame.cursor = frame.cursor.min(keys.len() - 1);
            frame.cursor_key = Some(keys[frame.cursor].clone());
        }
    }
}

/// The spinner beside the refresh count. Braille, as everywhere else in the
/// family.
pub const SPINNER: [char; 10] = [
    '\u{280b}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283c}', '\u{2834}', '\u{2826}', '\u{2827}',
    '\u{2807}', '\u{280f}',
];

fn summary(feeds: usize, unread: i64) -> String {
    let noun = if feeds == 1 { "feed" } else { "feeds" };
    format!("{feeds} {noun} \u{b7} {unread} unread")
}

/// The rows of the SOURCES list for one level: the special rows and the
/// folders at the root, one folder's feeds inside it.
pub fn build_source_rows(
    state: &crate::wire::State,
    folder: Option<FolderId>,
) -> (Vec<sources::SourceRow>, Vec<SourceKey>) {
    let mut rows = Vec::new();
    let mut keys = Vec::new();

    if let Some(id) = folder {
        for feed in state.feeds.iter().filter(|f| f.folder == Some(id)) {
            rows.push(feed_row(state, feed));
            keys.push(SourceKey::Feed(feed.id));
        }
        return (rows, keys);
    }

    let unread = state.unread_total();
    let specials: [(&'static str, &str, SourceKey, Option<i64>); 5] = [
        ("\u{25cf}", "All", SourceKey::All, Some(unread)),
        ("\u{25cb}", "Unread", SourceKey::Unread, Some(unread)),
        ("\u{2605}", "Starred", SourceKey::Starred, None),
        ("\u{25b6}", "Videos", SourceKey::Videos, None),
        ("\u{2315}", "Search\u{2026}", SourceKey::Search, None),
    ];
    for (glyph, name, key, count) in specials {
        rows.push(sources::SourceRow {
            glyph,
            name: name.to_string(),
            count,
            extra: None,
            kind: sources::RowKind::Special,
            dim: false,
        });
        keys.push(key);
    }

    for f in &state.folders {
        let count: i64 = state
            .feeds
            .iter()
            .filter(|feed| feed.folder == Some(f.id))
            .map(|feed| feed.unread)
            .sum();
        // A folder nothing is in is not worth a row: folders here exist
        // because a feed is in one (see `db::feeds::folder_named`), and an
        // empty one is a leftover rather than a place to go.
        if !state.feeds.iter().any(|feed| feed.folder == Some(f.id)) {
            continue;
        }
        rows.push(sources::SourceRow {
            glyph: "\u{25b8}",
            name: f.name.clone(),
            count: Some(count),
            extra: None,
            kind: sources::RowKind::Folder,
            dim: count == 0,
        });
        keys.push(SourceKey::Folder(f.id));
    }

    for feed in state.feeds.iter().filter(|f| f.folder.is_none()) {
        rows.push(feed_row(state, feed));
        keys.push(SourceKey::Feed(feed.id));
    }

    (rows, keys)
}

fn feed_row(state: &crate::wire::State, feed: &crate::wire::feed::FeedRow) -> sources::SourceRow {
    let fetching = state.is_fetching(feed.id);
    let extra = if fetching {
        Some("fetching\u{2026}".to_string())
    } else {
        feed.error.clone()
    };
    let kind = if feed.error.is_some() && !fetching {
        sources::RowKind::Failed
    } else {
        sources::RowKind::Feed
    };
    sources::SourceRow {
        glyph: match feed.kind {
            crate::wire::feed::FeedKind::Youtube => "\u{25b6}",
            _ => " ",
        },
        name: feed.display_title().to_string(),
        count: Some(feed.unread),
        extra,
        kind,
        dim: feed.unread == 0,
    }
}

/// The rows of the ENTRIES list, in the order the page's filter left them.
pub fn build_entry_rows(
    state: &crate::wire::State,
    now: jiff::Timestamp,
) -> (Vec<entries::EntryRow>, Vec<EntryId>, Vec<bool>) {
    let mut rows = Vec::new();
    let mut ids = Vec::new();
    let mut read = Vec::new();
    for row in state.page.visible_rows() {
        rows.push(entries::EntryRow {
            unread: !row.read,
            starred: row.starred,
            kind: row.kind,
            title: row.title.clone(),
            feed: row.feed_title.clone(),
            age: row
                .published
                .map(|at| entries::age(at, now))
                .unwrap_or_else(|| "-".into()),
            status: row.article_status,
        });
        ids.push(row.id);
        read.push(row.read);
    }
    (rows, ids, read)
}

/// What a selection is called, for the panel title and the status row.
pub fn selection_name(state: &crate::wire::State, sel: &Selection) -> String {
    match sel {
        Selection::All => "All".into(),
        Selection::Starred => "Starred".into(),
        Selection::Videos => "Videos".into(),
        Selection::Search(q) => format!("search: {q}"),
        Selection::Feed(id) => state
            .feed(*id)
            .map(|f| f.display_title().to_string())
            .unwrap_or_else(|| "a feed".into()),
        Selection::Folder(id) => state
            .folders
            .iter()
            .find(|f| f.id == *id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "a folder".into()),
    }
}
