# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **A refresh you asked for tries the failed articles again too.** `r` on a
  source, `r` in the list of entries — which is where it is new — and `R` for
  everything now offer every article in their scope that has no text of its
  own back to the extractor at once, ignoring both the delay a transient
  failure earned and the three-attempt ceiling: those are answers to "nobody
  asked", and a key somebody pressed is not a timer. The reasons a page failed
  yesterday are exactly the ones that change, and until now the only way to
  try one again was `e`, one article at a time, with the article open. A free
  sample behind a paywall is content rather than a failure and is left where
  it is; a video is untouched; and a refresh nobody asked for — the one on
  the clock, and the one at startup — still leaves the ceiling where it is,
  or a page that cannot be read would cost a request every launch. The
  status line says what is being refreshed and how many articles are being
  tried: `refreshing Phoronix · 3 articles to try again`.

### Fixed

- **A host that asks for a minute no longer costs a thread.** Reddit wants
  sixty-one seconds between requests, and a net thread waiting one out was a
  quarter of the pool asleep. A feed list with five subreddits in it put all
  four of them to sleep within seconds of the window opening, so a refresh of
  forty-one feeds sat at `14 of 16` for minutes and the pictures of the
  article being read — queued behind the refresh, on the same lane — never
  arrived at all. A wait longer than three seconds is now handed back instead
  of slept through: the job goes into a list the clock hands out again when
  the host is askable, and nothing is asked any sooner than politeness said
  it could be. Ordinary two-second gaps are still waited in place, so a feed
  list with no rule in it behaves exactly as it did, and `starwire fetch`
  from a timer still waits, because a subcommand has no clock to come back
  to. The pictures inside an open article are urgent work besides — somebody
  is looking at the rows they are going to fill — and a picture cancelled by
  moving to another article now gives its slot back rather than keeping it,
  which on its own was enough to stop a reader's pictures arriving for the
  rest of the session.

- **An article whose pictures are on their way is still an article.** A
  picture nothing yet knows the size of reserved the whole box it was allowed
  — the full width and a third of the reader — so on a 130-row terminal an
  article with two pictures near the top of it opened as seventy-six rows of
  `░` with its first sentence below the fold. A placeholder is not a picture:
  it now takes eight rows at most, or the cap where that is smaller, which is
  enough to show that something is coming and little enough that the prose is
  still the page. The real box takes over when the size lands, which moves
  the text under it once.

- **A `403` is asked once more, as a browser.** Not every one of them is a
  wall: IFLScience sits behind a CloudFront rule that filters on the user
  agent and nothing else, so STAR/WIRE's own agent got a 403 and a 919-byte
  error page where a browser gets the whole article — and twenty-two entries
  in a real database were written off as paywalls on the strength of it. The
  honest agent still goes first, every time; only a 403 buys a second request,
  with a browser's user agent and its `Accept` and `Accept-Language` beside it;
  a `401` or a `402` means what it says and is never asked twice; and a 403
  that refuses both is the wall it looks like. It happens inside the one
  attempt, so a page behind a firewall still has all three of its own, and the
  migration to schema 3 offers the 403s already in the file the same one go.

## [0.0.2] - 2026-09-21

### Added

- **Pictures inside articles.** The pictures an article carries are fetched,
  decoded and drawn in it: the real thing in a terminal with a graphics
  protocol, half blocks in one without, and `░` where one is going while it
  is on its way, so the text does not move when it lands. A picture is drawn
  at its own size when that fits — one image pixel per terminal pixel — and
  fitted to the text column when it does not; nothing is ever made larger
  than it is. None of them may take more than `[reading] image_rows`, which
  is a third of the reader by default, because a picture that leaves two
  lines of prose on the screen has taken the article over.
- **A click opens a picture.** On a desktop it goes to whatever shows
  pictures there, and what it is handed is the cached *file*, so an image
  viewer opens rather than a browser; `[player] image` names another program.
  On a session with no display of its own — over ssh, or a bare tty, where a
  viewer would open on the wrong machine — it opens an overlay instead, which
  grows the picture to the whole window and says by how much. `[reading]
  click_picture` decides outright. In the overlay `n` and `p` walk the
  article's pictures, `o` opens it outside, `y` copies its address, `esc`
  closes.
- **A picture cache under `cache/pictures`.** Named by a hash of the URL —
  a filename built out of a string somebody else published is a filename that
  can hold a `..` — holding the bytes as they arrived rather than as they were
  drawn, and swept once a day with the entries: by age first, then oldest
  first down to `[articles] pictures_mib`. Everything in it is re-fetchable,
  which is why there is a ceiling rather than a retention policy.
- **WebP.** Which is what a news site serves now. PNG, JPEG and the first
  frame of a GIF as well; anything else, AVIF included, is its alt line, as is
  a `data:` or an `http:` address.
