//! Handing a link to a browser or a player.
//!
//! Runs the configured argv, or `open` on macOS and `xdg-open` elsewhere,
//! with every fd on `/dev/null` and the child detached -- never through a
//! shell, so a URL with a semicolon or a space in it is a single argument
//! rather than a chance to inject one. A feed is somebody else's text, and
//! the URL in it is the most attacker-controlled string this program holds.
//!
//! The detachment matters more here than in STAR/FOLD. `mpv` on a YouTube
//! link runs for the length of the video; waiting on it would freeze the
//! reader for eleven minutes.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::PlayerConfig;

/// What a link is, and therefore what opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Browser(String),
    Video(String),
    /// A picture from inside an article.
    Picture(PictureSource),
}

/// Which of the two spellings of a picture is handed over.
///
/// The file where the bytes are already cached, because that is what makes
/// an image viewer open rather than a browser -- `xdg-open` on an `https`
/// URL is a browser whatever the URL points at, and a browser opening one
/// picture is a second fetch and a window somebody has to close. The URL is
/// the fallback for a picture that has not arrived yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PictureSource {
    File(PathBuf),
    Url(String),
}

impl Target {
    /// The URL this target names, where it names one. `None` for a picture
    /// already on disk, which is a path and not a link.
    pub fn url(&self) -> Option<&str> {
        match self {
            Target::Browser(u) | Target::Video(u) => Some(u),
            Target::Picture(PictureSource::Url(u)) => Some(u),
            Target::Picture(PictureSource::File(_)) => None,
        }
    }

    /// The single argument the opener is given.
    ///
    /// An `OsString` rather than a `String` because a cached picture is a
    /// path, and a path on this machine is not required to be UTF-8. It is
    /// always exactly one argument -- never appended to another, never
    /// split -- whatever is in it.
    pub fn argument(&self) -> OsString {
        match self {
            Target::Browser(u) | Target::Video(u) => OsString::from(u),
            Target::Picture(PictureSource::Url(u)) => OsString::from(u),
            Target::Picture(PictureSource::File(path)) => path.clone().into_os_string(),
        }
    }
}

/// What kind of thing a URL is, decided from the URL alone.
///
/// Pure and table-driven so the UI can call it while deciding what a click
/// on a link inside an article should do, without asking the core anything.
/// Deliberately narrow: a YouTube watch link, a `youtu.be` link and a bare
/// video file. Everything else is a page, because guessing wrong in the
/// other direction means handing a news article to a video player.
pub fn kind_of(url: &str) -> Target {
    let Ok(parsed) = url::Url::parse(url) else {
        return Target::Browser(url.to_string());
    };
    if super::youtube::video_id(&parsed).is_some() {
        return Target::Video(url.to_string());
    }
    let path = parsed.path().to_ascii_lowercase();
    if [".mp4", ".webm", ".mkv", ".m4v"]
        .iter()
        .any(|ext| path.ends_with(ext))
    {
        return Target::Video(url.to_string());
    }
    Target::Browser(url.to_string())
}

/// The argv `open` would run, without running it -- so the choice of command
/// can be tested without ever spawning a process.
pub fn argv(target: &Target, cfg: &PlayerConfig) -> Vec<OsString> {
    let configured = match target {
        Target::Browser(_) => &cfg.browser,
        Target::Video(_) => &cfg.video,
        Target::Picture(_) => &cfg.image,
    };
    let mut out: Vec<OsString> = if configured.is_empty() {
        vec![OsString::from(default_opener())]
    } else {
        configured.iter().map(OsString::from).collect()
    };
    // What is opened is always exactly one more argument, whatever it
    // contains -- never appended to an existing argument, never split.
    out.push(target.argument());
    out
}

#[cfg(target_os = "macos")]
fn default_opener() -> &'static str {
    "open"
}

#[cfg(not(target_os = "macos"))]
fn default_opener() -> &'static str {
    "xdg-open"
}

