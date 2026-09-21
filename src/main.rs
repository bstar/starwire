//! STAR/WIRE — a stack-based terminal news reader.

mod cli;
mod config;
mod paths;
mod ui;
mod wire;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser as _;

use paths::PATHS;
use wire::db::{articles, entries, feeds, youtube as db_youtube, Db};
use wire::feed::{ArticleStatus, EntryKind, FeedKind, Selection};
use wire::net::Http;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    PATHS.init_private_dirs();
    // The guard must outlive everything that logs; dropping it early loses
    // whatever the writer thread had buffered.
    let _log = starkit::logging::init(&PATHS, cli.verbose)?;

    let config_path = PATHS.config_file()?;
    let cfg = config::Config::load(&config_path)?;
    let core = cfg.core();

    // Every subcommand that reaches the network builds its client the same
    // way, and `--replay` is the one switch that changes what "the network"
    // means.
    let http = build_http(&core, cli.replay.as_deref())?;

    match cli.command {
        None => run_tui(&cfg, &config_path),
        Some(cli::Command::Fetch { feed, no_extract }) => run_fetch(
            &core,
            http.as_ref(),
            feed.as_deref(),
            no_extract,
            cli.replay.as_deref(),
        ),
        Some(cli::Command::List {
            unread,
            feed,
            feeds: list_feeds,
            limit,
            json,
        }) => run_list(unread, feed.as_deref(), list_feeds, limit, json),
        Some(cli::Command::Show { entry, format }) => run_show(entry, &format),
        Some(cli::Command::Add { url, title, folder }) => run_add(
            &core,
            http.as_ref(),
            &url,
            title.as_deref(),
            folder.as_deref(),
        ),
        Some(cli::Command::Remove { feed }) => run_remove(&feed),
        Some(cli::Command::Import(command)) => run_import(command),
        Some(cli::Command::Export(cli::ExportCommand::Opml { file })) => {
            run_export_opml(file.as_deref())
        }
        Some(cli::Command::Youtube(command)) => run_youtube(&core, http.as_ref(), command),
        Some(cli::Command::Extract { url }) => run_extract(&core, http.as_ref(), &url),
    }
}

fn build_http(core: &wire::WireConfig, replay: Option<&Path>) -> Result<Box<dyn Http>> {
    match replay {
        Some(dir) => Ok(Box::new(wire::net::Replay::open(dir)?)),
        None => Ok(Box::new(wire::net::Live::new(core))),
    }
}

fn open_db() -> Result<Db> {
    Db::open(&paths::db_file()?)
}

