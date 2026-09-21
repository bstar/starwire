//! End-to-end smoke tests, driving the built binary with no terminal
//! attached.
//!
//! `starwire` is a binary crate, so an integration test cannot reach the core
//! directly the way an in-crate test can; this runs the binary itself, the
//! same way a person or a systemd timer would. Nothing here needs a network:
//! every fetch goes through `--replay testdata/replay`, so these run on every
//! `cargo test`, including CI.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// `$STARWIRE_DIR` pointed at a fresh temporary directory, so a test run
/// never touches a real `~/.local/starwire` -- `main` calls
/// `PATHS.init_private_dirs()` before it even looks at the subcommand, and a
/// test must not create or write to a real reader's config, database or log.
fn command(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_starwire"));
    cmd.env("STARWIRE_DIR", home);
    cmd
}

fn testdata(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(relative)
}

fn run(home: &Path, args: &[&str]) -> Output {
    let output = command(home)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("running starwire {args:?}: {e}"));
    assert!(
        output.status.success(),
        "starwire {args:?} exited with {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn stdout(home: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&run(home, args).stdout).into_owned()
}

fn home() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary home")
}

#[test]
fn version_reports_the_crates_own_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_starwire"))
        .arg("--version")
        .output()
        .expect("running starwire --version");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected the crate version in: {stdout}"
    );
}

#[test]
fn a_feed_added_by_hand_turns_up_in_the_feed_list() {
    let home = home();
    let added = stdout(home.path(), &["add", "https://example.org/feed.xml"]);
    assert!(added.contains("added"), "{added}");

    let feeds = stdout(home.path(), &["list", "--feeds"]);
    assert!(feeds.contains("https://example.org/feed.xml"), "{feeds}");

    // Adding it again is not an error and does not double the list.
    let again = stdout(home.path(), &["add", "https://example.org/feed.xml"]);
    assert!(again.contains("already there"), "{again}");
    assert_eq!(stdout(home.path(), &["list", "--feeds"]).lines().count(), 1);
}

#[test]
fn a_youtube_channel_is_stored_under_its_canonical_feed_url() {
    let home = home();
    stdout(home.path(), &["add", "UCXuqSBlHAE6Xw-yeJA0Tunw"]);
    let feeds = stdout(home.path(), &["list", "--feeds"]);
    assert!(
        feeds.contains("videos.xml?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw"),
        "{feeds}"
    );
    let channels = stdout(home.path(), &["youtube", "list"]);
    assert!(channels.contains("UCXuqSBlHAE6Xw-yeJA0Tunw"), "{channels}");
}

#[test]
fn removing_a_feed_takes_it_off_the_list() {
    let home = home();
    stdout(home.path(), &["add", "https://example.org/feed.xml"]);
    let removed = stdout(home.path(), &["remove", "https://example.org/feed.xml"]);
    assert!(removed.contains("removed"), "{removed}");
    assert_eq!(stdout(home.path(), &["list", "--feeds"]).trim(), "");
}

#[test]
fn a_newsboat_urls_file_is_planned_without_writing_anything() {
    let home = home();
    let urls = testdata("import/urls");
    let out = stdout(
        home.path(),
        &[
            "import",
            "newsboat",
            "--urls",
            urls.to_str().unwrap(),
            "--dry-run",
        ],
    );

    assert!(
        out.contains("videos.xml?channel_id="),
        "the YouTube channels were not normalised: {out}"
    );
    assert!(
        !out.contains("scriptbarrel.com/xml.cgi"),
        "a scriptbarrel URL was left as it was: {out}"
    );
    assert!(
        out.contains("Kurzgesagt"),
        "the scriptbarrel `name=` titles were not picked up: {out}"
    );
    assert!(
        out.contains("https://www.reddit.com/"),
        "http was not rewritten to https, or old.reddit.com was left as it was: {out}"
    );
    assert!(
        out.contains("saved search"),
        "the query: line was not reported: {out}"
    );
    assert!(out.contains("nothing was written"), "{out}");

    assert_eq!(
        stdout(home.path(), &["list", "--feeds"]).trim(),
        "",
        "--dry-run wrote to the database"
    );
}

#[test]
fn a_newsboat_urls_file_imports_and_importing_it_twice_changes_nothing() {
    let home = home();
    let urls = testdata("import/urls");
    let args = ["import", "newsboat", "--urls", urls.to_str().unwrap()];

    let first = stdout(home.path(), &args);
    assert!(first.contains("added"), "{first}");
    let count = stdout(home.path(), &["list", "--feeds"]).lines().count();
    assert!(count >= 10, "{count} feeds imported");

    stdout(home.path(), &args);
    assert_eq!(
        stdout(home.path(), &["list", "--feeds"]).lines().count(),
        count,
        "a second import doubled the list"
    );
}

