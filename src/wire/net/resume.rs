//! Responses retained only for one unfinished net job. Replaying completed
//! hops lets a job resume redirects, browser retries and pagination without
//! repeating requests that would restart the same host's waiting period.
use super::{NetError, RequestOptions, Response};
use std::cell::RefCell;
use url::Url;

const MAX_HOPS: usize = 64;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Hop {
    pub response: Response,
    pub location: Option<String>,
}

#[derive(Default)]
pub struct RequestHistory {
    hops: Vec<(Url, RequestOptions, Hop)>,
    body_bytes: usize,
}

impl std::fmt::Debug for RequestHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestHistory")
            .field("hops", &self.hops.len())
            .field("body_bytes", &self.body_bytes)
            .finish()
    }
}

thread_local! {
    static HISTORY: RefCell<Option<RequestHistory>> = const { RefCell::new(None) };
}

/// Net workers run one synchronous job per thread. Its history travels with
/// the deferred job, so the next attempt can run on any worker. The guard
/// also clears the thread-local slot if a job unwinds.
pub struct RequestScope;
impl RequestScope {
    pub fn new(history: RequestHistory) -> Self {
        HISTORY.with(|slot| {
            assert!(slot.borrow().is_none(), "nested net job");
            *slot.borrow_mut() = Some(history);
        });
        Self
    }

    pub fn finish(self) -> RequestHistory {
        HISTORY.with(|slot| slot.take().unwrap_or_default())
    }
}
impl Drop for RequestScope {
    fn drop(&mut self) {
        HISTORY.with(|slot| slot.take());
    }
}

pub(super) fn request(
    url: &Url,
    options: &RequestOptions,
    fetch: impl FnOnce() -> Result<Hop, NetError>,
) -> Result<Hop, NetError> {
    let cached = HISTORY.with(|slot| {
        let history = slot.borrow();
        let Some(history) = history.as_ref() else {
            return Ok(None);
        };
        if let Some((_, _, hop)) = history
            .hops
            .iter()
            .find(|(u, o, _)| u == url && o == options)
        {
            return Ok(Some(hop.clone()));
        }
        if history.hops.len() >= MAX_HOPS {
            return Err(NetError::Transport(
                "Deferred job request limit exceeded".into(),
            ));
        }
        Ok(None)
    })?;
    if let Some(hop) = cached {
        return Ok(hop);
    }
    let hop = fetch()?;
    HISTORY.with(|slot| {
        if let Some(history) = slot.borrow_mut().as_mut() {
            let bytes = history.body_bytes.saturating_add(hop.response.body.len());
            if bytes > MAX_BODY_BYTES {
                return Err(NetError::TooLarge(MAX_BODY_BYTES as u64));
            }
            history.body_bytes = bytes;
            history
                .hops
                .push((url.clone(), options.clone(), hop.clone()));
        }
        Ok(hop)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hop() -> Hop {
        Hop {
            response: Response {
                status: 200,
                body: vec![1],
                etag: None,
                last_modified: None,
                content_type: None,
                retry_after: None,
                final_url: "https://example.org/".into(),
            },
            location: None,
        }
    }

    #[test]
    fn completed_jobs_and_unwinding_do_not_leave_responses_for_the_next_job() {
        let url = Url::parse("https://example.org/").unwrap();
        let options = RequestOptions::default();
        let scope = RequestScope::new(RequestHistory::default());
        request(&url, &options, || Ok(hop())).unwrap();
        request(&url, &options, || panic!("already fetched")).unwrap();
        drop(scope.finish());
        let _ = std::panic::catch_unwind(|| {
            let _scope = RequestScope::new(RequestHistory::default());
            request(&url, &options, || Ok(hop())).unwrap();
            panic!("job panicked");
        });
        let _scope = RequestScope::new(RequestHistory::default());
        let mut fetched = false;
        request(&url, &options, || {
            fetched = true;
            Ok(hop())
        })
        .unwrap();
        assert!(fetched);
    }

    #[test]
    fn history_is_bounded_without_evicting_hops_needed_for_progress() {
        let options = RequestOptions::default();
        let scope = RequestScope::new(RequestHistory::default());
        for n in 0..MAX_HOPS {
            request(
                &Url::parse(&format!("https://example.org/{n}")).unwrap(),
                &options,
                || Ok(hop()),
            )
            .unwrap();
        }
        assert!(request(
            &Url::parse("https://example.org/extra").unwrap(),
            &options,
            || panic!("limit must be checked before requesting")
        )
        .is_err());
        request(
            &Url::parse("https://example.org/0").unwrap(),
            &options,
            || panic!("previous progress must not be evicted"),
        )
        .unwrap();
        drop(scope.finish());
        let _scope = RequestScope::new(RequestHistory {
            body_bytes: MAX_BODY_BYTES,
            ..Default::default()
        });
        assert!(matches!(
            request(
                &Url::parse("https://example.org/").unwrap(),
                &options,
                || Ok(hop())
            ),
            Err(NetError::TooLarge(_))
        ));
    }
}
