# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **The extractor.** The page behind an entry is fetched and reduced to
  CommonMark: Mozilla's readability algorithm finds the article, a converter
  turns it into markdown, and a normalising pass collapses the blank lines,
  resolves every relative link against the page it came from, and cuts at a
  paragraph boundary. A fenced code block keeps the language the page
  declared. Nothing is rendered here — the core stores text, and the reader
  will parse it at the width it is drawing.
- **What is not scraped.** A video, a Reddit post, and a Hacker News item
  whose link goes back into the comment thread all use the text the feed
  itself carried. There is no `robots.txt` fetch; the politeness is
  structural instead — one request at a time per host with a gap between
  them, a fifteen-second timeout, size caps, an `Accept` header that says
  HTML, a user agent that names the program and links the repository, and at
  most three attempts at any one entry ever.
- **The database.** One SQLite file holding feeds, folders, entries,
  extracted articles and a full-text index over them, opened WAL so that a
  `starwire fetch` from a timer and an open reader can share it. Retention
  has two bounds — an age and a per-feed cap — and a starred entry survives
  both.
- **Fetching.** Conditional requests, so a refresh of forty feeds costs forty
  round trips and almost no bytes; a backoff that doubles per consecutive
  failure to a ceiling of a day, and honours a server's own `Retry-After`;
  and a per-host lease that keeps requests to one host serial.
- **Importing.** `starwire import newsboat` reads the urls file and, from the
  cache, the feed titles and the read marks — read-only, immutable, and never
  the article bodies. Read marks for entries that have not arrived yet are
  held until they do. `import opml` and `export opml` round trip exactly.
- **YouTube.** A channel URL, a video URL, an `@handle`, a bare `UC…`, a
  `videos.xml` feed and a `scriptbarrel` proxy for one all reduce to the same
  channel id and the same feed row. Google Takeout's `subscriptions.csv` and
  `yt-dlp`'s view of the account's subscriptions both import.
- **The command line.** `fetch`, `list`, `show`, `add`, `remove`, `import`,
  `export`, `youtube` and `extract`, all headless, and `--replay DIR` to serve
  a directory of saved responses instead of the network.
- **A terminal-free core.** Nothing under `src/wire/` may name `ratatui`,
  `crossterm` or the UI module, and a test greps its own sources to make sure
  of it. It is why every command above runs with no TTY attached.
- **Synthetic fixtures.** One file per feed format, pages for the extractor,
  a newsboat urls file, an OPML export and a Takeout CSV — none of it from a
  real site, and a replay directory that runs the whole pipeline with no
  socket. Everything that parses foreign input has a property test as well as
  table tests.
