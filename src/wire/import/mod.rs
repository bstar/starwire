//! Bringing a feed list in from somewhere else.
//!
//! Two sources, one shape. A newsboat `urls` file or an OPML export becomes
//! an [`ImportPlan`] -- a list of feeds with their canonical URLs worked out,
//! a list of read marks, and a list of what was left out and why -- and
//! [`apply_plan`] is the only thing that writes any of it.
//!
//! The plan is a separate step from applying it because `--dry-run` is worth
//! having: a person about to hand forty-one subscriptions to a new program
//! should be able to see what it made of them first. It is also what the
//! import overlay shows before asking.

pub mod newsboat;
pub mod opml;

use anyhow::Result;

use super::db::{feeds, schema::ChannelSource, youtube as db_youtube, Db};
use super::feed::{FeedKind, FolderId};
use super::youtube::ChannelId;

/// One feed, as the import worked it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFeed {
    /// The canonical URL: https, YouTube normalised.
    pub url: String,
    /// What the file actually said.
    pub source_url: String,
    pub kind: FeedKind,
    pub title: Option<String>,
    pub folder: Option<String>,
    pub channel: Option<ChannelId>,
}

/// What an import would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportPlan {
    pub feeds: Vec<PlannedFeed>,
    pub read: Vec<newsboat::ReadMark>,
    /// What was left out, and why, in the words the report prints.
    pub skipped: Vec<(String, String)>,
    /// Tags a newsboat line carried beyond the first, which became nothing.
    ///
    /// Recorded rather than silently dropped: STAR/WIRE has one folder per
    /// feed, and a person whose newsboat list used three tags on a feed
    /// deserves to be told which two did not survive.
    pub dropped_tags: Vec<(String, Vec<String>)>,
}

impl ImportPlan {
    pub fn is_empty(&self) -> bool {
        self.feeds.is_empty() && self.read.is_empty()
    }

    /// How many of the feeds are YouTube channels, which is the number the
    /// import overlay puts in its second line.
    pub fn video_count(&self) -> usize {
        self.feeds
            .iter()
            .filter(|f| f.kind == FeedKind::Youtube)
            .count()
    }

