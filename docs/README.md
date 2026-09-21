# STAR/WIRE documentation

The [README](../README.md) is the short version. These pages are the long one,
grouped by what you are trying to do.

## Getting started

- [Installing](installing.md): Nix, AppImage, `.deb`, tarball, Arch, macOS, and
  from source.
- [Reading](reading.md): what extraction does, what is scraped and what is
  not, and what happens when a page will not give up its article.
- [The stack](the-stack.md): how the column of modules fits together.
- [Keys and mouse](keys-and-mouse.md): every binding and gesture, per module.
- [Configuration](configuration.md): every setting, and where the files live.
- [If something is wrong](troubleshooting.md): the first things to check, and
  where the logs are.

## Going further

- [YouTube](youtube.md): the three ways a channel gets in, and what `mpv` and
  `yt-dlp` are for.
- [On the command line](cli.md): every subcommand, all of them headless.
- [Themes](themes.md): the format, your own themes, and the `[wire]` roles.

## About the project

- [Status](status.md): what is done, what is in progress, and what is not
  started.
- [Contributing](../CONTRIBUTING.md), [Security](../SECURITY.md),
  [Changelog](../CHANGELOG.md).

## Documented in the source instead

Some things go stale the moment they move away from the code, so they stayed
there:

- `src/wire/mod.rs` explains why nothing in the core knows the terminal
  exists.
- `src/wire/db/schema.rs` explains what is kept and the three decisions in the
  schema worth defending.
- `src/wire/extract/mod.rs` lists exactly what is fetched, what is not, and
  the politeness that stands in for a `robots.txt` request.
- `AGENTS.md` keeps working notes for whoever is changing the code next,
  including an honest list of what has not been verified yet.
