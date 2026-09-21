//! The one way out to the network.
//!
//! Every byte this program did not write itself comes through [`Http::get`].
//! That is the point of the trait: the live implementation is a thin wrapper
//! over the agent STAR/KIT hands back, and [`Replay`] serves a directory of
//! files instead, so the whole of fetching, parsing and extraction can be
//! tested -- and demonstrated, with `starwire --replay` -- without a socket.
//!
//! **https only.** The agent STAR/KIT builds refuses plaintext, and that is
//! kept rather than relaxed. It costs one thing: `http://old.reddit.com` in a
//! newsboat `urls` file does not work as typed. The answer is to rewrite
//! `http` to `https` on the way in (see [`crate::wire::youtube::canonicalise`])
//! rather than to keep a second, plaintext agent -- because a second agent
//! would be used for extraction too, and extraction follows links out of
//! pages. A reader that will fetch `http://` because one feed needed it is a
//! reader that fetches an attacker's injected link in the clear.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use url::Url;

/// What a request may ask for beyond the URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestOptions {
    /// `If-None-Match`, from the last fetch of this feed.
    pub etag: Option<String>,
    /// `If-Modified-Since`, likewise.
    pub last_modified: Option<String>,
    /// `Accept`. A feed asks for XML and gets HTML from a server that decided
    /// this was a browser; saying which is wanted is most of the fix.
    pub accept: Option<String>,
    /// The most bytes read off the body. Anything beyond is a failure rather
    /// than a truncation: half a feed is not a feed, and half a page extracts
    /// to nonsense.
    pub max_bytes: u64,
}

impl RequestOptions {
    /// What a feed fetch asks for.
    pub fn feed(max_bytes: u64) -> Self {
        Self {
            accept: Some(
                "application/atom+xml, application/rss+xml, application/xml;q=0.9, \
                 application/json;q=0.8, text/xml;q=0.8, */*;q=0.5"
                    .into(),
            ),
            max_bytes,
            ..Self::default()
        }
    }

    /// What a page fetch asks for. Deliberately narrow: this program reads
    /// HTML and nothing else, and a server that would rather send a PDF
    /// should be told so before it does.
    pub fn page(max_bytes: u64) -> Self {
        Self {
            accept: Some("text/html, application/xhtml+xml;q=0.9, */*;q=0.1".into()),
            max_bytes,
            ..Self::default()
        }
    }

    pub fn conditional(mut self, etag: Option<String>, last_modified: Option<String>) -> Self {
        self.etag = etag;
        self.last_modified = last_modified;
        self
    }
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
    /// `Retry-After` in seconds, where the server sent one. Reddit does, and
    /// honouring it is the difference between being rate limited and being
    /// blocked.
    pub retry_after: Option<i64>,
    /// Where the request ended up after redirects, which is what a relative
    /// link in the body has to be resolved against.
    pub final_url: String,
}

impl Response {
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn is_not_modified(&self) -> bool {
        self.status == 304
    }

    /// The body as text, replacing whatever is not valid UTF-8.
    ///
    /// Lossy rather than an error: a page in windows-1252 that `ureq`'s
    /// `charset` feature did not transcode -- because the server declared no
    /// encoding at all -- is still worth reading with three characters
    /// wrong.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// What went wrong on the way.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("the body was larger than the {0} byte limit")]
    TooLarge(u64),
    #[error("nothing in the replay directory answers {0}")]
    NoReplay(String),
    #[error("{0}")]
    Transport(String),
}

/// The one way out.
///
/// `&self` rather than `&mut self`: the net threads share one implementation
/// through an `Arc`, and the agent underneath has its own connection pool
/// which is the whole reason there is one agent rather than one per thread.
pub trait Http: Send + Sync {
    fn get(&self, url: &Url, options: &RequestOptions) -> Result<Response, NetError>;

    /// Whether this is the real thing. `--replay` says so in its first line
    /// of output, so that nobody mistakes a fixture run for a live one.
    fn is_live(&self) -> bool {
        true
    }
}

/// The real thing: `ureq` on the agent STAR/KIT configures.
pub struct Live {
    agent: ureq::Agent,
    politeness: Politeness,
}