    pub fn folder_count(&self) -> usize {
        self.feeds
            .iter()
            .filter_map(|f| f.folder.as_deref())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    /// Feeds that were already on the list. Not a failure: importing the
    /// same file twice is the ordinary way somebody checks it worked.
    pub already_there: usize,
    pub skipped: Vec<(String, String)>,
    pub folders: usize,
    pub videos: usize,
    pub read_marks: usize,
}

/// Read a newsboat `urls` file, and its cache if there is one, into a plan.
pub fn plan_newsboat(urls_text: &str, cache: Option<&newsboat::CacheRows>) -> ImportPlan {
    let mut plan = ImportPlan::default();
    let mut current_folder: Option<String> = None;

    for line in newsboat::parse_urls(urls_text) {
        match line {
            newsboat::UrlsLine::Blank => {}
            // A comment header is the only grouping the reference file has:
            // `# YouTube feeds` above twenty-six lines. It is *not* turned
            // into a folder in 0.0.1 -- folders are assigned in the app, and
            // guessing from comments would put every feed in a folder
            // nobody asked for -- but it is tracked so that a later
            // `--folders-from` can use it without re-parsing.
            newsboat::UrlsLine::Comment(text) => {
                current_folder = (!text.trim().is_empty()).then(|| text.trim().to_string());
            }
            newsboat::UrlsLine::Skipped { raw, reason } => {
                plan.skipped.push((raw, reason.to_string()));
            }
            newsboat::UrlsLine::Feed(feed) => {
                let canonical = match super::youtube::canonicalise(&feed.url) {
                    Ok(c) => c,
                    Err(e) => {
                        plan.skipped.push((feed.url.clone(), e.to_string()));
                        continue;
                    }
                };
                // Titles, best first: what the `urls` file said, then what
                // the URL itself carried (`scriptbarrel`'s `name=`), then
                // what newsboat's cache learned by fetching it. For the
                // reference list the third is the only one that ever fires.
                let title = feed
                    .title
                    .clone()
                    .or_else(|| canonical.title.clone())
                    .or_else(|| {
                        cache.and_then(|c| {
                            c.titles
                                .get(&feed.url)
                                .or_else(|| c.titles.get(&canonical.url))
                                .cloned()
                        })
                    });

                // One folder per feed. The first tag is it; the rest are
                // reported rather than dropped in silence.
                let mut tags = feed.tags.clone();
                let folder = if tags.is_empty() {
                    None
                } else {
                    Some(tags.remove(0))
                };
                if !tags.is_empty() {
                    plan.dropped_tags.push((feed.url.clone(), tags));
                }

                plan.feeds.push(PlannedFeed {
                    url: canonical.url,
                    source_url: feed.url,
                    kind: canonical.kind,
                    title,
                    folder,
                    channel: canonical.channel,
                });
            }
        }
    }
    let _ = current_folder;

    if let Some(cache) = cache {
        plan.read = cache.read.clone();
    }
    plan
}

/// Write a plan.
///
/// One transaction per feed rather than one for the whole import: forty-one
/// rows take milliseconds either way, and a file with one bad line should
/// import the other forty rather than nothing.
pub fn apply_plan(db: &Db, plan: &ImportPlan) -> Result<ImportReport> {
    let mut report = ImportReport {
        skipped: plan.skipped.clone(),
        ..ImportReport::default()
    };
    let mut folders: std::collections::HashMap<String, FolderId> = std::collections::HashMap::new();

    for planned in &plan.feeds {
        let folder = match &planned.folder {
            Some(name) => {
                let id = match folders.get(name) {
                    Some(id) => *id,
                    None => {
                        let id = feeds::folder_named(db, name)?;
                        folders.insert(name.clone(), id);
                        id
                    }
                };
                Some(id)
            }
            None => None,
        };

        let source = (planned.source_url != planned.url).then_some(planned.source_url.as_str());
        let added = feeds::add(
            db,
            &planned.url,
            source,
            planned.kind,
            planned.title.as_deref(),
            folder,
        )?;
        if added.is_new() {
            report.added += 1;
        } else {
            report.already_there += 1;
        }
        if planned.kind == FeedKind::Youtube {
            report.videos += 1;
        }
        if let Some(channel) = &planned.channel {
            db_youtube::upsert(
                &db.conn,
                channel,
                planned.title.as_deref(),
                None,
                Some(added.id()),
                ChannelSource::FeedImport,
            )?;
        }
    }
    report.folders = folders.len();

    for mark in &plan.read {
        super::db::entries::put_imported_read(
            &db.conn,
            &mark.feed_url,
            &mark.guid,
            mark.url.as_deref(),
        )?;
        report.read_marks += 1;
    }
    // Entries already in the table -- a second import over a list that has
    // been fetched since -- are marked here; the rest wait in
    // `imported_read` for the fetch that brings them.
    super::db::entries::apply_imported_read(db)?;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::feed::Selection;

    fn urls_fixture() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/import/urls"),
        )
        .unwrap()
    }

    #[test]
    fn the_reference_urls_file_plans_into_feeds_with_youtube_normalised() {
        let plan = plan_newsboat(&urls_fixture(), None);
        assert!(plan.feeds.len() >= 10, "{}", plan.feeds.len());
        assert!(plan.video_count() > 0);
        for feed in &plan.feeds {
            if feed.kind == FeedKind::Youtube {
                assert!(
                    feed.url
                        .starts_with("https://www.youtube.com/feeds/videos.xml?channel_id="),
                    "{}",
                    feed.url
                );
                assert!(feed.channel.is_some());
            }
            assert!(
                feed.url.starts_with("https://"),
                "{} was not made https",
                feed.url
            );
        }
    }

    #[test]
    fn a_scriptbarrel_line_carries_the_only_title_those_channels_have() {
        let plan = plan_newsboat(
            "https://scriptbarrel.com/xml.cgi?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw&name=Some%20Channel\n",
            None,
        );
        assert_eq!(plan.feeds[0].title.as_deref(), Some("Some Channel"));
        assert_eq!(
            plan.feeds[0].source_url,
            "https://scriptbarrel.com/xml.cgi?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw&name=Some%20Channel"
        );
    }

    #[test]
    fn the_cache_supplies_a_title_for_a_line_that_has_none() {
        let mut cache = newsboat::CacheRows::default();
        cache.titles.insert(
            "https://example.org/feed.xml".into(),
            "Example Journal".into(),
        );
        let plan = plan_newsboat("https://example.org/feed.xml\n", Some(&cache));
        assert_eq!(plan.feeds[0].title.as_deref(), Some("Example Journal"));
    }

    #[test]
    fn a_title_in_the_file_beats_one_in_the_cache() {
        let mut cache = newsboat::CacheRows::default();
        cache
            .titles
            .insert("https://example.org/feed.xml".into(), "From cache".into());
        let plan = plan_newsboat(
            "https://example.org/feed.xml ~\"From the file\"\n",
            Some(&cache),
        );
        assert_eq!(plan.feeds[0].title.as_deref(), Some("From the file"));
    }

    #[test]
    fn the_first_tag_is_the_folder_and_the_rest_are_reported() {
        let plan = plan_newsboat("https://example.org/feed.xml tech rust linux\n", None);
        assert_eq!(plan.feeds[0].folder.as_deref(), Some("tech"));
        assert_eq!(
            plan.dropped_tags,
            vec![(
                "https://example.org/feed.xml".to_string(),
                vec!["rust".to_string(), "linux".to_string()]
            )],
            "one folder per feed, and the reader is told what that cost"
        );
    }

    #[test]
    fn query_lines_are_skipped_with_a_reason_a_person_can_read() {
        let plan = plan_newsboat(
            "query:Unread:unread = \"yes\"\nhttps://example.org/feed.xml\n",
            None,
        );
        assert_eq!(plan.feeds.len(), 1);
        assert_eq!(plan.skipped.len(), 1);
        assert!(
            plan.skipped[0].1.contains("saved search"),
            "{:?}",
            plan.skipped
        );
    }

    #[test]
    fn applying_a_plan_twice_adds_nothing_the_second_time() {
        let db = Db::open_in_memory().unwrap();
        let plan = plan_newsboat(&urls_fixture(), None);
        let first = apply_plan(&db, &plan).unwrap();
        assert_eq!(first.added, plan.feeds.len());
        assert_eq!(first.already_there, 0);

        let second = apply_plan(&db, &plan).unwrap();
        assert_eq!(second.added, 0);
        assert_eq!(second.already_there, plan.feeds.len());
        assert_eq!(
            feeds::list_feeds_with_unread(&db).unwrap().len(),
            plan.feeds.len()
        );
    }

    #[test]
    fn the_same_channel_by_two_routes_is_one_feed() {
        let db = Db::open_in_memory().unwrap();
        let plan = plan_newsboat(
            "https://scriptbarrel.com/xml.cgi?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw&name=Thing\n\
             https://www.youtube.com/feeds/videos.xml?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw\n",
            None,
        );
        let report = apply_plan(&db, &plan).unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(report.already_there, 1);
        assert_eq!(db_youtube::list(&db).unwrap().len(), 1);
    }

    #[test]
    fn folders_are_made_once_and_counted() {
        let db = Db::open_in_memory().unwrap();
        let plan = plan_newsboat(
            "https://a.example/feed tech\nhttps://b.example/feed tech\nhttps://c.example/feed news\n",
            None,
        );
        let report = apply_plan(&db, &plan).unwrap();
        assert_eq!(report.folders, 2);
        assert_eq!(feeds::folders(&db).unwrap().len(), 2);
    }

    #[test]
    fn read_marks_are_held_until_their_entries_arrive() {
        let db = Db::open_in_memory().unwrap();
        let mut plan = plan_newsboat("https://example.org/feed.xml\n", None);
        plan.read = vec![newsboat::ReadMark {
            feed_url: "https://example.org/feed.xml".into(),
            guid: "g1".into(),
            url: None,
        }];
        let report = apply_plan(&db, &plan).unwrap();
        assert_eq!(report.read_marks, 1);

        let feed = feeds::find(&db, "https://example.org/feed.xml")
            .unwrap()
            .unwrap();
        let mut db = db;
        let stored = crate::wire::db::entries::upsert_parsed(
            &mut db,
            feed.id,
            FeedKind::Web,
            &crate::wire::feed::ParsedFeed {
                entries: vec![
                    crate::wire::feed::ParsedEntry {
                        guid: "g1".into(),
                        title: "Already read in newsboat".into(),
                        ..crate::wire::feed::ParsedEntry::default()
                    },
                    crate::wire::feed::ParsedEntry {
                        guid: "g2".into(),
                        title: "Not read".into(),
                        ..crate::wire::feed::ParsedEntry::default()
                    },
                ],
                ..crate::wire::feed::ParsedFeed::default()
            },
        )
        .unwrap();
        assert_eq!(stored.marked_read, 1);
        assert_eq!(
            crate::wire::db::entries::unread_count(&db, &Selection::All).unwrap(),
            1
        );
    }

    #[test]
    fn an_empty_plan_says_it_is_empty() {
        assert!(ImportPlan::default().is_empty());
        let plan = plan_newsboat("# just a comment\n\n", None);
        assert!(plan.is_empty());
    }

    #[test]
    fn a_plan_counts_what_the_overlay_needs_to_ask_about() {
        let plan = plan_newsboat(
            "https://a.example/feed tech\n\
             https://www.youtube.com/feeds/videos.xml?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw news\n",
            None,
        );
        assert_eq!(plan.feeds.len(), 2);
        assert_eq!(plan.video_count(), 1);
        assert_eq!(plan.folder_count(), 2);
    }

    proptest::proptest! {
        /// Whatever somebody's ten-year-old urls file holds, planning it has
        /// to return, and every feed it plans has to be something this
        /// program can actually fetch.
        #[test]
        fn planning_arbitrary_text_gives_only_fetchable_feeds(s: String) {
            let plan = plan_newsboat(&s, None);
            for feed in &plan.feeds {
                proptest::prop_assert!(
                    feed.url.starts_with("https://"),
                    "{:?}",
                    feed.url
                );
            }
        }
    }
}
