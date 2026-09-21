//! Where everything is, worked out once a frame.
//!
//! [`LayoutState::regions`] is the only place in the program that decides a
//! rectangle. It is called at the top of `draw`, its answer is kept in
//! [`LayoutState::last`], and the mouse handling reads that rather than
//! recomputing anything. Two pieces of code that have to agree about
//! geometry, one of which is only ever exercised by a pointer, is where a
//! layout bug lives; there is one here.
//!
//! ## One column
//!
//! ```text
//! SOURCES   folders and feeds, with the special rows at the top
//! ENTRIES   the list for whichever source is chosen
//! READER    the article, wrapped and centred
//! status    one row, never anything else
//! ```
//!
//! Every module is always there, in the order the feed list is drilled
//! through, and the arithmetic below is the whole of it: a plain top-down
//! stack, no tree and no `Layout`.
//!
//! ## Who gets the spare rows
//!
//! Everything starts at its floor -- the focused module at
//! [`EXPANDED_MIN_ROWS`], the other two at [`COLLAPSED_ROWS`] -- and what is
//! left over, `room`, goes out under three rules:
//!
//! 1. READER focused takes all of it.
//! 2. A list focused with no article open takes all of it.
//! 3. A list focused with an article open is capped at `[ui] list_rows` --
//!    the "peek" -- and READER takes the rest: the title, the byline and
//!    the first few lines, which is what makes moving down a long entry list
//!    worth doing with the article on screen.
//!
//! The third rule has a consequence worth stating, because it is the reason
//! the cap exists at all: READER's height does not change as focus moves
//! between the two lists. Clicking the folded SOURCES line while ENTRIES has
//! the keyboard expands one list and folds the other, and the panel under
//! the pointer stays exactly where it was.
//!
//! ## Short terminals
//!
//! There is no degradation ladder. Below [`MIN_COLS`] or [`MIN_ROWS`] the
//! caller draws one line saying so, in `theme.error`.

use starkit::chrome::header;
use starkit::ratatui::layout::Rect;

use super::panels::{ModuleId, COLUMN};

/// Below this the layout is not drawn at all.
///
/// Sixty columns is the narrowest an entry's title beside its feed and its
/// age is worth drawing, and the same floor STAR/CORD and STAR/FOLD use.
pub const MIN_COLS: u16 = 60;

/// A folded module: two borders, the header row the action words sit on, and
/// one row of content. A module with no content row is a box with a title.
pub const COLLAPSED_ROWS: u16 = 2 + header::ROWS + 1;

/// The fewest rows an expanded module is worth drawing at: the four a folded
/// one has, a crumb row, and seven rows of list or of article.
pub const EXPANDED_MIN_ROWS: u16 = COLLAPSED_ROWS + 1 + 7;

/// One module expanded at its floor, the other two folded, and the status
/// row. Twenty-one, which is arithmetic rather than judgement and the same
/// floor as the rest of the family's.
pub const MIN_ROWS: u16 = EXPANDED_MIN_ROWS + 2 * COLLAPSED_ROWS + 1;

// The floor is a number a person reads in an error message, so it is stated
// here as well as derived, and the two have to agree.
const _: () = assert!(MIN_ROWS == 21);
const _: () = assert!(COLLAPSED_ROWS == 4);
const _: () = assert!(EXPANDED_MIN_ROWS == 12);

/// One frame's geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct Regions {
    /// The whole of it, padding already taken off.
    pub area: Rect,
    /// One rect per module, indexed by [`ModuleId::index`], in [`COLUMN`]
    /// order. Every one of them is the full width of the area.
    modules: [Rect; 3],
    pub status: Rect,
}

impl Regions {
    pub fn rect_of(&self, m: ModuleId) -> Rect {
        self.modules[m.index()]
    }

    /// Which module a cell is in.
    pub fn hit(&self, x: u16, y: u16) -> Option<ModuleId> {
        COLUMN.into_iter().find(|m| {
            let r = self.rect_of(*m);
            x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
        })
    }
}

