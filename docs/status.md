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

## Not started

**The window.** The column of docked modules, the stack, the reader, the
overlays, the keymap and the `[wire]` theme roles are designed and none of it
is built. `starwire` with no arguments says so and exits cleanly.

The running core the window will talk to — the two thread pools, the single
`apply`, and the `Handle` that is the contract between them — is also not
built. Every headless command works without it, which is the point of the
split.

## Deliberately outside this milestone

- Pictures inside articles. The markdown keeps them and the reader will draw
  an `[image: alt]` line; nothing is downloaded.
- Find-in-article.
- Per-feed refresh intervals.
- Folders inferred from a newsboat file's `# comment` headers. The importer
  reads them and does nothing with them; folders are assigned in the app.
- Podcasts and enclosures.
- Read-later as a separate flag. Starred *is* read-later: two lists nobody
  empties are worse than one.

## Verified against the network, once

One live run: sixteen of the twenty entries on the Hacker News front page
extracted into readable markdown, with their headings, code blocks and links.
The four that did not are the shape to expect — a discussion, a PDF, a
paywall.

## Not verified against the real thing

Everything else is tested against fixtures in `testdata/`, which is a
different claim from having been run against the network. Specifically: what
extraction looks like across a whole feed list over a week, whether real
servers' conditional requests behave as expected, Reddit's rate limiter,
`yt-dlp` itself, a newsboat cache written by an older newsboat, and two
writers on one database at the same time. `AGENTS.md` keeps that list
current.
