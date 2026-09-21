# Working notes

Context that is not derivable from the code or the history, kept here rather
than in any one machine's notes because this is developed on both Linux and
macOS, with more than one assistant, and the repository is the only thing all
of them see.

This file is the only assistant-facing notes file in the repository. Do not add
a tool-specific notes file or directory beside it; every assistant reads
`AGENTS.md`.

## Building

Use the Nix flake for all builds and checks. Do not assume `cargo` is on the
ambient `PATH`.

```sh
CARGO_NET_GIT_FETCH_WITH_CLI=true nix develop -c cargo build --release
CARGO_NET_GIT_FETCH_WITH_CLI=true nix develop -c cargo test --all
```

There are no system libraries. That is worth stating for a program that talks
to forty web servers and keeps a database: TLS is rustls through `ureq`, and
SQLite is the amalgamation `rusqlite`'s `bundled` feature compiles from source
with the `cc` crate. Nothing runs bindgen. A change that adds a `-sys` crate
has to add the library to `flake.nix` and to CI in the same commit.

`mpv` and `yt-dlp` are in the devshell but are **not** build inputs. They are
the reader's own programs, looked up on `PATH` at the moment they are wanted.
The tests that would use `yt-dlp` skip when it is not there.

On a Mac with Xcode selected, `/usr/bin/git` is a shim that asks xcrun where
git is, and inside `nix develop` xcrun is nix's, which answers `tool 'git' not
found`. The devshell therefore carries nix's own git first on `PATH`, which is
what cargo's CLI fetch of STAR/KIT uses.

## STAR/KIT

The shared foundation — paths, logging, private file writes, themes,
keymap/help, chrome, text entry, wrapping, terminal graphics, and the HTTP
agent's defaults — lives in `starkit`, the crate STAR/AMP, STAR/CORD and
STAR/FOLD use too. It is a git dependency pinned to a revision, and the flake
takes it from the revision `Cargo.lock` names through
`cargoLock.allowBuiltinFetchGit`.

STAR/WIRE is the first consumer to turn the `net` feature on. `net::builder`
is what `wire::net::Live` configures its agent from, so `ureq` is a direct
dependency here as well as STAR/KIT's — the **same version**, deliberately, so
there is one copy and one connection pool. A second `ureq` in the tree would
mean `net::builder` handing back a type this crate cannot use.

It is also the one copy of `ratatui`, `crossterm`, `ratatui-image` and `image`
in the tree. Everything under `src/ui/` reaches them through `starkit::`, and
none of the four is a direct dependency: a widget built against a second copy
of ratatui does not satisfy a signature expecting the first, and the compiler
reports that as two versions carrying the same number.

`src/paths.rs` is a name for `starkit::paths::Paths` and nothing else. Keep the
name: the core takes `crate::paths::Paths` by value throughout, and `PATHS` is
the one place this application's three identifying strings are written down.

A local checkout is used through an uncommitted `.cargo/config.toml`:

```toml
[patch."https://github.com/bstar/starkit"]
starkit = { path = "../starkit" }
```

`.gitignore` already covers it, and it has to be taken away again before
anything is committed: with the patch in place `cargo` rewrites the `starkit`
entry in `Cargo.lock` to the path, and a lock file with no git source in it is
one the flake cannot build. **Never commit `.cargo/config.toml`, and never
commit a `Cargo.lock` written while it was in place** — this happened once in
STAR/FOLD, and the fix was reverting `Cargo.lock` back to the git-sourced
entry and deleting the file. If `git status` ever shows `Cargo.lock` changed
with no dependency version bump to explain it, check for this before anything
else.

Every public `starkit` item now has **four** consumers — check STAR/AMP,
STAR/CORD and STAR/FOLD before changing a signature.

## Nothing under src/wire/ draws

No `ratatui`, no `crossterm`, no `crate::ui` anywhere under `src/wire/`. A test
in `wire/mod.rs` greps the module's own sources and fails if any of them
appear; a line that has to mention one is exempted by carrying the marker
`NO-TERMINAL-HERE`, which is how the test's own list of forbidden strings gets
past itself.

This is not tidiness. It is why `starwire fetch` can run from a systemd timer
with no TTY attached, why `starwire extract <url>` can print markdown into a
pipe, why the core is testable with no window involved, and why a rendering
change cannot break how a page is read.

The dependency runs the other way as well. `src/ui/` will never reach into the
core's internals; it will talk to `Handle` and to the read-side query API on
`State`.

## One writer, and it is the database

`src/wire/db/` is the only thing in the program that writes. There is one
`rusqlite::Connection` and one thread holding it — the `starwire-db` thread
when the window is up, the calling thread when a headless subcommand is
running. The rule is kept by there being one `Db`, not by a mutex, and adding
a second place that opens the file for writing is how that stops being true.

`starwire fetch` from a timer and an open reader are two processes on one
file, which is what WAL, `busy_timeout` and the idempotent `(feed_id, guid)`
upsert are for. The reader notices the timer's writes through
`PRAGMA data_version`, which is cheap enough to ask once a second and does not
depend on an mtime that WAL does not move.

`wire.db` is **not a cache.** Read state, stars and every article ever pulled
out of a page that may since have gone behind a paywall exist nowhere else.
That is why `db::migrate` refuses a file from a newer build by naming the
version rather than suggesting it be deleted, and why it lives beside the
config rather than in `cache/`.

## Every URL goes through one function

`wire::youtube::canonicalise` is more than a YouTube function despite where it
lives. Everything entering the program — a typed URL, an imported line, an
OPML outline — goes through it, and it does three things: gives a bare host a
scheme, rewrites `http` to `https`, and normalises a YouTube channel to its
`videos.xml?channel_id=` feed.

The `http` rewrite is the one worth defending. The agent STAR/KIT builds
refuses plaintext, and that is kept rather than relaxed, so
`http://old.reddit.com/r/rust/.rss` — which is what a real `urls` file says —
would otherwise simply fail. A second, plaintext agent is not the answer: it
would be used for extraction too, and extraction follows links out of pages,
so a reader that will fetch `http://` because one feed needed it is a reader
that fetches an injected link in the clear. The original spelling is kept as
the feed's `source_url`, which is also what makes a second import of the same
file recognise what it already added.

