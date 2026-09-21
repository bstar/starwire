# Reading

The thing STAR/WIRE exists to do: an entry in a feed gives you a title, a link
and, if you are lucky, two sentences. What you want is the article.

## What happens to an entry

1. The feed is fetched and its entries are stored. Whatever text the feed
   itself carried is stored with them, so an entry is readable immediately.
2. If the entry's link is worth fetching, the page behind it is downloaded and
   run through Mozilla's readability algorithm — the same one behind Firefox's
   reader view — which finds the article inside the navigation, the newsletter
   box and the footer.
3. That HTML becomes CommonMark: headings, lists, quotes, tables, links, and
   fenced code blocks keeping the language the page declared.
4. The markdown is tidied. Runs of blank lines collapse, every relative link
   is resolved against the page it came from, and anything past
   `[articles] max_markdown_bytes` is cut at a paragraph boundary.

This happens in the background as entries arrive, which is why opening one is
instant rather than a wait.

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

When that happens the entry keeps whatever its feed carried, the reason is
recorded, and the entry is still there to read and to open in a browser. An
entry is tried at most three times, ever: a page that has failed three times is
a paywall, and a fourth request helps nobody.

`starwire extract <url>` runs the whole thing on one page and prints the
result, without a feed and without writing anything. It is the fastest way to
find out whether a site works before subscribing to it.

## How polite this is

There is no `robots.txt` request, and that is deliberate rather than an
oversight: this fetches pages you subscribed to and asked to read, one per
entry, at most three times ever. The politeness is in the construction:

- One request at a time per host, with `[fetch] min_host_interval_secs`
  between them.
- A fifteen-second timeout on the whole request, and at most five redirects.
- `[fetch] max_feed_bytes` and `[articles] max_article_bytes` caps.
- An `Accept` header that says what is wanted, so a server that would rather
  send something else can.
- Conditional requests, so a feed that has not changed costs a round trip and
  no body.
- A user agent that names the program and links the repository, so an
  administrator unhappy about the traffic knows who to ask. Add your own
  contact with `[fetch] user_agent_extra`.

Everything is https. A feed URL written as `http://` is rewritten on the way
in, and a redirect to plaintext is refused.

## Images

Pictures inside articles are not in this release. An image in the markdown is
kept as an image and the reader draws it as an `[image: alt]` line — or just
`[image]` when the page gave no alt text, which is most decorative header
images. Nothing is downloaded. `[articles] images = false` leaves the alt text
behind as a plain paragraph instead.
