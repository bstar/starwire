//! Eight laid-out articles, kept.
//!
//! Parsing an article is cheap and laying one out is not, and the answer
//! only changes when six things do: which entry it is, when its text was
//! last written, how wide the panel is, which theme is up, how many rows a
//! picture may take, and what is known about the pictures themselves. That
//! is exactly [`Key`], and it is the whole of the cache's cleverness.
//!
//! Eight entries, because eight is what `n` and `p` back and forth over a
//! handful of articles costs, plus the old width still being held while a
//! resize settles. The article the reader is looking at is one of them; the
//! other seven are what makes stepping back to the previous one instant.
//!
//! `extracted_at` in the key is why an extraction landing behind an
//! already-open article redraws it: the core writes the text and stamps it,
//! the stamp changes, the key misses, and the next frame lays it out again.
//! Nothing has to notice or invalidate anything.

use std::sync::Arc;

use jiff::Timestamp;

use super::layout::Rendered;
use crate::wire::feed::EntryId;

/// How many laid-out articles are kept. See the module doc.
pub const CAPACITY: usize = 8;

/// Everything a laid-out article depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub entry: EntryId,
    /// When the core last wrote this entry's text. `None` for an article
    /// that has none yet -- a pending extraction, or a video's description
    /// before it arrives.
    pub extracted_at: Option<Timestamp>,
    pub width: u16,
    /// Bumped whenever the theme changes, which is cheaper than comparing
    /// two resolved themes and is the only thing the cache needs to know
    /// about a cycle of `t`.
    pub theme_gen: u64,
    /// The cap one picture is laid out against: `[reading] image_rows`, or
    /// a third of the reader's body where that is zero -- so it changes
    /// when the window is resized as well as when the setting is.
    pub picture_rows: u16,
    /// Bumped whenever anything else a picture's rows depend on moved: one
    /// arrived or failed, the terminal reported a different cell size after
    /// a font zoom, the graphics mode changed, or the pictures were turned
    /// off. One number rather than a copy of the store, for `theme_gen`'s
    /// reason: the question is only ever "is this still the answer".
    pub pictures_gen: u64,
}

