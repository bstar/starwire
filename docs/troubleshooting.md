# If something is wrong

## The log

```
~/.local/starwire/cache/starwire.log
```

One file, appended to across runs, and safe to delete: it is under `cache/`
because nothing in it cannot be lost. `starwire --verbose <anything>` logs at
debug level. Nothing is ever logged to the terminal — stdout belongs to the
subcommands' output, and to the window when the window has it.

## A feed is not updating

```sh
starwire list --feeds
```

The last error is on the right of the line that has one. A feed that has
failed backs off, doubling from `[fetch] refresh_minutes` to a ceiling of a
day, so a feed that broke yesterday is not retried every fifteen minutes.

```sh
starwire fetch --feed <url-or-id>
```

ignores the backoff and tries it now, printing the reason if it fails again.

A few errors mean particular things:

- **`404`** — there is no feed at that address any more. The site probably
  moved it.
- **`410`** — it has been withdrawn, and it is not coming back. Remove it.
- **`401` / `403`** — it needs credentials, or it refuses this reader.
  STAR/WIRE sends no credentials and has no way to.
- **`429`** — too many requests. Raise `[fetch] min_host_interval_secs`, or
  lower `[fetch] refresh_minutes`. Reddit is the usual source of these.
- **`not a feed (HTML page; the site may be rate limiting)`** — a 200, but
  what came back was a web page. See Reddit, below.
- **`the body is not a feed`** — what came back parsed as neither RSS, Atom
  nor JSON Feed, and did not look like a web page either.
- **`this reads feeds over https, not http`** — the address could not be
  rewritten. Everything here is https by design; see
  [Reading](reading.md#how-polite-this-is).

## Reddit

Reddit rate limits a feed reader hard, and it does not always say so with a
`429`. Asked too often, `old.reddit.com/r/<sub>/.rss` answers **200 with an
HTML page** — its own "take a break" page — rather than the feed. STAR/WIRE
recognises that and records it as `not a feed (HTML page; the site may be
rate limiting)`, which is worth telling apart from a broken feed: the feed is
fine, and the answer is to ask less often.

- Raise `[fetch] min_host_interval_secs`. Every `old.reddit.com` feed shares
  one host, so five subreddits at a two-second gap are five requests in ten
  seconds.
- Raise `[fetch] refresh_minutes`.
- Leave it alone for a while. The failure earns the ordinary backoff, which
  doubles to a ceiling of a day, so a rate-limited feed already slows itself
  down.

Reddit posts are never scraped in any case — the feed carries the body, and
the link goes to a comment thread. [Reading](reading.md) says why.

## An article shows only the feed's two sentences

The page did not yield. Find out why on that one page, without touching
anything:

```sh
starwire extract <the entry's url>
```

It prints the markdown, or exits 1 with the reason. The usual reasons are a
paywall, a login wall, a page that is mostly JavaScript, or a link that goes
to a homepage rather than to an article.

An entry is tried at most three times ever. `e` in the reader forces another
attempt.

Some entries are never extracted on purpose: videos, Reddit posts, and Hacker
News items that link back into the comment thread. [Reading](reading.md) says
why.

## An import did not bring everything

```sh
starwire import newsboat --dry-run
```

prints every feed with the URL it would be stored under, every line that was
skipped and why, and every newsboat tag that was dropped. `query:`, `filter:`
and `exec:` lines are not feeds — they name something newsboat computes — and
are always skipped.

If the titles are missing, the cache was not found. Point at it:

```sh
starwire import newsboat --urls ~/.config/newsboat/urls \
                         --cache ~/.local/share/newsboat/cache.db
```

Importing twice is harmless: the second run adds nothing.

## A video will not play

STAR/WIRE only hands a URL to `mpv`. Try it directly:

```sh
mpv --terminal=no -- 'https://www.youtube.com/watch?v=...'
```

If that fails too, the fix is usually a newer `yt-dlp` — `mpv` finds it on
`PATH` and uses it to resolve the stream. Neither is a dependency of this
program.

## `youtube sync` says it needs cookies

It does. There is no public endpoint that returns an account's subscription
list, so the only way is `yt-dlp` with your browser's cookies. Either pass
`--cookies-from-browser firefox` or set `[youtube] cookies_from_browser`. A
Google Takeout export needs no cookies at all and is more complete; see
[YouTube](youtube.md).

## The database

```
~/.local/starwire/wire.db
```

It is ordinary SQLite, in WAL mode, and it is safe to read with `sqlite3`
while STAR/WIRE is running. It is **not** a cache: it holds your read state,
your stars and every article ever pulled out of a page, some of which may have
gone behind a paywall since. Back it up; do not delete it to fix something.

`starwire export opml` writes the feed list out in a form any other reader can
take, which is the thing worth keeping if you ever do start again.

## Starting from nothing

```sh
STARWIRE_DIR=$(mktemp -d) starwire list --feeds
```

runs against an empty directory without touching your real one. Every
subcommand honours it.
