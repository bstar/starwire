//! STAR/WIRE's theme: the shared one, plus the roles a reader needs.
//!
//! The sixteen built-in theme files are STAR/KIT's and are shared with
//! STAR/AMP, STAR/CORD and STAR/FOLD. None of them says anything about a
//! link in an article or a star on an entry, and they should not have to: a
//! theme is eight colours and a base16 scheme, and everything else is
//! derived from those. So `[wire]` is derived here, out of the same palette
//! the core resolved, and a file that *does* state a `[wire]` table has the
//! last word.
//!
//! Derivation rather than a table of literals is what keeps a theme honest:
//! one rule per role, run over sixteen palettes and asserted legible by a
//! test, is a hundred and sixty colours that are all correct. A failure in
//! that test is fixed in [`Wire::derive`], never by special-casing the theme
//! that tripped it.

use std::ops::Deref;

use serde::{Deserialize, Serialize};
use starkit::theme::color::Rgb;
use starkit::theme::{pick, Registry, Resolve, Theme as Core, ThemeFile};

/// The `[wire]` table, as a theme file may state it.
///
/// Every field optional: a theme states the two it cares about and lets the
/// rest fall out of its palette.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireColors {
    pub link_fg: Option<Rgb>,
    pub heading_fg: Option<Rgb>,
    pub code_bg: Option<Rgb>,
    pub quote_fg: Option<Rgb>,
    pub unread_fg: Option<Rgb>,
    pub star_fg: Option<Rgb>,
    pub video_fg: Option<Rgb>,
    pub byline_fg: Option<Rgb>,
    pub rule_fg: Option<Rgb>,
    pub image_fg: Option<Rgb>,
}

/// The same roles, resolved.
#[derive(Debug, Clone)]
pub struct Wire {
    /// A link in an article, drawn underlined and followed by its number.
    pub link_fg: Rgb,
    /// Every heading in an article; H1 and H2 are also bold.
    pub heading_fg: Rgb,
    /// Behind inline code and behind every row of a fenced block. A
    /// background, not a foreground: what is drawn on it is `row_fg`.
    pub code_bg: Rgb,
    /// A block quote's `▎` gutter and its text.
    pub quote_fg: Rgb,
    /// The `●` on an unread entry, and its title.
    pub unread_fg: Rgb,
    /// The `★` on a starred entry.
    pub star_fg: Rgb,
    /// The `▶` on a video.
    pub video_fg: Rgb,
    /// `author · site · date · N min` under an article's title.
    pub byline_fg: Rgb,
    /// A thematic break, drawn as a row of `─`.
    pub rule_fg: Rgb,
    /// `[image: alt]`, which is what a picture is in 0.0.1 -- see
    /// `docs/status.md`.
    pub image_fg: Rgb,
}

/// WCAG AA for normal text. Anything carrying words clears this against
/// whatever it is drawn on.
const TEXT_CONTRAST: f64 = 4.5;

/// WCAG AA for a graphical mark: a star, a video arrow, a rule. These are
/// glyphs rather than prose, and holding them to the text floor would drain
/// the colour out of exactly the marks that are meant to catch the eye.
const MARK_CONTRAST: f64 = 3.0;

/// How far the inline-code background is pulled from the panel toward the
/// foreground. Low: the code still has to be read on it, and the floor
/// below is what makes sure it can be.
const CODE_TINT: f64 = 0.10;

/// What the file said, or what the palette implies held to a contrast floor.
///
/// The floor is only ever applied to a *derived* colour. A theme that names
/// a role names it.
fn stated_or(stated: Option<Rgb>, derived: Rgb, against: Rgb, target: f64) -> Rgb {
    stated.unwrap_or_else(|| derived.ensure_contrast(against, target))
}