/// Print, and treat a closed pipe as the reader being done.
///
/// `starwire list | head` is the ordinary way to look at a long list, and a
/// closed pipe is not an error worth a backtrace.
fn print(out: &mut impl std::io::Write, line: &str) -> std::ops::ControlFlow<()> {
    match writeln!(out, "{line}") {
        Ok(()) => std::ops::ControlFlow::Continue(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::ops::ControlFlow::Break(()),
        Err(_) => std::ops::ControlFlow::Break(()),
    }
}

// ---------------------------------------------------------------- fetch ----

/// `starwire fetch`: a refresh with no window involved.
///
/// The same three functions the worker threads call -- `fetch::fetch`,
/// `entries::upsert_parsed`, `extract::run` + `articles::put` -- run here on
/// scoped threads, in two phases. Two phases rather than a pipeline because
/// the database has one writer by construction: the fetches happen in
/// parallel and the writes happen on this thread, and the same again for the
/// extractions.
///
/// Exits 1 when every feed failed, which is what a systemd timer needs in
/// order to report a unit as failed rather than to log nothing and succeed.
fn run_fetch(
    core: &wire::WireConfig,
    http: &dyn Http,
    only: Option<&str>,
    no_extract: bool,
    replay: Option<&Path>,
) -> Result<()> {
    let mut db = open_db()?;
    seed_replay_feeds(&db, http, replay)?;

    let due = match only {
        Some(needle) => {
            let feed = feeds::find(&db, needle)?
                .ok_or_else(|| anyhow::anyhow!("no feed here matches {needle}"))?;
            feeds::due(&db, true)?
                .into_iter()
                .filter(|f| f.id == feed.id)
                .collect()
        }
        None => feeds::due(&db, false)?,
    };

    if due.is_empty() {
        eprintln!("nothing to fetch: the feed list is empty, or everything is backing off");
        return Ok(());
    }

    let total = due.len();
    let max_feed_bytes = core.fetch.max_feed_bytes;
    let (send_result, results) = crossbeam_channel::unbounded();
    let (send_job, jobs) = crossbeam_channel::unbounded();
    for feed in due {
        send_job.send(feed).expect("the receiver is alive");
    }
    drop(send_job);

    std::thread::scope(|scope| {
        for _ in 0..core.fetch.parallel.min(total) {
            let jobs = jobs.clone();
            let send_result = send_result.clone();
            scope.spawn(move || {
                for feed in jobs {
                    let started = std::time::Instant::now();
                    let outcome =
                        wire::fetch::fetch(http, &feed.url, &feed.conditional, max_feed_bytes);
                    let _ = send_result.send((feed, outcome, started.elapsed()));
                }
            });
        }
        drop(send_result);
        drop(jobs);
    });

    let mut failed = 0usize;
    let mut new_entries = 0usize;
    let mut pending: Vec<(wire::feed::EntryId, String)> = Vec::new();

    for (feed, outcome, elapsed) in results {
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                failed += 1;
                feeds::record_failure(
                    &db,
                    feed.id,
                    &e.to_string(),
                    core.fetch.refresh_minutes,
                    None,
                )?;
                eprintln!("{}: {e}", feed.url);
                continue;
            }
        };
        match outcome {
            wire::fetch::FetchOutcome::NotModified => {
                feeds::record_ok(&db, feed.id, &feed.conditional, None, None)?;
                feeds::log_fetch(
                    &db,
                    feed.id,
                    Some(304),
                    Some(0),
                    Some(0),
                    elapsed.as_millis(),
                    None,
                )?;
            }
            wire::fetch::FetchOutcome::Fetched {
                feed: parsed,
                conditional,
                bytes,
            } => {
                feeds::record_ok(
                    &db,
                    feed.id,
                    &conditional,
                    parsed.title.as_deref(),
                    parsed.site_url.as_deref(),
                )?;
                let stored = entries::upsert_parsed(&mut db, feed.id, feed.kind, &parsed)?;
                new_entries += stored.new.len();
                pending.extend(stored.extractable);
                feeds::log_fetch(
                    &db,
                    feed.id,
                    Some(200),
                    Some(bytes),
                    Some(stored.new.len()),
                    elapsed.as_millis(),
                    None,
                )?;
            }
            wire::fetch::FetchOutcome::Failed {
                error,
                status,
                retry_after,
            } => {
                failed += 1;
                feeds::record_failure(
                    &db,
                    feed.id,
                    &error,
                    core.fetch.refresh_minutes,
                    retry_after,
                )?;
                feeds::log_fetch(
                    &db,
                    feed.id,
                    status,
                    None,
                    None,
                    elapsed.as_millis(),
                    Some(&error),
                )?;
                eprintln!("{}: {error}", feed.url);
            }
        }
    }

    let mut extracted = 0usize;
    if core.articles.extract && !no_extract {
        // Whatever this run brought in, plus whatever earlier runs left
        // pending -- an entry whose page was down an hour ago should be
        // picked up now rather than waiting for the feed to change.
        let mut queue = pending;
        let backlog = articles::pending(&db, 200)?;
        let already: std::collections::HashSet<_> = queue.iter().map(|(id, _)| *id).collect();
        queue.extend(backlog.into_iter().filter(|(id, _)| !already.contains(id)));
        extracted = extract_all(core, http, &mut db, &queue)?;
    }

    db.set_meta(
        wire::db::schema::meta::LAST_REFRESH,
        &wire::db::now().to_string(),
    )?;
    feeds::sweep_fetch_log(&db)?;

    let mut out = std::io::stdout().lock();
    let _ = print(
        &mut out,
        &format!(
            "{} feed{} · {new_entries} new · {extracted} extracted{}",
            total,
            if total == 1 { "" } else { "s" },
            if failed > 0 {
                format!(" · {failed} failed")
            } else {
                String::new()
            }
        ),
    );

    if failed == total {
        std::process::exit(1);
    }
    Ok(())
}

