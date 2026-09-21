//! The one way out to the network.
//!
//! Every byte this program did not write itself comes through [`Http::get`].
//! That is the point of the trait: the live implementation is a thin wrapper
//! over the agent STAR/KIT hands back, and [`Replay`] serves a directory of
//! files instead, so the whole of fetching, parsing and extraction can be
//! tested -- and demonstrated, with `starwire --replay` -- without a socket.
//!
//! **Redirects are followed here rather than by `ureq`.** A chain is a
//! sequence of requests to a sequence of hosts, and the politeness in this
//! file is per host: a loop that takes no lease reaches the second host at
//! whatever rate the first one answered. See `MAX_REDIRECTS`.
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
    #[error("it redirected to a login page at {0}")]
    Wall(String),
    #[error("more than {0} redirects")]
    TooManyRedirects(usize),
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

/// How many hops a redirect chain may take.
///
/// `ureq` follows redirects itself and is told here not to. Its loop takes no
/// lease, so a wrapper URL that redirects reaches the second host at whatever
/// rate the first one answered -- which is exactly what the reference
/// database recorded: seventeen 429s from `archive.is`, every one of them
/// reached through a `feedpress.me` wrapper with no gap in between, and every
/// one of them blamed on `feedpress.me` because that was the URL asked for.
/// Walking the chain here buys three things: a lease per hop, the host that
/// actually answered in `final_url`, and the chance to stop at a login page
/// rather than to extract one.
const MAX_REDIRECTS: usize = 5;

/// The real thing: `ureq` on the agent STAR/KIT configures.
pub struct Live {
    agent: ureq::Agent,
    politeness: Politeness,
}

impl Live {
    pub fn new(cfg: &super::WireConfig) -> Self {
        let agent: ureq::Agent = starkit::net::builder(&cfg.user_agent())
            .https_only(true)
            // Zero means ureq returns the 3xx as it is; `get` walks the chain
            // itself. See MAX_REDIRECTS.
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(cfg.fetch.timeout_secs.max(1))))
            .build()
            .into();
        Self {
            agent,
            politeness: Politeness::new(Duration::from_secs(cfg.fetch.min_host_interval_secs))
                .with_host_intervals(
                    cfg.fetch
                        .host_intervals
                        .iter()
                        .map(|(host, secs)| (host.clone(), *secs)),
                ),
        }
    }
}

impl Http for Live {
    fn get(&self, url: &Url, options: &RequestOptions) -> Result<Response, NetError> {
        let mut target = url.clone();

        for _ in 0..=MAX_REDIRECTS {
            // Held for the length of this hop and dropped before the next, so
            // every host in a chain waits its own turn.
            let lease = self.politeness.lease(target.host_str().unwrap_or(""));

            let mut req = self.agent.get(target.as_str());
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
            let location = header("location");

            // What a server says about its own limiter, where it is one this
            // believes. Reddit answers the first request of the minute with
            // nothing left and forty seconds to wait.
            let host = target.host_str().unwrap_or("").to_string();
            if self.politeness.honours_ratelimit_reset(&host) {
                if let Some(wait) =
                    ratelimit_wait(header("x-ratelimit-remaining"), header("x-ratelimit-reset"))
                {
                    self.politeness.hold(&host, wait);
                }
            }

            if let (true, Some(location)) = (is_redirect(status), location.as_deref()) {
                let next = target
                    .join(location.trim())
                    .map_err(|e| NetError::Transport(format!("{location}: {e}")))?;
                if is_a_wall(&next) {
                    return Err(NetError::Wall(next.to_string()));
                }
                // The body of a redirect is a courtesy page nobody reads, and
                // the connection is wanted back.
                drop(response);
                drop(lease);
                target = next;
                continue;
            }

            // A 304 has no body by definition, and asking for one on a
            // connection the server has already finished with is how a read
            // hangs.
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

            return Ok(Response {
                status,
                body,
                etag,
                last_modified,
                content_type,
                retry_after,
                final_url: target.to_string(),
            });
        }

        Err(NetError::TooManyRedirects(MAX_REDIRECTS))
    }
}