impl Wire {
    fn derive(core: &Core, f: &WireColors, b16: Option<&starkit::theme::schema::Base16>) -> Self {
        let bg = core.panel_bg;

        // Every role names the base16 slot it comes from. The spec's own
        // meanings: 08 red, 09 orange, 0A yellow, 0B green, 0C cyan, 0D blue,
        // 0E magenta. A link is blue for the same reason a directory is in
        // STAR/FOLD -- every browser since 1993 has drawn it that way, and a
        // reader that chose otherwise would be arguing with a reflex.
        //
        // The code background is the one that is not a `pick`. It is a
        // background, so it is a mix of the panel toward the foreground
        // rather than a hue out of the scheme, and the floor it is held to
        // is `row_fg`'s legibility *on it* rather than its own against
        // anything -- `ensure_contrast` pushes it away from `row_fg` until
        // the code on it reads, which on a dark theme means darker and on a
        // light one means lighter, without either being spelled out here.
        let code_bg = stated_or(
            f.code_bg,
            bg.mix(core.fg, CODE_TINT),
            core.row_fg,
            TEXT_CONTRAST,
        );

        Self {
            link_fg: stated_or(
                f.link_fg,
                pick(None, b16.map(|b| b.base0D), core.accent),
                bg,
                TEXT_CONTRAST,
            ),
            heading_fg: stated_or(f.heading_fg, core.accent, bg, TEXT_CONTRAST),
            code_bg,
            quote_fg: stated_or(f.quote_fg, core.dim, bg, TEXT_CONTRAST),
            unread_fg: stated_or(f.unread_fg, core.fg, bg, TEXT_CONTRAST),
            star_fg: stated_or(
                f.star_fg,
                pick(None, b16.map(|b| b.base0A), core.warn),
                bg,
                MARK_CONTRAST,
            ),
            video_fg: stated_or(
                f.video_fg,
                pick(None, b16.map(|b| b.base0E), core.accent),
                bg,
                MARK_CONTRAST,
            ),
            byline_fg: stated_or(f.byline_fg, core.row_meta_fg, bg, TEXT_CONTRAST),
            rule_fg: stated_or(f.rule_fg, core.divider, bg, MARK_CONTRAST),
            image_fg: stated_or(
                f.image_fg,
                pick(None, b16.map(|b| b.base0C), core.dim),
                bg,
                TEXT_CONTRAST,
            ),
        }
    }

    /// The roles that carry words, and what each is drawn on. The legibility
    /// test walks this; naming it here is what stops a new role being added
    /// without one.
    #[cfg(test)]
    fn text_roles(&self, core: &Core) -> Vec<(&'static str, Rgb, Rgb)> {
        let bg = core.panel_bg;
        vec![
            ("link_fg", self.link_fg, bg),
            ("heading_fg", self.heading_fg, bg),
            // The code background's floor is the code on it, not the panel
            // behind it.
            ("row_fg on code_bg", core.row_fg, self.code_bg),
            ("quote_fg", self.quote_fg, bg),
            ("unread_fg", self.unread_fg, bg),
            ("byline_fg", self.byline_fg, bg),
            ("image_fg", self.image_fg, bg),
        ]
    }

    /// Roles that carry no letters: two glyphs and a rule.
    #[cfg(test)]
    fn mark_roles(&self, core: &Core) -> Vec<(&'static str, Rgb, Rgb)> {
        let bg = core.panel_bg;
        vec![
            ("star_fg", self.star_fg, bg),
            ("video_fg", self.video_fg, bg),
            ("rule_fg", self.rule_fg, bg),
        ]
    }
}

/// The theme STAR/WIRE draws with: the shared one, plus `[wire]`.
///
/// `Deref` rather than a hundred delegating accessors, so `theme.accent` and
/// `theme.wire.link_fg` read the same way and every STAR/KIT widget takes
/// `&*theme`.
#[derive(Debug, Clone)]
pub struct Theme {
    core: Core,
    pub wire: Wire,
}

impl Deref for Theme {
    type Target = Core;

    fn deref(&self) -> &Core {
        &self.core
    }
}

impl Resolve for Theme {
    fn resolve(file: &ThemeFile) -> Self {
        let core = Core::resolve(file);
        // A malformed `[wire]` table costs the table, not the theme: this is
        // decoration over a working palette, and a typo in one colour should
        // not leave the reader with no theme at all.
        let stated: WireColors = file.table("wire").unwrap_or_else(|e| {
            tracing::warn!("{}: the [wire] table was ignored: {e}", core.id);
            WireColors::default()
        });
        let wire = Wire::derive(&core, &stated, file.base16.as_ref());
        Self { core, wire }
    }

    fn core(&self) -> &Core {
        &self.core
    }
}

/// Where the themes come from: the same directory as the rest of
/// STAR/WIRE's files, spelled with STAR/KIT's `Paths` because that is what
/// [`Registry`] takes.
pub const THEME_PATHS: starkit::paths::Paths = crate::paths::PATHS;

pub fn registry() -> Registry<Theme> {
    Registry::new(THEME_PATHS)
}

/// A resolved built-in, for the tests in every other module that need
/// something to draw with.
#[cfg(test)]
pub mod tests_support {
    use super::{Resolve as _, Theme, ThemeFile};

