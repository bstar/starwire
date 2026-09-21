//! OPML in and out: the format every other reader speaks.
//!
//! An OPML outline is a tree, and a reader's feed list is a tree exactly one
//! level deep -- a folder holding feeds. So: an outline with an `xmlUrl` is a
//! feed, an outline without one is a folder, and anything nested deeper than
//! that has its feeds hoisted into the nearest named ancestor rather than
//! dropped. Nobody's list is deeper than two levels, but a file written by
//! something that thought otherwise should still import.

use anyhow::{Context, Result};

use super::{ImportPlan, PlannedFeed};
use crate::wire::feed::{FeedRow, Folder};

/// Read an OPML file into a plan.
///
/// An empty `<body>` is an empty plan rather than an error: the `opml` crate
/// treats a body with no outlines as malformed, which is defensible by the
/// specification and unhelpful when the file came from this program's own
/// `export opml` on a list with nothing in it yet.
pub fn parse(text: &str) -> Result<ImportPlan> {
    let document = match opml::OPML::from_str(text) {
        Ok(d) => d,
        Err(opml::Error::BodyHasNoOutlines) => return Ok(ImportPlan::default()),
        Err(e) => return Err(anyhow::Error::new(e).context("reading the OPML file")),
    };

    let mut plan = ImportPlan::default();
    for outline in &document.body.outlines {
        walk(outline, None, &mut plan);
    }
    Ok(plan)
}

fn walk(outline: &opml::Outline, folder: Option<&str>, plan: &mut ImportPlan) {
    let name = outline
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .or(Some(outline.text.trim()))
        .filter(|t| !t.is_empty());

    match outline
        .xml_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        Some(url) => match crate::wire::youtube::canonicalise(url) {
            Ok(canonical) => plan.feeds.push(PlannedFeed {
                url: canonical.url,
                source_url: url.to_string(),
                kind: canonical.kind,
                // The outline's own title wins over one the URL carried:
                // somebody exported this list after naming things. An
                // outline whose `text` is simply its own address is not a
                // name -- that is what a reader writes when it has to fill
                // the attribute and has nothing to put in it.
                title: name
                    .filter(|n| *n != url)
                    .map(str::to_string)
                    .or(canonical.title),
                folder: folder.map(str::to_string),
                channel: canonical.channel,
            }),
            Err(e) => plan.skipped.push((url.to_string(), e.to_string())),
        },
        None => {
            // A folder. Its own name wins for its children; a nested folder
            // deeper than one level keeps the outermost name, because that
            // is the one a reader would recognise in the source list.
            let child_folder = folder.or(name);
            for child in &outline.outlines {
                walk(child, child_folder, plan);
            }
        }
    }
}

/// Write the feed list as OPML.
///
/// Version 2.0 and `type="rss"` on every feed, which is what every other
/// reader's importer looks for -- including readers that would happily have
/// taken 1.0, because there is no reason to emit the older one.
pub fn export(feeds: &[FeedRow], folders: &[Folder]) -> Result<String> {
    let mut document = opml::OPML {
        version: "2.0".into(),
        head: Some(opml::Head {
            title: Some("STAR/WIRE subscriptions".into()),
            date_created: Some(jiff::Timestamp::now().to_string()),
            ..opml::Head::default()
        }),
        body: opml::Body::default(),
    };

    let mut by_folder: std::collections::BTreeMap<i64, Vec<&FeedRow>> =
        std::collections::BTreeMap::new();
    let mut loose: Vec<&FeedRow> = Vec::new();
    for feed in feeds {
        match feed.folder {
            Some(folder) => by_folder.entry(folder.0).or_default().push(feed),
            None => loose.push(feed),
        }
    }

    for folder in folders {
        let Some(children) = by_folder.get(&folder.id.0) else {
            // An empty folder is not exported. Another reader would draw a
            // folder with nothing in it, and there is no way to tell it
            // apart from a broken import.
            continue;
        };
        document.body.outlines.push(opml::Outline {
            text: folder.name.clone(),
            title: Some(folder.name.clone()),
            outlines: children.iter().map(|f| outline_for(f)).collect(),
            ..opml::Outline::default()
        });
    }
    // A feed whose folder is not in the list passed in is still a feed, and
    // losing it on export would be losing a subscription.
    let known: std::collections::HashSet<i64> = folders.iter().map(|f| f.id.0).collect();
    for (id, children) in &by_folder {
        if !known.contains(id) {
            loose.extend(children.iter().copied());
        }
    }
    for feed in loose {
        document.body.outlines.push(outline_for(feed));
    }

    document.to_string().context("writing the OPML file")
}

