# Configuration

`~/.local/starwire/config.toml`, written with comments on the first run. Every
key has a default, and a file that omits a table gets that whole table's
defaults — so it is safe to keep only the lines you have changed.

`$STARWIRE_DIR` moves everything STAR/WIRE keeps; `$STARWIRE_CONFIG_DIR` moves
only this file.

## `[ui]` — how the column looks

| Key | Default | What |
| --- | --- | --- |
| `theme` | `"catppuccin-mocha"` | A built-in theme id, or `"system"` to follow the desktop. `t` and `T` cycle. See [Themes](themes.md). |
| `graphics` | `"auto"` | `auto`, `kitty`, `blocks` or `off`. |
| `padding_x`, `padding_y` | `0` | Blank cells around the whole layout, for a terminal whose window has none. |
| `list_rows` | `12` | How many rows a focused list opens to while an article is open behind it. |

## `[reading]` — how an article reads

| Key | Default | What |
| --- | --- | --- |
| `width` | `80` | Columns of text, centred in the reader. `<` and `>` step it by eight. |
| `show_byline` | `true` | The author, site and date line under the title. |
| `mark_read_on_open` | `true` | Opening an entry marks it read; `m` puts it back. |

## `[fetch]` — how and how often feeds are pulled

| Key | Default | What |
| --- | --- | --- |
| `refresh_minutes` | `15` | How often a background refresh of every feed starts. Also the base of the per-feed backoff after a failure. |
| `parallel` | `4` | How many feeds are fetched at once. Clamped to 1–16. |
| `timeout_secs` | `15` | The whole request, connection and body included. |
| `max_feed_bytes` | `8388608` | A feed body larger than this is a failure rather than something to parse. |
| `min_host_interval_secs` | `2` | The gap between two requests to one host. Requests to a host are serial regardless; this is how long the next one waits. |
| `user_agent_extra` | `""` | Appended to `starwire/<version> (+https://github.com/bstar/starwire)`. Put a contact address here if you would rather the sites you read had a way to reach you. |
| `refresh_on_start` | `true` | Refresh when the reader opens. |

## `[articles]` — what happens to an entry

| Key | Default | What |
| --- | --- | --- |
| `extract` | `true` | Fetch the linked page and pull the article out of it. `false` leaves every entry on the text its feed carried. |
| `max_article_bytes` | `2097152` | The most HTML downloaded for one article. |
| `max_markdown_bytes` | `524288` | Markdown longer than this is cut at a paragraph boundary. |
| `keep_days` | `30` | Entries older than this are swept. |
| `max_entries_per_feed` | `2000` | And the second bound, which is the one that matters for a busy feed: a month of `hnrss/newcomments` is about a hundred thousand rows. |
| `images` | `true` | Keep images as images. `false` leaves the alt text behind as a paragraph. Nothing is downloaded either way in this release. |
| `page_size` | `200` | How many entries are loaded at a time. Clamped to 10–5000. |

**Starred entries survive both retention bounds.** Starring something is how
you say keep this.

## `[player]` — what opens a link and what plays a video

```toml
[player]
video = ["mpv", "--terminal=no", "--"]
browser = []
```

Both are `argv`, never shell lines: the URL is appended as exactly one more
argument, whatever is in it. An empty `browser` means the desktop's own opener
— `open` on macOS, `xdg-open` elsewhere. `video = []` opens videos in the
browser too.

`--terminal=no` matters for `mpv`: without it, it takes over the terminal
STAR/WIRE is drawing in.

## `[youtube]`

```toml
[youtube]
cookies_from_browser = ""
yt_dlp = "yt-dlp"
```

`cookies_from_browser` is passed to `yt-dlp --cookies-from-browser` by
`starwire youtube sync`. Empty means that command is unavailable — there is no
way to read an account's subscriptions without them. See
[YouTube](youtube.md).

## Where each setting takes effect

Most apply the moment they are read. `[player]` and `[youtube]` argv changes
take effect on restart. The settings overlay in the reader writes to this file
and says which of the two a row is.
