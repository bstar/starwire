//! The window.
//!
//! **Half a stub.** The foundations are here -- the stack, the layout
//! arithmetic, the key table, the reader's markdown pipeline and the theme
//! roles -- and the application that draws with them lands in a later work
//! package; [`run`] still says so and exits cleanly.
//!
//! Everything under here reaches ratatui, crossterm, ratatui-image and image
//! through `starkit::` rather than depending on any of them directly, which
//! is what keeps one copy of each in the tree -- see `AGENTS.md`. There is a
//! grep test at the bottom of this file that says so, and a line that has to
//! mention one of the four in prose exempts itself with the marker
//! `VIA-STARKIT`, which is how this paragraph and the test's own list of
//! forbidden strings get past it. VIA-STARKIT
//!
//! The dependency runs one way only: this talks to the core through
//! `wire::Handle` and the read side of `wire::State`, and nothing in
//! `src/wire/` knows it exists.

use std::io::Write as _;
use std::path::Path;

use anyhow::Result;

pub mod keymap;

/// Say what is and is not here, and exit 0.
///
/// Exit 0 rather than 1 deliberately: nothing has gone wrong. The program is
/// installed, its config and its database are where they should be, and the
/// headless half of it works. A non-zero exit would tell a packaging test
/// that the build is broken when it is not.
pub fn run(cfg: &crate::config::Config, db_path: &Path, feed_count: usize) -> Result<()> {
    let _ = cfg;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "STAR/WIRE {}: the TUI lands in a later package. \
         {feed_count} feed{} in {}; `starwire --help` has the rest.",
        env!("CARGO_PKG_VERSION"),
        if feed_count == 1 { "" } else { "s" },
        db_path.display()
    );
    Ok(())
}