impl Live {
    pub fn new(cfg: &super::WireConfig) -> Self {
        let agent: ureq::Agent = starkit::net::builder(&cfg.user_agent())
            .https_only(true)
            .timeout_global(Some(Duration::from_secs(cfg.fetch.timeout_secs.max(1))))
            .build()
            .into();
        Self {
            agent,
            politeness: Politeness::new(Duration::from_secs(cfg.fetch.min_host_interval_secs)),
        }
    }
}

impl Http for Live {
    fn get(&self, url: &Url, options: &RequestOptions) -> Result<Response, NetError> {
        let _lease = self.politeness.lease(url.host_str().unwrap_or(""));

        let mut req = self.agent.get(url.as_str());
        if let Some(accept) = &options.accept {
            req = req.header("Accept", accept);
        }
        if let Some(etag) = &options.etag {
            req = req.header("If-None-Match", etag);
        }
        if let Some(lm) = &options.last_modified {
            req = req.header("If-Modified-Since", lm);
        }

        let response = req.call().map_err(|e| NetError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let etag = header("etag");
        let last_modified = header("last-modified");
        let content_type = header("content-type");
        let retry_after = header("retry-after").and_then(|v| v.trim().parse::<i64>().ok());
        // `ureq`'s redirect handling leaves the final URI on the response
        // extensions; where it is not there the request URL is the answer,
        // which is the no-redirect case.
        let final_url = {
            use ureq::ResponseExt as _;
            response.get_uri().to_string()
        };

        // A 304 has no body by definition, and asking for one on a connection
        // the server has already finished with is how a read hangs.
        let body = if status == 304 {
            Vec::new()
        } else {
            let limit = options.max_bytes.max(1);
            let body = response
                .into_body()
                .into_with_config()
                // One byte over the limit, so that a body exactly at the
                // limit is not mistaken for one that was cut short.
                .limit(limit + 1)
                .read_to_vec()
                .map_err(|e| NetError::Transport(e.to_string()))?;
            if body.len() as u64 > limit {
                return Err(NetError::TooLarge(limit));
            }
            body
        };

        Ok(Response {
            status,
            body,
            etag,
            last_modified,
            content_type,
            retry_after,
            final_url,
        })
    }
}

/// A directory of saved responses, served instead of the network.
///
/// The directory has an `index.tsv` of `URL<TAB>file` lines, and the files
/// beside it. That is all: no headers, no status codes, no recorded timing.
/// It is enough for the two jobs it has -- the test suite, and
/// `starwire --replay` as a way of seeing the whole pipeline work with no
/// network at all -- and a format anybody can produce with `curl` and a text
/// editor.
pub struct Replay {
    index: HashMap<String, std::path::PathBuf>,
    feeds: Vec<String>,
}

impl Replay {
    pub fn open(dir: &std::path::Path) -> anyhow::Result<Self> {
        let index_path = dir.join("index.tsv");
        let text = std::fs::read_to_string(&index_path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", index_path.display()))?;
        let mut index = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((url, file)) = line.split_once('\t') else {
                anyhow::bail!(
                    "{}: expected URL<TAB>file, got {line:?}",
                    index_path.display()
                );
            };
            index.insert(normalise_key(url.trim()), dir.join(file.trim()));
        }
        // Optional, and only used by `--replay`: the feed list to put in a
        // database that has none, so that a replay run has something to
        // fetch on a machine that has never run this before.
        let feeds = match std::fs::read_to_string(dir.join("feeds.tsv")) {
            Ok(text) => text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_string)
                .collect(),
            Err(_) => Vec::new(),
        };

        Ok(Self { index, feeds })
    }

    /// The feeds this directory suggests seeding an empty database with.
    pub fn feeds(&self) -> &[String] {
        &self.feeds
    }
}

/// Keys are compared without a trailing slash on a bare host, because that is
/// the one difference between what a person writes in an index and what
/// `Url::parse` produces.
fn normalise_key(url: &str) -> String {
    match Url::parse(url) {
        Ok(u) => u.to_string(),
        Err(_) => url.to_string(),
    }
}

