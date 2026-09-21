# STAR/WIRE

[![ci](https://github.com/bstar/starwire/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/bstar/starwire/actions/workflows/ci.yml)
[![nix](https://github.com/bstar/starwire/actions/workflows/nix.yml/badge.svg?branch=main)](https://github.com/bstar/starwire/actions/workflows/nix.yml)
[![debian](https://github.com/bstar/starwire/actions/workflows/debian.yml/badge.svg?branch=main)](https://github.com/bstar/starwire/actions/workflows/debian.yml)
[![arch](https://github.com/bstar/starwire/actions/workflows/arch.yml/badge.svg?branch=main)](https://github.com/bstar/starwire/actions/workflows/arch.yml)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A terminal news reader in the STAR family. Most readers hand you a list of
titles and then hand the article to a browser. This one pulls the whole piece
out of the page and wraps it as clean markdown in the terminal, the way
Instapaper did — and YouTube channels are feeds too.

<!-- The screenshot goes here, as docs/screenshot.png, the way STAR/AMP's
     README carries one. There is no picture yet: the window is still being
     built, and one of a half-drawn window would be retaken every week. -->

[Status](docs/status.md) says how much of that is built today, area by area.

Read the whole thing.

## Get it

Packages for Linux are on the
[releases page](https://github.com/bstar/starwire/releases/latest): an AppImage
that needs nothing installed, a `.deb` for Debian and Ubuntu, a portable
tarball, and the source the Arch `PKGBUILD` builds.

```sh
nix run github:bstar/starwire        # Nix, on Linux or Apple Silicon macOS
```

[Installing](docs/installing.md) covers every route, including building from
source. There are no system libraries to install first.

## Try it

```sh
starwire import newsboat --dry-run   # what it would make of your feed list
starwire import newsboat             # bring it in, read state and all
starwire fetch                       # pull everything, once
starwire list --unread
starwire show 42
```

And the probe, which needs no feeds and writes nothing — what a site yields to
the scraper, before subscribing to it:

```sh
starwire extract https://www.phoronix.com/news/Linux-6.19-Features
```

[On the command line](docs/cli.md) has the rest of it.

## What it does

- **The whole article, not the summary.** The page behind an entry is fetched
  and reduced to markdown: headings, lists, quotes, code blocks with their
  language, and numbered links. Extraction happens in the background as items
  arrive, so opening one is instant.
- **Feeds from wherever they are now.** `starwire import newsboat` takes the
  urls file and the read state; `import opml` and `export opml` move a list
  between any two readers.
- **YouTube channels as feeds.** Paste a channel URL, a video URL, an
  `@handle` or a bare id; import a Google Takeout `subscriptions.csv`; or pull
  the account's real subscriptions through `yt-dlp`. A video opens in `mpv`.
- **One file, and one folder.** Everything lives under `~/.local/starwire`:
  the config, the database, the log. `starwire fetch` from a systemd timer
  keeps it current while the reader is closed.
- **Headless all the way down.** Nothing in the core knows a terminal exists,
  which is why every one of those commands works in a pipe or a cron job.

## What it does not do yet

**The window.** The column of docked modules, the stack, the reader and the
sixteen themes are designed and are not built: `starwire` with no arguments
says so and exits. Everything above works today from the command line.

Beyond that, and beyond this milestone: pictures inside articles,
find-in-article, per-feed refresh intervals, folders inferred from a newsboat
file's comment headers, podcasts and enclosures.

## What it will not do

Not every page yields. A paywall, a login wall or a page that is mostly
JavaScript will not extract, and when that happens the feed's own text is
shown with a note and `o` opens the page in a browser. Reddit and Hacker News
comment threads are never scraped — the feed carries the post, and the link is
a discussion rather than an article.

YouTube plays through `mpv` and resolves through `yt-dlp`. Both are yours to
install and yours to keep working; STAR/WIRE only hands them a URL.

## Read more

The [documentation](docs/README.md) has a page for each of those. The ones
most people want first:

- [Reading](docs/reading.md): what is scraped, what is not, and why
- [YouTube](docs/youtube.md)
- [On the command line](docs/cli.md)
- [Configuration](docs/configuration.md)
- [If something is wrong](docs/troubleshooting.md)

[CONTRIBUTING.md](CONTRIBUTING.md) is for building and changing it,
[CHANGELOG.md](CHANGELOG.md) for what each release holds, and
[SECURITY.md](SECURITY.md) for reporting a vulnerability.

## License

MIT. See [LICENSE](LICENSE).