/// Pull pages in parallel and write the results here.
fn extract_all(
    core: &wire::WireConfig,
    http: &dyn Http,
    db: &mut Db,
    queue: &[(wire::feed::EntryId, String)],
) -> Result<usize> {
    if queue.is_empty() {
        return Ok(0);
    }
    let limits = wire::extract::Limits::from(&core.articles);
    let (send_result, results) = crossbeam_channel::unbounded();
    let (send_job, jobs) = crossbeam_channel::unbounded();
    for job in queue {
        send_job.send(job.clone()).expect("the receiver is alive");
    }
    drop(send_job);

    std::thread::scope(|scope| {
        for _ in 0..core.fetch.parallel.min(queue.len()) {
            let jobs = jobs.clone();
            let send_result = send_result.clone();
            scope.spawn(move || {
                for (entry, url) in jobs {
                    let result = wire::extract::run(http, &url, limits);
                    let _ = send_result.send((entry, result));
                }
            });
        }
        drop(send_result);
        drop(jobs);
    });

    let mut ok = 0usize;
    for (entry, result) in results {
        match result {
            Ok(result) => {
                if result.status == ArticleStatus::Extracted {
                    ok += 1;
                }
                articles::put(db, entry, &result)?;
            }
            Err(e) => tracing::warn!("extracting {entry}: {e}"),
        }
    }
    Ok(ok)
}

/// A replay directory may name the feeds to seed an empty database with, so
/// that `starwire --replay testdata/replay fetch` works on a machine that
/// has never run this before. A database with feeds in it is left alone.
fn seed_replay_feeds(db: &Db, http: &dyn Http, replay: Option<&Path>) -> Result<()> {
    let Some(dir) = replay else {
        return Ok(());
    };
    if http.is_live() || !feeds::list_feeds_with_unread(db)?.is_empty() {
        return Ok(());
    }
    let replay = wire::net::Replay::open(dir)?;
    for url in replay.feeds() {
        let canonical = wire::youtube::canonicalise(url)?;
        feeds::add(
            db,
            &canonical.url,
            (canonical.url != *url).then_some(url.as_str()),
            canonical.kind,
            canonical.title.as_deref(),
            None,
        )?;
    }
    Ok(())
}

// ----------------------------------------------------------------- list ----

