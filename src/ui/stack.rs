//! The stack of levels the column is drilled through.
//!
//! `▸ sources › Tech › Hacker News` is one [`Stack`]: every level visited on
//! the way down, in order, with [`Stack::active`] saying which one is open.
//! STAR/FOLD's `fold/stack.rs`, generalised from a directory to a [`Level`],
//! and the rules are the same ones `docs/the-stack.md` describes.
//!
//! [`Stack::pop`] ("back one level", `h`) *removes* the frame you back out
//! of -- the level is gone, and going back into it later starts fresh.
//! [`Stack::jump_to`] (`alt+up`, a crumb click) and [`Stack::forward`]
//! (`alt+down`) are the opposite: they only move [`Stack::active`], keeping
//! every frame on both sides of it, so a level you jumped away from rather
//! than backed out of is still there -- cursor, filter, scroll position and
//! all -- to jump straight back into.
//!
//! Pushing a *new* child drops anything past the active frame first: a
//! forward trail left by `jump_to`/`forward` was a guess about where the
//! reader was going, and a fresh navigation replaces it the way visiting a
//! new URL drops a browser's forward history. A `pop` never leaves a forward
//! trail to drop, because backing out has already removed it.
//!
//! ## The column only ever goes down
//!
//! A level's module is fixed by its shape -- sources, entries, article --
//! and the column has them in that order. So a push whose level belongs
//! *above* the active frame's module is not a navigation at all; it is a
//! mistake in whoever built it, and [`Stack::push`] asserts rather than
//! quietly producing a column that draws upside down. Pushing at the same
//! level is allowed: a folder inside a folder is two SOURCES frames, and a
//! search run from a list of entries is two ENTRIES frames.
//!
//! ## The reader is a module, not a level per article
//!
//! `n` and `p` in the reader call [`Stack::replace_active`] rather than
//! popping and pushing, so the frame id does not churn and the app's
//! `reader_scroll: HashMap<EntryId, usize>` keeps every position it has
//! learnt. The same call is what `u` (unread only) does to an ENTRIES frame.

use crate::wire::feed::{EntryId, FolderId, Selection};

use super::panels::ModuleId;

/// Identifies a [`Frame`] for the lifetime of its [`Stack`].
///
/// Not an index: an index shifts under a push or a pop and a stale one then
/// points at the wrong level. The core's jobs carry a [`Selection`] or an
/// [`EntryId`] rather than a frame id, but the *window* has to be able to
/// say "the answer that came back is for the frame that asked", and this is
/// what it says it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameId(pub u64);

/// What one level of the column is looking at.
///
/// `Entries` carries the core's own [`Selection`] rather than a parallel
/// enum of its own: the level *is* what the UI sends as `OpenFeed`, and two
/// spellings of the same six cases would eventually disagree. "Unread" is
/// not one of them -- it is `Selection::All` with `unread_only` set, which
/// is why that flag rides along here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    /// The feed list. `None` is the root: folders and the special rows.
    /// `Some(folder)` is one folder's feeds, drilled into.
    Sources {
        folder: Option<FolderId>,
    },
    Entries {
        source: Selection,
        unread_only: bool,
    },
    Article {
        entry: EntryId,
    },
}

impl Level {
    /// Which module of the column draws this level.
    pub fn module(&self) -> ModuleId {
        match self {
            Level::Sources { .. } => ModuleId::Sources,
            Level::Entries { .. } => ModuleId::Entries,
            Level::Article { .. } => ModuleId::Reader,
        }
    }
}

/// One level, and everything the window remembers about how it was being
/// looked at, so popping back into it -- or jumping straight back -- redraws
/// exactly what was left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: FrameId,
    pub level: Level,
    /// The position in the rows this frame currently draws. Kept in bounds
    /// by whoever rebuilds those rows; a frame with no rows sits at `0`.
    pub cursor: usize,
    /// What the cursor was *on*, kept beside the index so a refresh -- which
    /// can insert or remove rows above it -- finds the same feed or entry
    /// again rather than landing on whatever is now at that index. A feed's
    /// url, an entry's guid, whatever the panel that built the rows keyed
    /// them by.
    pub cursor_key: Option<String>,
    pub filter: String,
    /// The topmost row drawn, so paging remembers where the view was
    /// scrolled to rather than re-centring on the cursor every frame. In the
    /// reader it is the row of the article at the top of the panel.
    pub scroll: usize,
}

impl Frame {
    fn new(id: FrameId, level: Level) -> Self {
        Self {
            id,
            level,
            cursor: 0,
            cursor_key: None,
            filter: String::new(),
            scroll: 0,
        }
    }
}

