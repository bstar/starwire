//! The vocabulary: what a feed is, what an entry is, and what one looks like
//! on the way out of a parser and on the way out of the database.
//!
//! This is also **the one file in the tree that knows `chrono` exists**.
//! `feed-rs` hands back `chrono::DateTime<Utc>` and the rest of the family
//! keeps time in `jiff`; converting at this edge rather than letting both
//! spread through the tree is why a date comparison somewhere else cannot be
//! between two different crates' idea of an instant.

use jiff::Timestamp;

/// A row id in `feed`.
///
/// A newtype rather than a bare `i64` because `EntryId` and `FolderId` are
/// also `i64` and every one of the three is passed to functions that take
/// more than one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FeedId(pub i64);

/// A row id in `entry`. What `starwire show <id>` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(pub i64);

/// A row id in `folder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FolderId(pub i64);

impl std::fmt::Display for FeedId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for EntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for FolderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// What kind of source a feed is, which is what decides whether its entries
/// are worth scraping.
///
/// Stored as an integer rather than a string: these five are the whole set,
/// the numbers are in the schema, and a spelling mistake in a `TEXT` column
/// would silently make a feed behave like a web feed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FeedKind {
    /// An ordinary site. Its entries link to pages worth extracting.
    #[default]
    Web = 0,
    /// A YouTube channel feed. Every entry is a video; nothing is extracted.
    Youtube = 1,
    /// A subreddit. The entry body is the post; Reddit is never scraped,
    /// which is both politeness and the only way to stay under its rate
    /// limit.
    Reddit = 2,
    /// Hacker News through `hnrss`. An entry links out to a real page, which
    /// is extracted, unless the link goes back into `news.ycombinator.com`.
    Hn = 3,
}

impl FeedKind {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Youtube,
            2 => Self::Reddit,
            3 => Self::Hn,
            _ => Self::Web,
        }
    }

    pub fn as_i64(self) -> i64 {
        self as i64
    }

    /// What kind a URL is, before anything has been fetched from it.
    ///
    /// Decided from the host, which is the only thing available when a feed
    /// is added by hand and the only thing that stays true afterwards: a
    /// YouTube channel feed is an Atom feed and would otherwise look exactly
    /// like a blog.
    pub fn of_url(url: &url::Url) -> Self {
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host);
        if host == "youtube.com" || host.ends_with(".youtube.com") || host == "youtu.be" {
            Self::Youtube
        } else if host == "reddit.com" || host.ends_with(".reddit.com") {
            Self::Reddit
        } else if host == "hnrss.org" || host.ends_with(".hnrss.org") {
            Self::Hn
        } else {
            Self::Web
        }
    }
}

/// What kind of thing one entry is, which decides how the reader draws it and
/// what `v` and `o` do to it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    /// Something with a page behind it worth reading as text.
    #[default]
    Article = 0,
    /// A video. `v` plays it; it is never extracted.
    Video = 1,
    /// A forum or link-aggregator post. Its own body is the text; the page it
    /// would link to is a comment thread, not an article.
    Post = 2,
}

impl EntryKind {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Video,
            2 => Self::Post,
            _ => Self::Article,
        }
    }

    pub fn as_i64(self) -> i64 {
        self as i64
    }
}

/// How far extraction got for one entry. The numbers are in the schema.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArticleStatus {
    /// Nothing has been tried yet.
    #[default]
    Pending = 0,
    /// The page was fetched and reduced. This is the good case.
    Extracted = 1,
    /// Extraction was not attempted or did not yield, and the feed's own text
    /// is what is stored. Not a failure: a full-text feed lands here and
    /// reads perfectly.
    FeedContent = 2,
    /// Extraction was attempted and failed. Whatever the feed carried is
    /// still stored, so the entry is readable; `error` says what went wrong
    /// and `o` opens the page.
    Failed = 3,
    /// Extraction does not apply. A video's description, a post's body.
    NotApplicable = 4,
}