/// Spawn whatever `argv` names, and forget about it.
///
/// The child is never waited on: a browser or a player is meant to outlive
/// the moment it was asked for. Stdin, stdout and stderr are all nulled so
/// the child cannot read this program's keystrokes or write into the
/// alternate screen -- which `mpv` without `--terminal=no` will happily do.
pub fn open(target: &Target, cfg: &PlayerConfig) -> std::io::Result<()> {
    let mut args = argv(target, cfg);
    // `args` is never empty: `argv` always pushes at least the default
    // opener or the first configured word, then the URL.
    let program = args.remove(0);

    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Dropped rather than waited on: this is the detach.
    drop(child);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_link_is_a_video_and_a_page_is_a_page() {
        assert!(matches!(
            kind_of("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Target::Video(_)
        ));
        assert!(matches!(
            kind_of("https://youtu.be/dQw4w9WgXcQ"),
            Target::Video(_)
        ));
        assert!(matches!(
            kind_of("https://example.org/clip.mp4"),
            Target::Video(_)
        ));
        assert!(matches!(
            kind_of("https://example.org/posts/one"),
            Target::Browser(_)
        ));
        assert!(
            matches!(
                kind_of("https://example.org/mp4-considered-harmful"),
                Target::Browser(_)
            ),
            "the extension has to be the extension"
        );
    }

    #[test]
    fn something_that_is_not_a_url_is_still_handed_to_the_browser() {
        // Better a browser that says "cannot open" than this program
        // deciding on the reader's behalf that a link is unopenable.
        assert_eq!(kind_of("not a url"), Target::Browser("not a url".into()));
    }

    #[test]
    fn a_video_goes_to_the_player_and_a_page_to_the_browser() {
        let cfg = PlayerConfig::default();
        let video = argv(
            &Target::Video("https://www.youtube.com/watch?v=x".into()),
            &cfg,
        );
        assert_eq!(
            video,
            vec![
                OsString::from("mpv"),
                OsString::from("--terminal=no"),
                OsString::from("--"),
                OsString::from("https://www.youtube.com/watch?v=x"),
            ]
        );
        let page = argv(&Target::Browser("https://e.org/a".into()), &cfg);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0], OsString::from(default_opener()));
    }

    #[test]
    fn a_configured_browser_wins_over_the_platform_default() {
        let cfg = PlayerConfig {
            browser: vec!["firefox".into(), "--new-tab".into()],
            ..PlayerConfig::default()
        };
        let got = argv(&Target::Browser("https://e.org/a".into()), &cfg);
        assert_eq!(
            got,
            vec![
                OsString::from("firefox"),
                OsString::from("--new-tab"),
                OsString::from("https://e.org/a"),
            ]
        );
    }

    #[test]
    fn a_hostile_url_stays_one_argument() {
        let cfg = PlayerConfig::default();
        let nasty = "https://e.org/a; rm -rf ~ #' \"$(id)\"";
        let got = argv(&Target::Browser(nasty.into()), &cfg);
        assert_eq!(got.len(), 2, "the URL was split: {got:?}");
        assert_eq!(got[1], OsString::from(nasty));
    }

    /// A picture goes to the image opener, and the cached file is what it is
    /// given: `xdg-open` on an `https` URL is a browser however the URL
    /// ends, and a browser is not what somebody clicking a picture asked
    /// for.
    #[test]
    fn a_picture_is_opened_on_the_file_where_there_is_one() {
        let cfg = PlayerConfig::default();
        let cached = Target::Picture(PictureSource::File(PathBuf::from(
            "/home/somebody/.local/starwire/cache/pictures/abc.png",
        )));
        let got = argv(&cached, &cfg);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], OsString::from(default_opener()));
        assert_eq!(
            got[1],
            OsString::from("/home/somebody/.local/starwire/cache/pictures/abc.png")
        );
        assert_eq!(cached.url(), None, "a path is not a link");

        // Nothing on disk yet: the URL, which is still one argument.
        let remote = Target::Picture(PictureSource::Url("https://e.org/a.png".into()));
        assert_eq!(remote.url(), Some("https://e.org/a.png"));
        assert_eq!(
            argv(&remote, &cfg),
            vec![
                OsString::from(default_opener()),
                OsString::from("https://e.org/a.png")
            ]
        );

        // And `[player] image` wins over the desktop's own opener.
        let cfg = PlayerConfig {
            image: vec!["feh".into(), "--".into()],
            ..PlayerConfig::default()
        };
        assert_eq!(
            argv(&remote, &cfg),
            vec![
                OsString::from("feh"),
                OsString::from("--"),
                OsString::from("https://e.org/a.png"),
            ]
        );
    }

    /// A path with a newline, a quotation mark or a semicolon in it is
    /// somebody else's filename, and it stays one argument.
    #[test]
    fn a_hostile_path_stays_one_argument() {
        let cfg = PlayerConfig::default();
        let nasty = PathBuf::from("/tmp/a b; rm -rf ~\n\"$(id)\".png");
        let got = argv(&Target::Picture(PictureSource::File(nasty.clone())), &cfg);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[1], nasty.into_os_string());
    }

    #[test]
    fn the_platform_default_is_open_on_macos_and_xdg_open_elsewhere() {
        #[cfg(target_os = "macos")]
        assert_eq!(default_opener(), "open");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(default_opener(), "xdg-open");
    }

    proptest::proptest! {
        /// A URL out of a feed, whatever it is. The one thing that must hold
        /// is that it stays one argument.
        #[test]
        fn a_url_is_always_exactly_one_argument(s: String) {
            let cfg = PlayerConfig::default();
            let target = kind_of(&s);
            let got = argv(&target, &cfg);
            proptest::prop_assert_eq!(got.last(), Some(&OsString::from(s.as_str())));
        }
    }
}