A canonical URL canonicalises to itself. There is a property test, and the
whole of deduplication rests on it.

## What is fetched, and what is not

There is no `robots.txt` request, and that is a decision rather than an
oversight: this fetches pages a person subscribed to and asked to read, one
per entry, at most three times ever. The politeness is structural instead —
`wire::net::Politeness` holds a host serially for the length of a request and
sleeps the configured gap between them, there is a fifteen-second timeout and
a two-megabyte cap, the `Accept` header says HTML, and the user agent names
the program and links the repository so an unhappy administrator knows who to
ask.

Three kinds of entry are never fetched, and `wire::extract::policy` is where
that is decided: a video (the description is the text, and `mpv` is the
point), a Reddit post (the feed carries the body, the link is a comment
thread, and Reddit rate limits hard enough that scraping it would cost the
whole list its refreshes), and a Hacker News item whose link goes back into
`news.ycombinator.com`. An `hnrss` item that links out **is** fetched, which
is most of the value of reading HN in a reader at all.

## The two html5ever copies

`htmd` is on `html5ever` 0.38; `dom_smoothie` reaches 0.39 through
`dom_query`. So the binary carries two HTML tokenizers. `cargo tree -d` shows
it, `deny.toml` allows it with a note, and it is accepted for 0.0.1 because
both are behind one seam: `wire::extract::markdown::to_markdown` is the only
place `htmd` is named anywhere in the tree.

The planned fix, if `htmd`'s output ever disappoints, is a serializer over the
`dom_query` tree that is already in the graph — roughly 350 lines, one file,
and no change anywhere else. Keeping `htmd::convert` out of every other module
is what makes that true, so do not call it directly.

Keeping classes through readability (`keep_classes: true`) is related and also
deliberate: the language on a fenced code block lives in a `class` attribute
and nowhere else, so stripping classes makes every code block in every article
come out as a bare fence.

## Testing

In-module `#[cfg(test)]`, as in STAR/AMP, STAR/CORD and STAR/FOLD. Anything
that reads foreign input — a feed, a page, a ten-year-old hand-edited `urls`
file, an OPML export, a Takeout CSV, a typed search — gets a proptest as well
as table tests.

Everything in `testdata/` is synthetic and `testdata/README.md` says exactly
what and why. `testdata/replay/` is a directory of saved responses served
through `wire::net::Replay` instead of the network, so `cargo test` works on a
machine with no network at all; `testdata/replay/feeds.tsv` is the feed list
`--replay` seeds an empty database with, which is what makes
`starwire --replay testdata/replay fetch` work on a fresh machine.

The newsboat cache fixture is built in memory from newsboat's own DDL in
`wire/import/newsboat.rs` rather than checked in: the real one is 123 MB and
the columns this reads are five.

`STARWIRE_TEST_NET=1` gates anything that reaches the real network; the
`yt-dlp` tests also want the binary on `PATH`.

## Not yet verified

Nothing in this milestone has been run against a live feed list yet —
everything above is built and passes its own tests against fixtures, which is
a different claim. This is where a claim that turned out to be a guess rather
than an observation gets recorded, the same way STAR/CORD keeps its own list
of protocol details still waiting on one. Specifically, still unverified:

- **Extraction against real sites.** Every page the extractor has seen is
  `testdata/pages/article.html`. How often a real feed list's articles yield,
  and what the failures look like, is the first thing to find out with the
  network on: `starwire extract <url>` is the probe.
- **Conditional requests against real servers.** `ETag` and
  `Last-Modified` are stored and sent back; no server has yet answered 304 to
  this program.
- **Reddit's rate limiter.** The per-host gap, the user agent and the
  `Retry-After` handling are all written to a reading of how it behaves, not
  to an observation of it.
- **`yt-dlp`.** Neither `resolve_with_yt_dlp` nor `yt_subscriptions` has been
  run against the real binary; the parsing is tested against fixture output.
  `:ytsubs` in particular lists recent subscription *videos* rather than the
  subscription list, so a channel silent in the window is missed — that is
  documented in `docs/youtube.md` but has not been measured.
- **A newsboat cache in the wild.** The importer reads the schema as
  documented and as the reference cache has it. A cache written by an older
  newsboat has not been tried.
- **Two writers at once.** WAL, `busy_timeout` and the `data_version` poll are
  the design; a timer and a reader have not actually been run against one file
  at the same time.
- **The window.** It does not exist yet. `src/ui/mod.rs` is a stub.
