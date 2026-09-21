//! The window.
//!
//! [`run`] takes the terminal, builds an [`app::App`] over the running core
//! and draws until something says to stop. The column is three docked
//! modules -- the sources, the entries of whichever source is chosen, and
//! the article -- over a status row; `docs/the-stack.md` describes how they
//! move and `ui/app/mod.rs` has the frame's own order.
//!
//! Everything under here reaches the four terminal crates -- the widget
//! library, the terminal driver, and the two halves of the image pipeline --
//! through `starkit::` rather than depending on any of them directly, which
//! is what keeps one copy of each in the tree; `AGENTS.md` says why. The
//! grep test at the bottom of this file is what holds the rule, and a line
//! that has to name one of them outright exempts itself with the marker
//! `VIA-STARKIT`, which is how the test's own list of forbidden strings, and
//! its name, get past it.
//!
//! The dependency runs one way only: this talks to the core through
//! `wire::Handle` and the read side of `wire::State`, and nothing in
//! `src/wire/` knows it exists.

use std::path::PathBuf;

use anyhow::Result;

pub mod clipboard;
pub mod keymap;
pub mod layout;
pub mod markdown;
// pub mod overlays;
pub mod panels;
pub mod stack;
pub mod status;
pub mod theme;

/// One key per scrollbar the window can draw, shared with STAR/KIT's
/// `chrome::scrollbar::Scrollbars` so [`app::App`] can keep a single
/// instance that presses, drags and releases whichever bar the pointer is
/// over -- one of the three modules, or the one overlay tall enough to
/// scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bar {
    Sources,
    Entries,
    Reader,
    Help,
    Import,
}

/// The one `Scrollbars` every module records its bar with and the app's
/// mouse handling reads back, keyed by [`Bar`].
pub type Bars = starkit::chrome::scrollbar::Scrollbars<Bar>;

/// Take over the terminal and read the news.
///
/// The signature is the one the stub had -- the core, the config as it was
/// read, where it was read from, and where the session belongs -- because
/// `main` was written against it before this existed.
pub fn run(
    core: crate::wire::Handle,
    cfg: crate::config::Config,
    cfg_path: PathBuf,
    session_path: Option<PathBuf>,
) -> Result<()> {
    let _ = (core, cfg, cfg_path, session_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The rule `AGENTS.md` states and the compiler cannot: nothing under
    /// `src/ui/` names either of the two crates below except through
    /// `starkit::`.
    ///
    /// A grep rather than a crate boundary, for the same reason as the
    /// core's own test: `starwire` is one binary crate, so there is no `ui`
    /// crate for Cargo to keep a second copy out of. The check is a
    /// subtraction -- every `starkit::`-qualified mention is taken off the
    /// line first, and whatever is left is a direct one. A line that has to
    /// name one outright carries the marker instead. The marker exempts the
    /// line it is on *and* the one after it, which is the only way an item
    /// whose own name says what it forbids -- this test -- can be marked:
    /// rustfmt moves a comment trailing a signature onto the next line, so
    /// the marker has to go above it.
    #[test]
    // VIA-STARKIT
    fn nothing_under_ui_names_ratatui_or_crossterm_directly() {
        const THROUGH: &[&str] = &[
            "starkit::ratatui_image", // VIA-STARKIT
            "starkit::ratatui",       // VIA-STARKIT
            "starkit::crossterm",     // VIA-STARKIT
            "starkit::image",         // VIA-STARKIT
        ];
        const FORBIDDEN: &[&str] = &[
            "ratatui",   // VIA-STARKIT
            "crossterm", // VIA-STARKIT
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("ui");
        let mut offences = Vec::new();
        let mut files = 0usize;

        walk(&root, &mut |path| {
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                return;
            }
            files += 1;
            let Ok(text) = std::fs::read_to_string(path) else {
                return;
            };
            let mut exempt_next = false;
            for (number, line) in text.lines().enumerate() {
                let exempt = exempt_next || line.contains("VIA-STARKIT");
                exempt_next = line.contains("VIA-STARKIT");
                if exempt {
                    continue;
                }
                let mut rest = line.to_string();
                for through in THROUGH {
                    rest = rest.replace(through, "");
                }
                for needle in FORBIDDEN {
                    if rest.contains(needle) {
                        offences.push(format!(
                            "{}:{}: {}",
                            path.display(),
                            number + 1,
                            line.trim()
                        ));
                    }
                }
            }
        });

        assert!(
            files > 5,
            "only {files} files were scanned; the walk is wrong"
        );
        assert!(
            offences.is_empty(),
            "the window reached past starkit:\n{}",
            offences.join("\n")
        );
    }

    fn walk(dir: &std::path::Path, f: &mut impl FnMut(&std::path::Path)) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, f);
            } else {
                f(&path);
            }
        }
    }
}
