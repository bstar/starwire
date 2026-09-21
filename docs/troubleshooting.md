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
  add a line for that host to `[fetch] host_intervals`, or lower
  `[fetch] refresh_minutes`.
- **`not a feed (HTML page; the site may be rate limiting)`** — a 200, but
  what came back was a web page. Often a login page a redirect led to; see
  Reddit, below.
- **`it redirected to a login page at …`** — the address is not public any
  more, or not public to a reader without an account.
- **`the body is not a feed`** — what came back parsed as neither RSS, Atom
  nor JSON Feed, and did not look like a web page either.
- **`this reads feeds over https, not http`** — the address could not be
  rewritten. Everything here is https by design; see
  [Reading](reading.md#how-polite-this-is).

## Reddit

Two things, and the first was diagnosed as the second for a while.

**`old.reddit.com` no longer serves feeds.** As of September 2026 it answers
`/r/<sub>/.rss` with a redirect to its login page, and the login page is the
HTML that arrives where a feed should be. STAR/WIRE now asks
`www.reddit.com/r/<sub>/.rss` instead, rewrites the old host to the new one on
the way in, and moves the feeds already in the database across the first time
0.0.2 opens it. There is nothing to do; if you have a feed subscribed to under
both spellings, both are kept and one of them can go.

**Reddit rate limits a feed reader hard.** Measured: with STAR/WIRE's own user
agent it serves the feed, and then says `x-ratelimit-remaining: 0.0` and
`x-ratelimit-reset: 40` after a single request. A browser's user agent gets a
`429` outright. That works out at roughly one request a minute for the whole
of `reddit.com`, however many subreddits you follow.

So `reddit.com` has a built-in gap of sixty-one seconds, and the reset header
is waited out on top of it. Five subreddit feeds therefore take about five
minutes to come round, which they do in the background. Nothing needs
configuring. If Reddit tightens further, `[fetch] host_intervals` is where to
say so:

```toml
[fetch.host_intervals]
"reddit.com" = 120
```

Reddit posts are never scraped in any case — the feed carries the body, and
the link goes to a comment thread. [Reading](reading.md) says why.

## An article shows only the feed's two sentences

The page did not yield, and the line above the text says why. That the feed's
text is there at all is the point: an entry is readable whatever its page did.
Find out why on that one page, without touching anything:

```sh
starwire extract <the entry's url>
```

It prints the markdown, or exits 1 with the reason. The usual reasons are a
paywall, a login wall, a page that is mostly JavaScript, or a link that goes
to a homepage rather than to an article.

`paywall` in particular means the page answered normally and gave a sample:
the first paragraph or two above the wall, which is usually the same text the
feed carried and is stored as the feed's rather than as an article.

A `429`, a `5xx` or a timeout comes round again by itself, after
`[fetch] refresh_minutes` and doubling from there. The rest stay as they are.
Either way an entry is tried at most three times ever, and `e` in the reader
forces another attempt now.

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
