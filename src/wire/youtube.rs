//! YouTube channels, and the one place a URL is made canonical.
//!
//! A channel arrives here in five spellings -- a channel page, a video, an
//! `@handle`, a bare `UC…`, a `videos.xml?channel_id=` feed, or a
//! `scriptbarrel.com/xml.cgi?channel_id=…&name=…` proxy for one -- and every
//! one of them reduces to a [`ChannelId`], because the channel id is the only
//! thing about a channel that never changes. A handle can be given up and
//! taken by somebody else; a channel's name changes whenever its owner feels
//! like it; the id does not.
//!
//! [`canonicalise`] is more than a YouTube function despite living here: it
//! is what every URL entering this program goes through, and the reason a
//! feed cannot end up in the list twice under two spellings. YouTube is
//! simply the case with the most spellings.

use anyhow::{Context, Result};
use url::Url;

use super::feed::FeedKind;

/// A YouTube channel id: `UC` followed by twenty-two characters of base64url.
///
/// Validated on the way in, because everything downstream builds a URL out of
/// it, and a "channel id" that is really a search query would become a
/// request for a page that does not exist -- or, worse, a `videos.xml` URL
/// with somebody's typed text in the query string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId(String);

impl ChannelId {
    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim();
        if s.len() != 24 || !s.starts_with("UC") {
            return None;
        }
        if !s[2..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }
        Some(Self(s.to_string()))
    }

    /// A value read back out of the database, which was validated when it
    /// went in. Separate from `parse` so that a row written by a version
    /// with different rules still loads rather than vanishing from the list.
    pub fn from_stored(s: String) -> Self {
        Self(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The Atom feed this channel publishes. The one URL a channel is stored
    /// under, whatever it was typed as.
    pub fn feed_url(&self) -> String {
        format!(
            "https://www.youtube.com/feeds/videos.xml?channel_id={}",
            self.0
        )
    }

    /// The channel's page, for `o`.
    pub fn page_url(&self) -> String {
        format!("https://www.youtube.com/channel/{}", self.0)
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// What somebody typed, recognised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detected {
    /// Already a channel id, or a URL that carried one.
    Channel(ChannelId),
    /// An `@handle`, which needs a round trip to turn into an id.
    Handle(String),
    /// A YouTube URL with no id in it -- a `/user/`, a `/c/`, a video page
    /// with only a video id -- which also needs a round trip.
    Page(String),
}

/// Recognise a channel from whatever was typed or pasted.
///
/// Returns `None` for anything that is not about YouTube at all, which is
/// how `AddFeed` tells a channel from an ordinary feed URL.
pub fn detect(input: &str) -> Option<Detected> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(id) = ChannelId::parse(raw) {
        return Some(Detected::Channel(id));
    }
    if let Some(handle) = as_handle(raw) {
        return Some(Detected::Handle(handle));
    }

    let url = Url::parse(raw)
        .ok()
        .or_else(|| Url::parse(&format!("https://{raw}")).ok())?;
    let host = host_of(&url);
    if !is_youtube_host(&host) && host != "scriptbarrel.com" {
        return None;
    }
    if let Some((id, _)) = parse_feed_url(&url) {
        return Some(Detected::Channel(id));
    }
    // `/channel/UC…` is the one page URL that carries the id outright.
    let segments: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
    if let Some(pos) = segments.iter().position(|s| *s == "channel") {
        if let Some(id) = segments.get(pos + 1).and_then(|s| ChannelId::parse(s)) {
            return Some(Detected::Channel(id));
        }
    }
    // `youtube.com/@handle`, which is the ordinary shape of a channel URL
    // today.
    if let Some(handle) = segments.first().and_then(|s| as_handle(s)) {
        return Some(Detected::Handle(handle));
    }
    Some(Detected::Page(url.to_string()))
}

fn as_handle(s: &str) -> Option<String> {
    let s = s.trim();
    let rest = s.strip_prefix('@')?;
    // YouTube handles are 3 to 30 characters of letters, digits, underscore,
    // hyphen and full stop. Checking is not pedantry: without it, an email
    // address pasted into the add box becomes a request for a channel page.
    if !(3..=30).contains(&rest.chars().count()) {
        return None;
    }
    rest.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        .then(|| s.to_string())
}

fn host_of(url: &Url) -> String {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

fn is_youtube_host(host: &str) -> bool {
    host == "youtube.com" || host.ends_with(".youtube.com") || host == "youtu.be"
}

/// The channel id and, where the URL carried one, a title.
///
/// The title is the whole reason `scriptbarrel.com` is recognised at all: a
/// `scriptbarrel` URL has `&name=Some%20Channel` on it, and for the seven
/// feeds the reference list routes that way it is the only place a name for
/// the channel exists before the first fetch.
pub fn parse_feed_url(url: &Url) -> Option<(ChannelId, Option<String>)> {
    let host = host_of(url);
    let is_youtube_feed = is_youtube_host(&host) && url.path() == "/feeds/videos.xml";
    let is_scriptbarrel = host == "scriptbarrel.com" && url.path() == "/xml.cgi";
    if !is_youtube_feed && !is_scriptbarrel {
        return None;
    }
    let mut id = None;
    let mut name = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "channel_id" => id = ChannelId::parse(&value),
            "name" => {
                let value = value.trim().to_string();
                if !value.is_empty() {
                    name = Some(value);
                }
            }
            _ => {}
        }
    }
    id.map(|id| (id, name))
}