/// The levels of one column, from the root to the deepest level still
/// remembered.
#[derive(Debug, Clone)]
pub struct Stack {
    frames: Vec<Frame>,
    active: usize,
    next_id: u64,
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}

impl Stack {
    /// A stack with one frame, open on the root of the feed list. That frame
    /// is never popped: there is nothing behind the sources.
    pub fn new() -> Self {
        Self {
            frames: vec![Frame::new(FrameId(0), Level::Sources { folder: None })],
            active: 0,
            next_id: 1,
        }
    }

    /// Drill into `level`. Anything past the active frame -- a trail from
    /// before the reader jumped away and went somewhere else -- is dropped
    /// first, the way a fresh navigation replaces a browser's forward
    /// history.
    ///
    /// # Panics
    ///
    /// If `level`'s module is *above* the active frame's in the column. See
    /// the module doc: that is a mistake in the caller, not a navigation.
    pub fn push(&mut self, level: Level) -> FrameId {
        let from = self.active().level.module();
        let to = level.module();
        assert!(
            to.index() >= from.index(),
            "a push from {from:?} to {to:?} goes up the column"
        );
        self.frames.truncate(self.active + 1);
        let id = FrameId(self.next_id);
        self.next_id += 1;
        self.frames.push(Frame::new(id, level));
        self.active = self.frames.len() - 1;
        id
    }

    /// Back one level. Unlike `jump_to`, the frame backed out of is
    /// discarded outright. `false`, changing nothing, at the root -- there
    /// is nowhere to go.
    pub fn pop(&mut self) -> bool {
        if self.active == 0 {
            return false;
        }
        self.frames.truncate(self.active);
        self.active -= 1;
        true
    }

    /// Jump straight to a level by its position in [`frames`](Self::frames)
    /// (which for an index `<= active` is also its position in
    /// [`crumbs`](Self::crumbs)), keeping every frame on both sides of it.
    /// `false`, changing nothing, for an index past the end.
    pub fn jump_to(&mut self, index: usize) -> bool {
        if index >= self.frames.len() {
            return false;
        }
        self.active = index;
        true
    }

    /// Step one frame towards the end of the trail -- how `alt+down` steps
    /// back into a level `alt+up` left behind rather than popped. `false`
    /// when the active frame is already the last one remembered.
    pub fn forward(&mut self) -> bool {
        if self.active + 1 >= self.frames.len() {
            return false;
        }
        self.active += 1;
        true
    }

    /// Put a different level in the active frame, keeping its id.
    ///
    /// The cursor, its key and the scroll are reset, because they described
    /// the level that has just been replaced; the filter is kept, because
    /// toggling unread-only under a half-typed filter should not throw the
    /// filter away.
    ///
    /// # Panics
    ///
    /// If the new level belongs to a different module. Replacing is for
    /// changing what a module is looking at -- the next article, the same
    /// list unread-only -- and a level that moved modules would leave the
    /// column drawing a frame in somebody else's panel.
    pub fn replace_active(&mut self, level: Level) -> FrameId {
        let was = self.active().level.module();
        assert_eq!(
            level.module(),
            was,
            "replace_active moved the frame from {was:?} to another module"
        );
        let frame = &mut self.frames[self.active];
        frame.level = level;
        frame.cursor = 0;
        frame.cursor_key = None;
        frame.scroll = 0;
        frame.id
    }

    /// The deepest frame drawn by `m`, wherever it is -- ahead of the active
    /// frame as well as behind it.
    ///
    /// Looking past the active frame is the point. `alt+3` with an article
    /// open and the cursor back up in the entries has to reach the reader,
    /// and the reader's frame is in the forward trail; `App::focus` is
    /// `stack.jump_to(stack.top_of(m))` and would otherwise answer "nothing
    /// open" for a module plainly on the screen.
    pub fn top_of(&self, m: ModuleId) -> Option<usize> {
        self.frames.iter().rposition(|f| f.level.module() == m)
    }