- **A failure degrades to the feed's own text.** Every entry carries the text
  its feed gave from the moment it arrives, and a failed extraction never
  overwrites it — so an entry that used to be blank with an explanation in the
  middle of it is now the feed's summary with one line above it saying why
  there is no more: `extraction failed: <why> · e retries · o opens the
  page`. The 0.0.1 failures already in the file are backfilled by the
  migration to schema 2.
- **A free sample is the feed's text, not an article.** A page that answers
  with two paragraphs and an invitation to subscribe is stored as the feed's
  own words with `paywall` as the reason, rather than as a successful
  extraction of 494 bytes.
- **Transient failures come back round.** A `429`, a `5xx` and a timeout are
  facts about that minute, so the entry rejoins the queue after
  `[fetch] refresh_minutes`, doubling per attempt. A `401`, a `402`, a `403`,
  a `404` and "does not read like an article" are facts about the page and
  stay where they are — a browser's user agent gets the same codes, which has
  been measured. Upgrading offers the failures already in the file another
  go, where they earned one.
- **Redirects are followed here, with a lease and a name for every hop.**
  `ureq`'s own loop took no lease, so a wrapper URL reached the second host
  at whatever rate the first answered — seventeen `429`s from `archive.is` in
  one afternoon, every one of them reached through a `feedpress.me` wrapper
  and every one of them blamed on the wrapper. A failure now names the host
  that actually answered, and a chain that lands on a login or consent page
  stops there rather than extracting it.
- **Per-host gaps.** `reddit.com` every 61 seconds with its own
  `x-ratelimit-reset` believed on top of that, `archive.is` every 10, and
  `[fetch] host_intervals` for a site that has asked you to slow down.
- **A page's own JSON-LD, and short articles.** The readability gate refused
  five Arch news items and three Substack posts that were real, short
  articles; it does not now, and where it still refuses, the `articleBody`
  a page carries in its JSON-LD is tried before giving up.
- **Site rules, as a table rather than a heuristic.** GamingOnLinux without
  its footer, KitGuru without its share bar or its "Check Also" list, Indie
  Retro News without `[Become a Patron!]`, Hearst without "Article continues
  below this ad", and Phoronix reviews joined up across their pages — at most
  eight, same host, same path, one lease each.
- **Cleaner markdown.** Links with nothing visible in them are gone (they
  were in 121 of 333 articles), hard line breaks survive the collapse (none
  did), a lazy-loading page's real picture address is found in `srcset`, in a
  `<picture>`, in `data-src` or in the `<noscript>`, and a plain-text feed
  description keeps the lines it was written with.
- **`[articles] timeout_secs`**, thirty seconds, separate from a feed's
  fifteen: a feed is a file the server already has, and an article is often
  rendered when it is asked for.

### Changed

- **Three more settings rows**, and a mouse gesture: `pictures`,
  `picture rows` and `click a picture` in the `,` box, and `click a picture ·
  open it` in the reader's half of the mouse table.
- **The filter says how to leave.** While the `/` field is open the status
  row stops offering the list's keys — every letter is typing, so none of
  them would work — and says `enter keep`, `esc clear` and that `alt+…`
  still works instead. The `/text` on the crumb row is drawn in the accent
  while the field is up, so it reads as a mode rather than as a label, and a
  filter kept with `enter` leaves `esc clear filter` at the head of that
  module's hints for as long as it is narrowing the list. `esc` now clears a
  kept filter as well as an open one, which is what that hint promises.

### Fixed

- **Reddit feeds work again.** `old.reddit.com/r/<sub>/.rss` now answers a 302
  to a login page, which is the "not a feed (HTML page)" every Reddit feed in
  a real list had recorded against it. The host is rewritten to
  `www.reddit.com` on import, on `add`, and by the migration for the feeds
  already in the file — and with the 61-second gap above, five subreddits
  refresh at about one a minute instead of being rate limited.
- **`/` filters the list you are looking at.** It always filtered the
  entries, whichever module had the keyboard, against what
  `docs/keys-and-mouse.md` said it did. In SOURCES it now narrows the feed
  list where it stands, matched on the folder and feed names by the matcher
  the entries use: the keyboard stays on the list, `enter` opens what is left
  the way it always does, and the crumb row says `/text` where the summary
  was. The two filters are independent of each other. In READER, where there
  is no list to narrow, `/` still moves to ENTRIES and filters those — now
  said out loud in the help and in `docs/the-stack.md` rather than left to be
  discovered.

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

[Unreleased]: https://github.com/bstar/starwire/compare/v0.0.2...HEAD
[0.0.2]: https://github.com/bstar/starwire/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/bstar/starwire/releases/tag/v0.0.1
