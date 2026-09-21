//! The window.
//!
//! **A stub.** The column, the stack, the reader and everything else the
//! design calls for land in a later work package; what is here is enough for
//! `starwire` with no arguments to say so and exit cleanly rather than to
//! panic or to print nothing.
//!
//! When it does arrive, everything under here reaches `ratatui`,
//! `crossterm`, `ratatui-image` and `image` through `starkit::` and never as
//! a direct dependency -- see `AGENTS.md` -- and it talks to the core only
//! through `wire::Handle` and the read side of `wire::State`. Nothing in
//! `src/wire/` will know it exists; there is a test in `wire/mod.rs` that
//! makes sure of it.

use std::io::Write as _;
use std::path::Path;

use anyhow::Result;

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
