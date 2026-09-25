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

mod resume;
pub use resume::{RequestHistory, RequestScope};
#[cfg(test)]
mod resume_tests;

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
    /// A timeout for this request alone, where it wants one longer or shorter
    /// than the agent's. A page is not a feed: forty-one feeds want to be
    /// quick about it, and one article behind a redirect wrapper on a site
    /// that renders it on demand is allowed to take longer.
    pub timeout_secs: Option<u64>,
    /// The gap this request is willing to leave after the last one to the
    /// same host, where the default is not the right one. An article's
    /// pictures are half a dozen files from the server whose page is already
    /// being read, and two seconds apiece would be twelve seconds of `░`;
    /// they are also already-rendered files, which is the opposite of what
    /// the default gap is protecting a server from. It never shortens a host
    /// the table or the reader has a number for -- see
    /// [`Politeness::interval_for_request`].
    pub host_gap: Option<Duration>,
    /// The `User-Agent` this one request names itself with, where the honest
    /// one was refused. Set only by the second of the two requests
    /// `extract::run` makes at a 403, and it brings a browser's `Accept` and
    /// `Accept-Language` with it: see [`BROWSER_USER_AGENT`].
    pub user_agent: Option<String>,
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

    /// What a picture inside an article asks for.
    ///
    /// Built with `..Default::default()` rather than field by field, so a
    /// field added to the struct is carried here without this needing an
    /// edit.
    pub fn picture(max_bytes: u64) -> Self {
        Self {
            accept: Some("image/webp,image/png,image/jpeg,image/*;q=0.8".into()),
            max_bytes,
            host_gap: Some(PICTURE_GAP),
            ..Self::default()
        }
    }

    pub fn conditional(mut self, etag: Option<String>, last_modified: Option<String>) -> Self {
        self.etag = etag;
        self.last_modified = last_modified;
        self
    }

    /// Give this one request its own timeout.
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = Some(secs);
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
    /// The host's gap has not passed, and it is longer than a worker thread
    /// should be asleep for. Not something that went wrong: see [`MAX_PARK`]
    /// and [`take_deferral`].
    #[error("the host cannot be asked again yet")]
    NotBefore(Instant),
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

/// The longest a lease will hold a thread asleep before it defers instead.
///
/// Three seconds, and the number matters in both directions. The ordinary
/// gap between two requests to one host is two seconds, so an ordinary
/// refresh still waits in place and behaves exactly as it did. A host with a
/// rule of its own does not: `reddit.com` is once every sixty-one seconds,
/// and a reference feed list has five subreddits in it, so a refresh that
/// parked on them put all four net threads to sleep one after another and
/// the whole lane stopped for minutes -- with the reader's own pictures
/// queued behind it.
///
/// What happens instead is [`NetError::NotBefore`]: the slot is given back,
/// the job goes into `State::deferred`, and the clock hands it out again
/// when its time comes. Nothing is asked any sooner than politeness said;
/// the difference is only which thread is waiting, and now none is.
pub const MAX_PARK: Duration = Duration::from_secs(3);