    pub fn active(&self) -> &Frame {
        &self.frames[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Frame {
        &mut self.frames[self.active]
    }

    /// Which frame is active, as an index into [`frames`](Self::frames).
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// Every level from the root to the active one, in drill-down order --
    /// what the folded rows and the active module's crumb line are drawn
    /// from.
    pub fn crumbs(&self) -> &[Frame] {
        &self.frames[..=self.active]
    }

    /// How many levels deep the active frame is, counting from one.
    pub fn depth(&self) -> usize {
        self.active + 1
    }

    /// How many frames the stack remembers in total, including whatever is
    /// past the active one and still reachable with `forward`. Always at
    /// least one: a `Stack` is never empty.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Whether `forward` has somewhere to go.
    pub fn has_forward(&self) -> bool {
        self.active + 1 < self.frames.len()
    }

    /// Every frame the stack remembers, active or not, root to deepest.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The same, mutably -- how a refresh that changed the rows under
    /// several levels at once re-finds each one's cursor.
    pub fn frames_mut(&mut self) -> impl Iterator<Item = &mut Frame> {
        self.frames.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::feed::FeedId;
    use proptest::prelude::*;

    fn entries(feed: i64) -> Level {
        Level::Entries {
            source: Selection::Feed(FeedId(feed)),
            unread_only: false,
        }
    }

    fn article(entry: i64) -> Level {
        Level::Article {
            entry: EntryId(entry),
        }
    }

    fn folder(id: i64) -> Level {
        Level::Sources {
            folder: Some(FolderId(id)),
        }
    }

    #[test]
    fn a_fresh_stack_is_the_root_of_the_feed_list() {
        let s = Stack::new();
        assert_eq!(s.len(), 1);
        assert_eq!(s.active().level, Level::Sources { folder: None });
        assert_eq!(s.active().id, FrameId(0));
        assert_eq!(s.depth(), 1);
    }

    #[test]
    fn pushing_truncates_the_forward_trail_left_by_a_jump() {
        let mut s = Stack::new();
        s.push(entries(1));
        s.push(article(10));
        assert_eq!(s.len(), 3);

        // Jump back to the root without popping: the article is still there,
        // reachable with `forward`.
        s.jump_to(0);
        assert!(s.has_forward());

        // A fresh push from here must drop it, because the reader just went
        // somewhere else.
        s.push(entries(2));
        assert_eq!(s.len(), 2);
        assert_eq!(s.active().level, entries(2));
        assert!(s.frames().iter().all(|f| f.level != article(10)));
    }

    #[test]
    fn back_removes_the_frame_it_leaves_rather_than_just_stepping_off_it() {
        let mut s = Stack::new();
        s.push(entries(1));
        s.push(article(10));
        assert_eq!(s.len(), 3);

        assert!(s.pop());
        assert_eq!(s.len(), 2, "the frame backed out of is gone, not kept");
        assert_eq!(s.active().level, entries(1));
        assert!(
            !s.has_forward(),
            "there is nothing left to step forward into"
        );
    }

    #[test]
    fn popping_at_the_root_does_nothing() {
        let mut s = Stack::new();
        assert!(!s.pop());
        assert_eq!(s.depth(), 1);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn jump_to_keeps_every_frame_on_both_sides() {
        let mut s = Stack::new();
        s.push(entries(1));
        s.push(article(10));
        assert!(s.jump_to(0));
        assert_eq!(s.active().level, Level::Sources { folder: None });
        assert_eq!(s.len(), 3);
        assert!(s.jump_to(2));
        assert_eq!(s.active().level, article(10));
    }

    #[test]
    fn jump_to_past_the_end_fails_and_changes_nothing() {
        let mut s = Stack::new();
        assert!(!s.jump_to(5));
        assert_eq!(s.depth(), 1);
    }

    #[test]
    fn forward_steps_into_a_level_a_jump_left_behind() {
        let mut s = Stack::new();
        s.push(entries(1));
        assert!(s.jump_to(0));
        assert!(s.has_forward());

        assert!(s.forward());
        assert_eq!(s.active().level, entries(1));
        assert!(!s.forward(), "there is nothing past the last frame");
    }

    /// `n` in the reader: the article changes, the frame does not. The id is
    /// what a job that is still in flight named, and the app's reading
    /// positions are keyed by entry rather than by frame, so both survive.
    #[test]
    fn replace_active_keeps_the_frame_and_its_id() {
        let mut s = Stack::new();
        s.push(entries(1));
        let id = s.push(article(10));
        s.active_mut().scroll = 40;
        s.active_mut().cursor = 7;

        let again = s.replace_active(article(11));
        assert_eq!(again, id, "the frame id churned");
        assert_eq!(s.len(), 3, "replacing is not pushing");
        assert_eq!(s.active().level, article(11));
        assert_eq!(s.active().scroll, 0, "the new article starts at the top");
        assert_eq!(s.active().cursor, 0);
    }

    /// `u` on a list of entries: same module, same frame, the filter kept.
    #[test]
    fn replace_active_keeps_a_half_typed_filter() {
        let mut s = Stack::new();
        s.push(entries(1));
        s.active_mut().filter = "borrow".into();
        s.replace_active(Level::Entries {
            source: Selection::Feed(FeedId(1)),
            unread_only: true,
        });
        assert_eq!(s.active().filter, "borrow");
    }

    #[test]
    #[should_panic(expected = "another module")]
    fn replace_active_refuses_to_change_module() {
        let mut s = Stack::new();
        s.push(entries(1));
        s.replace_active(article(10));
    }

    /// `alt+3` with the cursor back up in the entries has to find the
    /// reader, and the reader's frame is past the active one.
    #[test]
    fn top_of_finds_a_module_in_the_forward_trail() {
        let mut s = Stack::new();
        s.push(entries(1));
        let reader = s.push(article(10));
        s.jump_to(1);
        assert!(s.has_forward());

        let at = s.top_of(ModuleId::Reader).expect("the article is open");
        assert_eq!(s.frames()[at].id, reader);
        assert!(s.jump_to(at));
        assert_eq!(s.active().level, article(10));

        // And a module with no frame answers nothing, which is what the app
        // turns into the note "nothing open".
        let mut empty = Stack::new();
        assert_eq!(empty.top_of(ModuleId::Sources), Some(0));
        assert_eq!(empty.top_of(ModuleId::Entries), None);
        assert_eq!(empty.top_of(ModuleId::Reader), None);
        empty.push(entries(1));
        assert_eq!(empty.top_of(ModuleId::Entries), Some(1));
    }

    /// The deepest one, not the first: a search run from inside a list is a
    /// second ENTRIES frame and is the one `alt+2` should land on.
    #[test]
    fn top_of_answers_the_deepest_frame_of_that_module() {
        let mut s = Stack::new();
        s.push(entries(1));
        s.push(Level::Entries {
            source: Selection::Search("lifetimes".into()),
            unread_only: false,
        });
        assert_eq!(s.top_of(ModuleId::Entries), Some(2));
    }

    /// Two levels of the same module stack: a folder inside a folder, a
    /// search run from a list.
    #[test]
    fn a_push_within_one_module_is_allowed() {
        let mut s = Stack::new();
        s.push(folder(3));
        s.push(folder(4));
        assert_eq!(s.len(), 3);
        assert_eq!(s.active().level.module(), ModuleId::Sources);
    }

    #[test]
    #[should_panic(expected = "goes up the column")]
    fn a_push_that_goes_up_the_column_is_refused() {
        let mut s = Stack::new();
        s.push(article(10));
        s.push(entries(1));
    }

    #[test]
    fn crumbs_stop_at_the_active_frame() {
        let mut s = Stack::new();
        s.push(entries(1));
        s.push(article(10));
        s.jump_to(1);
        let crumbs: Vec<&Level> = s.crumbs().iter().map(|f| &f.level).collect();
        assert_eq!(crumbs, vec![&Level::Sources { folder: None }, &entries(1)]);
    }

    #[test]
    fn a_level_knows_which_module_draws_it() {
        assert_eq!(Level::Sources { folder: None }.module(), ModuleId::Sources);
        assert_eq!(entries(1).module(), ModuleId::Entries);
        assert_eq!(article(1).module(), ModuleId::Reader);
    }

    proptest! {
        /// Whatever sequence of navigations arrives, the active frame is in
        /// bounds, the stack is never empty, and the column never runs
        /// backwards -- the frames' modules are non-decreasing from the root
        /// down, which is the invariant `push`'s assertion exists to keep.
        #[test]
        fn navigation_never_empties_the_stack_or_inverts_the_column(
            ops in proptest::collection::vec(0u8..5, 0..40)
        ) {
            let mut s = Stack::new();
            for (i, op) in ops.iter().enumerate() {
                match op {
                    0 => {
                        // Only ever push something at or below the active
                        // frame's module: the app has the same duty, and the
                        // assertion is tested on its own above.
                        let level = match s.active().level.module() {
                            ModuleId::Sources => folder(i as i64),
                            ModuleId::Entries => entries(i as i64),
                            ModuleId::Reader => article(i as i64),
                        };
                        s.push(level);
                    }
                    1 => { s.pop(); }
                    2 => { s.jump_to(i % 4); }
                    3 => { s.forward(); }
                    _ => {
                        let level = s.active().level.clone();
                        s.replace_active(level);
                    }
                }
                prop_assert!(s.active < s.frames.len());
                prop_assert!(!s.is_empty());
                let mut last = 0usize;
                for f in s.frames() {
                    let at = f.level.module().index();
                    prop_assert!(at >= last, "the column runs backwards");
                    last = at;
                }
            }
        }
    }
}
