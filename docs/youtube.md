# YouTube

A YouTube channel publishes an Atom feed, so a channel is a feed like any
other. What is special is how many ways there are to name one.

## Subscribing

All of these produce exactly the same feed row:

```sh
starwire add UCXuqSBlHAE6Xw-yeJA0Tunw                       # the channel id
starwire add https://www.youtube.com/channel/UCXuq...       # a channel page
starwire add https://www.youtube.com/feeds/videos.xml?channel_id=UCXuq...
starwire add https://scriptbarrel.com/xml.cgi?channel_id=UCXuq...&name=Name
starwire add @veritasium                                    # a handle
starwire add https://www.youtube.com/watch?v=dQw4w9WgXcQ    # any of its videos
```

The first four are worked out without a request. The last two need one: a
handle can be given up and taken by somebody else, and a video page has to be
read to find out whose channel it is. STAR/WIRE reads the page and looks for
the channel id in it; if that fails — because YouTube changed the page, most
likely — it falls back to `yt-dlp`.

Everything is stored under the channel id, because it is the only thing about a
channel that never changes. A handle changes, a name changes, the id does not.

## Importing a whole subscription list

**Google Takeout.** Ask Google for your YouTube data; the export has a
`subscriptions.csv` with a row per channel.

```sh
starwire youtube import-takeout subscriptions.csv --dry-run
starwire youtube import-takeout subscriptions.csv
```

This is the complete and reliable route. It needs no cookies and no `yt-dlp`,
and it lists every channel you are subscribed to including the quiet ones.

**Through `yt-dlp`.** If you would rather not wait for a Takeout export:

```sh
starwire youtube sync --cookies-from-browser firefox
```

This asks `yt-dlp` for `:ytsubs` with your browser's cookies. **It is not
complete, and the reason is worth knowing.** There is no public endpoint that
returns an account's subscription *list*; `:ytsubs` is the subscriptions
*feed* — recent videos from everything you follow. STAR/WIRE subscribes to the
distinct channels in that window, so a channel that has published nothing
recently is missed. Run it again later, or use Takeout.

It needs cookies, and without them it says so rather than failing obscurely.
Set `[youtube] cookies_from_browser` in `config.toml` to avoid passing the
flag every time.

## `scriptbarrel.com`

Some feed lists route YouTube channels through `scriptbarrel.com/xml.cgi`,
which proxies the same Atom document from a different host. STAR/WIRE
recognises those URLs and rewrites them to the channel's own feed — with one
thing kept: the `&name=` parameter on a scriptbarrel URL is, for those
channels, the only place a name exists before the first fetch, so it is taken
as the feed's title.

The original URL is kept as the feed's `source_url`, which is what makes a
second import of the same file recognise what it already added.

## Watching

A video entry is never scraped. Its description from the feed is its text, and
the video opens in a player:

```toml
[player]
video = ["mpv", "--terminal=no", "--"]
```

`--terminal=no` matters: without it `mpv` takes over the terminal STAR/WIRE is
drawing in. The URL is always appended as exactly one more argument, never
through a shell.

`mpv` finds `yt-dlp` on `PATH` and uses it to resolve the stream. Both are
your programs — STAR/WIRE only hands them a URL, and if a YouTube change
breaks playback the fix is a newer `yt-dlp`, not a newer STAR/WIRE.

Set `video = []` to open videos in the browser instead.

## Listing what you are subscribed to

```sh
starwire youtube list
```

One line per channel: its id and whatever name is known. Unsubscribing is
`starwire remove <the feed>`, which takes the channel with it.