fn run_list(
    unread_only: bool,
    feed: Option<&str>,
    list_feeds: bool,
    limit: usize,
    json: bool,
) -> Result<()> {
    let db = open_db()?;
    let mut out = std::io::stdout().lock();

    if list_feeds {
        for row in feeds::list_feeds_with_unread(&db)? {
            let line = if json {
                serde_json::json!({
                    "id": row.id.0,
                    "url": row.url,
                    "title": row.display_title(),
                    "kind": format!("{:?}", row.kind).to_lowercase(),
                    "unread": row.unread,
                    "failures": row.failures,
                    "error": row.error,
                })
                .to_string()
            } else {
                let mark = match &row.error {
                    Some(e) => format!("  ! {e}"),
                    None => String::new(),
                };
                format!(
                    "{:>6}  {:>5}  {}{mark}",
                    row.id.0,
                    row.unread,
                    row.display_title()
                )
            };
            if print(&mut out, &line).is_break() {
                return Ok(());
            }
        }
        return Ok(());
    }

    let selection = match feed {
        Some(needle) => {
            let row = feeds::find(&db, needle)?
                .ok_or_else(|| anyhow::anyhow!("no feed here matches {needle}"))?;
            Selection::Feed(row.id)
        }
        None => Selection::All,
    };

    let page = entries::page(&db, &selection, unread_only, 0, limit)?;
    for row in page.rows {
        let line = if json {
            serde_json::json!({
                "id": row.id.0,
                "feed_id": row.feed_id.0,
                "feed": row.feed_title,
                "title": row.title,
                "author": row.author,
                "url": row.url,
                "published": row.published.map(|t| t.to_string()),
                "kind": format!("{:?}", row.kind).to_lowercase(),
                "read": row.read,
                "starred": row.starred,
                "article": format!("{:?}", row.article_status).to_lowercase(),
            })
            .to_string()
        } else {
            let flags = format!(
                "{}{}{}",
                if row.read { " " } else { "*" },
                if row.starred { "+" } else { " " },
                match row.kind {
                    EntryKind::Video => "v",
                    EntryKind::Post => "p",
                    EntryKind::Article => " ",
                }
            );
            format!(
                "{:>6}  {flags}  {:<22}  {}",
                row.id.0,
                elide(&row.feed_title, 22),
                row.title
            )
        };
        if print(&mut out, &line).is_break() {
            return Ok(());
        }
    }
    Ok(())
}

