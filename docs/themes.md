# Themes

STAR/WIRE draws with the same theme engine as the rest of the STAR family, so
a theme set in one looks the same in another. Sixteen are built in; a theme is
a small TOML file naming sixteen base16 colours plus whatever roles it wants
to state outright.

```toml
[ui]
theme = "catppuccin-mocha"   # or "system" to follow the desktop
```

`t` and `T` cycle through them while it is running, and the settings overlay
writes the choice back to `config.toml`.

Your own themes go in `~/.local/starwire/themes/`, one file each, and the file
name without its extension is the id. On NixOS the flake's home-manager module
can generate one from the active Stylix scheme:

```nix
programs.starwire.stylix.enable = true;
```

## The `[wire]` table

Every STAR application adds a *table* of its own colour roles to the shared
theme format — never a whole file. A theme that does not mention `[wire]` still
works: each role is derived from the base16 colours and checked for contrast
against the panel it is drawn on, and a stated colour always wins over a
derived one.

The roles STAR/WIRE adds are the ones a reader needs and a file manager does
not: a link, a heading, an inline code background, a quote, an unread marker, a
star, a video marker, a byline, a rule and an image placeholder.

**This table is not implemented yet.** It arrives with the window, and this
page will list each role, what it is derived from, what it is checked against
and what its contrast floor is, once there is something drawing with it. See
[Status](status.md).
