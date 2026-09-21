# If something is wrong

## The log

```
~/.local/starwire/cache/
```

One file per run, rotated. `starwire --verbose <anything>` logs at debug
level. Nothing is ever logged to the terminal — stdout belongs to the
subcommands' output and, later, to the window.

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
- **`the body is not a feed`** — what came back parsed as neither RSS, Atom
  nor JSON Feed. Often an HTML error page served with a 200.
- **`this reads feeds over https, not http`** — the address could not be
  rewritten. Everything here is https by design; see
  [Reading](reading.md#how-polite-this-is).

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
~/.local/starwire/data/wire.db
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
