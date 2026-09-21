# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.1] - 2026-09-21

### Added

- **The extractor.** The page behind an entry is fetched and reduced to
  CommonMark: Mozilla's readability algorithm finds the article, a converter
  turns it into markdown, and a normalising pass collapses the blank lines,
  resolves every relative link against the page it came from, and cuts at a
  paragraph boundary. A fenced code block keeps the language the page
  declared. Nothing is rendered here — the core stores text, and the reader
  parses it at the width it is drawing.
- **What is not scraped.** A video, a Reddit post, and a Hacker News item
  whose link goes back into the comment thread all use the text the feed
  itself carried. There is no `robots.txt` fetch; the politeness is
  structural instead — one request at a time per host with a gap between
  them, a fifteen-second timeout, size caps, an `Accept` header that says
  HTML, a user agent that names the program and links the repository, and at
  most three attempts at any one entry ever.
- **The window.** `starwire` with no arguments takes the terminal and draws
  one column of three docked modules — the sources, the entries of whichever
  source is chosen, and the article — over a status row. The focused module
  is expanded and the other two fold to a line that says what is open in
  them. Thirty frames a second, synchronous, with the core on its own
  threads behind a handle: one read lock a frame, dropped before anything is
  drawn.
- **Reading.** An article is drawn at `[reading] width`, centred, with its
  headline drawn once however many times the page said it. `<` and `>` step
  the width and write the line back to `config.toml`. `n` and `p` replace
  what the reader is looking at rather than pushing a level, so the place
  you had reached in each of the last few articles is still there when you
  come back to one — and the session file keeps those places between runs,
  along with the source and the entry that were open.
- **The reader's pipeline.** An article's markdown is parsed into blocks and
  styled runs, laid out at whatever width the panel is, and the result kept
  for the last eight -- keyed by the entry, when its text was last written,
  the width and the theme, so an extraction landing behind an open article
  redraws it with nothing having to notice. Links are numbered in document
  order, so the number beside one does not change under a resize. Code
  blocks are never reflowed; a table too wide for the panel becomes one
  `header: cell` line per cell rather than four unreadable columns.
- **The window's foundations.** The column is a stack of levels -- the
  sources, the entries of whichever source is chosen, the article -- and a
  level carries the core's own `Selection` rather than a second spelling of
  it. Backing out discards the level; jumping away keeps it, cursor, filter
  and scroll position and all, to jump back into. The reader is a module
  rather than a level per article, which is what lets `n` step to the next
  one without losing where you were in the last.
- **The layout arithmetic.** Sixty by twenty-one, which is the sum of the
  constants rather than a judgement. Three rules share the spare rows: the
  reader focused takes them, a list focused with nothing open takes them, and
  a list focused with an article open is capped at `[ui] list_rows` so the
  article behind it stays visible. The cap exists for its consequence --
  the reader's height does not change as focus moves between the two lists,
  so clicking a fold does not move the panel under the pointer.
- **Six overlays.** The key list, the import, a feed to add, the settings,
  a confirmation before anything destructive, and a search. One is ever open,
  it takes every key while it is, `esc` closes it and `ctrl+c` still quits.
  Every settings row changes the running program and rewrites exactly one
  line of `config.toml`, leaving the comments the template was written to
  carry.
- **The mouse.** Click to move the cursor, double-click to open, right-click
  an entry to star it, click a crumb to jump there, click a link in an
  article to open it, click a header word to do what it says, drag a
  scrollbar, and click a folded module to open it.
- **One key table.** Every action, its keys and its help text in one place,
  which drives both the dispatch and `docs/keys-and-mouse.md`; a test fails
  if the document and the table ever disagree. Two chords: `gg` for the top
  of a list, and `o<n>` for a numbered link in an article, which closes as
  soon as the number it holds cannot grow.