// A deferral the requests on this thread have just run into, and until when.
// See `take_deferral` for why it lives here rather than in a return type.
thread_local! {
    static DEFERRED: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Forget any deferral left on this thread. Called before a job starts.
pub fn clear_deferral() {
    DEFERRED.with(|cell| cell.set(None));
}

/// What this thread's requests ran into, if anything, and until when. Taking
/// it forgets it.
///
/// A thread-local rather than a value threaded back through every return
/// type, because nothing in between could carry it. A lease is refused
/// inside [`Live::get`], and between there and the worker that has to act on
/// it sit `fetch::fetch`, `extract::run` and `pictures::fetch`, each of which
/// deliberately turns every network failure into a sentence recorded against
/// a feed, an entry or a picture. Widening all three so that one value could
/// mean "not yet" would write the same case out three times and still miss
/// the fourth: a redirect hop and a `rel="next"` follow take leases of their
/// own, and the call that took one is not the call the worker made.
///
/// It is sound because of the shape of the pool rather than by luck. A net
/// thread runs one job at a time, start to finish, and
/// [`crate::wire::worker::perform_net`] is the only reader -- it clears this
/// before the job and takes it after, so a deferral noted on a thread doing
/// anything else is dropped at the next clear.
pub fn take_deferral() -> Option<Instant> {
    DEFERRED.with(std::cell::Cell::take)
}

/// Note a refused lease. Never moved earlier, for the same reason
/// [`Politeness::hold`] is never shortened: two hosts on one job's path that
/// both said to wait are two waits, and the job comes back after the later
/// of them.
fn note_deferral(until: Instant) {
    DEFERRED.with(|cell| {
        cell.set(Some(match cell.get() {
            Some(current) if current > until => current,
            _ => until,
        }));
    });
}

/// The gap a picture leaves between one request and the next to one host.
///
/// Short, because the alternative is the default two seconds between each of
/// an article's half-dozen pictures while somebody watches the placeholders;
/// not zero, because a browser opening six connections at once is exactly the
/// behaviour this program does not have. A host with a rule of its own keeps
/// it -- nothing here reaches reddit.com faster than once a minute.
pub const PICTURE_GAP: Duration = Duration::from_millis(250);

/// A desktop Chrome's `User-Agent`, for a page behind an edge firewall.
///
/// Measured against `iflscience.com` in September 2026: CloudFront answers the
/// honest `starwire/0.0.2 (+https://github.com/bstar/starwire)` with a 403,
/// `x-cache: Error from cloudfront` and a 919-byte error page -- bare `curl`
/// gets the same -- and answers this string with the whole article. That is a
/// firewall filtering on the user agent, not a paywall: there is nothing to
/// pay and nothing to log into, and the page is public to anybody with a
/// browser.
///
/// The audit that classed a 403 as permanent measured NYT, WSJ, Reuters and
/// Medium, where a browser's user agent is refused the same way. True there,
/// and not universal, which is the whole of the difference this exists for.
///
/// **The honest agent is always tried first.** Naming the program and linking
/// the repository is what lets an unhappy administrator find out who to ask,
/// and it is worth keeping for every site that does not refuse it; this is
/// only ever the second request, only after a 403, and only from
/// `extract::run`. A 401 or a 402 is a wall by definition and is never retried
/// this way.
pub const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36";

/// What the browser this claims to be would ask for, and in what language.
///
/// Sent with [`BROWSER_USER_AGENT`] and only with it. A firewall that reads the
/// user agent reads the rest of the header set as well, and a request that says
/// Chrome and then asks for `*/*;q=0.1` in no particular language is a shape no
/// browser has. These are the two headers the request that was measured to work
/// carried.
const BROWSER_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
const BROWSER_ACCEPT_LANGUAGE: &str = "en-GB,en;q=0.9";

/// The `Accept` and the `Accept-Language` one request goes out with.
///
/// A function of the options alone, so that what a browser request sends can be
/// tested without a socket. A request naming itself a browser sends the
/// browser's pair *instead of* this program's narrower `Accept` rather than as
/// well: two `Accept` headers is not something a browser does either.
fn accept_headers(options: &RequestOptions) -> (Option<&str>, Option<&'static str>) {
    match options.user_agent {
        Some(_) => (Some(BROWSER_ACCEPT), Some(BROWSER_ACCEPT_LANGUAGE)),
        None => (options.accept.as_deref(), None),
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

    /// The same client for a run with nowhere to defer a job to.
    ///
    /// `starwire fetch` from a timer is a list of feeds and an exit: there
    /// is no clock to hand a deferred job back to and nothing else the
    /// threads could be doing, so they wait out whatever gap a host asks
    /// for, which is what every version of this program has done. The window
    /// uses [`Self::new`], where a long wait is [`MAX_PARK`]'s business.
    pub fn patient(mut self) -> Self {
        self.politeness.max_park = Duration::MAX;
        self
    }
}

impl Http for Live {
    fn get(&self, url: &Url, options: &RequestOptions) -> Result<Response, NetError> {
        let mut target = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            let hop = resume::request(&target, options, || self.get_hop(&target, options))?;
            if let (true, Some(location)) =
                (is_redirect(hop.response.status), hop.location.as_deref())
            {
                let next = target
                    .join(location.trim())
                    .map_err(|e| NetError::Transport(format!("{location}: {e}")))?;
                if is_a_wall(&next) {
                    return Err(NetError::Wall(next.to_string()));
                }
                target = next;
                continue;
            }
            return Ok(hop.response);
        }
        Err(NetError::TooManyRedirects(MAX_REDIRECTS))
    }
}

impl Live {
    fn get_hop(&self, target: &Url, options: &RequestOptions) -> Result<resume::Hop, NetError> {
        // Held for the length of this hop and dropped before the next, so
        // every host in a chain waits its own turn.
        let _lease = self
            .politeness
            .lease(target.host_str().unwrap_or(""), options.host_gap)
            .inspect_err(|e| {
                if let NetError::NotBefore(until) = e {
                    note_deferral(*until);
                }
            })?;

        let mut req = self.agent.get(target.as_str());
        if let Some(secs) = options.timeout_secs {
            // Per request rather than per agent, because there is one
            // agent and one connection pool for feeds and pages both.
            req = req
                .config()
                .timeout_global(Some(Duration::from_secs(secs.max(1))))
                .build();
        }
        let (accept, accept_language) = accept_headers(options);
        if let Some(accept) = accept {
            req = req.header("Accept", accept);
        }
        if let Some(language) = accept_language {
            req = req.header("Accept-Language", language);
        }
        if let Some(user_agent) = &options.user_agent {
            // Overrides the agent's own rather than needing a second
            // agent: `ureq` adds the configured user agent only to a
            // request that carries no header of its own.
            req = req.header("User-Agent", user_agent);
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

        // A 304 has no body by definition, and asking for one on a
        // connection the server has already finished with is how a read
        // hangs.
        let body = if status == 304 || (is_redirect(status) && location.is_some()) {
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

        Ok(resume::Hop {
            response: Response {
                status,
                body,
                etag,
                last_modified,
                content_type,
                retry_after,
                final_url: target.to_string(),
            },
            location,
        })
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
            // The pictures an article carries. Without these a replayed
            // picture has no declared type, and the cache would name every
            // one of them by whatever the URL's tail happened to be.
            Some("png") => Some("image/png".to_string()),
            Some("jpg" | "jpeg") => Some("image/jpeg".to_string()),
            Some("webp") => Some("image/webp".to_string()),
            Some("gif") => Some("image/gif".to_string()),
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
    /// How long a lease will sleep before it defers instead. [`MAX_PARK`] in
    /// the window; `Duration::MAX` for a run that waits, see
    /// [`Live::patient`].
    max_park: Duration,
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
            max_park: MAX_PARK,
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
        self.interval_for_request(host, None)
    }

    /// The same, for a request that has a gap of its own in mind.
    ///
    /// A named host keeps its number whatever the request asks for: the whole
    /// value of the table is that `reddit.com` is once a minute for
    /// everything this program does, and a request that could shorten it
    /// would be a way of getting the program blocked one picture at a time.
    /// Where there is no rule and no override, what the request asked for is
    /// what it gets, and the default is for a request that asked for nothing.
    pub fn interval_for_request(&self, host: &str, requested: Option<Duration>) -> Duration {
        match self.named_interval(host) {
            Some(named) => named,
            None => requested.unwrap_or(self.min_interval),
        }
    }

    /// The gap somebody has written down for this host, by hand or in the
    /// table -- as opposed to the default, which is merely what is left.
    fn named_interval(&self, host: &str) -> Option<Duration> {
        let host = host.to_ascii_lowercase();
        self.overrides
            .iter()
            .filter(|(suffix, _)| host_matches(&host, suffix))
            .max_by_key(|(suffix, _)| suffix.len())
            .map(|(_, interval)| *interval)
            .or_else(|| {
                HOST_RULES
                    .iter()
                    .filter(|rule| host_matches(&host, rule.host_suffix))
                    .max_by_key(|rule| rule.host_suffix.len())
                    .map(|rule| rule.min_interval)
            })
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
    /// the clock and lets the next thread in. `gap` is the caller's own
    /// interval, where it has one; see [`Self::interval_for_request`].
    ///
    /// A wait longer than `max_park` is not waited out. The slot goes back
    /// and the caller is told when the host may be asked, so the thread can
    /// go and do something else -- see [`MAX_PARK`]. Waiting for whoever
    /// holds the slot is not that wait: a request is bounded by its own
    /// timeout, and two threads that both walked away from a host neither
    /// had asked yet would be a host nothing ever asks.
    pub fn lease(&self, host: &str, gap: Option<Duration>) -> Result<HostLease, NetError> {
        let slot = self.slot(host);
        let interval = self.interval_for_request(host, gap);

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
        if wait > self.max_park {
            // Given back before anybody is told anything: a thread that
            // walks away still holding the slot is a host nothing can ask
            // again. The clock is *not* stamped -- no request was made, and
            // stamping it would push the next attempt a whole gap further
            // out every time one of these came round.
            let mut state = slot.state.lock().unwrap_or_else(|e| e.into_inner());
            state.busy = false;
            drop(state);
            slot.free.notify_one();
            return Err(NetError::NotBefore(Instant::now() + wait));
        }
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        Ok(HostLease { slot })
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
        assert_eq!(
            page.timeout_secs, None,
            "the agent's own timeout, unless asked"
        );
        assert_eq!(page.timeout(30).timeout_secs, Some(30));
        assert_eq!(
            feed.host_gap, None,
            "a feed leaves the gap to the host's own rule"
        );

        let conditional = RequestOptions::feed(1).conditional(Some("\"e\"".into()), None);
        assert_eq!(conditional.etag.as_deref(), Some("\"e\""));
    }

    /// The browser's headers go out together or not at all, and a page is
    /// asked with the honest agent until something refuses it.
    #[test]
    fn a_browsers_headers_are_sent_with_its_user_agent_and_not_otherwise() {
        let page = RequestOptions::page(1 << 20);
        assert_eq!(
            page.user_agent, None,
            "the agent that names the program is always tried first"
        );
        let (accept, language) = accept_headers(&page);
        assert_eq!(accept, page.accept.as_deref(), "this program's own Accept");
        assert_eq!(language, None, "and no Accept-Language at all");

        let browser = RequestOptions {
            user_agent: Some(BROWSER_USER_AGENT.to_string()),
            ..RequestOptions::page(1 << 20)
        };
        let (accept, language) = accept_headers(&browser);
        assert_eq!(accept, Some(BROWSER_ACCEPT));
        assert_eq!(language, Some(BROWSER_ACCEPT_LANGUAGE));
        assert!(
            BROWSER_USER_AGENT.starts_with("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36"),
            "the string that was measured, in one piece: {BROWSER_USER_AGENT}"
        );
        assert!(
            BROWSER_USER_AGENT.contains("Chrome/"),
            "{BROWSER_USER_AGENT}"
        );
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
                    let _lease = politeness.lease("example.org", None).expect("the slot");
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
                    let _lease = politeness.lease(host, None).expect("the slot");
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
        drop(politeness.lease("example.org", None).expect("the slot"));
        let started = Instant::now();
        drop(politeness.lease("example.org", None).expect("the slot"));
        assert!(
            started.elapsed() >= Duration::from_millis(30),
            "{:?}",
            started.elapsed()
        );
    }

    /// The whole of the fix for a refresh that stopped for minutes: a wait
    /// nobody should sleep through comes back as a time instead, at once,
    /// and the host is left free for whoever is ready to ask it.
    #[test]
    fn a_long_wait_defers_instead_of_parking() {
        let politeness = Politeness::new(Duration::from_millis(1));
        // Reddit's own rule: sixty-one seconds, which is twenty times
        // MAX_PARK.
        drop(politeness.lease("www.reddit.com", None).expect("the first"));

        let started = Instant::now();
        let until = match politeness.lease("www.reddit.com", None) {
            Err(NetError::NotBefore(until)) => until,
            Ok(_) => panic!("a sixty-one second gap was taken as a lease"),
            Err(e) => panic!("{e}"),
        };
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "the thread was parked after all: {:?}",
            started.elapsed()
        );
        // The rule's sixty-one seconds from when the lease was asked for,
        // give or take the call itself -- `started` is read before it.
        let wait = until.saturating_duration_since(started);
        assert!(
            wait > MAX_PARK && wait <= Duration::from_secs(62),
            "{wait:?}"
        );

        // And the slot is free: a second thread asking gets the same answer
        // rather than waiting on a flag nobody will clear.
        assert!(matches!(
            politeness.lease("www.reddit.com", None),
            Err(NetError::NotBefore(_))
        ));

        // `max_park` is the whole of the difference, and a run with nowhere
        // to defer to sets it out of reach -- see `Live::patient`. Measured
        // at forty milliseconds rather than at sixty-one seconds, because
        // what is being checked is which branch was taken.
        let impatient = Politeness {
            max_park: Duration::ZERO,
            ..Politeness::new(Duration::from_millis(40))
        };
        drop(impatient.lease("example.org", None).expect("the first"));
        assert!(matches!(
            impatient.lease("example.org", None),
            Err(NetError::NotBefore(_))
        ));

        let patient = Politeness {
            max_park: Duration::MAX,
            ..Politeness::new(Duration::from_millis(40))
        };
        drop(patient.lease("example.org", None).expect("the first"));
        assert!(
            patient.lease("example.org", None).is_ok(),
            "a patient run waits the gap out instead"
        );
    }

    /// The ordinary gap is still slept through, so a refresh of a feed list
    /// with no rule in it behaves exactly as it did.
    #[test]
    fn a_short_wait_is_still_waited_in_place() {
        let politeness = Politeness::new(Duration::from_millis(40));
        drop(politeness.lease("example.org", None).expect("the first"));
        let started = Instant::now();
        assert!(politeness.lease("example.org", None).is_ok());
        assert!(started.elapsed() >= Duration::from_millis(30));
        assert!(
            Duration::from_secs(2) < MAX_PARK,
            "the default gap has to stay under the park ceiling, or every \
             second request to a host would defer"
        );
    }

    #[test]
    fn a_host_is_matched_without_regard_to_case() {
        let politeness = Politeness::new(Duration::from_millis(40));
        drop(politeness.lease("Example.ORG", None).expect("the slot"));
        let started = Instant::now();
        drop(politeness.lease("example.org", None).expect("the slot"));
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

    /// A picture asks for a shorter gap than the default and gets it, and a
    /// host somebody has written a number down for keeps that number: the
    /// whole point of the table is that nothing in this program reaches
    /// reddit.com more than once a minute, pictures included.
    #[test]
    fn a_picture_lease_waits_its_own_gap() {
        let options = RequestOptions::picture(1 << 20);
        assert_eq!(options.host_gap, Some(PICTURE_GAP));
        assert!(options.accept.as_deref().unwrap().contains("image/webp"));
        assert_eq!(options.max_bytes, 1 << 20);

        let politeness = Politeness::new(Duration::from_secs(2))
            .with_host_intervals([("slow.example".to_string(), 30)]);
        assert_eq!(
            politeness.interval_for_request("example.org", options.host_gap),
            PICTURE_GAP
        );
        assert_eq!(
            politeness.interval_for_request("www.reddit.com", options.host_gap),
            Duration::from_secs(61),
            "a picture must not talk a rule down"
        );
        assert_eq!(
            politeness.interval_for_request("slow.example", options.host_gap),
            Duration::from_secs(30),
            "nor the reader's own number"
        );
        assert_eq!(
            politeness.interval_for_request("example.org", None),
            Duration::from_secs(2),
            "a request with nothing in mind still waits the default"
        );

        // And the gap is really waited: two pictures off one host are serial
        // with the short gap between them rather than the long one.
        let politeness = Politeness::new(Duration::from_secs(30));
        drop(
            politeness
                .lease("example.org", Some(Duration::from_millis(50)))
                .expect("the slot"),
        );
        let started = Instant::now();
        drop(
            politeness
                .lease("example.org", Some(Duration::from_millis(50)))
                .expect("the slot"),
        );
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(30), "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");
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
        drop(politeness.lease("example.org", None).expect("the slot"));
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
