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

The one table STAR/WIRE adds. It is optional, and usually absent: none of the
sixteen shared files says anything about a link in an article or a star on an
entry, and none of them should have to — a theme is a palette. So the `[wire]`
roles are *derived* from that palette, one rule per role, read from a base16
slot where one applies and held to a contrast floor afterwards.

State one when the derivation gets it wrong for your palette:

```toml
[wire]
link_fg = "#89b4fa"
star_fg = "#f5c518"
```

A role you state is used exactly as written — the contrast floor applies only
to derived colours, because a derivation is a way of not writing ten colours,
not a committee sitting over the ones somebody did write. A `[wire]` table
that does not parse costs the table and nothing else: the theme still loads,
with every role derived.

| Role | Is | Derived from |
| --- | --- | --- |
| `link_fg` | a link in an article, underlined and numbered | base16 `base0D` (blue) |
| `heading_fg` | every heading; `#` and `##` are bold as well | the theme's accent colour |
| `code_bg` | behind inline code and every row of a fenced block | the panel background mixed 10% toward the foreground |
| `quote_fg` | a block quote's `▎` gutter and its text | the theme's dim colour |
| `unread_fg` | the `●` on an unread entry, and its title | the theme's foreground |
| `star_fg` | the `★` on a starred entry | base16 `base0A` (yellow) |
| `video_fg` | the `▶` on a video | base16 `base0E` (magenta) |
| `byline_fg` | `author · site · date · N min` under a title | the theme's row-metadata colour |
| `rule_fg` | a thematic break, drawn as a row of `─` | the theme's divider colour |
| `image_fg` | `[image: alt]`, where a picture cannot be drawn | base16 `base0C` (cyan) |

Every built-in is checked against WCAG AA in the test suite. Anything carrying
words clears 4.5:1 against what it is drawn on; a role that carries no letters
of its own — the star, the video arrow, the rule — clears 3:1. `code_bg` is
the odd one: it is a background, so what is checked is the code *on* it, which
is why it is pushed away from the row colour rather than away from the panel.

`image_fg` is for the line that stands in for a picture: one that will never
arrive, one whose address cannot be fetched, and every picture at all when
`[articles] images` is off or `[ui] graphics = "off"`. A picture that is drawn
carries no theme colour, being a picture. See [Reading](reading.md).