fn outline_for(feed: &FeedRow) -> opml::Outline {
    // A feed nothing has named yet has no title to write. `text` still has to
    // be something -- the specification requires the attribute -- so it gets
    // the URL, but `title` is left off, and `walk` ignores a `text` that is
    // the URL. Without that pair, exporting and re-importing an unfetched
    // list turns every feed's *name* into its address for ever.
    let named = feed
        .custom_title
        .as_deref()
        .filter(|s| !s.is_empty())
        .or(feed.title.as_deref().filter(|s| !s.is_empty()));
    opml::Outline {
        text: named.unwrap_or(&feed.url).to_string(),
        title: named.map(str::to_string),
        r#type: Some("rss".into()),
        xml_url: Some(feed.url.clone()),
        html_url: feed.site_url.clone(),
        ..opml::Outline::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::feed::{FeedId, FeedKind, FolderId};

    fn feed(id: i64, url: &str, title: &str, folder: Option<i64>) -> FeedRow {
        FeedRow {
            id: FeedId(id),
            url: url.into(),
            source_url: None,
            kind: FeedKind::Web,
            title: Some(title.into()),
            custom_title: None,
            site_url: None,
            folder: folder.map(FolderId),
            position: 0,
            unread: 0,
            last_ok: None,
            error: None,
            failures: 0,
            backoff_until: None,
        }
    }

    #[test]
    fn folders_and_feeds_come_out_of_a_two_level_file() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/import/blogroll.opml"),
        )
        .unwrap();
        let plan = parse(&text).unwrap();
        assert!(plan.feeds.len() >= 3, "{:?}", plan.feeds);
        assert!(
            plan.feeds
                .iter()
                .any(|f| f.folder.as_deref() == Some("Tech")),
            "{:?}",
            plan.feeds
        );
        assert!(
            plan.feeds.iter().any(|f| f.folder.is_none()),
            "a feed at the top level has no folder"
        );
    }

    #[test]
    fn a_youtube_outline_is_normalised_on_the_way_in() {
        let xml = r#"<opml version="2.0"><body>
<outline text="A channel" type="rss"
  xmlUrl="https://scriptbarrel.com/xml.cgi?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw&amp;name=A%20channel"/>
</body></opml>"#;
        let plan = parse(xml).unwrap();
        assert_eq!(
            plan.feeds[0].url,
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCXuqSBlHAE6Xw-yeJA0Tunw"
        );
        assert_eq!(plan.feeds[0].kind, FeedKind::Youtube);
        assert!(plan.feeds[0].channel.is_some());
    }

    #[test]
    fn an_outline_with_an_address_that_cannot_be_fetched_is_skipped_with_a_reason() {
        let xml = r#"<opml version="2.0"><body>
<outline text="Bad" xmlUrl="ftp://example.org/feed"/>
<outline text="Good" xmlUrl="https://example.org/feed"/>
</body></opml>"#;
        let plan = parse(xml).unwrap();
        assert_eq!(plan.feeds.len(), 1);
        assert_eq!(plan.skipped.len(), 1);
        assert!(plan.skipped[0].0.starts_with("ftp://"));
    }

    #[test]
    fn a_feed_nobody_has_named_does_not_come_back_named_after_its_own_url() {
        let feeds = vec![FeedRow {
            title: None,
            ..feed(1, "https://a.example/feed", "ignored", None)
        }];
        let xml = export(&feeds, &[]).unwrap();
        let plan = parse(&xml).unwrap();
        assert_eq!(plan.feeds.len(), 1);
        assert_eq!(plan.feeds[0].title, None, "an address is not a name: {xml}");
    }

    #[test]
    fn a_list_round_trips_through_export_and_import() {
        let folders = vec![
            Folder {
                id: FolderId(1),
                name: "Tech".into(),
                position: 0,
            },
            Folder {
                id: FolderId(2),
                name: "Empty".into(),
                position: 1,
            },
        ];
        let feeds = vec![
            feed(1, "https://a.example/feed", "A", Some(1)),
            feed(2, "https://b.example/feed", "B", Some(1)),
            feed(3, "https://c.example/feed", "C", None),
        ];

        let xml = export(&feeds, &folders).unwrap();
        let plan = parse(&xml).unwrap();

        assert_eq!(plan.feeds.len(), 3, "{xml}");
        assert!(plan.skipped.is_empty());
        let mut got: Vec<(String, Option<String>, Option<String>)> = plan
            .feeds
            .iter()
            .map(|f| (f.url.clone(), f.title.clone(), f.folder.clone()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (
                    "https://a.example/feed".to_string(),
                    Some("A".to_string()),
                    Some("Tech".to_string())
                ),
                (
                    "https://b.example/feed".to_string(),
                    Some("B".to_string()),
                    Some("Tech".to_string())
                ),
                (
                    "https://c.example/feed".to_string(),
                    Some("C".to_string()),
                    None
                ),
            ]
        );
        assert!(!xml.contains("Empty"), "an empty folder is not exported");
    }

    #[test]
    fn a_title_with_markup_in_it_survives_the_round_trip() {
        let feeds = vec![feed(
            1,
            "https://a.example/feed",
            r#"Tom & Jerry's <b>"best"</b>"#,
            None,
        )];
        let xml = export(&feeds, &[]).unwrap();
        let plan = parse(&xml).unwrap();
        assert_eq!(
            plan.feeds[0].title.as_deref(),
            Some(r#"Tom & Jerry's <b>"best"</b>"#),
            "{xml}"
        );
    }

    #[test]
    fn an_empty_list_exports_and_imports_as_nothing_rather_than_as_an_error() {
        let xml = export(&[], &[]).unwrap();
        let plan = parse(&xml).unwrap();
        assert!(plan.feeds.is_empty());
    }

    #[test]
    fn a_feed_in_a_folder_nobody_named_is_still_exported() {
        let feeds = vec![feed(1, "https://a.example/feed", "A", Some(99))];
        let xml = export(&feeds, &[]).unwrap();
        let plan = parse(&xml).unwrap();
        assert_eq!(plan.feeds.len(), 1, "a subscription was lost: {xml}");
        assert!(plan.feeds[0].folder.is_none());
    }

    #[test]
    fn feeds_nested_deeper_than_a_folder_are_hoisted_rather_than_lost() {
        let xml = r#"<opml version="2.0"><body>
<outline text="Outer"><outline text="Inner">
  <outline text="Deep" type="rss" xmlUrl="https://deep.example/feed"/>
</outline></outline>
</body></opml>"#;
        let plan = parse(xml).unwrap();
        assert_eq!(plan.feeds.len(), 1);
        assert_eq!(plan.feeds[0].folder.as_deref(), Some("Outer"));
    }

    #[test]
    fn something_that_is_not_opml_at_all_says_so() {
        assert!(parse("not xml").is_err());
        assert!(parse("<html><body>no</body></html>").is_err());
    }

    proptest::proptest! {
        /// A file exported by somebody else's reader.
        #[test]
        fn arbitrary_text_never_panics(s: String) {
            let _ = parse(&s);
        }
    }
}
