//! What is kept, and the pragmas the file is opened under.
//!
//! Three decisions here are worth defending.
//!
//! **Read state lives on the entry, as bits, and unread counts are always a
//! query.** A cached count on the `feed` row would be one more thing to keep
//! true across an import, a retention sweep and a second process writing
//! through WAL, and SQLite counts a partial index faster than any of those
//! could go wrong.
//!
//! **`article` is a separate table with a row created alongside every
//! entry.** Extraction is the slow part and the part that fails, and keeping
//! its status, its attempt count and its error beside the text means
//! `pending(limit)` is one indexed scan rather than a join against a table of
//! everything ever seen.
//!
//! **`imported_read` is a table rather than a pass over `entry`.** newsboat's
//! read marks arrive keyed by feed URL and guid, for entries this program has
//! mostly not fetched yet. Holding them until the matching entry turns up in
//! a fetch is the only way an import can mark as read something that is not
//! there yet, and it is why importing read state before the first refresh
//! works at all.

/// Bumped when the DDL below changes in a way an existing file needs helping
/// across. `db::migrate` is where that help goes.
pub const SCHEMA_VERSION: i32 = 1;

/// The same set STAR/AMP opens its index with, for the same reasons.
///
/// `journal_mode = WAL` is the one that matters most here: `starwire fetch`
/// from a systemd timer and an open window are two processes on one file, and
/// WAL is what lets the window keep reading while the timer writes.
/// `busy_timeout` is what turns the remaining overlap into a wait rather than
/// an error.
pub const PRAGMAS: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA cache_size   = -65536;
";