impl ArticleStatus {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Extracted,
            2 => Self::FeedContent,
            3 => Self::Failed,
            4 => Self::NotApplicable,
            _ => Self::Pending,
        }
    }

    pub fn as_i64(self) -> i64 {
        self as i64
    }

    /// Whether another attempt is due without anybody asking for one.
    ///
    /// Only `Pending`. A `Failed` row comes back only when its reason was
    /// transient -- a 429, a 5xx or a timeout, which `article.retry_after`
    /// records and `db::articles::pending` reads -- or when somebody asks
    /// with `e` in the reader. The usual reason is a paywall, and retrying a
    /// paywall hourly helps nobody.
    pub fn is_pending(self) -> bool {
        matches!(self, Self::Pending)
    }
}

/// A folder. One per feed, created on demand by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
    pub position: i64,
}

/// A feed as the lists draw it: the stored row plus the two numbers that are
/// always a query rather than a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedRow {
    pub id: FeedId,
    /// The canonical URL: https, and YouTube normalised to
    /// `videos.xml?channel_id=`.
    pub url: String,
    /// What was typed or imported, kept so that an import can be recognised
    /// again and so that a scriptbarrel URL is still visible after being
    /// rewritten.
    pub source_url: Option<String>,
    pub kind: FeedKind,
    /// What the feed calls itself.
    pub title: Option<String>,
    /// What the reader renamed it to, which wins.
    pub custom_title: Option<String>,
    pub site_url: Option<String>,
    pub folder: Option<FolderId>,
    pub position: i64,
    pub unread: i64,
    pub last_ok: Option<Timestamp>,
    pub error: Option<String>,
    pub failures: i64,
    pub backoff_until: Option<Timestamp>,
}

impl FeedRow {
    /// What to put on screen: the rename, else what the feed calls itself,
    /// else the URL. The same `COALESCE` the query uses, so a row built by
    /// hand in a test and a row read back out of SQLite agree.
    pub fn display_title(&self) -> &str {
        self.custom_title
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(self.title.as_deref().filter(|s| !s.is_empty()))
            .unwrap_or(&self.url)
    }
}

/// An entry as the list draws it. Deliberately without the body: a page of
/// two hundred of these is loaded at a time, and `content_html` on each would
/// be megabytes to move for rows nobody has opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRow {
    pub id: EntryId,
    pub feed_id: FeedId,
    pub feed_title: String,
    pub title: String,
    pub author: Option<String>,
    pub url: Option<String>,
    pub published: Option<Timestamp>,
    pub kind: EntryKind,
    pub read: bool,
    pub starred: bool,
    pub article_status: ArticleStatus,
    pub thumbnail_url: Option<String>,
}

/// One entry's text, as the reader shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleView {
    pub entry: EntryId,
    pub title: String,
    pub byline: Option<String>,
    pub site_name: Option<String>,
    pub url: Option<String>,
    /// CommonMark. `Arc<str>` because the reader's render cache holds on to
    /// it across frames and re-layouts it on a width change, and copying a
    /// long article's text once per frame is the one allocation in the draw
    /// path worth avoiding.
    pub markdown: std::sync::Arc<str>,
    pub status: ArticleStatus,
    pub image_url: Option<String>,
    /// Why the page did not yield, where it did not.
    ///
    /// **It is set beside kept text, not instead of it.** Since 0.0.2 every
    /// article row carries the feed's own text from the moment the entry is
    /// stored, so a [`ArticleStatus::Failed`] view has `markdown` *and*
    /// `error`: the reader draws the text and puts the reason above it rather
    /// than drawing a reason where an article should be. `paywall` beside
    /// [`ArticleStatus::FeedContent`] is the same shape -- the page was
    /// fetched, what came back was a stub, and the feed's text is better.
    pub error: Option<String>,
    /// When the text was last written. The reader's cache keys on this: it is
    /// what changes when an extraction lands behind an already-open entry.
    pub extracted_at: Option<Timestamp>,
}

