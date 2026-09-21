# Security

## Reporting

Use GitHub's private vulnerability reporting on this repository
(Security → Report a vulnerability). Please do not open a public issue for
something exploitable.

I work on this in my spare time, so expect a first reply within a week rather
than a day.

## Threat model

Who the attacker is, at each place STAR/WIRE takes input from somewhere else.

| Boundary | In scope |
| --- | --- |
| **A feed or a page on the network** | Yes. This is the boundary that matters. Everything this program parses — an Atom document, an RSS channel, a JSON Feed, an HTML page, a redirect chain — came off somebody else's web server and was not asked twice. The parsers are bounded by size caps, timeouts and element limits; nothing runs scripts; requests are https only, including after a redirect; and every one of those parsers has a property test that throws arbitrary bytes at it. A crash, a hang or a way past one of those limits is worth a report. |
| **The extracted markdown** | Yes. The text stored in the database is derived from somebody else's page, and it is what the reader will lay out and what `show` prints. A way to make it escape the reader — a control sequence that reaches the terminal, a link that is not the link it appears to be — is a vulnerability. |
| **The link handed to a browser or a player** | Yes. `o` and `v` pass a URL out of a feed as a single `argv` element, never through a shell. Anything that turns that into a shell line, or that lets a title or a URL inject an argument, is a vulnerability. |
| **`yt-dlp` and the player** | No. They are your programs, running as you, on a URL you subscribed to. STAR/WIRE looks them up on `PATH` and hands them arguments; what they then do with a page is between you and them. A bug in *how* they are invoked — an argument that is not quoted as one — is in scope under the row above. |
| **Other local users** | Yes, on a shared machine. Everything STAR/WIRE writes for itself lives under `~/.local/starwire`, the directory is mode 0700, and the files in it — the config, the database, the session file, the log — are mode 0600. The database holds what you read and when. |
| **The person running STAR/WIRE** | No. The feeds you subscribe to, the pages you ask it to fetch, and the player and browser you configure are your own authority. |
| **The build** | Yes. What the release workflow downloads is pinned and checksummed, and what CI runs is pinned to commits. |

## What is worth reporting

- **A feed or a page that crashes, hangs or exhausts memory.** Every limit is
  in `config.toml` and every one of them is meant to hold: `max_feed_bytes`,
  `max_article_bytes`, `max_markdown_bytes`, the fifteen-second timeout, the
  redirect cap, and readability's element limit. A document that gets past one
  of them, or that costs minutes inside a limit, is worth a report — with the
  bytes, if you can share them.
- **Anything that leaves https.** The agent is built `https_only`, `http` is
  rewritten to `https` before a feed is ever stored, and a redirect to
  plaintext is refused. A request that goes out in the clear is a
  vulnerability, not a compatibility bug.
- **Anything that reads a file it was not pointed at.** The importer opens
  newsboat's cache read-only and immutable and reads five columns; nothing
  else in this program opens a file it was not given the path to.
- **The external opener treating a URL as anything other than one argument.**
  `[player] video` and `[player] browser` are `argv`, never shell lines, and
  the URL is appended as a single element. An entry whose link causes
  something other than "run this program with this URL" is worth a report.

Nothing here listens on a network port. Every connection is outbound, to a
host that is in your feed list or linked from an entry in it.

## What is not a vulnerability

- **A site refusing to be read.** A paywall, a login wall, a 403 or a page
  that is mostly JavaScript will not extract. That is the site's decision and
  this program reports it and offers the browser.
- **A feed that lies.** A feed can claim any title, any date and any link for
  its own entries. STAR/WIRE shows what the feed said; deciding whether to
  trust a publisher is subscribing to it.
- **Tracking parameters that survive.** `clean_url` strips a list of known
  analytics parameters and is deliberately conservative — a parameter that
  might be tracking and might be a page selector is kept, because losing a
  real one turns a link into a 404. A parameter that should be on the list is
  a pull request, not a security report.