pub const SCHEMA: &str = r#"
-- ---------- the feed list ----------
CREATE TABLE IF NOT EXISTS folder (
  id       INTEGER PRIMARY KEY,
  name     TEXT NOT NULL UNIQUE,
  position INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS feed (
  id            INTEGER PRIMARY KEY,
  -- Canonical: https, and a YouTube channel normalised to
  -- videos.xml?channel_id=. UNIQUE, so adding a feed twice is a no-op
  -- rather than two rows whose unread counts disagree.
  url           TEXT NOT NULL UNIQUE,
  -- As typed or imported. A scriptbarrel URL rewritten to the YouTube one
  -- is still recognisable here, which is what makes a second import of the
  -- same newsboat file idempotent.
  source_url    TEXT,
  kind          INTEGER NOT NULL,            -- 0 web, 1 youtube, 2 reddit, 3 hn
  title         TEXT,
  custom_title  TEXT,
  site_url      TEXT,
  folder_id     INTEGER REFERENCES folder(id) ON DELETE SET NULL,
  position      INTEGER NOT NULL DEFAULT 0,
  -- Conditional GET. A feed that answers 304 costs a round trip and no body,
  -- which over 41 feeds every fifteen minutes is most of the traffic saved.
  etag          TEXT,
  last_modified TEXT,
  last_fetch    INTEGER,
  last_ok       INTEGER,
  last_error    TEXT,
  failures      INTEGER NOT NULL DEFAULT 0,
  backoff_until INTEGER,
  added_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS feed_folder_idx ON feed(folder_id, position);
CREATE INDEX IF NOT EXISTS feed_due_idx    ON feed(last_fetch);

-- ---------- what arrived ----------
CREATE TABLE IF NOT EXISTS entry (
  id            INTEGER PRIMARY KEY,
  feed_id       INTEGER NOT NULL REFERENCES feed(id) ON DELETE CASCADE,
  -- feed-rs's Entry::id, synthesised from the link and title when the feed
  -- carries neither a guid nor an Atom id -- the same thing newsboat does,
  -- and the reason read state survives a feed with no ids.
  guid          TEXT NOT NULL,
  url           TEXT,
  title         TEXT NOT NULL DEFAULT '',
  author        TEXT,
  kind          INTEGER NOT NULL DEFAULT 0,  -- 0 article, 1 video, 2 post
  published     INTEGER,
  fetched_at    INTEGER NOT NULL,
  content_html  TEXT,
  thumbnail_url TEXT,
  video_id      TEXT,
  duration_secs INTEGER,
  read          INTEGER NOT NULL DEFAULT 0,
  -- Starred is also read-later: there is one flag in 0.0.1, deliberately,
  -- because two lists nobody empties are worse than one.
  starred       INTEGER NOT NULL DEFAULT 0,
  UNIQUE(feed_id, guid)
);
CREATE INDEX IF NOT EXISTS entry_feed_pub_idx ON entry(feed_id, published DESC);
-- Partial, because counting unread entries is the query the source list runs
-- once per feed on every refresh, and the read rows are the overwhelming
-- majority of the table a month in.
CREATE INDEX IF NOT EXISTS entry_unread_idx   ON entry(feed_id) WHERE read = 0;
CREATE INDEX IF NOT EXISTS entry_url_idx      ON entry(url);
CREATE INDEX IF NOT EXISTS entry_starred_idx  ON entry(starred) WHERE starred = 1;
CREATE INDEX IF NOT EXISTS entry_kind_pub_idx ON entry(kind, published DESC);
CREATE INDEX IF NOT EXISTS entry_pub_idx      ON entry(published DESC);

-- ---------- what was made of it ----------
CREATE TABLE IF NOT EXISTS article (
  entry_id     INTEGER PRIMARY KEY REFERENCES entry(id) ON DELETE CASCADE,
  -- 0 pending, 1 extracted, 2 the feed's own text, 3 failed (the fallback
  -- text is still stored), 4 not applicable (a video or a post).
  status       INTEGER NOT NULL,
  title        TEXT,
  -- CommonMark. The core does not parse it; the reader does, at the width
  -- it is drawing.
  markdown     TEXT,
  byline       TEXT,
  site_name    TEXT,
  image_url    TEXT,
  excerpt      TEXT,
  source_url   TEXT,
  extracted_at INTEGER,
  attempts     INTEGER NOT NULL DEFAULT 0,
  error        TEXT
);
CREATE INDEX IF NOT EXISTS article_pending_idx ON article(status) WHERE status = 0;

-- External-content FTS5: the index holds the tokens and the `article` table
-- holds the text, rather than a second copy of every article in the file.
-- `content_rowid` is entry_id, which is article's own primary key.
CREATE VIRTUAL TABLE IF NOT EXISTS article_fts USING fts5(
  title,
  markdown,
  content='article',
  content_rowid='entry_id',
  tokenize='unicode61 remove_diacritics 2'
);

-- The three triggers an external-content table needs to stay in step. The
-- delete rows use the 'delete' command with the *old* values, which is how
-- FTS5 is told to remove tokens it can no longer read out of the content
-- table.
CREATE TRIGGER IF NOT EXISTS article_ai AFTER INSERT ON article BEGIN
  INSERT INTO article_fts(rowid, title, markdown)
  VALUES (new.entry_id, new.title, new.markdown);
END;
CREATE TRIGGER IF NOT EXISTS article_ad AFTER DELETE ON article BEGIN
  INSERT INTO article_fts(article_fts, rowid, title, markdown)
  VALUES ('delete', old.entry_id, old.title, old.markdown);
END;
CREATE TRIGGER IF NOT EXISTS article_au AFTER UPDATE ON article BEGIN
  INSERT INTO article_fts(article_fts, rowid, title, markdown)
  VALUES ('delete', old.entry_id, old.title, old.markdown);
  INSERT INTO article_fts(rowid, title, markdown)
  VALUES (new.entry_id, new.title, new.markdown);
END;

-- ---------- YouTube ----------
-- Channels are tracked beside their feeds rather than only as feeds, because
-- a channel id is what every one of the three import routes produces and
-- what deduplicates them: the same channel arrives as a handle from a paste,
-- as a bare id from Takeout, and as a videos.xml URL from newsboat.
CREATE TABLE IF NOT EXISTS youtube_channel (
  channel_id TEXT PRIMARY KEY,
  title      TEXT,
  handle     TEXT,
  feed_id    INTEGER REFERENCES feed(id) ON DELETE CASCADE,
  source     INTEGER NOT NULL,              -- 0 feed import, 1 add, 2 takeout, 3 ytsubs
  added_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS youtube_feed_idx ON youtube_channel(feed_id);

-- ---------- bookkeeping ----------
-- One row per fetch, kept a week. `starwire list --feeds` reads the last
-- error out of the feed row; this is what makes "it has been failing since
-- Tuesday" answerable at all.
CREATE TABLE IF NOT EXISTS fetch_log (
  id          INTEGER PRIMARY KEY,
  feed_id     INTEGER REFERENCES feed(id) ON DELETE CASCADE,
  at          INTEGER NOT NULL,
  status      INTEGER,
  bytes       INTEGER,
  new_entries INTEGER,
  millis      INTEGER,
  error       TEXT
);
CREATE INDEX IF NOT EXISTS fetch_log_at_idx ON fetch_log(at);

-- newsboat's `unread = 0` rows, held until the matching entry arrives in a
-- fetch. Keyed by feed URL and guid because that is all newsboat's cache
-- knows and this program has no entry ids for them yet.
CREATE TABLE IF NOT EXISTS imported_read (
  feed_url TEXT NOT NULL,
  guid     TEXT NOT NULL,
  url      TEXT,
  PRIMARY KEY (feed_url, guid)
);

CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT
);
"#;

/// Keys in `meta`, named rather than spelled at each call site.
pub mod meta {
    pub const LAST_REFRESH: &str = "last_refresh";
    pub const LAST_RETENTION: &str = "last_retention";
    pub const NEWSBOAT_IMPORTED_AT: &str = "newsboat_imported_at";
    /// Set by `DismissImportOffer`, so the offer is made once and never
    /// again -- a prompt that comes back after being refused is a bug, not a
    /// reminder.
    pub const NEWSBOAT_OFFER_DECLINED: &str = "newsboat_offer_declined";
}

/// Where a YouTube channel came from, for `youtube_channel.source`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChannelSource {
    /// It was already in a feed list that was imported.
    #[default]
    FeedImport = 0,
    /// `starwire youtube add`, or the `a` overlay.
    Added = 1,
    /// Google Takeout's `subscriptions.csv`.
    Takeout = 2,
    /// `yt-dlp :ytsubs`, with the browser's cookies.
    YtSubs = 3,
}

impl ChannelSource {
    pub fn as_i64(self) -> i64 {
        self as i64
    }

    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Added,
            2 => Self::Takeout,
            3 => Self::YtSubs,
            _ => Self::FeedImport,
        }
    }
}
