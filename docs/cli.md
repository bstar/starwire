# On the command line

Every subcommand here is headless: none of them opens a terminal UI, and all of
them work in a pipe, a script or a systemd timer. That is not a consolation
prize — it is why the core is written the way it is.

`starwire` with no arguments opens the window. [Status](status.md) says how
much of the window exists today.

Two switches are global and may go before or after a subcommand:

- `--verbose` logs at debug level, to the log file, never to the terminal.
- `--replay DIR` serves a directory of saved responses instead of the network.

## Keeping up to date

```sh
starwire fetch                    # every feed
starwire fetch --feed 12          # one, by id or by URL, ignoring its backoff
starwire fetch --no-extract       # store what each feed carried, fetch no pages
```

Refreshes every feed that is not backing off, stores what arrived, and then
pulls the pages behind the new entries. Prints one summary line. **Exits 1 when
every feed failed**, so a systemd timer reports a failure rather than logging
nothing and succeeding.

A feed that fails gets a backoff that doubles per consecutive failure from
`[fetch] refresh_minutes`, to a ceiling of a day, and a server's own
`Retry-After` wins over that. `starwire fetch --feed <it>` ignores the backoff.

## Looking at what arrived

```sh
starwire list                     # the 50 newest entries
starwire list --unread --limit 20
starwire list --feed 12
starwire list --feeds             # the feeds themselves, with unread counts
starwire list --json              # one JSON object per line
```

The first column is the entry id. The flags beside it are `*` unread, `+`
starred, and `v` for a video or `p` for a forum post.

```sh
starwire show 42                  # a header, then the text
starwire show 42 --markdown       # the markdown alone
starwire show 42 --html           # the HTML the feed carried, if any
starwire show 42 --url            # the link, and nothing else
```

The header is the title, who wrote it, the feed, the date, the link, and one
word for what is stored: `extracted`, `feed text`, `pending` or `failed`. When
there is no text at all — the page has not been pulled yet, or it failed, or
the feed carried nothing — the header still prints, and where the article
would be there is one line saying which of those it was.

**`--markdown` prints nothing and exits 0 when there is no markdown.** That is
the contract, not an oversight: it is the format a script reads, and an empty
article should be an empty stream rather than a blank line, an apology or a
failure. Ask `starwire show <id>` without the flag when you want to know why.

## Subscribing

```sh
starwire add https://example.org/feed.xml
starwire add example.org/feed.xml --folder Tech --title "Example"
starwire add @veritasium          # a YouTube handle; resolved, then subscribed
starwire remove 12                # by id, or by any URL it was added under
```

Anything you type is canonicalised the same way: a bare host gets `https://`,
`http` is rewritten to `https`, and a YouTube channel in any of its spellings
becomes its `videos.xml` feed. Adding the same feed twice is a no-op and says
so.

## Moving a feed list in and out

```sh
starwire import newsboat --dry-run          # print the plan, write nothing
starwire import newsboat                    # ~/.config/newsboat/urls, and the cache
starwire import newsboat --urls PATH --cache PATH
starwire import opml blogroll.opml
starwire import opml blogroll.opml --dry-run
starwire export opml                        # to stdout
starwire export opml subscriptions.opml
```

`import newsboat` reads the urls file and, if it can find it, the cache — for
the feed titles (which for most feeds is the only place a name exists) and the
read marks. The cache is opened read-only and immutable, and the article bodies
are never read. Read marks for entries that have not arrived yet are held until
they do, so importing read state before the first `fetch` works.

`--dry-run` prints exactly what would happen: every feed with its canonical
URL, which are YouTube channels, which lines were skipped and why, and which
newsboat tags were dropped. Nothing is written.

Importing the same file twice adds nothing the second time.

## YouTube

```sh
starwire youtube add https://www.youtube.com/@veritasium
starwire youtube import-takeout subscriptions.csv
starwire youtube sync --cookies-from-browser firefox
starwire youtube list
```

`import-takeout` and `sync` both take `--dry-run`.

[YouTube](youtube.md) covers what each of those does and what it needs.

## The probe

```sh
starwire extract https://www.phoronix.com/news/Linux-6.19-Features
```

Runs the scraper on one page and prints what it made of it. No database, no
feed, nothing stored. This is how to find out whether a site yields before
subscribing to it, and how to put the actual markdown in a bug report when it
does not. It exits 1 with the reason on stderr when the page gives up nothing.

## Running it with no network at all

```sh
starwire --replay testdata/replay fetch
starwire list --unread
starwire show <id> --markdown
```

A replay directory holds an `index.tsv` of `URL<TAB>file` lines and the files
beside it, and optionally a `feeds.tsv` of feed URLs to seed an empty database
with. Producing one takes `curl` and a text editor, which is the point: a bug
report can carry the page that broke the extractor.