/// Cut a string to `width` display columns, with an ellipsis where it was
/// cut. On character boundaries, because a feed title is somebody else's
/// text and multi-byte characters are the ordinary case.
fn elide(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let kept: String = s.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

// ----------------------------------------------------------------- show ----

fn run_show(entry: i64, format: &cli::ShowFormat) -> Result<()> {
    let db = open_db()?;
    let id = wire::feed::EntryId(entry);
    let row = entries::get(&db, id)?
        .ok_or_else(|| anyhow::anyhow!("there is no entry {entry}; `starwire list` has the ids"))?;
    let article = articles::get(&db, id)?;
    let mut out = std::io::stdout().lock();

    if format.url {
        if let Some(url) = row.url.as_deref() {
            let _ = print(&mut out, url);
        }
        return Ok(());
    }

    if format.html {
        match entries::content_html(&db, id)? {
            Some(html) => {
                let _ = print(&mut out, &html);
            }
            None => anyhow::bail!("entry {entry} carried no HTML of its own"),
        }
        return Ok(());
    }

    let markdown = article
        .as_ref()
        .map(|a| a.markdown.to_string())
        .unwrap_or_default();

    if format.markdown {
        let _ = print(&mut out, markdown.trim_end());
        return Ok(());
    }

    // The default: a header a person can read, then the text.
    let _ = print(&mut out, &row.title);
    let mut meta = Vec::new();
    if let Some(author) = &row.author {
        meta.push(author.clone());
    }
    meta.push(row.feed_title.clone());
    if let Some(published) = row.published {
        meta.push(published.to_string());
    }
    let _ = print(&mut out, &meta.join(" · "));
    if let Some(url) = &row.url {
        let _ = print(&mut out, url);
    }
    if let Some(article) = &article {
        if article.status == ArticleStatus::Failed {
            let reason = article.error.as_deref().unwrap_or("it did not yield");
            let _ = print(&mut out, &format!("(the page was not extracted: {reason})"));
        }
    }
    let _ = print(&mut out, "");
    if markdown.trim().is_empty() {
        let _ = print(&mut out, "(nothing has been read from this entry yet)");
    } else {
        let _ = print(&mut out, markdown.trim_end());
    }
    Ok(())
}

// ------------------------------------------------------------ add/remove ----

fn run_add(
    core: &wire::WireConfig,
    http: &dyn Http,
    what: &str,
    title: Option<&str>,
    folder: Option<&str>,
) -> Result<()> {
    let db = open_db()?;
    let mut out = std::io::stdout().lock();

    // A handle or a channel page has to be resolved before there is a URL to
    // store; everything else canonicalises without a request.
    let (canonical, channel, resolved_title) = match wire::youtube::detect(what) {
        Some(wire::youtube::Detected::Channel(id)) => {
            (wire::youtube::canonicalise(&id.feed_url())?, Some(id), None)
        }
        Some(
            detected @ (wire::youtube::Detected::Handle(_) | wire::youtube::Detected::Page(_)),
        ) => {
            let resolved = wire::youtube::resolve(http, &detected, &core.youtube)?;
            (
                wire::youtube::canonicalise(&resolved.channel.feed_url())?,
                Some(resolved.channel),
                resolved.title,
            )
        }
        None => (wire::youtube::canonicalise(what)?, None, None),
    };

    let folder_id = match folder {
        Some(name) => Some(feeds::folder_named(&db, name)?),
        None => None,
    };
    let title = title
        .map(str::to_string)
        .or(resolved_title)
        .or(canonical.title.clone());

    let added = feeds::add(
        &db,
        &canonical.url,
        (canonical.url != what).then_some(what),
        canonical.kind,
        title.as_deref(),
        folder_id,
    )?;
    if let Some(channel) = &channel {
        db_youtube::upsert(
            &db.conn,
            channel,
            title.as_deref(),
            match wire::youtube::detect(what) {
                Some(wire::youtube::Detected::Handle(h)) => Some(h),
                _ => None,
            }
            .as_deref(),
            Some(added.id()),
            wire::db::schema::ChannelSource::Added,
        )?;
    }

    let _ = print(
        &mut out,
        &format!(
            "{} {}  {}",
            if added.is_new() {
                "added"
            } else {
                "already there:"
            },
            added.id(),
            canonical.url
        ),
    );
    Ok(())
}

fn run_remove(needle: &str) -> Result<()> {
    let db = open_db()?;
    let row = feeds::find(&db, needle)?
        .ok_or_else(|| anyhow::anyhow!("no feed here matches {needle}"))?;
    let title = row.display_title().to_string();
    feeds::remove(&db, row.id)?;
    let mut out = std::io::stdout().lock();
    let _ = print(&mut out, &format!("removed {} ({title})", row.id));
    Ok(())
}

// --------------------------------------------------------------- import ----

fn run_import(command: cli::ImportCommand) -> Result<()> {
    match command {
        cli::ImportCommand::Newsboat {
            urls,
            cache,
            dry_run,
        } => run_import_newsboat(urls, cache, dry_run),
        cli::ImportCommand::Opml { file, dry_run } => run_import_opml(&file, dry_run),
    }
}

fn run_import_newsboat(urls: Option<PathBuf>, cache: Option<PathBuf>, dry_run: bool) -> Result<()> {
    let home = home_dir();
    let urls_path = match urls {
        Some(path) => path,
        None => wire::import::newsboat::default_urls_paths(&home)
            .into_iter()
            .find(|p| p.exists())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no newsboat urls file found; name one with --urls, \
                     or see docs/cli.md for where it looks"
                )
            })?,
    };
    let text = std::fs::read_to_string(&urls_path)
        .with_context(|| format!("reading {}", urls_path.display()))?;

    // The cache is optional and its absence is not an error: read state is a
    // bonus, and a person who has newsboat's urls file but not its cache
    // should still get their feeds.
    let cache_path = cache.or_else(|| {
        wire::import::newsboat::default_cache_paths(&home)
            .into_iter()
            .find(|p| p.exists())
    });
    let cache_rows = match &cache_path {
        Some(path) => match wire::import::newsboat::read_cache(path) {
            Ok(rows) => Some(rows),
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                None
            }
        },
        None => None,
    };

    let plan = wire::import::plan_newsboat(&text, cache_rows.as_ref());
    finish_import(plan, dry_run, &urls_path.display().to_string())
}

fn run_import_opml(file: &Path, dry_run: bool) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let plan = wire::import::opml::parse(&text)?;
    finish_import(plan, dry_run, &file.display().to_string())
}

