# Reading

The thing STAR/WIRE exists to do: an entry in a feed gives you a title, a link
and, if you are lucky, two sentences. What you want is the article.

## What happens to an entry

1. The feed is fetched and its entries are stored. Whatever text the feed
   itself carried is stored with them, so an entry is readable immediately —
   and stays readable whatever happens next. A description with no markup in
   it keeps the lines it was written with, which is what a YouTube chapter
   list is.
2. If the entry's link is worth fetching, the page behind it is downloaded and
   run through Mozilla's readability algorithm — the same one behind Firefox's
   reader view — which finds the article inside the navigation, the newsletter
   box and the footer.
3. Where the site splits an article across pages and STAR/WIRE knows it does,
   the rest of the pages are fetched and joined on: same host, same path, at
   most eight, one at a time with the usual gap between them. Phoronix
   reviews are the case this exists for.
4. If the page gives up nothing at all, its own structured data is asked next:
   a page that draws itself with JavaScript often still carries the whole
   article in a `<script type="application/ld+json">` for search engines, and
   that is the only honest copy of it the page will give anybody.
5. That HTML becomes CommonMark: headings, lists, quotes, tables, links, and
   fenced code blocks keeping the language the page declared. Pictures are
   resolved first — `srcset`, `<picture>` and the `data-src` a lazy-loading
   page hides the real address in — so what lands in the text is the picture
   and not a one-pixel spacer.
6. The site's own furniture comes off. A share bar, a "more like this" list, a
   patron link and an ad slot inside the prose are all inside the element
   readability scored, so a short table of per-site rules takes them out
   again.
7. The markdown is tidied. Runs of blank lines collapse, every relative link
   is resolved against the page it came from, and anything past
   `[articles] max_markdown_bytes` is cut at a paragraph boundary.

This happens in the background as entries arrive, which is why opening one is
instant rather than a wait.

In order, then: **the page, its other pages, its JSON-LD, the feed's own
text** — and a `403` is tried once more as a browser, since some edge
firewalls refuse anything else, while a wall that refuses both is left alone.
Something at every step, and the feed's text underneath all of it.

## In the reader

An article is drawn at `[reading] width` columns, centred, with the headline
above it and — when `[reading] show_byline` is on — whatever the page said
about who wrote it, where, and when, plus how long it is to read. The headline
is drawn once however many times the page said it: most sites put it in the
title *and* at the top of the body.

- `<` and `>` step the width by eight, between 40 and 160, and write it back
  to `config.toml`. There is no wrong answer; eighty is what a century of
  typography says and what most of these articles were written for.
- Links are numbered `[1]`, `[2]`, … in the order they appear, and `o` then a
  number opens one. The number closes as soon as it cannot grow, so `o1` in an
  article with three links opens link one at once. `oo` opens the article
  itself; so does `o` from the entry list.
- `e` pulls the page again, for a site that was down the first time.
- `space` and `b` page; `n` and `p` step to the next and previous entry,
  keeping your place in the one you are leaving; `N` finds the next unread
  even if it is in another feed.
- `esc` closes the article.

An article still being fetched says `extracting…`; one that failed says why,
and `o` opens the page; a video shows its description and `v` plays it.

## What is not scraped, and why

**A video.** Its description is the text and the video is the point. `v` plays
it.

**A Reddit post.** The feed carries the post body already; the link goes to a
comment thread rather than to an article; and Reddit rate limits hard enough
that scraping it would cost the whole feed list its refreshes.

**A Hacker News item that links back into `news.ycombinator.com`.** That is
the discussion, not the piece — and when the submission *is* a discussion,
there is no piece. An `hnrss` item that links out is fetched, which is most of
the reason to read HN in a reader at all.

**An entry with no link, or a link this cannot fetch.** A `magnet:` URI, a
`mailto:`, anything that is not http.

You can turn extraction off entirely with `[articles] extract = false`, which
leaves every entry on the text its feed carried. For a full-text feed that is
the same thing.

## When a page does not yield

Not every page does. A paywall, a login wall, a page that is mostly JavaScript,
or a "page" that is really a PDF will all fail — and so will a link to a
homepage, which readability is asked about first and refuses, because
otherwise it would happily return a page's navigation as an "article".

**The entry keeps the text its feed carried.** It is stored the moment the
entry arrives and a failed extraction never overwrites it, so what you see is
the feed's summary with one line above it saying why there is no more and
offering to open the page. That is the difference between an entry that is two
sentences and an entry that is blank.

A page that answers with the first two paragraphs and an invitation to
subscribe is treated the same way: the feed's own text, and `paywall` as the
reason. Both are usually the same two paragraphs, and the feed's version does
not pretend to be the article.

**Some failures come back on their own.** A `429`, a `5xx` and a timeout are
facts about that minute, so the entry rejoins the queue after
`[fetch] refresh_minutes`, doubling for each attempt and capped at a day. A
`401`, a `402`, a `404` and "does not read like an article" are facts about the
page and stay where they are — a browser's user agent gets the same codes,
which has been measured.