/// A small least-recently-used cache of laid-out articles.
///
/// A `Vec` rather than a map: eight entries is a linear scan of eight
/// comparisons, and the ordering an LRU needs is free when the entries are
/// in a list -- the most recently used one is moved to the end and eviction
/// takes the front.
#[derive(Debug, Default)]
pub struct Cache {
    entries: Vec<(Key, Arc<Rendered>)>,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(CAPACITY),
        }
    }

    /// The laid-out article for `key`, if it is still here. Asking for one
    /// counts as using it.
    pub fn get(&mut self, key: &Key) -> Option<Arc<Rendered>> {
        let at = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(at);
        let rendered = Arc::clone(&entry.1);
        self.entries.push(entry);
        Some(rendered)
    }

    /// Put one in, evicting the least recently used if that makes nine.
    pub fn put(&mut self, key: Key, rendered: Arc<Rendered>) {
        if let Some(at) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(at);
        }
        self.entries.push((key, rendered));
        while self.entries.len() > CAPACITY {
            self.entries.remove(0);
        }
    }

    /// The laid-out article for `key`, laying it out if it is not here.
    ///
    /// The way the panel actually uses this: it holds the key it wants and
    /// a closure that can build the answer, and never has to ask whether a
    /// hit or a miss happened.
    pub fn get_or_insert_with(
        &mut self,
        key: Key,
        build: impl FnOnce() -> Rendered,
    ) -> Arc<Rendered> {
        if let Some(hit) = self.get(&key) {
            return hit;
        }
        let rendered = Arc::new(build());
        self.put(key, Arc::clone(&rendered));
        rendered
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Throw the lot away. Not needed for a theme change or a resize -- the
    /// key covers both -- but it is what a "reload everything" ought to do.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Roughly how many bytes are held, for a note in the log rather than
    /// for a budget: the count is what bounds this cache.
    pub fn weight(&self) -> usize {
        self.entries.iter().map(|(_, r)| r.weight()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(entry: i64, width: u16) -> Key {
        Key {
            entry: EntryId(entry),
            extracted_at: None,
            width,
            theme_gen: 0,
            picture_rows: 0,
            pictures_gen: 0,
        }
    }

    fn rendered(text: &str) -> Rendered {
        Rendered {
            plain: text.to_string(),
            ..Rendered::default()
        }
    }

    #[test]
    fn a_hit_is_the_same_arc_and_not_a_second_layout() {
        let mut c = Cache::new();
        let mut built = 0;
        let first = c.get_or_insert_with(key(1, 80), || {
            built += 1;
            rendered("one")
        });
        let again = c.get_or_insert_with(key(1, 80), || {
            built += 1;
            rendered("one")
        });
        assert_eq!(built, 1, "it laid the same article out twice");
        assert!(Arc::ptr_eq(&first, &again));
    }

    /// Each of the six parts of the key really is part of it.
    #[test]
    fn every_part_of_the_key_is_a_miss_when_it_changes() {
        let base = Key {
            entry: EntryId(1),
            extracted_at: None,
            width: 80,
            theme_gen: 0,
            picture_rows: 0,
            pictures_gen: 0,
        };
        let stamped = Timestamp::from_second(1_700_000_000).unwrap();
        let others = [
            Key {
                entry: EntryId(2),
                ..base
            },
            Key {
                extracted_at: Some(stamped),
                ..base
            },
            Key { width: 72, ..base },
            Key {
                theme_gen: 1,
                ..base
            },
            Key {
                picture_rows: 8,
                ..base
            },
            Key {
                pictures_gen: 1,
                ..base
            },
        ];
        for other in others {
            let mut c = Cache::new();
            c.put(base, Arc::new(rendered("base")));
            assert!(c.get(&other).is_none(), "{other:?} hit the base entry");
        }
    }

    /// Eight in, nine in, and the one nobody has touched since is the one
    /// that goes.
    #[test]
    fn the_least_recently_used_is_what_is_evicted() {
        let mut c = Cache::new();
        for i in 0..CAPACITY as i64 {
            c.put(key(i, 80), Arc::new(rendered("x")));
        }
        assert_eq!(c.len(), CAPACITY);

        // Touch the oldest, so it is no longer the oldest.
        assert!(c.get(&key(0, 80)).is_some());
        c.put(key(99, 80), Arc::new(rendered("new")));

        assert_eq!(c.len(), CAPACITY);
        assert!(c.get(&key(0, 80)).is_some(), "the touched one survived");
        assert!(c.get(&key(1, 80)).is_none(), "the untouched one went");
        assert!(c.get(&key(99, 80)).is_some());
    }

    /// `n` and `p` back and forth over two articles at one width stays warm
    /// forever: that is the case the cache exists for.
    #[test]
    fn stepping_back_and_forth_between_two_articles_never_misses() {
        let mut c = Cache::new();
        let mut built = 0;
        for _ in 0..20 {
            for entry in [1, 2] {
                c.get_or_insert_with(key(entry, 80), || {
                    built += 1;
                    rendered("x")
                });
            }
        }
        assert_eq!(built, 2);
    }

    /// Putting the same key twice replaces rather than doubling.
    #[test]
    fn putting_a_key_twice_keeps_one_entry() {
        let mut c = Cache::new();
        c.put(key(1, 80), Arc::new(rendered("first")));
        c.put(key(1, 80), Arc::new(rendered("second")));
        assert_eq!(c.len(), 1);
        assert_eq!(c.get(&key(1, 80)).unwrap().plain, "second");
    }

    #[test]
    fn clearing_empties_it() {
        let mut c = Cache::new();
        c.put(key(1, 80), Arc::new(rendered("x")));
        assert!(!c.is_empty());
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.weight(), 0);
    }
}