fn finish_import(plan: wire::import::ImportPlan, dry_run: bool, source: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    let _ = print(&mut out, &format!("from {source}:"));

    for feed in &plan.feeds {
        let title = feed.title.as_deref().unwrap_or("");
        let note = if feed.kind == FeedKind::Youtube {
            "  [youtube]"
        } else {
            ""
        };
        let folder = match &feed.folder {
            Some(f) => format!("  ({f})"),
            None => String::new(),
        };
        let line = format!("  {}{note}{folder}  {title}", feed.url);
        if print(&mut out, line.trim_end()).is_break() {
            return Ok(());
        }
    }
    for (url, reason) in &plan.skipped {
        let _ = print(&mut out, &format!("  skipped {url}: {reason}"));
    }
    for (url, tags) in &plan.dropped_tags {
        let _ = print(
            &mut out,
            &format!(
                "  {url}: kept the first tag as a folder, dropped {}",
                tags.join(", ")
            ),
        );
    }
    let _ = print(
        &mut out,
        &format!(
            "{} feeds ({} youtube), {} folders, {} read marks, {} skipped",
            plan.feeds.len(),
            plan.video_count(),
            plan.folder_count(),
            plan.read.len(),
            plan.skipped.len()
        ),
    );

    if dry_run {
        let _ = print(&mut out, "nothing was written (--dry-run)");
        return Ok(());
    }

    let db = open_db()?;
    let report = wire::import::apply_plan(&db, &plan)?;
    db.set_meta(
        wire::db::schema::meta::NEWSBOAT_IMPORTED_AT,
        &wire::db::now().to_string(),
    )?;
    let _ = print(
        &mut out,
        &format!(
            "added {}, already there {}, folders {}, read marks {}",
            report.added, report.already_there, report.folders, report.read_marks
        ),
    );
    Ok(())
}

fn run_export_opml(file: Option<&Path>) -> Result<()> {
    let db = open_db()?;
    let xml =
        wire::import::opml::export(&feeds::list_feeds_with_unread(&db)?, &feeds::folders(&db)?)?;
    match file {
        Some(path) => {
            starkit::fs::write_atomic(path, xml.as_bytes())
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => {
            let mut out = std::io::stdout().lock();
            let _ = print(&mut out, &xml);
        }
    }
    Ok(())
}

// -------------------------------------------------------------- youtube ----

fn run_youtube(
    core: &wire::WireConfig,
    http: &dyn Http,
    command: cli::YoutubeCommand,
) -> Result<()> {
    let db = open_db()?;
    let mut out = std::io::stdout().lock();

    match command {
        cli::YoutubeCommand::Add { what } => {
            drop(out);
            run_add(core, http, &what, None, None)
        }
        cli::YoutubeCommand::ImportTakeout { file, dry_run } => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let channels = wire::youtube::parse_takeout(&text)?;
            add_channels(
                &db,
                &mut out,
                &channels,
                wire::db::schema::ChannelSource::Takeout,
                dry_run,
            )
        }
        cli::YoutubeCommand::Sync {
            cookies_from_browser,
            dry_run,
        } => {
            let mut cfg = core.youtube.clone();
            if let Some(browser) = cookies_from_browser {
                cfg.cookies_from_browser = browser;
            }
            let channels = wire::youtube::yt_subscriptions(&cfg, 500)?;
            add_channels(
                &db,
                &mut out,
                &channels,
                wire::db::schema::ChannelSource::YtSubs,
                dry_run,
            )
        }
        cli::YoutubeCommand::List => {
            for channel in db_youtube::list(&db)? {
                let line = format!(
                    "{}  {}",
                    channel.channel_id,
                    channel.title.as_deref().unwrap_or("")
                );
                if print(&mut out, line.trim_end()).is_break() {
                    return Ok(());
                }
            }
            Ok(())
        }
    }
}