/// The video id in a link, if it is a link to a video.
///
/// Every shape YouTube serves: `watch?v=`, `youtu.be/`, `/shorts/`,
/// `/embed/` and `/live/`. Used to mark an entry as a video wherever its
/// link came from, which is why a video linked out of a blog post plays with
/// `v` rather than opening a browser.
pub fn video_id(url: &Url) -> Option<String> {
    let host = host_of(url);
    if !is_youtube_host(&host) {
        return None;
    }
    if host == "youtu.be" {
        return url
            .path_segments()
            .and_then(|mut s| s.next())
            .map(str::to_string)
            .filter(|s| !s.is_empty());
    }
    if url.path() == "/watch" {
        return url
            .query_pairs()
            .find(|(k, _)| k == "v")
            .map(|(_, v)| v.into_owned())
            .filter(|s| !s.is_empty());
    }
    let segments: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
    if let [kind, id, ..] = segments.as_slice() {
        if matches!(*kind, "shorts" | "embed" | "live" | "v") && !id.is_empty() {
            return Some((*id).to_string());
        }
    }
    None
}

/// A URL, and what it is, after normalisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canonical {
    /// The form the feed is stored under.
    pub url: String,
    pub kind: FeedKind,
    /// The channel, where this turned out to be one.
    pub channel: Option<ChannelId>,
    /// A title the URL itself carried -- `scriptbarrel`'s `name=`, and
    /// nothing else.
    pub title: Option<String>,
}

/// Every URL entering this program goes through here.
///
/// Three things happen, and each is the answer to a real problem:
///
/// 1. **A bare host gets a scheme.** Somebody typing `example.org/feed` into
///    the add box means `https://example.org/feed`.
/// 2. **`http` becomes `https`.** The agent refuses plaintext by design (see
///    `net.rs`), so `http://old.reddit.com/r/rust/.rss` -- which is what the
///    reference `urls` file actually says -- would otherwise simply fail.
///    The original is kept as the feed's `source_url`. The same step applies
///    [`super::feed::canonical_url`], which is where `old.reddit.com`
///    becomes `www.reddit.com`.
/// 3. **A YouTube channel becomes its `videos.xml` feed.** So the same
///    channel added as a handle, imported from `scriptbarrel`, and pasted as
///    a video URL is one row rather than three.
pub fn canonicalise(raw: &str) -> Result<Canonical> {
    let trimmed = raw.trim();
    anyhow::ensure!(!trimmed.is_empty(), "an empty address is not a feed");

    if let Some(id) = ChannelId::parse(trimmed) {
        return Ok(Canonical {
            url: id.feed_url(),
            kind: FeedKind::Youtube,
            channel: Some(id),
            title: None,
        });
    }

    let mut url = Url::parse(trimmed)
        .or_else(|_| Url::parse(&format!("https://{trimmed}")))
        .with_context(|| format!("{trimmed} is not an address this can fetch"))?;

    if url.scheme() == "http" {
        // Infallible for an http URL: `set_scheme` only refuses a change
        // between a special scheme and a non-special one.
        let _ = url.set_scheme("https");
    }
    // The host rewrites that are not YouTube's, `old.reddit.com` among them.
    super::feed::canonical_url(&mut url);
    anyhow::ensure!(
        url.scheme() == "https",
        "{trimmed}: this reads feeds over https, not {}",
        url.scheme()
    );

    if let Some((id, name)) = parse_feed_url(&url) {
        return Ok(Canonical {
            url: id.feed_url(),
            kind: FeedKind::Youtube,
            channel: Some(id),
            title: name,
        });
    }

    let kind = FeedKind::of_url(&url);
    Ok(Canonical {
        url: url.to_string(),
        kind,
        channel: None,
        title: None,
    })
}