    pub fn theme(id: &str) -> Theme {
        let builtin = starkit::theme::builtin::BUILTINS
            .iter()
            .find(|b| b.id == id)
            .unwrap_or_else(|| panic!("no built-in theme {id}"));
        Theme::resolve(&ThemeFile::parse(builtin.toml).expect("a built-in parses"))
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::theme;
    use super::*;
    use starkit::theme::builtin::BUILTINS;

    /// The test the whole derivation exists to pass.
    ///
    /// Sixteen palettes, ten roles, and nobody looking at any of them. A
    /// rule that produces an unreadable colour on one scheme in sixteen is
    /// the normal outcome of writing rules for colours, and this is what
    /// catches it.
    #[test]
    fn every_builtin_wire_role_is_legible() {
        for b in BUILTINS {
            assert_legible(b.id, &theme(b.id));
        }

        // And the desktop's own palette, where there is one -- the one theme
        // nobody here chose, and exactly the case a rule written against
        // sixteen known palettes can fail on. Skipped rather than faked
        // where no desktop theme is set, because a synthesised one would be
        // a seventeenth builtin with a misleading name.
        if let Some((file, _)) = starkit::theme::system::theme() {
            assert_legible("system", &Theme::resolve(&file));
        }
    }

    fn assert_legible(id: &str, t: &Theme) {
        assert_ne!(
            t.wire.code_bg, t.panel_bg,
            "{id}: a fenced block is invisible against the panel"
        );
        for (role, fg, bg) in t.wire.text_roles(&t.core) {
            let c = bg.contrast(fg);
            assert!(
                c >= TEXT_CONTRAST,
                "{id}: {role} is {c:.2}:1 against its background"
            );
        }
        for (role, fg, bg) in t.wire.mark_roles(&t.core) {
            let c = bg.contrast(fg);
            assert!(
                c >= MARK_CONTRAST,
                "{id}: {role} is {c:.2}:1 against its background"
            );
        }
    }

    /// A file that states a role gets that role, unchanged, whatever the
    /// derivation would have produced. Themes are allowed to be exact.
    #[test]
    fn a_stated_role_wins() {
        let f = ThemeFile::parse(
            r##"
            [meta]
            name = "Stated"
            variant = "dark"
            [app]
            bg = "#000000"
            fg = "#ffffff"
            [wire]
            link_fg = "#ff0000"
            code_bg = "#010203"
            "##,
        )
        .unwrap();
        let t = Theme::resolve(&f);
        assert_eq!(t.wire.link_fg, Rgb::new(0xff, 0, 0));
        assert_eq!(
            t.wire.code_bg,
            Rgb::new(0x01, 0x02, 0x03),
            "a stated background is not held to the floor either"
        );
    }

    /// A `[wire]` table that is not a `[wire]` table costs the table and
    /// nothing else. The core tables still fail loudly; this one is
    /// decoration over a working palette.
    #[test]
    fn a_malformed_wire_table_does_not_lose_the_theme() {
        let f = ThemeFile::parse(
            r##"
            [meta]
            name = "Broken"
            variant = "dark"
            [app]
            bg = "#101010"
            fg = "#e0e0e0"
            [wire]
            link_fg = "not a colour"
            "##,
        )
        .unwrap();
        let t = Theme::resolve(&f);
        assert_eq!(t.bg, Rgb::new(0x10, 0x10, 0x10));
        assert!(t.panel_bg.contrast(t.wire.link_fg) >= TEXT_CONTRAST);
    }

    /// `[wire]` is TOML written by a person, so every field it can state has
    /// to survive being read back exactly.
    #[test]
    fn the_wire_table_round_trips_through_serde() {
        let stated = WireColors {
            link_fg: Some(Rgb::new(0x11, 0x22, 0x33)),
            heading_fg: Some(Rgb::new(0x44, 0x55, 0x66)),
            code_bg: None,
            quote_fg: Some(Rgb::new(0x77, 0x88, 0x99)),
            unread_fg: None,
            star_fg: Some(Rgb::new(0xaa, 0xbb, 0xcc)),
            video_fg: None,
            byline_fg: Some(Rgb::new(0x01, 0x02, 0x03)),
            rule_fg: None,
            image_fg: Some(Rgb::new(0xff, 0x00, 0xff)),
        };
        let text = toml::to_string(&stated).expect("a wire table serialises");
        let back: WireColors = toml::from_str(&text).expect("it parses back");
        assert_eq!(back.link_fg, stated.link_fg);
        assert_eq!(back.heading_fg, stated.heading_fg);
        assert_eq!(back.code_bg, stated.code_bg);
        assert_eq!(back.quote_fg, stated.quote_fg);
        assert_eq!(back.unread_fg, stated.unread_fg);
        assert_eq!(back.star_fg, stated.star_fg);
        assert_eq!(back.video_fg, stated.video_fg);
        assert_eq!(back.byline_fg, stated.byline_fg);
        assert_eq!(back.rule_fg, stated.rule_fg);
        assert_eq!(back.image_fg, stated.image_fg);

        // And through a whole theme file: what a person wrote in `[wire]` is
        // what they get back, not a derivation over it.
        let file = ThemeFile::parse(
            r##"
            [meta]
            name = "Round Trip"
            variant = "light"
            [app]
            bg = "#fafafa"
            fg = "#202020"
            [wire]
            heading_fg = "#112233"
            "##,
        )
        .unwrap();
        let t = Theme::resolve(&file);
        assert_eq!(t.wire.heading_fg, Rgb::new(0x11, 0x22, 0x33));
    }
}
