# Test fixtures

Everything here is synthetic. No file in this directory came off a real
website, and none of it is anybody's data:

- The hosts are `example.org`, `example.com`-style reserved names (RFC 2606)
  and `*.example` (RFC 6761), none of which resolve.
- The YouTube channel ids and video ids are well-formed and arbitrary. They
  are never fetched: every test that would reach the network goes through
  `replay/` instead.
- `import/urls` is in the shape of a real newsboat file — no tags, no titles,
  comment headers as the only grouping — but the feeds in it are invented.
- The newsboat `cache.db` fixture is **not** here. It is built in memory from
  newsboat's own DDL in `src/wire/import/newsboat.rs`, because the real one
  is 123 MB and the columns that matter are five.

## What is where

| Directory | What it holds |
| --- | --- |
| `feeds/` | One file per feed format the reader has to read: Atom, RSS 2.0, JSON Feed, a YouTube channel feed, the same through the scriptbarrel proxy, two shapes of `hnrss`, a subreddit, and one deliberately broken document. |
| `pages/` | Pages for the extractor: one shaped like an article, one with nothing in it that reads like one, and two YouTube pages that carry a channel id. |
| `import/` | A newsboat `urls` file, an OPML export, a Google Takeout `subscriptions.csv`, and `yt-dlp` output for `:ytsubs`. |
| `replay/` | `index.tsv` maps a URL to one of the files above; `feeds.tsv` is the feed list `--replay` seeds an empty database with. |

## Running the whole thing with no network

```sh
starwire --replay testdata/replay fetch
starwire list --unread
starwire show <id> --markdown
```

The third prints the fixture article: a heading, a list, a fenced code block
and two numbered links.