/// A channel, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub channel: ChannelId,
    pub title: Option<String>,
    pub handle: Option<String>,
}

/// Turn a handle or a channel page into a channel id.
///
/// The page first: a YouTube channel page carries its own id in a
/// `"channelId":"UC…"` field and in a `<link rel="canonical">`, and reading
/// it costs one ordinary request. `yt-dlp` is the fallback rather than the
/// first choice because it is somebody else's program, it may not be
/// installed, and it costs several requests of its own -- but it is also the
/// thing that keeps working when YouTube changes the page, which is why it
/// is here at all.
pub fn resolve(
    http: &dyn super::net::Http,
    what: &Detected,
    cfg: &super::YoutubeConfig,
) -> Result<Resolved> {
    let (page_url, handle) = match what {
        Detected::Channel(id) => {
            return Ok(Resolved {
                channel: id.clone(),
                title: None,
                handle: None,
            })
        }
        Detected::Handle(handle) => (
            format!("https://www.youtube.com/{handle}"),
            Some(handle.clone()),
        ),
        Detected::Page(url) => (url.clone(), None),
    };

    let page_error = match fetch_channel_page(http, &page_url) {
        Ok(mut resolved) => {
            resolved.handle = resolved.handle.or(handle.clone());
            return Ok(resolved);
        }
        Err(e) => e,
    };

    match resolve_with_yt_dlp(&page_url, cfg) {
        Ok(mut resolved) => {
            resolved.handle = resolved.handle.or(handle);
            Ok(resolved)
        }
        Err(dlp_error) => anyhow::bail!(
            "could not work out which channel {page_url} is.\n  \
             reading the page: {page_error}\n  \
             asking {}: {dlp_error}",
            cfg.yt_dlp
        ),
    }
}

fn fetch_channel_page(http: &dyn super::net::Http, url: &str) -> Result<Resolved> {
    let parsed = Url::parse(url).with_context(|| format!("{url} is not a URL"))?;
    let response = http
        .get(&parsed, &super::net::RequestOptions::page(4 * 1024 * 1024))
        .with_context(|| format!("fetching {url}"))?;
    anyhow::ensure!(response.is_ok(), "{url} answered {}", response.status);
    let html = response.text();
    let channel =
        channel_id_in_page(&html).ok_or_else(|| anyhow::anyhow!("no channel id in the page"))?;
    Ok(Resolved {
        channel,
        title: title_in_page(&html),
        handle: None,
    })
}

/// Find a channel id in a channel page.
///
/// Two places, because neither alone is reliable: the `"channelId":"UC…"`
/// field in the embedded JSON, and the `<link rel="canonical">` that points
/// at `/channel/UC…`. A plain scan rather than a parse -- the page is
/// megabytes of script and this needs twenty-four characters out of it.
pub fn channel_id_in_page(html: &str) -> Option<ChannelId> {
    for marker in ["\"channelId\":\"", "\"externalChannelId\":\"", "/channel/"] {
        let mut rest = html;
        while let Some(at) = rest.find(marker) {
            let after = &rest[at + marker.len()..];
            let candidate: String = after.chars().take(24).collect();
            if let Some(id) = ChannelId::parse(&candidate) {
                return Some(id);
            }
            rest = after;
        }
    }
    None
}