impl Http for Replay {
    fn get(&self, url: &Url, _options: &RequestOptions) -> Result<Response, NetError> {
        let key = normalise_key(url.as_str());
        let Some(path) = self.index.get(&key) else {
            return Err(NetError::NoReplay(key));
        };
        let body = std::fs::read(path)
            .map_err(|e| NetError::Transport(format!("{}: {e}", path.display())))?;
        let content_type = match path.extension().and_then(|e| e.to_str()) {
            Some("html") => Some("text/html; charset=utf-8".to_string()),
            Some("json") => Some("application/json".to_string()),
            Some("xml") => Some("application/xml".to_string()),
            _ => None,
        };
        Ok(Response {
            status: 200,
            body,
            etag: None,
            last_modified: None,
            content_type,
            retry_after: None,
            final_url: url.to_string(),
        })
    }

    fn is_live(&self) -> bool {
        false
    }
}

/// One request to one host at a time, with a gap between them.
///
/// Two separate guarantees, and both matter. **Serial per host** is what
/// stops four threads opening four connections to the same server the moment
/// a refresh starts -- with a feed list like the reference one, nineteen of
/// the forty-one feeds are on `youtube.com`. **A gap** is what keeps the
/// request rate down afterwards; Reddit's rate limiter counts requests per
/// minute, not concurrency.
///
/// The lease is held for the whole request rather than taken and released
/// around the sleep, which is what makes it serial: a second thread wanting
/// the same host waits on the mutex until the first has its answer.
pub struct Politeness {
    min_interval: Duration,
    hosts: Mutex<HashMap<String, Arc<HostSlot>>>,
}

/// One host's turn, and when its last request finished.
///
/// A `Condvar` rather than holding a `MutexGuard` for the length of the
/// request: a guard held across a return would have to borrow from a map this
/// function does not own, and the only ways to write that are unsafe or a
/// self-referential crate. A flag plus a condition variable says the same
/// thing -- one request at a time, everybody else waits -- in safe code.
struct HostSlot {
    state: Mutex<HostState>,
    free: std::sync::Condvar,
}

struct HostState {
    busy: bool,
    last: Instant,
}

impl Politeness {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// Take the lease for `host`, waiting for whoever has it and then for the
    /// gap since their request to pass. Dropping the returned guard stamps
    /// the clock and lets the next thread in.
    pub fn lease(&self, host: &str) -> HostLease {
        let slot = {
            let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(hosts.entry(host.to_ascii_lowercase()).or_insert_with(|| {
                Arc::new(HostSlot {
                    state: Mutex::new(HostState {
                        busy: false,
                        // Long enough ago that the first request to a host
                        // never waits.
                        last: Instant::now() - Duration::from_secs(3600),
                    }),
                    free: std::sync::Condvar::new(),
                })
            }))
        };

        let elapsed = {
            // Poisoning is tolerated throughout: a panic in the middle of one
            // request must not make a host unfetchable for the rest of the
            // session.
            let mut state = slot.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.busy {
                state = slot.free.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            state.busy = true;
            state.last.elapsed()
        };
        if elapsed < self.min_interval {
            std::thread::sleep(self.min_interval - elapsed);
        }
        HostLease { slot }
    }
}

/// Held for the length of one request to one host.
pub struct HostLease {
    slot: Arc<HostSlot>,
}

impl Drop for HostLease {
    fn drop(&mut self) {
        let mut state = self.slot.state.lock().unwrap_or_else(|e| e.into_inner());
        state.busy = false;
        // Stamped when the request *finished*, not when it started: the gap
        // is meant to be between the end of one request and the start of the
        // next, so a slow server does not earn itself a faster follow-up.
        state.last = Instant::now();
        drop(state);
        self.slot.free.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replay_directory_answers_the_urls_in_its_index() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.xml"), b"<rss/>").unwrap();
        std::fs::write(
            dir.path().join("index.tsv"),
            "# a comment\n\nhttps://example.org/feed\ta.xml\n",
        )
        .unwrap();

        let replay = Replay::open(dir.path()).unwrap();
        assert!(!replay.is_live());
        let got = replay
            .get(
                &Url::parse("https://example.org/feed").unwrap(),
                &RequestOptions::default(),
            )
            .unwrap();
        assert_eq!(got.status, 200);
        assert_eq!(got.body, b"<rss/>");
        assert_eq!(got.content_type.as_deref(), Some("application/xml"));

        assert!(replay.feeds().is_empty(), "there is no feeds.tsv here");

        let missing = replay.get(
            &Url::parse("https://example.org/other").unwrap(),
            &RequestOptions::default(),
        );
        assert!(matches!(missing, Err(NetError::NoReplay(_))));
    }

