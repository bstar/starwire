# The stack

STAR/WIRE's window is one column of three docked modules — SOURCES, ENTRIES,
READER — in the order a feed list is drilled through. The one with the
keyboard is expanded; the others fold to a single line that says what is open
in them. Drilling in pushes a level, backing out pops it, and the column only
ever goes down.

It is the same shape STAR/FOLD uses for directories, applied to feeds and
articles.

```
╔═ S T A R / W I R E ══════════════════════════════════ sources ═╗
║                                                   add  refresh ║
║▸ sources › Tech › Hacker News            41 feeds · 312 unread ║
╚════════════════════════════════════════════════════════════════╝
╔═ ENTRIES — Hacker News ═════════════════════════════ 12 unread ═╗
║ …                                                              ║
╚════════════════════════════════════════════════════════════════╝
╔═ READER — Why the borrow checker says no ═══════════════ ready ═╗
║ …                                                              ║
╚════════════════════════════════════════════════════════════════╝
? help  space page  n next  m read           Hacker News · 1/40 · 3%
```

## The three modules

**SOURCES** is the feed list. At its root it holds five special rows —
everything, unread, starred, videos, and a search — then the folders, then any
feed that is not in one. A folder is a level of its own: `l` on it opens its
feeds, and `enter` on it reads the whole folder as one list. Those are two
different things and they are deliberately two different keys.

**ENTRIES** is the list for whichever source is chosen: `●` unread, `★`
starred, `▶` a video, `!` a page that could not be read, then the headline,
the feed it came from and how old it is. The feed column only appears at
eighty columns or more, and only on a list drawn from more than one feed — on
one feed's own list every row would say the same thing.

**READER** is the article: the headline, the byline, and the markdown wrapped
at `[reading] width` and centred. `<` and `>` step that width and write it
back to `config.toml`.

## What pushes and what pops

`enter`, `l` and a double-click **push** a level. The level behind it folds to
a crumb, and anything that was ahead of it — a trail left by jumping rather
than backing out — is dropped, the way visiting a new page drops a browser's
forward history.

`h`, `left` and `backspace` **pop**, and the frame popped out of is *gone*:
going back into that level later starts fresh. `esc` in the reader is the same
thing said with the key a reader's hand is already on.

`alt+up` and `alt+down` are the opposite: they only move which frame is
active, keeping every frame on both sides of it. A level jumped away from is
still there — cursor, filter, scroll position and all — to jump straight back
into. That is what makes the peek work.

Focus and the stack agree by construction. `tab`, `alt+1`, `alt+2` and
`alt+3` are all the same operation: jump the stack to the deepest frame that
module draws. A module holding no frame says so rather than quietly doing
nothing.

## The peek

While an article is open and the keyboard is on one of the two lists, the list
is capped at `[ui] list_rows` rows and READER takes the rest: the headline,
the byline and the first few lines of the article. That is what makes moving
down a long entry list worth doing with the article on screen.

The cap has a consequence worth stating, because it is the reason the cap
exists: READER's height does not change as focus moves between the two lists.
Clicking the folded SOURCES line while ENTRIES has the keyboard expands one
list and folds the other, and the panel under the pointer stays exactly where
it was.

`list_rows = 12` — the default — is the smallest a focused list can be, so out
of the box there is no peek and the reader gets everything above it. Raise it
and the peek appears.

## The reader is a module, not a level per article

`n` and `p` **replace** what the reader is looking at rather than popping and
pushing, so the frame's identity does not churn. That is what keeps the
per-entry reading position: read half of one article, step to the next, come
back, and you are where you were. The positions are kept in the session file
too, so that survives quitting.

It is also why the reader has its own key group. `n` in ENTRIES is "the next
unread"; `n` in READER is "the next entry". `esc` in READER closes the
article, which is the one place in the program a module deliberately shadows a
global key — `src/ui/keymap.rs`'s `SHADOWS` has that single entry, with why.

## Search is a level like any other

`alt+f` asks for a query and pushes `Selection::Search(q)` as an ENTRIES
level. There is no separate results panel and no separate mode: the rows are
entries, the entry keys work on them, `h` backs out of the search, and
`alt+up` leaves it there to come back to.

`/` is a different thing and is not an overlay at all: it narrows the rows
already on screen, on every keystroke, without a query. "Where was that thing
about lifetimes" and "which of these forty is the one about the borrow
checker" are different questions.

## Below the floor

There is no degradation ladder. Below 60×21 the window draws one line saying
so, in the theme's error colour, and comes back the moment the terminal is big
enough again.

## The first run

On a first start with an empty feed list and a newsboat installation in the
usual place, an overlay offers to import it: how many feeds, where from, and
how many of them are YouTube channels. `y` imports and shows what came of it,
including every line that was skipped and why; `n` declines, and the offer
never returns — the refusal is recorded in the database, not in a file
somebody might not think to keep.

`alt+i` opens it again later, and on a machine with no newsboat it asks for
the path to an OPML file instead.
