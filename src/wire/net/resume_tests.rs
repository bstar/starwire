use super::*;
use crate::wire::{
    db::feeds::Conditional,
    extract::Limits,
    feed::{ArticleStatus, EntryId, FeedId, FeedKind},
    state::State,
    worker::{perform_net, Done, Lane, NetJob},
    WireConfig,
};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        RwLock,
    },
    thread::{self, JoinHandle},
};

// Loopback only. Drop stops the server even when an assertion fails.
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    fn new(reply: impl Fn(&str) -> String + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(vec![]));
        let stopped = stop.clone();
        let recorded = requests.clone();
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut request = String::new();
                        loop {
                            let mut line = String::new();
                            reader.read_line(&mut line).unwrap();
                            if line == "\r\n" || line.is_empty() {
                                break;
                            }
                            request.push_str(&line);
                        }
                        recorded.lock().unwrap().push(request.clone());
                        stream.write_all(reply(&request).as_bytes()).unwrap();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
fn live() -> Live {
    Live {
        agent: ureq::Agent::config_builder()
            .https_only(false)
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into(),
        politeness: Politeness::new(Duration::from_secs(4)),
    }
}
fn ready(http: &Live) {
    // Advance the host clock instead of sleeping four seconds per hop.
    let slot = http.politeness.slot("127.0.0.1");
    slot.state.lock().unwrap().last = Instant::now() - Duration::from_secs(5);
}
fn run(job: NetJob, http: &Live, cfg: &WireConfig, state: &Arc<RwLock<State>>) -> Done {
    // A resumed job must work on a different net thread, with no old TLS.
    let mut done = thread::scope(|s| {
        s.spawn(|| perform_net(job, Lane::Background, http, cfg, state))
            .join()
            .unwrap()
    });
    assert_eq!(done.len(), 1);
    done.pop().unwrap()
}
fn deferred(done: Done) -> NetJob {
    let Done::Deferred { job, until, .. } = done else {
        panic!("{done:?}")
    };
    assert!(until > Instant::now());
    job
}

#[test]
fn deferred_redirect_chain_resumes_on_another_worker_without_repeating_hops() {
    let feed = crate::wire::testing::feed_bytes("atom-basic.xml");
    let server = Server::new(move |request| {
        if request.starts_with("GET /start ") {
            response("302 Found", "Location: /middle\r\n", "")
        } else if request.starts_with("GET /middle ") {
            response("302 Found", "Location: /finish\r\n", "")
        } else {
            response(
                "200 OK",
                "Content-Type: application/atom+xml\r\n",
                std::str::from_utf8(&feed).unwrap(),
            )
        }
    });
    let http = live();
    let cfg = WireConfig::default();
    let state = Arc::new(RwLock::new(State::new(&cfg)));
    let job = NetJob::Fetch {
        feed: FeedId(1),
        url: format!("{}/start", server.url),
        kind: FeedKind::Web,
        conditional: Conditional::default(),
        max_bytes: 1 << 20,
        generation: 0,
    };
    let job = deferred(run(job, &http, &cfg, &state));
    // Retrying too early must neither make a request nor discard progress.
    let job = deferred(run(job, &http, &cfg, &state));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    ready(&http);
    let job = deferred(run(job, &http, &cfg, &state));
    ready(&http);
    let Done::Fetched { outcome, .. } = run(job, &http, &cfg, &state) else {
        panic!("fetch did not complete")
    };
    assert!(!outcome.is_failure(), "{outcome:?}");
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for (request, path) in requests.iter().zip(["/start", "/middle", "/finish"]) {
        assert!(request.starts_with(&format!("GET {path} ")));
    }
}

#[test]
fn browser_retry_keeps_first_response_but_does_not_replay_it_for_other_headers() {
    let html = crate::wire::testing::page("article.html");
    let server = Server::new(move |request| {
        if request.contains(BROWSER_USER_AGENT) {
            response("200 OK", "Content-Type: text/html\r\n", &html)
        } else {
            response("403 Forbidden", "", "")
        }
    });
    let http = live();
    let cfg = WireConfig::default();
    let state = Arc::new(RwLock::new(State::new(&cfg)));
    let job = NetJob::Extract {
        entry: EntryId(1),
        url: format!("{}/article", server.url),
        limits: Limits::default(),
        generation: 0,
    };
    let job = deferred(run(job, &http, &cfg, &state));
    ready(&http);
    let Done::Extracted { result, .. } = run(job, &http, &cfg, &state) else {
        panic!("extraction did not complete")
    };
    assert_eq!(result.status, ArticleStatus::Extracted, "{result:?}");
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[test]
fn cancelled_resumed_fetch_never_reaches_the_network() {
    let server = Server::new(|_| response("302 Found", "Location: /finish\r\n", ""));
    let http = live();
    let cfg = WireConfig::default();
    let state = Arc::new(RwLock::new(State::new(&cfg)));
    let job = NetJob::Fetch {
        feed: FeedId(1),
        url: format!("{}/start", server.url),
        kind: FeedKind::Web,
        conditional: Conditional::default(),
        max_bytes: 1024,
        generation: 0,
    };
    let job = deferred(run(job, &http, &cfg, &state));
    state.write().unwrap().refresh_generation += 1;
    ready(&http);
    assert!(perform_net(job, Lane::Background, &http, &cfg, &state).is_empty());
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}