/// How long a server's own rate-limit headers ask for, where they ask for
/// anything.
///
/// Reddit sends `x-ratelimit-remaining` as a float (`0.0`) and
/// `x-ratelimit-reset` as whole seconds. Nothing is asked of a header that
/// does not parse: a limiter this does not understand is one to leave to the
/// ordinary gap.
fn ratelimit_wait(remaining: Option<String>, reset: Option<String>) -> Option<Duration> {
    let remaining: f64 = remaining?.trim().parse().ok()?;
    if remaining >= 1.0 {
        return None;
    }
    let reset: u64 = reset?.trim().parse().ok()?;
    // A limiter asking for an hour is a limiter this program has
    // misunderstood; the ordinary backoff is what that case is for.
    (reset > 0 && reset <= 300).then(|| Duration::from_secs(reset))
}

/// The status codes that mean "it is somewhere else".
fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Whether a redirect has landed somewhere there is no article behind.
///
/// Worth stopping at rather than following: `old.reddit.com/r/<sub>/.rss`
/// answers a 302 to `/login/?reason=lor2`, and what the program recorded
/// against all five Reddit feeds was "not a feed (HTML page)" -- true, and
/// two steps removed from what happened. The list is deliberately short and
/// matches whole path segments, so an article at `/2026/09/logins-considered`
/// is untouched.
fn is_a_wall(url: &Url) -> bool {
    const SEGMENTS: &[&str] = &[
        "login",
        "signin",
        "sign-in",
        "sign_in",
        "consent",
        "captcha",
        "checkpoint",
    ];
    url.path_segments().is_some_and(|mut segments| {
        segments.any(|segment| SEGMENTS.contains(&segment.to_ascii_lowercase().as_str()))
    })
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

/// A host that has said, by how it behaves, that the default gap is not
/// enough.
///
/// Data rather than code, and short on purpose: a table of special cases is
/// a table of things measured, and anything speculative in it is a slower
/// reader for no reason.
#[derive(Debug, Clone, Copy)]
pub struct HostRule {
    /// Matched against the whole host or against a dot-prefixed tail of it,
    /// so `reddit.com` covers `www.reddit.com` and `old.reddit.com` and does
    /// not cover `notreddit.com`.
    pub host_suffix: &'static str,
    pub min_interval: Duration,
    /// Whether to read `x-ratelimit-remaining` and `x-ratelimit-reset` off
    /// the answer and hold the host until the reset when nothing is left.
    /// Only for servers known to send them honestly.
    pub honour_ratelimit_reset: bool,
}

/// The hosts a feed list of any size will meet, and what they want.
///
/// **Reddit**, probed live in September 2026: `www.reddit.com/r/<sub>/.rss`
/// serves Atom to this program's user agent -- a browser's gets a 429 -- and
/// answers the very first request with `x-ratelimit-remaining: 0.0` and
/// `x-ratelimit-reset: 40`. That is roughly one request a minute per address,
/// for all five subreddits together, so sixty-one seconds is the gap and the
/// reset header is believed on top of it.
///
/// **archive.is** is what `feedpress.me` wrappers redirect to, seventeen
/// times in one afternoon in the reference database, every one of them a 429.
const HOST_RULES: &[HostRule] = &[
    HostRule {
        host_suffix: "reddit.com",
        min_interval: Duration::from_secs(61),
        honour_ratelimit_reset: true,
    },
    HostRule {
        host_suffix: "archive.is",
        min_interval: Duration::from_secs(10),
        honour_ratelimit_reset: false,
    },
];

/// Whether a host is the one a rule or a configured override names.
fn host_matches(host: &str, suffix: &str) -> bool {
    host == suffix || host.ends_with(&format!(".{suffix}"))
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
    /// What `[fetch] host_intervals` said, matched the same way the built-in
    /// rules are and winning over them: a reader who has been asked by an
    /// administrator to slow down should not have to wait for a release.
    overrides: HashMap<String, Duration>,
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
    /// Set from a server's own rate-limit headers, and never moved backwards.
    /// Reddit says `x-ratelimit-reset: 40` after one request; waiting the
    /// forty seconds is the difference between being rate limited and being
    /// blocked.
    not_before: Option<Instant>,
}

impl Politeness {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            overrides: HashMap::new(),
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// The reader's own table of gaps, from `[fetch] host_intervals`.
    pub fn with_host_intervals<I>(mut self, intervals: I) -> Self
    where
        I: IntoIterator<Item = (String, u64)>,
    {
        self.overrides = intervals
            .into_iter()
            .map(|(host, secs)| (host.trim().to_ascii_lowercase(), Duration::from_secs(secs)))
            .filter(|(host, _)| !host.is_empty())
            .collect();
        self
    }

    /// The gap this host wants: what the reader configured, else what the
    /// table above knows, else the default. The longest matching suffix wins,
    /// so a rule for one subdomain beats a rule for its parent.
    pub fn interval_for(&self, host: &str) -> Duration {
        let host = host.to_ascii_lowercase();
        let configured = self
            .overrides
            .iter()
            .filter(|(suffix, _)| host_matches(&host, suffix))
            .max_by_key(|(suffix, _)| suffix.len())
            .map(|(_, interval)| *interval);
        configured
            .or_else(|| {
                HOST_RULES
                    .iter()
                    .filter(|rule| host_matches(&host, rule.host_suffix))
                    .max_by_key(|rule| rule.host_suffix.len())
                    .map(|rule| rule.min_interval)
            })
            .unwrap_or(self.min_interval)
    }

    /// Whether this host's rate-limit headers are worth believing.
    pub fn honours_ratelimit_reset(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        HOST_RULES
            .iter()
            .filter(|rule| host_matches(&host, rule.host_suffix))
            .max_by_key(|rule| rule.host_suffix.len())
            .is_some_and(|rule| rule.honour_ratelimit_reset)
    }

    /// Do not ask this host anything for `wait`.
    ///
    /// Additive with the gap rather than instead of it, and never shortened:
    /// two threads reading two answers from one rate limiter must not talk
    /// each other into asking sooner.
    pub fn hold(&self, host: &str, wait: Duration) {
        let slot = self.slot(host);
        let until = Instant::now() + wait;
        let mut state = slot.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.not_before.is_none_or(|current| until > current) {
            state.not_before = Some(until);
        }
    }

    fn slot(&self, host: &str) -> Arc<HostSlot> {
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(hosts.entry(host.to_ascii_lowercase()).or_insert_with(|| {
            Arc::new(HostSlot {
                state: Mutex::new(HostState {
                    busy: false,
                    // Long enough ago that the first request to a host
                    // never waits.
                    last: Instant::now() - Duration::from_secs(3600),
                    not_before: None,
                }),
                free: std::sync::Condvar::new(),
            })
        }))
    }

    /// Take the lease for `host`, waiting for whoever has it and then for the
    /// gap since their request to pass. Dropping the returned guard stamps
    /// the clock and lets the next thread in.
    pub fn lease(&self, host: &str) -> HostLease {
        let slot = self.slot(host);
        let interval = self.interval_for(host);

        let (elapsed, not_before) = {
            // Poisoning is tolerated throughout: a panic in the middle of one
            // request must not make a host unfetchable for the rest of the
            // session.
            let mut state = slot.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.busy {
                state = slot.free.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            state.busy = true;
            (state.last.elapsed(), state.not_before)
        };
        let mut wait = interval.saturating_sub(elapsed);
        if let Some(until) = not_before {
            wait = wait.max(until.saturating_duration_since(Instant::now()));
        }
        if !wait.is_zero() {
            std::thread::sleep(wait);
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
    fn the_status_codes_that_mean_somewhere_else_are_the_five_that_do() {
        for status in [301, 302, 303, 307, 308] {
            assert!(is_redirect(status), "{status}");
        }
        for status in [200, 204, 304, 400, 403, 429, 500] {
            assert!(!is_redirect(status), "{status}");
        }
    }

    #[test]
    fn a_redirect_into_a_login_page_is_recognised_and_an_article_is_not() {
        // What `old.reddit.com/r/<sub>/.rss` answers with, which is what the
        // reference database recorded as "not a feed (HTML page)".
        assert!(is_a_wall(
            &Url::parse("https://old.reddit.com/login/?reason=lor2").unwrap()
        ));
        assert!(is_a_wall(
            &Url::parse("https://e.org/accounts/sign-in?next=/a").unwrap()
        ));
        assert!(is_a_wall(&Url::parse("https://e.org/consent/").unwrap()));

        // Whole segments, so an article that talks about one is untouched.
        assert!(!is_a_wall(
            &Url::parse("https://e.org/2026/09/logins-considered-harmful").unwrap()
        ));
        assert!(!is_a_wall(
            &Url::parse("https://e.org/posts/one?from=login").unwrap()
        ));
        assert!(!is_a_wall(&Url::parse("https://e.org/").unwrap()));
    }

    #[test]
    fn the_hosts_with_a_rule_get_their_own_gap_and_everything_else_the_default() {
        let politeness = Politeness::new(Duration::from_secs(2));
        assert_eq!(
            politeness.interval_for("www.reddit.com"),
            Duration::from_secs(61)
        );
        assert_eq!(
            politeness.interval_for("old.reddit.com"),
            Duration::from_secs(61),
            "a rule is a tail of the host, not the whole of it"
        );
        assert_eq!(
            politeness.interval_for("archive.is"),
            Duration::from_secs(10)
        );
        assert_eq!(
            politeness.interval_for("example.org"),
            Duration::from_secs(2)
        );
        assert_eq!(
            politeness.interval_for("notreddit.com"),
            Duration::from_secs(2),
            "a suffix has to start at a dot"
        );
    }

    #[test]
    fn a_configured_gap_beats_the_table_and_the_longest_match_wins() {
        let politeness = Politeness::new(Duration::from_secs(2)).with_host_intervals([
            ("reddit.com".to_string(), 5),
            ("OLD.reddit.com".to_string(), 90),
            ("example.org".to_string(), 30),
            ("  ".to_string(), 7),
        ]);
        assert_eq!(
            politeness.interval_for("www.reddit.com"),
            Duration::from_secs(5),
            "the reader's own number wins over the built-in one"
        );
        assert_eq!(
            politeness.interval_for("old.reddit.com"),
            Duration::from_secs(90)
        );
        assert_eq!(
            politeness.interval_for("example.org"),
            Duration::from_secs(30)
        );
        assert_eq!(politeness.interval_for("e.example"), Duration::from_secs(2));
    }

    #[test]
    fn only_the_hosts_that_send_honest_rate_limit_headers_are_believed() {
        let politeness = Politeness::new(Duration::from_secs(2));
        assert!(politeness.honours_ratelimit_reset("www.reddit.com"));
        assert!(!politeness.honours_ratelimit_reset("archive.is"));
        assert!(!politeness.honours_ratelimit_reset("example.org"));
    }

    #[test]
    fn a_servers_own_limiter_is_read_only_when_it_says_there_is_nothing_left() {
        // Reddit's answer to the first request of the minute.
        assert_eq!(
            ratelimit_wait(Some("0.0".into()), Some("40".into())),
            Some(Duration::from_secs(40))
        );
        assert_eq!(ratelimit_wait(Some("98.0".into()), Some("40".into())), None);
        assert_eq!(ratelimit_wait(None, Some("40".into())), None);
        assert_eq!(ratelimit_wait(Some("0.0".into()), None), None);
        assert_eq!(ratelimit_wait(Some("0.0".into()), Some("0".into())), None);
        assert_eq!(
            ratelimit_wait(Some("0.0".into()), Some("86400".into())),
            None,
            "an hour is a header this has misunderstood"
        );
        assert_eq!(
            ratelimit_wait(Some("nonsense".into()), Some("4".into())),
            None
        );
    }

    #[test]
    fn a_host_held_by_its_own_limiter_waits_that_long_and_not_less() {
        let politeness = Politeness::new(Duration::from_millis(1));
        politeness.hold("example.org", Duration::from_millis(60));
        // A shorter hold does not talk the longer one down.
        politeness.hold("example.org", Duration::from_millis(1));
        let started = Instant::now();
        drop(politeness.lease("example.org"));
        assert!(
            started.elapsed() >= Duration::from_millis(40),
            "{:?}",
            started.elapsed()
        );
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