/// The channel's name from its page's `og:title`.
pub fn title_in_page(html: &str) -> Option<String> {
    let at = html.find(r#"property="og:title""#)?;
    // The meta tag may put `content` before or after `property`, so the
    // window is the whole tag rather than what follows the marker.
    let start = html[..at].rfind('<')?;
    let end = html[at..].find('>')? + at;
    let tag = &html[start..end];
    let content = tag.find(r#"content=""#)? + r#"content=""#.len();
    let rest = &tag[content..];
    let close = rest.find('"')?;
    let title = decode_entities(&rest[..close]).trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// The five XML entities, and nothing else.
///
/// A full entity table is what a parser is for, and there is one two stages
/// downstream; this only has to be right for the handful that appear in a
/// `<meta>` attribute.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Ask `yt-dlp` which channel a URL is.
///
/// `--flat-playlist --playlist-items 1` so that it reads one entry rather
/// than enumerating a channel's entire back catalogue, which on a large
/// channel is thousands of requests.
pub fn resolve_with_yt_dlp(url: &str, cfg: &super::YoutubeConfig) -> Result<Resolved> {
    let mut command = std::process::Command::new(&cfg.yt_dlp);
    command.args([
        "--flat-playlist",
        "--playlist-items",
        "1",
        "--no-warnings",
        "--print",
        "%(channel_id)s\t%(channel)s",
    ]);
    if !cfg.cookies_from_browser.trim().is_empty() {
        command.args(["--cookies-from-browser", cfg.cookies_from_browser.trim()]);
    }
    command.arg("--").arg(url);

    let output = command
        .output()
        .with_context(|| format!("running {}", cfg.yt_dlp))?;
    anyhow::ensure!(
        output.status.success(),
        "{} exited {}: {}",
        cfg.yt_dlp,
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_yt_dlp_line(&stdout)
        .ok_or_else(|| anyhow::anyhow!("{} printed no channel id", cfg.yt_dlp))
}

fn parse_yt_dlp_line(stdout: &str) -> Option<Resolved> {
    for line in stdout.lines() {
        let (id, title) = line.split_once('\t').unwrap_or((line, ""));
        if let Some(channel) = ChannelId::parse(id) {
            let title = title.trim();
            return Some(Resolved {
                channel,
                // yt-dlp prints the literal string "NA" for a field it could
                // not fill, which as a channel name would be a lie rather
                // than a blank.
                title: (!title.is_empty() && title != "NA").then(|| title.to_string()),
                handle: None,
            });
        }
    }
    None
}

/// Google Takeout's `subscriptions.csv`.
///
/// Three columns: `Channel Id,Channel Url,Channel Title`. A real parser
/// rather than a split on commas, because a channel title with a comma in it
/// is quoted and there are several of those in any real export.
pub fn parse_takeout(csv_text: &str) -> Result<Vec<(ChannelId, Option<String>)>> {
    let mut reader = csv::ReaderBuilder::new()
        // Takeout writes a header row, but a file somebody has edited may
        // not have one, and a header that is not there would eat the first
        // channel. Headers off, and a row whose first field is not a
        // channel id is skipped -- which is exactly what a header row is.
        .has_headers(false)
        .flexible(true)
        .from_reader(csv_text.as_bytes());

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for record in reader.records() {
        let record = record.context("reading subscriptions.csv")?;
        let Some(id) = record.get(0).and_then(ChannelId::parse) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let title = record
            .get(2)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        out.push((id, title));
    }
    Ok(out)
}

/// The account's real subscriptions, through `yt-dlp` and the browser's
/// cookies.
///
/// `:ytsubs` is yt-dlp's name for the subscriptions *feed* -- recent videos
/// from everything the account follows -- rather than for the subscription
/// list itself, which has no public endpoint. The distinct channel ids in
/// that feed are subscribed to; a channel that has published nothing inside
/// the window is missed. That is a real limitation and it is written down in
/// `docs/youtube.md` rather than papered over here.
pub fn yt_subscriptions(
    cfg: &super::YoutubeConfig,
    limit: usize,
) -> Result<Vec<(ChannelId, Option<String>)>> {
    anyhow::ensure!(
        !cfg.cookies_from_browser.trim().is_empty(),
        "reading your subscriptions needs your browser's cookies: \
         set [youtube] cookies_from_browser, or pass --cookies-from-browser"
    );
    let mut command = std::process::Command::new(&cfg.yt_dlp);
    command.args([
        "--flat-playlist",
        "--no-warnings",
        "--playlist-items",
        &format!("1:{limit}"),
        "--print",
        "%(channel_id)s\t%(channel)s",
        "--cookies-from-browser",
        cfg.cookies_from_browser.trim(),
        "--",
        ":ytsubs",
    ]);
    let output = command
        .output()
        .with_context(|| format!("running {}", cfg.yt_dlp))?;
    anyhow::ensure!(
        output.status.success(),
        "{} exited {}: {}",
        cfg.yt_dlp,
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(parse_ytsubs(&String::from_utf8_lossy(&output.stdout)))
}

/// `channel_id<TAB>channel` lines, deduplicated, in the order first seen.
pub fn parse_ytsubs(stdout: &str) -> Vec<(ChannelId, Option<String>)> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in stdout.lines() {
        let (id, title) = line.split_once('\t').unwrap_or((line, ""));
        let Some(id) = ChannelId::parse(id) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let title = title.trim();
        out.push((
            id,
            (!title.is_empty() && title != "NA").then(|| title.to_string()),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "UCXuqSBlHAE6Xw-yeJA0Tunw";

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn a_channel_id_is_exactly_what_a_channel_id_looks_like() {
        assert!(ChannelId::parse(ID).is_some());
        assert!(ChannelId::parse(&format!("  {ID}  ")).is_some());
        assert!(ChannelId::parse("UCtooshort").is_none());
        assert!(ChannelId::parse("ABXuqSBlHAE6Xw-yeJA0Tunw").is_none());
        assert!(ChannelId::parse("UCXuqSBlHAE6Xw yeJA0Tunw").is_none());
        assert!(ChannelId::parse("").is_none());
        assert!(
            ChannelId::parse("UC../../etc/passwd/xxxxx").is_none(),
            "a channel id goes straight into a URL"
        );
    }

    #[test]
    fn every_spelling_of_a_channel_reduces_to_the_same_feed_url() {
        let wanted = format!("https://www.youtube.com/feeds/videos.xml?channel_id={ID}");
        for input in [
            ID,
            &format!("https://www.youtube.com/feeds/videos.xml?channel_id={ID}"),
            &format!("http://www.youtube.com/feeds/videos.xml?channel_id={ID}"),
            &format!("https://youtube.com/feeds/videos.xml?channel_id={ID}"),
            &format!("https://scriptbarrel.com/xml.cgi?channel_id={ID}&name=Thing"),
        ] {
            let got = canonicalise(input).unwrap();
            assert_eq!(got.url, wanted, "{input}");
            assert_eq!(got.kind, FeedKind::Youtube, "{input}");
            assert_eq!(got.channel.as_ref().map(ChannelId::as_str), Some(ID));
        }
    }

    #[test]
    fn a_scriptbarrel_url_gives_up_the_only_title_the_feed_list_has() {
        let got = canonicalise(&format!(
            "https://scriptbarrel.com/xml.cgi?channel_id={ID}&name=Some%20Channel"
        ))
        .unwrap();
        assert_eq!(got.title.as_deref(), Some("Some Channel"));
    }

    #[test]
    fn the_canonical_form_is_a_fixed_point() {
        for input in [
            ID,
            &format!("https://scriptbarrel.com/xml.cgi?channel_id={ID}&name=Thing"),
            "http://old.reddit.com/r/rust/.rss",
            "https://hnrss.org/frontpage",
            "example.org/feed.xml",
        ] {
            let once = canonicalise(input).unwrap();
            let twice = canonicalise(&once.url).unwrap();
            assert_eq!(once.url, twice.url, "{input}");
            assert_eq!(once.kind, twice.kind, "{input}");
        }
    }

    #[test]
    fn http_becomes_https_because_the_agent_will_not_speak_anything_else() {
        let got = canonicalise("http://old.reddit.com/r/rust/.rss").unwrap();
        assert_eq!(
            got.url, "https://www.reddit.com/r/rust/.rss",
            "and old.reddit.com now answers a feed request with a login page"
        );
        assert_eq!(got.kind, FeedKind::Reddit);
    }

    #[test]
    fn a_bare_host_is_given_a_scheme() {
        let got = canonicalise("example.org/feed.xml").unwrap();
        assert_eq!(got.url, "https://example.org/feed.xml");
    }

    #[test]
    fn something_that_is_not_an_address_at_all_is_refused() {
        assert!(canonicalise("").is_err());
        assert!(canonicalise("   ").is_err());
        assert!(
            canonicalise("ftp://example.org/feed").is_err(),
            "this reads feeds over https"
        );
        assert!(canonicalise("file:///etc/passwd").is_err());
    }

    #[test]
    fn detect_recognises_the_five_ways_a_channel_is_named() {
        assert_eq!(
            detect(ID),
            Some(Detected::Channel(ChannelId::parse(ID).unwrap()))
        );
        assert_eq!(
            detect(&format!("https://www.youtube.com/channel/{ID}")),
            Some(Detected::Channel(ChannelId::parse(ID).unwrap()))
        );
        assert_eq!(
            detect(&format!(
                "https://scriptbarrel.com/xml.cgi?channel_id={ID}&name=x"
            )),
            Some(Detected::Channel(ChannelId::parse(ID).unwrap()))
        );
        assert_eq!(
            detect("@veritasium"),
            Some(Detected::Handle("@veritasium".into()))
        );
        assert_eq!(
            detect("https://www.youtube.com/@veritasium"),
            Some(Detected::Handle("@veritasium".into()))
        );
        assert_eq!(
            detect("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some(Detected::Page(
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()
            ))
        );
    }

    #[test]
    fn detect_says_nothing_about_a_feed_that_is_not_youtube() {
        assert_eq!(detect("https://www.phoronix.com/rss.php"), None);
        assert_eq!(detect("https://hnrss.org/frontpage"), None);
        assert_eq!(detect(""), None);
        assert_eq!(
            detect("someone@example.org"),
            None,
            "an email address is not a handle"
        );
    }

    #[test]
    fn a_handle_has_to_look_like_a_handle() {
        assert!(as_handle("@ab").is_none(), "too short");
        assert!(
            as_handle(&format!("@{}", "a".repeat(31))).is_none(),
            "too long"
        );
        assert!(as_handle("@with space").is_none());
        assert!(as_handle("@ok.name-1_2").is_some());
    }

    #[test]
    fn every_shape_of_video_link_gives_up_its_id() {
        for (input, want) in [
            ("https://www.youtube.com/watch?v=dQw4w9WgXcQ", "dQw4w9WgXcQ"),
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42",
                "dQw4w9WgXcQ",
            ),
            ("https://youtu.be/dQw4w9WgXcQ", "dQw4w9WgXcQ"),
            ("https://www.youtube.com/shorts/dQw4w9WgXcQ", "dQw4w9WgXcQ"),
            ("https://www.youtube.com/embed/dQw4w9WgXcQ", "dQw4w9WgXcQ"),
            ("https://www.youtube.com/live/dQw4w9WgXcQ", "dQw4w9WgXcQ"),
        ] {
            assert_eq!(video_id(&url(input)).as_deref(), Some(want), "{input}");
        }
        assert_eq!(video_id(&url("https://example.org/watch?v=x")), None);
        assert_eq!(video_id(&url("https://www.youtube.com/@name")), None);
    }

    #[test]
    fn a_channel_id_is_found_in_a_channel_page() {
        let html = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("testdata/pages/youtube-handle.html"),
        )
        .unwrap();
        let id = channel_id_in_page(&html).expect("the fixture carries an id");
        assert_eq!(id.as_str(), ID);
        assert_eq!(title_in_page(&html).as_deref(), Some("Example Channel"));
    }

    #[test]
    fn a_page_with_no_channel_id_says_so_rather_than_inventing_one() {
        assert!(channel_id_in_page("<html><body>nothing here</body></html>").is_none());
        assert!(channel_id_in_page("").is_none());
        assert!(
            channel_id_in_page(r#"{"channelId":"UCtooshort"}"#).is_none(),
            "a near miss is not a channel id"
        );
    }

    #[test]
    fn a_title_with_an_entity_in_it_is_decoded() {
        let html = r#"<meta property="og:title" content="Tom &amp; Jerry">"#;
        assert_eq!(title_in_page(html).as_deref(), Some("Tom & Jerry"));
    }

    #[test]
    fn a_takeout_export_survives_a_comma_in_a_channel_name() {
        let csv = "Channel Id,Channel Url,Channel Title\n\
                   UCXuqSBlHAE6Xw-yeJA0Tunw,https://www.youtube.com/channel/UCXuqSBlHAE6Xw-yeJA0Tunw,\"Smith, Jones and Co\"\n\
                   UCBa659QWEk1AI4Tg--mrJ2A,https://www.youtube.com/channel/UCBa659QWEk1AI4Tg--mrJ2A,Tom Scott\n";
        let got = parse_takeout(csv).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].1.as_deref(), Some("Smith, Jones and Co"));
        assert_eq!(got[1].1.as_deref(), Some("Tom Scott"));
    }

    #[test]
    fn a_takeout_export_skips_its_header_and_anything_else_that_is_not_a_channel() {
        let csv = "Channel Id,Channel Url,Channel Title\n\
                   \n\
                   not-a-channel,x,y\n\
                   UCXuqSBlHAE6Xw-yeJA0Tunw,,Only One\n\
                   UCXuqSBlHAE6Xw-yeJA0Tunw,,Duplicate\n";
        let got = parse_takeout(csv).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1.as_deref(), Some("Only One"));
    }

    #[test]
    fn the_takeout_fixture_parses() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/import/takeout.csv"),
        )
        .unwrap();
        let got = parse_takeout(&text).unwrap();
        assert!(got.len() >= 3, "{got:?}");
    }

    #[test]
    fn ytsubs_output_is_the_distinct_channels_in_the_order_seen() {
        let stdout = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/import/ytsubs.tsv"),
        )
        .unwrap();
        let got = parse_ytsubs(&stdout);
        assert!(got.len() >= 2);
        let ids: Vec<&str> = got.iter().map(|(id, _)| id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "the same channel twice: {ids:?}");
    }

    #[test]
    fn yt_dlp_printing_a_placeholder_for_a_name_is_not_a_name() {
        let got = parse_yt_dlp_line(&format!("{ID}\tNA\n")).unwrap();
        assert_eq!(got.channel.as_str(), ID);
        assert!(got.title.is_none());
    }

    #[test]
    fn yt_dlp_printing_nothing_useful_is_not_a_resolution() {
        assert!(parse_yt_dlp_line("").is_none());
        assert!(parse_yt_dlp_line("ERROR: unable to download\n").is_none());
    }

    #[test]
    fn a_sync_with_no_cookies_says_what_is_missing_rather_than_failing_obscurely() {
        let cfg = super::super::YoutubeConfig::default();
        let err = yt_subscriptions(&cfg, 500).unwrap_err().to_string();
        assert!(err.contains("cookies"), "{err}");
    }

    proptest::proptest! {
        /// Whatever is pasted into the add box.
        #[test]
        fn detect_and_canonicalise_never_panic(s: String) {
            let _ = detect(&s);
            let _ = canonicalise(&s);
        }

        /// A canonical URL canonicalises to itself. This is the property the
        /// whole deduplication rests on: if it did not hold, importing a
        /// list twice would double it.
        #[test]
        fn canonicalising_is_a_fixed_point(s: String) {
            if let Ok(once) = canonicalise(&s) {
                let twice = canonicalise(&once.url).expect("a canonical url canonicalises");
                proptest::prop_assert_eq!(once.url, twice.url);
            }
        }

        #[test]
        fn takeout_and_ytsubs_never_panic(s: String) {
            let _ = parse_takeout(&s);
            let _ = parse_ytsubs(&s);
        }
    }
}