fn add_channels(
    db: &Db,
    out: &mut impl std::io::Write,
    channels: &[(wire::youtube::ChannelId, Option<String>)],
    source: wire::db::schema::ChannelSource,
    dry_run: bool,
) -> Result<()> {
    let known = db_youtube::known_ids(db)?;
    let mut added = 0usize;
    let mut already = 0usize;

    for (id, title) in channels {
        if known.contains(id.as_str()) {
            already += 1;
            continue;
        }
        let line = format!("  {}  {}", id, title.as_deref().unwrap_or(""));
        if print(out, line.trim_end()).is_break() {
            return Ok(());
        }
        if dry_run {
            added += 1;
            continue;
        }
        let feed = feeds::add(
            db,
            &id.feed_url(),
            None,
            FeedKind::Youtube,
            title.as_deref(),
            None,
        )?;
        db_youtube::upsert(
            &db.conn,
            id,
            title.as_deref(),
            None,
            Some(feed.id()),
            source,
        )?;
        added += 1;
    }

    let _ = print(
        out,
        &format!(
            "{added} new, {already} already subscribed{}",
            if dry_run {
                " (--dry-run, nothing written)"
            } else {
                ""
            }
        ),
    );
    Ok(())
}

// -------------------------------------------------------------- extract ----

/// `starwire extract <url>`: the probe.
///
/// Runs the scraper on one page and prints what it made of it. No database,
/// no feed, nothing stored -- this is how to find out whether a site yields
/// before subscribing to it, and how to put the actual markdown in a bug
/// report when it does not.
fn run_extract(core: &wire::WireConfig, http: &dyn Http, url: &str) -> Result<()> {
    let result = wire::extract::run(http, url, wire::extract::Limits::from(&core.articles))?;
    let mut out = std::io::stdout().lock();

    if result.status == ArticleStatus::Failed {
        anyhow::bail!(
            "{url}: {}",
            result.error.as_deref().unwrap_or("it did not yield")
        );
    }

    if let Some(title) = &result.title {
        let _ = print(&mut out, title);
    }
    let mut meta = Vec::new();
    if let Some(byline) = &result.byline {
        meta.push(byline.clone());
    }
    if let Some(site) = &result.site_name {
        meta.push(site.clone());
    }
    if !meta.is_empty() {
        let _ = print(&mut out, &meta.join(" · "));
    }
    let _ = print(&mut out, "");
    let _ = print(
        &mut out,
        result.markdown.as_deref().unwrap_or("").trim_end(),
    );
    Ok(())
}

// ------------------------------------------------------------------ tui ----

/// The window.
///
/// The config template is written before anything else, so a first run
/// leaves a commented file to edit; the database is opened here rather than
/// inside the UI so that a locked or unreadable one is a clear error on the
/// terminal rather than a panic behind the alternate screen.
fn run_tui(cfg: &config::Config, config_path: &Path) -> Result<()> {
    match config::Config::write_template(config_path) {
        Ok(true) => tracing::info!("wrote a starting config.toml to {}", config_path.display()),
        Ok(false) => {}
        Err(e) => tracing::warn!("could not write a starting config.toml: {e}"),
    }

    let db_path = paths::db_file()?;
    let db = Db::open(&db_path)?;
    let feed_count = feeds::list_feeds_with_unread(&db)?.len();
    drop(db);

    ui::run(cfg, &db_path, feed_count)
}

fn home_dir() -> PathBuf {
    std::env::home_dir()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_title_is_cut_with_an_ellipsis_and_never_mid_character() {
        assert_eq!(elide("short", 22), "short");
        let long = "é".repeat(40);
        let got = elide(&long, 10);
        assert_eq!(got.chars().count(), 10);
        assert!(got.ends_with('…'));
        assert!(std::str::from_utf8(got.as_bytes()).is_ok());
    }

    #[test]
    fn eliding_to_nothing_does_not_panic() {
        assert_eq!(elide("abc", 0), "…");
        assert_eq!(elide("", 0), "");
    }
}