/// Which module has the keyboard, how far a focused list opens while an
/// article is behind it, and the last answer.
#[derive(Debug, Clone)]
pub struct LayoutState {
    focus: ModuleId,
    /// `[ui] list_rows`: the whole height a focused list opens to while an
    /// article is open, borders and header included. Twelve by default,
    /// which is [`EXPANDED_MIN_ROWS`] -- the smallest peek is no peek at
    /// all, and the reader gets everything above it.
    pub list_rows: u16,
    /// Whether an article is open, as the last call to [`Self::regions`]
    /// was told. Kept so [`Self::is_open`] can answer without being asked
    /// again.
    reader_open: bool,
    pub last: Option<Regions>,
}

impl LayoutState {
    pub fn new(list_rows: u16) -> Self {
        Self {
            focus: ModuleId::Sources,
            list_rows,
            reader_open: false,
            last: None,
        }
    }

    /// The whole geometry of one frame, or `None` when the terminal is too
    /// small to draw anything honest in.
    ///
    /// `reader_open` is whether an article is open at all -- whether the
    /// stack has a READER frame. It is not "is the reader focused": see the
    /// module doc's three rules.
    pub fn regions(&mut self, full: Rect, pad: (u16, u16), reader_open: bool) -> Option<&Regions> {
        self.reader_open = reader_open;

        let area = inset(full, pad);
        if area.width < MIN_COLS || area.height < MIN_ROWS {
            self.last = None;
            return None;
        }

        // The status line first, off the bottom, because it is never hidden,
        // never focused and never resized.
        let status = Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        };
        let body = Rect {
            height: area.height - 1,
            ..area
        };

        // Everyone at their floor is the baseline; `room` is how much taller
        // than that the body is. The `MIN_ROWS` check above guarantees this
        // does not underflow.
        let room = body.height - EXPANDED_MIN_ROWS - 2 * COLLAPSED_ROWS;

        // Rule 3: a focused list with an article open is capped, and what it
        // does not take the reader does. Rules 1 and 2 are the two ways the
        // focused module takes the lot.
        let peeking = reader_open && self.focus != ModuleId::Reader;
        let focus_extra = if peeking {
            self.list_rows.saturating_sub(EXPANDED_MIN_ROWS).min(room)
        } else {
            room
        };
        let reader_extra = if peeking { room - focus_extra } else { 0 };

        let height = |m: ModuleId| {
            let floor = if m == self.focus {
                EXPANDED_MIN_ROWS
            } else {
                COLLAPSED_ROWS
            };
            let extra = if m == self.focus {
                focus_extra
            } else if m == ModuleId::Reader {
                reader_extra
            } else {
                0
            };
            floor + extra
        };

        let mut modules = [Rect::new(0, 0, 0, 0); 3];
        let mut y = body.y;
        for m in COLUMN {
            let h = height(m);
            modules[m.index()] = Rect {
                y,
                height: h,
                ..body
            };
            y += h;
        }

        self.last = Some(Regions {
            area,
            modules,
            status,
        });
        self.last.as_ref()
    }

    pub fn focus(&self) -> ModuleId {
        self.focus
    }

    pub fn focus_set(&mut self, m: ModuleId) {
        self.focus = m;
    }

    pub fn focus_next(&mut self) {
        let i = (self.focus.index() + 1) % COLUMN.len();
        self.focus_set(COLUMN[i]);
    }

    pub fn focus_prev(&mut self) {
        let i = (self.focus.index() + COLUMN.len() - 1) % COLUMN.len();
        self.focus_set(COLUMN[i]);
    }

    /// Whether `m` is drawing more than its one folded line right now.
    ///
    /// The reader is the one module that can be open without having the
    /// keyboard: that is the peek.
    pub fn is_open(&self, m: ModuleId) -> bool {
        match m {
            ModuleId::Reader => self.focus == ModuleId::Reader || self.reader_open,
            _ => self.focus == m,
        }
    }
}