- **The `[wire]` theme roles.** Ten colours a reader needs and a file manager
  does not -- a link, a heading, an inline-code background, a quote, an
  unread marker, a star, a video arrow, a byline, a rule and an image
  placeholder -- derived from whatever palette the theme already had, with a
  stated colour winning outright. All sixteen built-ins are asserted legible
  against WCAG AA.
- **The first run.** On an empty feed list with a newsboat installation in
  the usual place, an overlay offers to import it — how many feeds, where
  from, and how many of them turn out to be YouTube channels once every
  spelling of a channel has been reduced to its feed. Saying yes shows what
  came of it, including every line that was skipped and why; saying no
  records the refusal in the database, so the offer never returns. `alt+i`
  opens it again, and on a machine with no newsboat it asks for an OPML file
  instead.
- **Importing.** `starwire import newsboat` reads the urls file and, from the
  cache, the feed titles and the read marks — read-only, immutable, and never
  the article bodies. Read marks for entries that have not arrived yet are
  held until they do. `import opml` and `export opml` round trip exactly.
- **YouTube.** A channel URL, a video URL, an `@handle`, a bare `UC…`, a
  `videos.xml` feed and a `scriptbarrel` proxy for one all reduce to the same
  channel id and the same feed row. Google Takeout's `subscriptions.csv` and
  `yt-dlp`'s view of the account's subscriptions both import.
- **Fetching.** Conditional requests, so a refresh of forty feeds costs forty
  round trips and almost no bytes; a backoff that doubles per consecutive
  failure to a ceiling of a day, and honours a server's own `Retry-After`;
  and a per-host lease that keeps requests to one host serial.
- **The database.** One SQLite file holding feeds, folders, entries,
  extracted articles and a full-text index over them, opened WAL so that a
  `starwire fetch` from a timer and an open reader can share it. Retention
  has two bounds — an age and a per-feed cap — and a starred entry survives
  both.
- **The command line.** `fetch`, `list`, `show`, `add`, `remove`, `import`,
  `export`, `youtube` and `extract`, all headless, and `--replay DIR` to serve
  a directory of saved responses instead of the network.
- **The running core.** One `starwire-db` thread owns the connection and the
  single-writer rule with it, and a pool of `starwire-net` threads fetches
  feeds and pulls pages on two lanes — the feed on screen and the article
  somebody is waiting for go ahead of a refresh of the whole list. One pure
  `apply` is the only thing that writes to the state, so a worker takes the
  lock just long enough to fold a result in. A cancel bumps a generation and
  what is queued is dropped before a connection is opened; a job that does
  not fit its queue is dropped rather than waited on and picked up by the
  next tick. Another process writing to the file — `starwire fetch` from a
  timer — is noticed through `PRAGMA data_version` once a second.
- **The handle the window programs against.** Send a command, drain
  events once a frame, read the truth behind a lock. Events carry no state,
  only the news that something changed, so one may be coalesced or dropped
  and the next frame still draws what is true; a drop is counted and the next
  event that fits is preceded by a refresh. A read mark moves the row and the
  count before the write lands. Dropping the handle joins every thread.
- **A terminal-free core.** Nothing under `src/wire/` may name `ratatui`,
  `crossterm` or the UI module, and a test greps its own sources to make sure
  of it. It is why every command above runs with no TTY attached.
- **Synthetic fixtures.** One file per feed format, pages for the extractor,
  a newsboat urls file, an OPML export and a Takeout CSV — none of it from a
  real site, and a replay directory that runs the whole pipeline with no
  socket. Everything that parses foreign input has a property test as well as
  table tests.
- **Frame snapshots.** Every scenario the window can draw — both themes, a
  hundred by thirty and the sixty-by-twenty-one floor, the three articles
  that are not text, the overlays, and both ways below the floor — is kept
  as a snapshot, so a layout change is a diff of a drawn screen rather than
  an argument about rectangles.

[Unreleased]: https://github.com/bstar/starwire/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/bstar/starwire/releases/tag/v0.0.1
