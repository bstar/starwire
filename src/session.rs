//! What STAR/WIRE remembers between runs.
//!
//! Not settings -- those are `config.toml`, which a person edits by hand.
//! This is where the reader was: which source was open, which entry, how far
//! down each article had been read, and whether the list was hiding what had
//! already been read. A session file that will not parse is not an error
//! worth reporting; it is a cache of one convenience, and losing it costs a
//! feed and a scroll position.
//!
//! The reading positions are the part that earns the file. An article is a
//! thousand rows long and somebody reads it over two evenings; without this
//! the second evening starts at the top.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::wire::feed::{EntryId, FeedId, FolderId, Selection};

/// How many reading positions are kept.
///
/// Five hundred articles is more than a month of reading at forty feeds, and
/// the file is still under twenty kilobytes. The oldest go first, which here
/// means the ones nearest the front of the list: [`Session::remember`] moves
/// an entry to the back every time its position is written, so the cap drops
/// whatever has been untouched longest.
pub const MAX_POSITIONS: usize = 500;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// The last open source, spelled by [`encode_source`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_entry: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_only: Option<bool>,
    /// `(entry id, first row drawn)`, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub positions: Vec<(i64, usize)>,
}

/// Read the session, or the defaults if there is none or it will not parse.
/// Either is logged rather than returned: nothing in the caller would do
/// anything with the error but fall back to these same defaults.
pub fn load(path: &Path) -> Session {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Session::default();
    };
    match toml::from_str(&text) {
        Ok(session) => session,
        Err(e) => {
            tracing::warn!("ignoring an unreadable session file: {e}");
            Session::default()
        }
    }
}

impl Session {
    /// Write it, through a temporary file in the same directory, so an
    /// interrupted write cannot leave a truncated session behind.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context("serialising the session")?;
        starkit::fs::write_atomic(path, text.as_bytes())
            .with_context(|| format!("writing {}", path.display()))
    }

    /// Where an article was left, if it is remembered.
    pub fn position(&self, entry: EntryId) -> Option<usize> {
        self.positions
            .iter()
            .find(|(id, _)| *id == entry.0)
            .map(|(_, row)| *row)
    }

    /// Record where an article was left, moving it to the back of the list
    /// so the cap drops what has been untouched longest.
    pub fn remember(&mut self, entry: EntryId, row: usize) {
        self.positions.retain(|(id, _)| *id != entry.0);
        self.positions.push((entry.0, row));
        let len = self.positions.len();
        if len > MAX_POSITIONS {
            self.positions.drain(..len - MAX_POSITIONS);
        }
    }

    /// Take a whole map of positions in, newest last, and cap it.
    pub fn set_positions(&mut self, mut positions: Vec<(i64, usize)>) {
        let len = positions.len();
        if len > MAX_POSITIONS {
            positions.drain(..len - MAX_POSITIONS);
        }
        self.positions = positions;
    }

    pub fn source(&self) -> Option<Selection> {
        self.last_source.as_deref().and_then(decode_source)
    }
}

/// A [`Selection`] as one line of TOML.
///
/// Spelled here rather than derived on the core's own enum: `Selection` is
/// the core's vocabulary and has no business carrying a serde derive for the
/// window's cache file. The ids are database row ids, which are stable for
/// the life of the database -- and a session naming a feed that has since
/// been removed simply falls back to `All`, which is what
/// [`decode_source`]'s caller does with a `None` anyway.
pub fn encode_source(sel: &Selection) -> String {
    match sel {
        Selection::All => "all".into(),
        Selection::Starred => "starred".into(),
        Selection::Videos => "videos".into(),
        Selection::Feed(id) => format!("feed:{}", id.0),
        Selection::Folder(id) => format!("folder:{}", id.0),
        Selection::Search(q) => format!("search:{q}"),
    }
}

/// The inverse. `None` for anything this version does not recognise, which
/// is what a session written by a later build looks like.
pub fn decode_source(text: &str) -> Option<Selection> {
    match text {
        "all" => return Some(Selection::All),
        "starred" => return Some(Selection::Starred),
        "videos" => return Some(Selection::Videos),
        _ => {}
    }
    let (kind, rest) = text.split_once(':')?;
    match kind {
        "feed" => Some(Selection::Feed(FeedId(rest.parse().ok()?))),
        "folder" => Some(Selection::Folder(FolderId(rest.parse().ok()?))),
        // A search is not restored as a *query* by accident: the string is
        // taken as it was typed, empty included, because an empty search is
        // a level the reader can still see and back out of.
        "search" => Some(Selection::Search(rest.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");

        let session = Session {
            last_source: Some(encode_source(&Selection::Feed(FeedId(7)))),
            last_entry: Some(42),
            unread_only: Some(true),
            positions: vec![(42, 120), (43, 0)],
        };
        session.save(&path).unwrap();
        assert_eq!(load(&path), session);
    }

    #[test]
    fn a_missing_or_broken_file_loads_as_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("nothing.toml")), Session::default());

        let path = dir.path().join("session.toml");
        std::fs::write(&path, "this is not [ toml").unwrap();
        assert_eq!(load(&path), Session::default());
    }

    #[test]
    fn every_selection_survives_being_written_down() {
        for sel in [
            Selection::All,
            Selection::Starred,
            Selection::Videos,
            Selection::Feed(FeedId(3)),
            Selection::Folder(FolderId(9)),
            Selection::Search("borrow checker".into()),
        ] {
            let text = encode_source(&sel);
            assert_eq!(decode_source(&text), Some(sel.clone()), "{text}");
        }
    }

    #[test]
    fn a_source_this_build_does_not_know_is_ignored_rather_than_fatal() {
        assert_eq!(decode_source("later"), None);
        assert_eq!(decode_source("feed:not-a-number"), None);
        assert_eq!(decode_source(""), None);
    }

    /// The cap drops what has been untouched longest, and re-reading an
    /// article moves it back to safety rather than leaving it near the edge.
    #[test]
    fn the_positions_are_capped_and_the_oldest_go_first() {
        let mut s = Session::default();
        for i in 0..(MAX_POSITIONS as i64 + 10) {
            s.remember(EntryId(i), i as usize);
        }
        assert_eq!(s.positions.len(), MAX_POSITIONS);
        assert_eq!(s.position(EntryId(0)), None, "the oldest was dropped");
        assert_eq!(s.position(EntryId(509)), Some(509));

        s.remember(EntryId(15), 3);
        assert_eq!(
            s.positions.len(),
            MAX_POSITIONS,
            "a repeat is not a new row"
        );
        assert_eq!(s.position(EntryId(15)), Some(3));
        assert_eq!(
            s.positions.last(),
            Some(&(15, 3)),
            "and it moved to the back"
        );
    }

    #[test]
    fn a_missing_parent_directory_is_created_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("session.toml");
        Session::default().save(&path).unwrap();
        assert!(path.is_file());
    }
}