/// Shrink a rect by the configured padding, never past nothing.
fn inset(area: Rect, pad: (u16, u16)) -> Rect {
    let (x, y) = pad;
    let width = area.width.saturating_sub(x.saturating_mul(2));
    let height = area.height.saturating_sub(y.saturating_mul(2));
    if width == 0 || height == 0 {
        return Rect {
            width: 0,
            height: 0,
            ..area
        };
    }
    Rect {
        x: area.x + x,
        y: area.y + y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn state() -> LayoutState {
        LayoutState::new(12)
    }

    fn min_height(s: &LayoutState, m: ModuleId) -> u16 {
        if s.focus() == m {
            EXPANDED_MIN_ROWS
        } else {
            COLLAPSED_ROWS
        }
    }

    /// The property the whole module rests on: the modules cover the body
    /// exactly, top to bottom, each of them the full width. A gap is a cell
    /// nothing redraws, which keeps whatever was there last; an overlap is
    /// two modules writing the same cell in an order nobody chose.
    #[test]
    fn the_modules_tile_the_body_from_top_to_bottom() {
        for width in [59u16, 60, 61, 100, 200] {
            for height in [20u16, 21, 22, 30, 60] {
                for focus in COLUMN {
                    for reader_open in [false, true] {
                        for list_rows in [0u16, 4, 12, 40] {
                            let mut s = LayoutState::new(list_rows);
                            s.focus = focus;
                            let full = Rect::new(0, 0, width, height);
                            let Some(r) = s.regions(full, (0, 0), reader_open).cloned() else {
                                assert!(
                                    width < MIN_COLS || height < MIN_ROWS,
                                    "{width}x{height} refused to lay out"
                                );
                                continue;
                            };

                            let mut y = r.area.y;
                            for m in COLUMN {
                                let rect = r.rect_of(m);
                                assert_eq!(rect.x, r.area.x, "{m:?} at {width}x{height}");
                                assert_eq!(rect.width, r.area.width, "{m:?} at {width}x{height}");
                                assert_eq!(rect.y, y, "{m:?} at {width}x{height} is not stacked");
                                assert!(
                                    rect.height >= min_height(&s, m),
                                    "{m:?} is {rect:?}, below its floor"
                                );
                                y += rect.height;
                            }
                            assert_eq!(y, r.status.y, "the column does not reach the status line");
                            assert_eq!(r.status.height, 1);
                            assert_eq!(r.status.y, r.area.y + r.area.height - 1);
                        }
                    }
                }
            }
        }
    }

    /// Below the floor there is no layout at all, and the caller draws one
    /// line saying so.
    #[test]
    fn a_terminal_below_the_floor_gets_nothing() {
        for (w, h) in [(59u16, 30u16), (60, 20), (40, 8), (0, 0)] {
            let mut s = state();
            let r = s.regions(Rect::new(0, 0, w, h), (0, 0), true);
            assert!(r.is_none(), "{w}x{h} laid out");
            assert!(s.last.is_none());
            // And the next frame at a workable size recovers.
            assert!(s.regions(Rect::new(0, 0, 100, 30), (0, 0), true).is_some());
        }
    }

    /// Padding comes off the outside and the floor is measured after it, so
    /// a padded sixty-four-column terminal is a sixty-column layout.
    #[test]
    fn padding_is_taken_before_the_floor_is_measured() {
        let mut s = state();
        let r = s
            .regions(Rect::new(0, 0, MIN_COLS + 4, MIN_ROWS + 2), (2, 1), false)
            .cloned()
            .expect("60x21 left");
        assert_eq!(r.area, Rect::new(2, 1, MIN_COLS, MIN_ROWS));

        let mut s = state();
        assert!(
            s.regions(Rect::new(0, 0, MIN_COLS + 3, MIN_ROWS + 2), (2, 1), false)
                .is_none(),
            "59 columns after padding is below the floor"
        );
    }

    /// At the floor the focused module is exactly its minimum and the other
    /// two are folded, whatever is open and however big `list_rows` is.
    #[test]
    fn at_the_floor_every_module_is_its_minimum() {
        assert_eq!(MIN_ROWS, 21);
        assert_eq!(COLLAPSED_ROWS, 4);
        assert_eq!(EXPANDED_MIN_ROWS, 12);
        for focus in COLUMN {
            for reader_open in [false, true] {
                let mut s = LayoutState::new(40);
                s.focus = focus;
                let r = s
                    .regions(Rect::new(0, 0, MIN_COLS, MIN_ROWS), (0, 0), reader_open)
                    .cloned()
                    .unwrap();
                for m in COLUMN {
                    let want = if m == focus {
                        EXPANDED_MIN_ROWS
                    } else {
                        COLLAPSED_ROWS
                    };
                    assert_eq!(r.rect_of(m).height, want, "{m:?} focused {focus:?}");
                }
            }
        }
    }

    /// Rule 1: the reader focused takes the room, article open or not.
    #[test]
    fn the_reader_focused_takes_the_room() {
        for reader_open in [false, true] {
            let mut s = state();
            s.focus = ModuleId::Reader;
            let r = s
                .regions(Rect::new(0, 0, 100, 40), (0, 0), reader_open)
                .cloned()
                .unwrap();
            assert_eq!(r.rect_of(ModuleId::Sources).height, COLLAPSED_ROWS);
            assert_eq!(r.rect_of(ModuleId::Entries).height, COLLAPSED_ROWS);
            assert_eq!(
                r.rect_of(ModuleId::Reader).height,
                39 - 2 * COLLAPSED_ROWS,
                "the reader did not take the spare rows"
            );
        }
    }

    /// Rule 2: a list focused with nothing open takes the room, and the
    /// reader is folded to its line under it.
    #[test]
    fn a_focused_list_with_nothing_open_takes_the_room() {
        for focus in [ModuleId::Sources, ModuleId::Entries] {
            let mut s = state();
            s.focus = focus;
            let r = s
                .regions(Rect::new(0, 0, 100, 40), (0, 0), false)
                .cloned()
                .unwrap();
            assert_eq!(r.rect_of(focus).height, 39 - 2 * COLLAPSED_ROWS);
            assert_eq!(r.rect_of(ModuleId::Reader).height, COLLAPSED_ROWS);
            assert!(!s.is_open(ModuleId::Reader));
        }
    }

    /// Rule 3: a list focused with an article open is capped at `list_rows`
    /// and the reader takes the rest -- and the reader's height is the same
    /// whichever of the two lists has the keyboard, so a click on a fold
    /// does not move the panel under the pointer.
    #[test]
    fn a_focused_list_with_an_article_open_is_capped_and_the_reader_keeps_its_height() {
        let full = Rect::new(0, 0, 100, 40);

        let mut s = LayoutState::new(16);
        s.focus = ModuleId::Entries;
        let r = s.regions(full, (0, 0), true).cloned().unwrap();
        assert_eq!(r.rect_of(ModuleId::Entries).height, 16, "the peek's cap");
        assert_eq!(r.rect_of(ModuleId::Sources).height, COLLAPSED_ROWS);
        let reader = r.rect_of(ModuleId::Reader).height;
        assert_eq!(reader, 39 - 16 - COLLAPSED_ROWS);
        assert!(s.is_open(ModuleId::Reader), "the peek is the reader open");

        s.focus_set(ModuleId::Sources);
        let r = s.regions(full, (0, 0), true).cloned().unwrap();
        assert_eq!(r.rect_of(ModuleId::Sources).height, 16);
        assert_eq!(r.rect_of(ModuleId::Entries).height, COLLAPSED_ROWS);
        assert_eq!(
            r.rect_of(ModuleId::Reader).height,
            reader,
            "the reader moved when focus went from one list to the other"
        );
    }

    /// A `list_rows` below the expanded floor buys nothing -- the focused
    /// module is never shorter than [`EXPANDED_MIN_ROWS`] -- and one larger
    /// than the terminal is clamped by the room there is.
    #[test]
    fn the_peek_is_clamped_at_both_ends() {
        let full = Rect::new(0, 0, 100, 40);

        let mut s = LayoutState::new(2);
        s.focus = ModuleId::Entries;
        let r = s.regions(full, (0, 0), true).cloned().unwrap();
        assert_eq!(r.rect_of(ModuleId::Entries).height, EXPANDED_MIN_ROWS);

        let mut s = LayoutState::new(200);
        s.focus = ModuleId::Entries;
        let r = s.regions(full, (0, 0), true).cloned().unwrap();
        assert_eq!(r.rect_of(ModuleId::Entries).height, 39 - 2 * COLLAPSED_ROWS);
        assert_eq!(r.rect_of(ModuleId::Reader).height, COLLAPSED_ROWS);
    }

    #[test]
    fn focus_next_and_prev_cycle_the_column() {
        let mut s = state();
        assert_eq!(s.focus(), ModuleId::Sources);
        s.focus_next();
        assert_eq!(s.focus(), ModuleId::Entries);
        s.focus_next();
        assert_eq!(s.focus(), ModuleId::Reader);
        s.focus_next();
        assert_eq!(s.focus(), ModuleId::Sources);
        s.focus_prev();
        assert_eq!(s.focus(), ModuleId::Reader);
    }

    #[test]
    fn hit_testing_answers_the_module_that_was_drawn() {
        let mut s = state();
        let r = s
            .regions(Rect::new(0, 0, 100, 30), (0, 0), true)
            .cloned()
            .unwrap();
        for m in COLUMN {
            let rect = r.rect_of(m);
            assert_eq!(r.hit(rect.x, rect.y), Some(m));
            assert_eq!(
                r.hit(rect.x + rect.width - 1, rect.y + rect.height - 1),
                Some(m)
            );
        }
        assert_eq!(
            r.hit(r.status.x, r.status.y),
            None,
            "the status is not a module"
        );
    }

    #[derive(Debug, Clone, Copy)]
    enum Op {
        FocusSet(usize),
        FocusNext,
        FocusPrev,
        Open(bool),
    }

    proptest! {
        /// However focus and the open article are moved about, the column
        /// tiles the body, focus is always one of the three modules, and the
        /// focused module always has at least its expanded floor.
        #[test]
        fn focus_and_open_sequences_hold_the_invariants(
            list_rows in 0u16..40,
            height in MIN_ROWS..80,
            ops in proptest::collection::vec(
                prop_oneof![
                    (0usize..3).prop_map(Op::FocusSet),
                    Just(Op::FocusNext),
                    Just(Op::FocusPrev),
                    any::<bool>().prop_map(Op::Open),
                ],
                0..40,
            ),
        ) {
            let mut s = LayoutState::new(list_rows);
            let full = Rect::new(0, 0, 100, height);
            let mut open = false;
            for op in ops {
                match op {
                    Op::FocusSet(i) => s.focus_set(COLUMN[i]),
                    Op::FocusNext => s.focus_next(),
                    Op::FocusPrev => s.focus_prev(),
                    Op::Open(v) => open = v,
                }
                let r = s.regions(full, (0, 0), open).cloned().expect("above the floor");
                prop_assert!(COLUMN.contains(&s.focus()));
                prop_assert!(r.rect_of(s.focus()).height >= EXPANDED_MIN_ROWS);
                let mut y = r.area.y;
                for m in COLUMN {
                    prop_assert_eq!(r.rect_of(m).y, y);
                    y += r.rect_of(m).height;
                }
                prop_assert_eq!(y, r.status.y);
            }
        }
    }
}