**A `403` is asked once more, as a browser.** Not every one of them is a wall:
some sites sit behind an edge firewall that filters on the user agent and
nothing else, and IFLScience is one — CloudFront refuses STAR/WIRE's own agent
with a 403 and a 919-byte error page, and serves a browser the whole article.
So a 403 costs a second request straight away, with a browser's user agent and
its headers, and a 403 that refuses that too is the wall it looks like and is
left alone. The honest agent goes first every time, and a site that does not
refuse it never sees the other one.

Either way an entry costs at most three attempts ever, and `e` in the reader
forces another.

`starwire extract <url>` runs the whole thing on one page and prints the
result, without a feed and without writing anything. It is the fastest way to
find out whether a site works before subscribing to it, and it exits 1 with
the reason when the page gives nothing.

## How polite this is

There is no `robots.txt` request, and that is deliberate rather than an
oversight: this fetches pages you subscribed to and asked to read, one per
entry, at most three times ever. The politeness is in the construction:

- One request at a time per host, with `[fetch] min_host_interval_secs`
  between them — **including every hop of a redirect.** A wrapper URL that
  points somewhere else is two requests to two hosts, and each waits its own
  turn.
- A longer gap for the hosts that have asked for one: `reddit.com` every
  sixty-one seconds, `archive.is` every ten, plus whatever
  `[fetch] host_intervals` says. Where a server sends its own rate-limit
  headers and STAR/WIRE believes them — Reddit does — the reset it names is
  waited out.
- `[fetch] timeout_secs` on a feed and `[articles] timeout_secs` on a page,
  and at most five redirects, which are followed one at a time rather than by
  the HTTP library.
- A redirect into a login page stops there and is reported, rather than being
  fetched and extracted.
- `[fetch] max_feed_bytes` and `[articles] max_article_bytes` caps.
- An `Accept` header that says what is wanted, so a server that would rather
  send something else can.
- Conditional requests, so a feed that has not changed costs a round trip and
  no body.
- A user agent that names the program and links the repository, so an
  administrator unhappy about the traffic knows who to ask. Add your own
  contact with `[fetch] user_agent_extra`. It is what every request goes out
  with, and the only exception is the second try at a page that answered 403,
  above.

Everything is https. A feed URL written as `http://` is rewritten on the way
in, and a redirect to plaintext is refused.

## Pictures

**The pictures an article carries are drawn in it.** In a terminal with a
graphics protocol they are the pictures; in one without, half blocks, which
are coarse and are still a picture. `░` stands where one is going while it is
on its way, so the text does not move when it lands.

A picture is drawn at **its own size** when that fits: one image pixel per
terminal pixel, which on a typical font is a 640-pixel-wide photograph across
eighty columns. One wider than the text column is fitted to it, and nothing is
ever made *bigger* than it is — a 160-pixel logo blown across a third of the
page is not a service.

**Never more than a third of the reader.** `[reading] image_rows` is the cap
in rows, and `0` means a third of whatever height the panel has. It is the
reason a page of prose stays a page of prose: a picture that leaves two lines
of text on the screen has taken the article over.

**A click opens it.** On a desktop it goes to whatever shows pictures there —
the cached file, so an image viewer opens rather than a browser, and `[player]
image` names another program if you would rather. On a session with no display
of its own — over ssh, or a bare tty — it opens an overlay instead, which
grows the picture to the whole window and says by how much. `[reading]
click_picture` is `auto`, `viewer` or `external` if you would rather decide.
In the overlay, `n` and `p` walk the article's pictures, `o` opens it outside,
`y` copies its address and `esc` closes.

**Two switches turn them off.** `[articles] images = false` leaves the alt
text behind as a plain paragraph and fetches nothing; `[ui] graphics = "off"`
draws the `[image: alt]` line instead of the picture. Either way what is drawn
is `[image: alt]`, or just `[image]` when the page gave no alt text, which is
most decorative header images.

The bytes live in `~/.local/starwire/cache/pictures`, named by a hash of the
URL and swept once a day with the entries — by age first, then oldest-first
down to `[articles] pictures_mib`. Everything in it is re-fetchable, which is
why there is a ceiling rather than a retention policy. PNG, JPEG, WebP and the
first frame of a GIF are drawn; anything else, AVIF included, is its alt line.

The address kept for one is the address the picture is really at, which on a
modern page takes finding: the `src` is often a spacer or a base64 blur, with
the real one in `srcset`, in a `<picture>`, in `data-src`, or in the
`<noscript>` the page shows to a browser without JavaScript. A picture whose
only address is its own bytes is dropped and its alt text kept: nothing can
fetch it, and one blur placeholder is two kilobytes of the size cap. An
`http:` picture is its alt line too — everything this program fetches is
https, and one picture is not the reason to keep a second agent that is not.
