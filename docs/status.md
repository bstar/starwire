# Status

What is built, what is in progress, and what is not started. Kept honest:
"built" here means built and tested, and where something has not been run
against the real thing this says so.

`AGENTS.md` has the longer list of what has not been verified yet.

## Built

| Area | State |
| --- | --- |
| **Fetching** | Conditional requests, per-host serialisation with a gap, size caps, a timeout, and a per-feed backoff that doubles to a ceiling of a day and honours a server's `Retry-After`. |
| **Parsing** | RSS 2.0, Atom and JSON Feed through one parser, plus the MediaRSS a YouTube entry carries its description and thumbnail in. Arbitrary bytes are a recorded failure, never a crash. |
| **Extraction** | Readability, with a gate in front of it that refuses homepages and paywall stubs; HTML to CommonMark keeping headings, lists, quotes, tables, links and fenced blocks with their language; and a normalising pass that resolves relative links and caps the size at a paragraph boundary. |
| **The database** | Feeds, folders, entries, articles and a full-text index over them, in one WAL file that a timer and a reader can share. Two retention bounds, and starred entries survive both. |
| **Importing** | newsboat's urls file and cache (titles and read marks, read-only, never the bodies), OPML in and out, Google Takeout, `yt-dlp` subscriptions. |
| **YouTube** | Six spellings of a channel reduce to one feed row. Handles and video pages resolve by reading the page, falling back to `yt-dlp`. |
| **The command line** | `fetch`, `list`, `show`, `add`, `remove`, `import`, `export`, `youtube`, `extract`, and `--replay` to run the whole thing with no network. |
| **The running core** | One thread owns the database; a pool fetches and extracts on an urgent lane and a background one. One pure `apply` is the only writer, a cancel drops what is queued before it opens a connection, and another process's writes are noticed through `PRAGMA data_version`. `Handle` is the contract the window programs against: send a command, drain events, read the state. |
| **The window** | The column of three docked modules, the stack they are drilled through, the reader's markdown pipeline, the six overlays, the key table and the `[wire]` theme roles. `starwire` with no arguments takes the terminal; [The stack](the-stack.md) describes how it moves and [Keys and the mouse](keys-and-mouse.md) is generated from the table itself. Whole frames are kept as snapshots, so a layout change is a diff of a drawn screen. |

## Not started

Nothing in 0.0.1. What is deliberately outside it is below.

## Deliberately outside 0.0.1

- Pictures inside articles. The markdown keeps them and the reader draws an
  `[image: alt]` line -- or just `[image]` when the page gave no alt text;
  nothing is downloaded.
- Find-in-article.
- Per-feed refresh intervals.
- Folders inferred from a newsboat file's `# comment` headers. The importer
  reads them and does nothing with them; folders are assigned in the app.
- Podcasts and enclosures.
- An optional summary from a local model, for an article too long to face at
  the end of an evening. A key in the reader, a model on the machine, and
  nothing sent anywhere.
- A page for STAR/WIRE on the STAR/FLEET site, alongside the rest of the
  family.
- Read-later as a separate flag. Starred *is* read-later: two lists nobody
  empties are worse than one.

## Verified against the network, once

One live run: sixteen of the twenty entries on the Hacker News front page
extracted into readable markdown, with their headings, code blocks and links.
The four that did not are the shape to expect — a discussion, a PDF, a
paywall.

## Not verified against the real thing

Everything else is tested against fixtures in `testdata/`, and the window has
been run by hand in kitty and in tmux against the replay fixture and against a
live feed list. Both are a different claim from having met the real thing.
This list is `AGENTS.md`'s, in short; that file is where it is kept current.

- **Extraction at scale.** Twenty entries from one feed is not forty-one feeds
  over a week. What the failure reasons look like across a real list is still
  open.
- **Conditional requests against real servers.** `ETag` and `Last-Modified`
  are stored and sent back; no server has yet answered 304 to this program.
- **Reddit's rate limiter.** The per-host gap, the user agent, the
  `Retry-After` handling and the HTML-page-instead-of-a-feed case are written
  to a reading of how it behaves, not to an observation of it.
- **`yt-dlp`.** Neither the handle resolver nor the subscription sync has been
  run against the real binary; the parsing is tested against fixture output.
- **A newsboat cache in the wild.** The importer reads the schema as
  documented. A cache written by an older newsboat has not been tried.
- **Two writers at once.** WAL, `busy_timeout` and the `data_version` poll are
  the design; a timer and a reader have not been run against one file at the
  same time.
- **The window, in other terminals.** Nothing here has seen it in Alacritty,
  WezTerm, Ghostty or the macOS Terminal, and the graphics probe is the part
  most likely to answer differently in one of them.
- **The window, for a whole evening.** Forty-one feeds at sixty columns, an
  article a thousand rows long, a refresh running while somebody reads.
- **Launching somebody else's program from inside the alternate screen.**
  `mpv --terminal=no` and `xdg-open` are detached with every fd on
  `/dev/null`, and that has been seen working on one desktop. A browser that
  insists on stealing the terminal has not been met.
- **The clipboard over ssh.** `y` copies through a clipboard that needs a
  display at the other end. The failure path is a note in the status line and
  is tested; the success path over a forwarded display is not.