#[test]
fn a_feed_list_round_trips_through_opml() {
    let home = home();
    let blogroll = testdata("import/blogroll.opml");
    stdout(home.path(), &["import", "opml", blogroll.to_str().unwrap()]);
    let before: Vec<String> = stdout(home.path(), &["list", "--feeds"])
        .lines()
        .map(|l| l.split_whitespace().skip(2).collect::<Vec<_>>().join(" "))
        .collect();
    assert!(!before.is_empty());

    let exported = stdout(home.path(), &["export", "opml"]);
    assert!(exported.contains("<opml"), "{exported}");

    // Into a second, empty database, which is what round-tripping actually
    // means: what came out has to be enough to rebuild the list.
    let second = home_with_opml(&exported);
    let after: Vec<String> = stdout(second.0.path(), &["list", "--feeds"])
        .lines()
        .map(|l| l.split_whitespace().skip(2).collect::<Vec<_>>().join(" "))
        .collect();
    assert_eq!(before, after);
}

/// A fresh home with an OPML file imported into it.
fn home_with_opml(xml: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = home();
    let path = dir.path().join("exported.opml");
    std::fs::write(&path, xml).unwrap();
    stdout(dir.path(), &["import", "opml", path.to_str().unwrap()]);
    (dir, path)
}

#[test]
fn a_takeout_export_subscribes_to_its_channels() {
    let home = home();
    let csv = testdata("import/takeout.csv");
    let out = stdout(
        home.path(),
        &["youtube", "import-takeout", csv.to_str().unwrap()],
    );
    assert!(out.contains("4 new"), "{out}");
    let listed = stdout(home.path(), &["youtube", "list"]);
    assert!(listed.contains("Example Channel"), "{listed}");

    let again = stdout(
        home.path(),
        &["youtube", "import-takeout", csv.to_str().unwrap()],
    );
    assert!(again.contains("0 new"), "{again}");
}

#[test]
fn a_replay_run_fetches_reads_and_prints_the_fixture_article() {
    let home = home();
    let replay = testdata("replay");
    let replay = replay.to_str().unwrap();

    let fetched = stdout(home.path(), &["--replay", replay, "fetch"]);
    assert!(fetched.contains("new"), "{fetched}");

    let unread = stdout(home.path(), &["list", "--unread"]);
    assert!(
        unread.contains("Why the borrow checker says no"),
        "{unread}"
    );

    let id = unread
        .lines()
        .find(|l| l.contains("Why the borrow checker says no"))
        .and_then(|l| l.split_whitespace().next())
        .expect("an entry id in the first column")
        .to_string();

    let markdown = stdout(home.path(), &["show", &id, "--markdown"]);
    assert!(markdown.contains("Three rules"), "the heading: {markdown}");
    assert!(
        markdown.contains("Every value has exactly one owner."),
        "the list: {markdown}"
    );
    assert!(
        markdown.contains("fn longest"),
        "the code block: {markdown}"
    );
    assert!(
        markdown.contains("https://doc.rust-lang.org/nomicon/"),
        "the link: {markdown}"
    );
    assert!(
        !markdown.contains("Subscribe to the newsletter"),
        "the page furniture came with it: {markdown}"
    );

    // The default format is a header and then the same text.
    let full = stdout(home.path(), &["show", &id]);
    assert!(full.contains("Why the borrow checker says no"), "{full}");
    assert!(full.contains("Three rules"), "{full}");

    // And `--url` is the link alone, with the tracking parameter the feed
    // carried taken off.
    let url = stdout(home.path(), &["show", &id, "--url"]);
    assert_eq!(url.trim(), "https://example.org/posts/borrow-checker");
}

/// `fetch --no-extract` leaves every page unpulled, which is the shape an
/// entry has between arriving and being extracted. Since 0.0.2 that shape is
/// still readable: the feed's own text is stored with the entry, so `show`
/// prints it rather than one line saying there is nothing.
#[test]
fn an_entry_whose_page_is_not_pulled_still_reads_as_what_the_feed_carried() {
    let home = home();
    let replay = testdata("replay");
    stdout(
        home.path(),
        &[
            "--replay",
            replay.to_str().unwrap(),
            "fetch",
            "--no-extract",
        ],
    );

    let unread = stdout(home.path(), &["list", "--unread"]);
    let line = unread
        .lines()
        .find(|l| l.contains("Why the borrow checker says no"))
        .unwrap_or_else(|| panic!("{unread}"));
    let id = line.split_whitespace().next().expect("an entry id");

    let shown = stdout(home.path(), &["show", id]);
    assert!(
        shown.contains("Why the borrow checker says no"),
        "the header: {shown}"
    );
    assert!(
        shown.contains("pending"),
        "nothing has been fetched yet: {shown}"
    );
    assert!(
        shown.contains("Points: 212"),
        "the feed's own text is what there is to read: {shown}"
    );

    // And `--markdown`, the pipe-friendly format, is that same text.
    let markdown = stdout(home.path(), &["show", id, "--markdown"]);
    assert!(markdown.contains("Points: 212"), "{markdown}");
}