/// What the ENTRIES list is currently showing.
///
/// `Videos` is every entry of kind [`EntryKind::Video`] across every feed,
/// not a folder that happens to hold the YouTube channels: a video linked
/// from a blog post belongs in it too.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum Selection {
    #[default]
    All,
    Feed(FeedId),
    Folder(FolderId),
    Starred,
    Videos,
    /// A full-text search, run by FTS5 rather than by the `/` filter.
    Search(String),
}

/// A feed as a parser handed it back, before anything has been stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedFeed {
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub entries: Vec<ParsedEntry>,
}

/// One entry as a parser handed it back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedEntry {
    /// `feed-rs`'s `Entry::id`, which it synthesises from the link and title
    /// when the feed carries neither a `<guid>` nor an Atom `<id>` -- the same
    /// thing newsboat does, and the reason a feed with no ids still has
    /// stable read state.
    pub guid: String,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub published: Option<Timestamp>,
    pub content_html: Option<String>,
    pub thumbnail_url: Option<String>,
    pub video_id: Option<String>,
    pub duration_secs: Option<i64>,
}

/// The most `content_html` a stored entry may carry.
///
/// A full-text feed can put a whole article in every item; two hundred of
/// those is the page the list loads, and a feed that puts a megabyte in each
/// is not one whose extra bytes are the reader's.
pub const MAX_CONTENT_HTML: usize = 256 * 1024;

/// `chrono` to `jiff`, the only place in the tree it happens.
///
/// Through the unix second and the nanosecond within it rather than a
/// formatted string: the two crates agree on what an instant is and disagree
/// on how to spell one, and a round trip through RFC 3339 would be a parser
/// in the path for no reason.
///
/// Taking two numbers rather than the `chrono::DateTime` itself is not
/// squeamishness -- `chrono` arrives under `feed-rs` and is deliberately not
/// a dependency of this package, so its types cannot be named here. The two
/// call sites read the numbers off the value they already hold.
fn to_timestamp(secs: i64, nanos: u32) -> Option<Timestamp> {
    Timestamp::new(secs, nanos.min(999_999_999) as i32).ok()
}

/// What `feed-rs` parsed, reduced to what this program stores.
///
/// Everything dropped here is dropped on purpose: categories, contributors,
/// the feed's own rights statement and its generator are all things a reader
/// would have to be asked about before showing, and nobody has asked.
pub fn from_parsed(parsed: feed_rs::model::Feed) -> ParsedFeed {
    let site_url = parsed
        .links
        .iter()
        // `rel="alternate"` is the human page; a feed's `self` link is the
        // feed itself, and taking the first link regardless would sometimes
        // make a feed's site URL its own address.
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| parsed.links.first())
        .map(|l| l.href.clone());

    ParsedFeed {
        title: parsed
            .title
            .map(|t| clean_text(&t.content))
            .filter(|s| nonempty(s)),
        site_url,
        entries: parsed.entries.into_iter().map(entry_from_parsed).collect(),
    }
}

