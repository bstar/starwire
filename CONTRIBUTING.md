# Contributing

Thanks for looking. This is a small project with a maintainer who works on it
in the evenings, so the most useful thing you can do before writing code is
open an issue and say what you have in mind.

## Getting it to build

The one-command path:

```sh
nix develop
cargo build
```

Without Nix you need a Rust toolchain, 1.90 or newer, and nothing else. There
are no system libraries and no `-sys` crates: SQLite is the amalgamation
`rusqlite` compiles from source, TLS is rustls through `ureq`, and nothing
runs bindgen. If a change adds a system dependency it has to add it to
`flake.nix` and to CI in the same commit, and say why in the dependency's
comment.

`mpv` and `yt-dlp` are not build dependencies. They are the reader's own
programs, looked up on `PATH` when a video is played or a handle resolved,
and everything else works without them.

## What CI will run

```sh
./scripts/check-version.sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings -A dead_code
cargo test --all
cargo deny check
```

`dead_code` is allowed because the core lands milestone by milestone, complete
and tested, ahead of the UI that will reach it. Every other lint is an error.

Tests that reach the real network are gated behind `STARWIRE_TEST_NET=1` and
skip cleanly when it is unset; the `yt-dlp` ones also want the binary on
`PATH`. Everything else runs against `testdata/replay`, so `cargo test` works
on a machine with no network at all.

All of it in one line before you push:

```sh
CARGO_NET_GIT_FETCH_WITH_CLI=true nix develop -c sh -c './scripts/check-version.sh \
  && cargo fmt --check \
  && cargo clippy --all-targets -- -D warnings -A dead_code \
  && cargo test --all \
  && cargo deny check'
```

If you touched a workflow, `nix shell nixpkgs#actionlint -c actionlint
.github/workflows/*.yml` as well. Its shellcheck pass catches the `run:` blocks
nothing else reads.

## Four rules that are not visible from the type system

**Nothing under `src/wire/` knows the terminal exists.** No `ratatui`, no
`crossterm`, no `crate::ui`. A test in `wire/mod.rs` greps the module's own
sources and fails if any of those appear. It is the reason `starwire fetch`
can run from a systemd timer with no TTY, the reason the core can be driven by
tests with no window involved, and the reason a rendering change cannot break
how a page is read.

**`src/wire/db/` is the only thing that writes.** One connection, owned by one
thread — the `starwire-db` thread when the window is up, the calling thread
when a headless subcommand is running. The single-writer rule is kept by there
being one `Db`, not by a mutex.

**Every URL entering the program goes through `wire::youtube::canonicalise`.**
It gives a bare host a scheme, rewrites `http` to `https`, and normalises a
YouTube channel to its `videos.xml` feed. A canonical URL canonicalises to
itself — there is a property test — and that is the whole reason importing a
list twice does not double it.

**Not everything with a link is fetched.** A video, a Reddit post and a Hacker
News item whose link goes back into the comment thread all use the text the
feed carried. There is no `robots.txt` request; the politeness is structural,
and `src/wire/extract/mod.rs` lists every part of it. A change that makes this
program fetch more pages, or fetch them more often, needs a reason in the same
commit.

## Packaging

Release targets are Linux Nix, Linux AppImage (x86_64), and a native macOS
Apple Silicon archive. `scripts/build-dist.sh nix` builds the Nix package;
`./scripts/build-dist.sh appimage` uses Docker or Podman with an isolated
old-glibc build directory; `./scripts/build-dist.sh macos` runs on a Mac.
Debian, Arch and standalone Linux tarballs are no longer release targets.

## Taking a new STAR/KIT

Its own commit, "Take STAR/KIT 0.Y", and nothing else in it:

1. Change the `tag` and the `version` requirement on the `starkit` dependency
   in `Cargo.toml`.
2. `cargo update -p starkit` so `Cargo.lock` records the new revision.
3. Run the checks above. STAR/KIT's own CI has a job that builds its
   consumers against the tip of the library, so a break should have been
   caught there first, but the version this repository actually pins is the
   one that matters.

Keeping it separate is the point: what arrived with the new version is then one
diff to read rather than a line buried in a feature commit. STAR/KIT is 0.x, so
a minor bump may change an API and a patch bump may not.

To work on STAR/KIT and this at the same time, check it out beside this
repository and point the build at it with an untracked `.cargo/config.toml`:

```toml
[patch."https://github.com/bstar/starkit"]
starkit = { path = "../starkit" }
```

It is in `.gitignore`, because a committed one points CI at a path that does
not exist. It also rewrites `Cargo.lock`, so do not use it and `nix build` in
the same tree. Delete it once the change is tagged and this repository has
taken the new tag.

## Releasing

1. Bump `version` in `Cargo.toml` and refresh `Cargo.lock` through the Nix
   devshell; run `./scripts/check-version.sh` there too.
2. Add the release to `CHANGELOG.md` and run the checks above.
3. Commit, tag `vX.Y.Z`, and push. The release workflow validates Nix, builds
   the AppImage and macOS archive, and verifies the AppImage across distributions.
4. Review the draft release with its checksums and provenance before publishing.

A manual dispatch on `main` creates downloadable workflow artifacts without a
release or a tag. A tag dispatch creates the same draft as a tag push.

## Commit messages

Present tense, plain prose, no conventional-commits prefix, no trailers. What
the commit does and, where it is not obvious, why. The existing log is the
style guide.

## Comments

The codebase explains decisions rather than mechanics, and usually says what
was measured or observed. If you change something a comment justifies, change
the comment. If you leave a comment that says something is a certain way for a
reason, make sure the reason is true.