#[test]
fn a_replay_run_marks_a_video_as_a_video_and_never_fetches_it() {
    let home = home();
    let replay = testdata("replay");
    stdout(
        home.path(),
        &["--replay", replay.to_str().unwrap(), "fetch"],
    );

    let listed = stdout(home.path(), &["list", "--limit", "200"]);
    let video = listed
        .lines()
        .find(|l| l.contains("How a wire carries a signal"))
        .unwrap_or_else(|| panic!("{listed}"));
    assert!(
        video.contains(" v "),
        "a YouTube entry was not marked as a video: {video}"
    );

    let id = video.split_whitespace().next().unwrap();
    let markdown = stdout(home.path(), &["show", id, "--markdown"]);
    assert!(
        markdown.contains("not, in fact, full of electrons"),
        "a video's description is its text: {markdown}"
    );
}

#[test]
fn the_json_output_is_one_object_per_line() {
    let home = home();
    let replay = testdata("replay");
    stdout(
        home.path(),
        &["--replay", replay.to_str().unwrap(), "fetch"],
    );

    let json = stdout(home.path(), &["list", "--json", "--limit", "5"]);
    let lines: Vec<&str> = json.lines().collect();
    assert!(!lines.is_empty());
    for line in lines {
        assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
        assert!(line.contains("\"id\":"), "{line}");
    }

    let feeds = stdout(home.path(), &["list", "--feeds", "--json"]);
    for line in feeds.lines() {
        assert!(line.contains("\"unread\":"), "{line}");
    }
}

#[test]
fn extracting_one_page_prints_it_without_touching_the_database() {
    let home = home();
    let replay = testdata("replay");
    let out = stdout(
        home.path(),
        &[
            "--replay",
            replay.to_str().unwrap(),
            "extract",
            "https://example.org/posts/borrow-checker",
        ],
    );
    assert!(out.contains("Three rules"), "{out}");
    assert_eq!(
        stdout(home.path(), &["list", "--feeds"]).trim(),
        "",
        "the probe wrote to the database"
    );
}

/// A page that answers 200 with a free sample. The probe says so rather than
/// printing the sample, because the sample is what the feed already had.
#[test]
fn a_paywall_stub_is_reported_as_one_rather_than_printed() {
    let home = home();
    let replay = testdata("replay");
    let output = command(home.path())
        .args([
            "--replay",
            replay.to_str().unwrap(),
            "extract",
            "https://example.org/posts/paywalled",
        ])
        .output()
        .expect("running starwire extract");
    assert!(!output.status.success(), "{:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("paywall"), "{stderr}");
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
}

#[test]
fn a_page_that_does_not_yield_says_why_and_exits_with_a_failure_code() {
    let home = home();
    let replay = testdata("replay");
    let output = command(home.path())
        .args([
            "--replay",
            replay.to_str().unwrap(),
            "extract",
            "https://example.org/posts/unreadable",
        ])
        .output()
        .expect("running starwire extract");
    assert!(!output.status.success());
    assert!(
        !String::from_utf8_lossy(&output.stderr).is_empty(),
        "the reason belongs on stderr"
    );
}

#[test]
fn asking_for_an_entry_that_is_not_there_fails_with_a_reason() {
    let home = home();
    let output = command(home.path())
        .args(["show", "9999"])
        .output()
        .expect("running starwire show");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("9999"), "{stderr}");
}

/// The window is the one thing in this program that cannot run in a pipe,
/// and a test harness is a pipe. What is asserted is that it says so in its
/// own words rather than in the operating system's -- and that the first run
/// still left a config behind, because everything before the terminal is
/// taken over has already happened by then.
#[test]
fn running_it_with_no_terminal_says_so_and_still_writes_the_config() {
    let home = home();
    let output = command(home.path())
        .output()
        .expect("running starwire with no arguments");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("needs a terminal"), "{stderr}");
    assert!(stderr.contains("starwire --help"), "{stderr}");
    assert!(
        home.path().join("config.toml").exists(),
        "a first run leaves a config to edit"
    );
}