    #[test]
    fn a_replay_directory_may_name_the_feeds_to_seed_with() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.tsv"), "").unwrap();
        std::fs::write(
            dir.path().join("feeds.tsv"),
            "# a comment\n\nhttps://a.example/feed\nhttps://b.example/feed\n",
        )
        .unwrap();
        let replay = Replay::open(dir.path()).unwrap();
        assert_eq!(
            replay.feeds(),
            ["https://a.example/feed", "https://b.example/feed"]
        );
    }

    #[test]
    fn the_checked_in_replay_directory_serves_the_fixture_article() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/replay");
        let replay = Replay::open(&dir).unwrap();
        assert!(!replay.feeds().is_empty());
        let got = replay
            .get(
                &Url::parse("https://example.org/posts/borrow-checker").unwrap(),
                &RequestOptions::page(1 << 20),
            )
            .unwrap();
        assert!(got.text().contains("Three rules"));
    }

    #[test]
    fn a_replay_index_that_is_not_tab_separated_says_so() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.tsv"), "https://e.org/feed a.xml\n").unwrap();
        let err = match Replay::open(dir.path()) {
            Ok(_) => panic!("a line with no tab should not have parsed"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("URL<TAB>file"), "{err}");
    }

    #[test]
    fn a_request_option_set_carries_what_the_two_callers_need() {
        let feed = RequestOptions::feed(1024);
        assert!(feed.accept.as_deref().unwrap().contains("atom"));
        assert_eq!(feed.max_bytes, 1024);
        let page = RequestOptions::page(2048);
        assert!(page.accept.as_deref().unwrap().starts_with("text/html"));

        let conditional = RequestOptions::feed(1).conditional(Some("\"e\"".into()), None);
        assert_eq!(conditional.etag.as_deref(), Some("\"e\""));
    }

    #[test]
    fn politeness_is_serial_per_host_and_parallel_across_them() {
        let politeness = Arc::new(Politeness::new(Duration::from_millis(30)));
        let order = Arc::new(Mutex::new(Vec::new()));

        std::thread::scope(|scope| {
            for n in 0..3 {
                let politeness = Arc::clone(&politeness);
                let order = Arc::clone(&order);
                scope.spawn(move || {
                    let _lease = politeness.lease("example.org");
                    order.lock().unwrap().push(n);
                    std::thread::sleep(Duration::from_millis(5));
                });
            }
        });
        assert_eq!(order.lock().unwrap().len(), 3, "all three got through");

        // Two different hosts do not wait for one another.
        let started = Instant::now();
        std::thread::scope(|scope| {
            for host in ["a.example", "b.example"] {
                let politeness = Arc::clone(&politeness);
                scope.spawn(move || {
                    let _lease = politeness.lease(host);
                });
            }
        });
        assert!(
            started.elapsed() < Duration::from_millis(30),
            "different hosts must not serialise: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_second_request_to_one_host_waits_for_the_gap() {
        let politeness = Politeness::new(Duration::from_millis(40));
        drop(politeness.lease("example.org"));
        let started = Instant::now();
        drop(politeness.lease("example.org"));
        assert!(
            started.elapsed() >= Duration::from_millis(30),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_host_is_matched_without_regard_to_case() {
        let politeness = Politeness::new(Duration::from_millis(40));
        drop(politeness.lease("Example.ORG"));
        let started = Instant::now();
        drop(politeness.lease("example.org"));
        assert!(started.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn a_response_knows_whether_it_carried_anything() {
        let mut r = Response {
            status: 200,
            body: b"hello".to_vec(),
            etag: None,
            last_modified: None,
            content_type: None,
            retry_after: None,
            final_url: "https://e.org/".into(),
        };
        assert!(r.is_ok());
        assert!(!r.is_not_modified());
        assert_eq!(r.text(), "hello");
        r.status = 304;
        assert!(!r.is_ok());
        assert!(r.is_not_modified());
        // Invalid UTF-8 is still readable rather than an error.
        r.body = vec![0xff, b'a'];
        assert!(r.text().ends_with('a'));
    }
}
