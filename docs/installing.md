# Installing

There are no system libraries to install first. SQLite is compiled from the
amalgamation the build carries, TLS is rustls, and nothing runs bindgen.

Two programs are worth having but are **not** required: `mpv`, to play a video
entry, and `yt-dlp`, to resolve a YouTube handle and to read an account's
subscriptions. Everything else works without either.

## Nix

```sh
nix run github:bstar/starwire
nix profile install github:bstar/starwire
```

The flake also exports a home-manager module, so STAR/WIRE can be installed
and configured declaratively the way the rest of a NixOS setup is:

```nix
programs.starwire = {
  enable = true;
  theme = "catppuccin-mocha";          # or stylix.enable = true;
  settings.fetch.refresh_minutes = 30;
};
```

`settings` is merged table by table into `~/.local/starwire/config.toml`, so
setting one key in `[fetch]` leaves the rest of `[fetch]` alone.

## A release build

Packages are on the
[releases page](https://github.com/bstar/starwire/releases/latest). Every file
there is built by CI, hashed into `SHA256SUMS` over the bytes actually
uploaded, and attested to this repository and the commit it was built from.

| File | What it is |
| --- | --- |
| `starwire-<version>-x86_64.AppImage` | one file, nothing installed first. `chmod +x` it and run it. |
| `starwire_<version>-1~<release>_amd64.deb` | one per Debian generation — `bookworm`, `trixie`, `ubuntu24.04` — because they differ only in the glibc they were built against. |
| `starwire-<version>-x86_64-linux-gnu.tar.gz` | a portable build for any other distribution, built against the oldest glibc still worth supporting. |
| `starwire-<version>.tar.gz` | the source. This is what `packaging/PKGBUILD` builds from; the Arch package is built and tested in CI but not published, so on Arch you build it yourself. |

## From source

```sh
cargo install --git https://github.com/bstar/starwire
```

or, to work on it:

```sh
git clone https://github.com/bstar/starwire
cd starwire
cargo build --release
```

Rust 1.90 or newer. STAR/KIT is a git dependency, so the first build needs
network access; `CARGO_NET_GIT_FETCH_WITH_CLI=true` is what makes cargo fetch
it with the `git` on your `PATH`, which matters if you rewrite GitHub URLs to
ssh.

## Where it puts things

Everything lives under one directory, `~/.local/starwire`, rather than spread
across the XDG roots — so it can be backed up, moved between machines or
deleted by moving one folder:

| Path | What |
| --- | --- |
| `config.toml` | the settings |
| `data/wire.db` | the feeds, the entries and every article ever extracted |
| `session.toml` | where the reader was when it was last closed |
| `cache/` | the log |

`$STARWIRE_DIR` moves all of it. `$STARWIRE_CONFIG_DIR` moves only the
configuration, which is what a dotfile manager wants.

## Keeping it current while it is closed

`starwire fetch` is meant for a timer. A systemd user unit:

```ini
# ~/.config/systemd/user/starwire-fetch.service
[Unit]
Description=Refresh STAR/WIRE feeds

[Service]
Type=oneshot
ExecStart=%h/.nix-profile/bin/starwire fetch
```

```ini
# ~/.config/systemd/user/starwire-fetch.timer
[Unit]
Description=Refresh STAR/WIRE feeds every 30 minutes

[Timer]
OnBootSec=5min
OnUnitActiveSec=30min

[Install]
WantedBy=timers.target
```

```sh
systemctl --user enable --now starwire-fetch.timer
```

It is safe to run while the reader is open: the database is WAL, the upserts
are idempotent, and the reader notices the new rows.
