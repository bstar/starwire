//! The channel table beside the feeds.
//!
//! A YouTube channel is a feed like any other, and the feed row is where its
//! entries hang. This table exists for the one thing the feed row cannot
//! answer: *is this channel already subscribed to*, when the thing being
//! asked about is a `@handle`, a bare `UC…` from a Takeout export, or a
//! `scriptbarrel.com` URL. All three reduce to a channel id, and the channel
//! id is this table's key.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use super::{now, schema::ChannelSource, Db};
use crate::wire::feed::FeedId;
use crate::wire::youtube::ChannelId;

/// A subscribed channel, as `starwire youtube list` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub channel_id: ChannelId,
    pub title: Option<String>,
    pub handle: Option<String>,
    pub feed: Option<FeedId>,
    pub source: ChannelSource,
}

/// Record a channel, filling in what is known and leaving alone what is not.
///
/// The `COALESCE`s matter: a channel that arrived from a newsboat import has
/// a title and no handle, the same channel arriving later from `youtube add
/// @name` has a handle and possibly a better title, and neither should erase
/// the other's contribution. `source` is *not* overwritten -- where a channel
/// first came from is a fact about history, and the later route did not
/// subscribe to it.
pub fn upsert(
    db: &rusqlite::Connection,
    channel: &ChannelId,
    title: Option<&str>,
    handle: Option<&str>,
    feed: Option<FeedId>,
    source: ChannelSource,
) -> Result<()> {
    db.execute(
        "INSERT INTO youtube_channel(channel_id, title, handle, feed_id, source, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(channel_id) DO UPDATE SET
           title   = COALESCE(excluded.title, youtube_channel.title),
           handle  = COALESCE(excluded.handle, youtube_channel.handle),
           feed_id = COALESCE(excluded.feed_id, youtube_channel.feed_id)",
        params![
            channel.as_str(),
            title.map(str::trim).filter(|t| !t.is_empty()),
            handle.map(str::trim).filter(|t| !t.is_empty()),
            feed.map(|f| f.0),
            source.as_i64(),
            now(),
        ],
    )?;
    Ok(())
}

pub fn list(db: &Db) -> Result<Vec<Channel>> {
    let mut stmt = db.conn.prepare(
        "SELECT c.channel_id, COALESCE(c.title, f.custom_title, f.title), c.handle,
                c.feed_id, c.source
         FROM youtube_channel c
         LEFT JOIN feed f ON f.id = c.feed_id
         ORDER BY COALESCE(c.title, f.custom_title, f.title, c.channel_id) COLLATE NOCASE",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Channel {
                channel_id: ChannelId::from_stored(r.get::<_, String>(0)?),
                title: r.get(1)?,
                handle: r.get(2)?,
                feed: r.get::<_, Option<i64>>(3)?.map(FeedId),
                source: ChannelSource::from_i64(r.get(4)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Whether this channel is already subscribed to. What makes a second
/// Takeout import, or a `youtube sync` over a list already imported from
/// newsboat, add nothing.
pub fn known(db: &Db, channel: &ChannelId) -> Result<bool> {
    Ok(db
        .conn
        .query_row(
            "SELECT 1 FROM youtube_channel WHERE channel_id = ?1",
            [channel.as_str()],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}

/// Every channel id already on the list, for deduplicating a sync of a few
/// hundred subscriptions without a query each.
pub fn known_ids(db: &Db) -> Result<std::collections::HashSet<String>> {
    let mut stmt = db.conn.prepare("SELECT channel_id FROM youtube_channel")?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::db::feeds;
    use crate::wire::feed::FeedKind;

    fn channel(s: &str) -> ChannelId {
        ChannelId::parse(s).unwrap()
    }

    #[test]
    fn a_channel_gathers_what_each_route_knows_about_it() {
        let db = Db::open_in_memory().unwrap();
        let id = channel("UCXuqSBlHAE6Xw-yeJA0Tunw");

        // From an imported feed list: a title, no handle.
        upsert(
            &db.conn,
            &id,
            Some("Linus Tech Tips"),
            None,
            None,
            ChannelSource::FeedImport,
        )
        .unwrap();
        // Later, from a paste: a handle, no title.
        let feed = feeds::add(
            &db,
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw",
            None,
            FeedKind::Youtube,
            None,
            None,
        )
        .unwrap()
        .id();
        upsert(
            &db.conn,
            &id,
            None,
            Some("@LinusTechTips"),
            Some(feed),
            ChannelSource::Added,
        )
        .unwrap();

        let rows = list(&db).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title.as_deref(), Some("Linus Tech Tips"));
        assert_eq!(rows[0].handle.as_deref(), Some("@LinusTechTips"));
        assert_eq!(rows[0].feed, Some(feed));
        assert_eq!(
            rows[0].source,
            ChannelSource::FeedImport,
            "where it first came from is not rewritten by a later sighting"
        );
    }

    #[test]
    fn a_known_channel_is_recognised_however_it_is_asked_about() {
        let db = Db::open_in_memory().unwrap();
        let id = channel("UCXuqSBlHAE6Xw-yeJA0Tunw");
        assert!(!known(&db, &id).unwrap());
        upsert(&db.conn, &id, None, None, None, ChannelSource::Takeout).unwrap();
        assert!(known(&db, &id).unwrap());
        assert!(known_ids(&db).unwrap().contains(id.as_str()));
    }

    #[test]
    fn removing_the_feed_removes_the_channel_with_it() {
        let db = Db::open_in_memory().unwrap();
        let id = channel("UCXuqSBlHAE6Xw-yeJA0Tunw");
        let feed = feeds::add(
            &db,
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw",
            None,
            FeedKind::Youtube,
            None,
            None,
        )
        .unwrap()
        .id();
        upsert(&db.conn, &id, None, None, Some(feed), ChannelSource::Added).unwrap();
        feeds::remove(&db, feed).unwrap();
        assert!(
            !known(&db, &id).unwrap(),
            "unsubscribing from the feed is unsubscribing from the channel"
        );
    }

    #[test]
    fn a_channel_with_no_title_anywhere_still_lists_under_its_id() {
        let db = Db::open_in_memory().unwrap();
        let id = channel("UCXuqSBlHAE6Xw-yeJA0Tunw");
        upsert(&db.conn, &id, None, None, None, ChannelSource::YtSubs).unwrap();
        let rows = list(&db).unwrap();
        assert_eq!(rows[0].channel_id, id);
        assert!(rows[0].title.is_none());
    }
}