fn entry_from_parsed(entry: feed_rs::model::Entry) -> ParsedEntry {
    // Through `clean_url`, which is what makes this string a key: two links
    // to one article that differ only in which newsletter they came from are
    // one article, and the campaign parameters a feed puts on its own links
    // would otherwise make the same page a different entry in every feed
    // that carried it.
    let url = entry
        .links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| entry.links.first())
        .map(|l| crate::wire::extract::normalise::clean_url(&l.href));

    // The body, in order of preference: `<content>`, then `<summary>`. A
    // summary feed has only the second; a full-text feed has both and the
    // first is the whole article.
    let mut content_html = entry
        .content
        .as_ref()
        .and_then(|c| c.body.clone())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            entry
                .summary
                .as_ref()
                .map(|t| t.content.clone())
                .filter(|s| !s.trim().is_empty())
        });

    // MediaRSS. A YouTube entry carries its description and its thumbnail in
    // `media:group` and nowhere else, so an entry with an empty `<content>`
    // and a `media:description` is not an empty entry.
    let media = entry.media.first();
    let thumbnail_url = media
        .and_then(|m| m.thumbnails.first())
        .map(|t| t.image.uri.clone());
    if content_html.is_none() {
        content_html = media
            .and_then(|m| m.description.as_ref())
            .map(|t| t.content.clone())
            .filter(|s| !s.trim().is_empty());
    }
    let duration_secs = media
        .and_then(|m| m.duration)
        .or_else(|| media.and_then(|m| m.content.first().and_then(|c| c.duration)))
        .map(|d| d.as_secs() as i64);

    if let Some(html) = &mut content_html {
        truncate_on_char_boundary(html, MAX_CONTENT_HTML);
    }

    let video_id = url
        .as_deref()
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| crate::wire::youtube::video_id(&u));

    ParsedEntry {
        guid: entry.id,
        url,
        title: entry
            .title
            .map(|t| clean_text(&t.content))
            .unwrap_or_default(),
        author: entry
            .authors
            .first()
            .map(|p| clean_text(&p.name))
            .filter(|s| nonempty(s)),
        // `published` where the feed gives one, else `updated`: an Atom feed
        // is allowed to carry only the second, and an entry with no date at
        // all sorts by when it was first seen instead.
        published: entry
            .published
            .or(entry.updated)
            .and_then(|dt| to_timestamp(dt.timestamp(), dt.timestamp_subsec_nanos())),
        content_html,
        thumbnail_url,
        video_id,
        duration_secs,
    }
}

fn nonempty(s: &str) -> bool {
    !s.is_empty()
}

/// Collapse the whitespace a title picked up from being indented XML, and
/// drop the control characters that would otherwise move the cursor when the
/// title is drawn in a list.
fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if ch.is_control() {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(ch);
    }
    out
}

