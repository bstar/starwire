//! The system clipboard, going one way.
//!
//! STAR/CORD's `ui/clipboard.rs` with the picture half taken out: a news
//! reader copies a link and nothing else, so this is three lines and one
//! dependency feature rather than an encoder.
//!
//! ## Why a fresh connection every time
//!
//! Under Wayland the clipboard is owned by a live connection, and holding
//! one open for the life of the program means holding a socket for a feature
//! used a few times an hour. Under X11 `wayland-data-control` is not in play
//! at all. Failure is a note rather than an error either way: a terminal
//! with no clipboard at the other end -- over ssh, in a bare tty, on a
//! headless box running this from a timer -- is a perfectly ordinary place
//! to be reading the news.

/// Put text on the clipboard.
///
/// The error is a `String` because the only thing done with it is put it in
/// the status line: there is no failure here a caller can tell apart from
/// another, and nothing it could do differently if it could.
pub fn copy(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text.to_string()))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// There is no clipboard in a test runner and there is not meant to be.
    /// What this asserts is the contract the status line depends on: a
    /// missing clipboard is an error with something to say, never a panic
    /// and never a hang.
    #[test]
    fn copying_with_no_clipboard_is_an_error_rather_than_a_panic() {
        match copy("https://example.org/") {
            Ok(()) => {}
            Err(reason) => assert!(!reason.is_empty(), "a failure with nothing to say"),
        }
    }
}