/// Cut a string to at most `max` bytes without splitting a character.
///
/// `String::truncate` panics on a boundary it is not allowed to split, and
/// the input here is somebody else's HTML: a multi-byte character straddling
/// the cap is the ordinary case, not the unlikely one.
pub fn truncate_on_char_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> ParsedFeed {
        from_parsed(feed_rs::parser::parse(xml.as_bytes()).expect("the fixture parses"))
    }

    #[test]
    fn every_kind_round_trips_through_its_number() {
        for k in [
            FeedKind::Web,
            FeedKind::Youtube,
            FeedKind::Reddit,
            FeedKind::Hn,
        ] {
            assert_eq!(FeedKind::from_i64(k.as_i64()), k);
        }
        for k in [EntryKind::Article, EntryKind::Video, EntryKind::Post] {
            assert_eq!(EntryKind::from_i64(k.as_i64()), k);
        }
        for s in [
            ArticleStatus::Pending,
            ArticleStatus::Extracted,
            ArticleStatus::FeedContent,
            ArticleStatus::Failed,
            ArticleStatus::NotApplicable,
        ] {
            assert_eq!(ArticleStatus::from_i64(s.as_i64()), s);
        }
    }

    #[test]
    fn an_unknown_number_reads_as_the_default_rather_than_panicking() {
        // A database written by a later version is the case this exists for:
        // a feed of a kind this build has never heard of behaves like a web
        // feed instead of taking the process down.
        assert_eq!(FeedKind::from_i64(99), FeedKind::Web);
        assert_eq!(EntryKind::from_i64(-1), EntryKind::Article);
        assert_eq!(ArticleStatus::from_i64(7), ArticleStatus::Pending);
    }

    #[test]
    fn a_feeds_kind_is_decided_by_its_host() {
        let cases = [
            (
                "https://www.youtube.com/feeds/videos.xml?channel_id=UC1",
                FeedKind::Youtube,
            ),
            ("https://youtu.be/abc", FeedKind::Youtube),
            ("https://old.reddit.com/r/rust/.rss", FeedKind::Reddit),
            ("https://hnrss.org/frontpage", FeedKind::Hn),
            ("https://www.phoronix.com/rss.php", FeedKind::Web),
            ("https://example.org/atom.xml", FeedKind::Web),
        ];
        for (url, want) in cases {
            let u = url::Url::parse(url).unwrap();
            assert_eq!(FeedKind::of_url(&u), want, "{url}");
        }
    }

    #[test]
    fn a_feeds_display_title_prefers_the_rename_then_the_feeds_own() {
        let mut row = FeedRow {
            id: FeedId(1),
            url: "https://example.org/feed".into(),
            source_url: None,
            kind: FeedKind::Web,
            title: None,
            custom_title: None,
            site_url: None,
            folder: None,
            position: 0,
            unread: 0,
            last_ok: None,
            error: None,
            failures: 0,
            backoff_until: None,
        };
        assert_eq!(row.display_title(), "https://example.org/feed");
        row.title = Some("Example".into());
        assert_eq!(row.display_title(), "Example");
        row.custom_title = Some("My name for it".into());
        assert_eq!(row.display_title(), "My name for it");
        // An empty rename is not a rename; a feed whose title was cleared in
        // a text field should fall back rather than draw a blank row.
        row.custom_title = Some(String::new());
        assert_eq!(row.display_title(), "Example");
    }

    #[test]
    fn an_atom_feed_becomes_a_parsed_feed() {
        let feed = parse(
            r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>  Example
     Feed </title>
  <link rel="self" href="https://example.org/atom.xml"/>
  <link rel="alternate" href="https://example.org/"/>
  <updated>2026-09-20T10:00:00Z</updated>
  <entry>
    <id>tag:example.org,2026:1</id>
    <title>First post</title>
    <link rel="alternate" href="https://example.org/first"/>
    <published>2026-09-20T09:00:00Z</published>
    <author><name>Jane Example</name></author>
    <content type="html">&lt;p&gt;Hello.&lt;/p&gt;</content>
  </entry>
</feed>"#,
        );
        assert_eq!(feed.title.as_deref(), Some("Example Feed"));
        assert_eq!(feed.site_url.as_deref(), Some("https://example.org/"));
        assert_eq!(feed.entries.len(), 1);
        let e = &feed.entries[0];
        assert_eq!(e.guid, "tag:example.org,2026:1");
        assert_eq!(e.title, "First post");
        assert_eq!(e.url.as_deref(), Some("https://example.org/first"));
        assert_eq!(e.author.as_deref(), Some("Jane Example"));
        assert_eq!(e.content_html.as_deref(), Some("<p>Hello.</p>"));
        assert_eq!(
            e.published,
            Some("2026-09-20T09:00:00Z".parse::<Timestamp>().unwrap())
        );
    }

    #[test]
    fn an_entry_with_no_id_still_gets_a_stable_guid() {
        // RSS 2.0 without `<guid>`, which is most of the feeds that exist.
        // `feed-rs` synthesises one from the link and title; what matters
        // here is that parsing the same bytes twice gives the same string,
        // because that string is the read-state key.
        let xml = r#"<rss version="2.0"><channel>
  <title>No ids</title>
  <link>https://example.org/</link>
  <item><title>A post</title><link>https://example.org/a</link></item>
</channel></rss>"#;
        let a = parse(xml);
        let b = parse(xml);
        assert!(!a.entries[0].guid.is_empty());
        assert_eq!(a.entries[0].guid, b.entries[0].guid);
    }

    #[test]
    fn a_youtube_entry_keeps_its_description_thumbnail_and_video_id() {
        let feed = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns:yt="http://www.youtube.com/xml/schemas/2015"
      xmlns:media="http://search.yahoo.com/mrss/"
      xmlns="http://www.w3.org/2005/Atom">
  <title>A Channel</title>
  <link rel="alternate" href="https://www.youtube.com/channel/UCtest"/>
  <entry>
    <id>yt:video:dQw4w9WgXcQ</id>
    <yt:videoId>dQw4w9WgXcQ</yt:videoId>
    <title>A video</title>
    <link rel="alternate" href="https://www.youtube.com/watch?v=dQw4w9WgXcQ"/>
    <published>2026-09-19T12:00:00+00:00</published>
    <media:group>
      <media:title>A video</media:title>
      <media:thumbnail url="https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"/>
      <media:description>What the video is about.</media:description>
    </media:group>
  </entry>
</feed>"#,
        );
        let e = &feed.entries[0];
        assert_eq!(e.video_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(
            e.thumbnail_url.as_deref(),
            Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg")
        );
        assert_eq!(
            e.content_html.as_deref(),
            Some("What the video is about."),
            "media:description is the only body a YouTube entry has"
        );
    }

    #[test]
    fn an_entrys_link_has_its_tracking_parameters_taken_off() {
        let feed = parse(
            r#"<rss version="2.0"><channel><title>T</title><link>https://e.org/</link>
<item><title>x</title><link>https://e.org/x?utm_source=feed&amp;page=2</link></item>
</channel></rss>"#,
        );
        assert_eq!(
            feed.entries[0].url.as_deref(),
            Some("https://e.org/x?page=2"),
            "the link is the key two feeds carrying one article are matched on"
        );
    }

    #[test]
    fn a_feeds_site_url_is_the_alternate_link_not_its_own_address() {
        let feed = parse(
            r#"<feed xmlns="http://www.w3.org/2005/Atom">
  <title>T</title>
  <link rel="self" href="https://example.org/atom.xml"/>
  <link rel="alternate" href="https://example.org/blog"/>
</feed>"#,
        );
        assert_eq!(feed.site_url.as_deref(), Some("https://example.org/blog"));
    }

    #[test]
    fn a_body_longer_than_the_cap_is_cut_on_a_character_boundary() {
        let long = "é".repeat(MAX_CONTENT_HTML);
        let xml = format!(
            r#"<rss version="2.0"><channel><title>T</title><link>https://e.org/</link>
<item><title>x</title><link>https://e.org/x</link><description>{long}</description></item>
</channel></rss>"#
        );
        let feed = parse(&xml);
        let body = feed.entries[0].content_html.as_deref().unwrap();
        assert!(body.len() <= MAX_CONTENT_HTML, "{}", body.len());
        // The real assertion: it is still a string. A naive truncate would
        // have panicked before getting here.
        assert!(std::str::from_utf8(body.as_bytes()).is_ok());
    }

    #[test]
    fn truncating_never_splits_a_character() {
        for max in 0..12usize {
            let mut s = "aé漢🙂".to_string();
            truncate_on_char_boundary(&mut s, max);
            assert!(s.len() <= max);
            assert!(std::str::from_utf8(s.as_bytes()).is_ok());
        }
    }

    proptest::proptest! {
        /// Any bytes at all, through the parser and the conversion. The
        /// input here is a network response: this must return or fail, never
        /// panic.
        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            if let Ok(f) = feed_rs::parser::parse(bytes.as_slice()) {
                let parsed = from_parsed(f);
                for e in &parsed.entries {
                    if let Some(body) = &e.content_html {
                        proptest::prop_assert!(body.len() <= MAX_CONTENT_HTML);
                    }
                }
            }
        }

        #[test]
        fn cleaning_text_leaves_no_control_characters_or_runs_of_space(s: String) {
            let out = clean_text(&s);
            proptest::prop_assert!(!out.chars().any(char::is_control));
            proptest::prop_assert!(!out.contains("  "));
            proptest::prop_assert_eq!(out.trim(), out.as_str());
        }
    }
}
