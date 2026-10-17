// SPDX-License-Identifier: AGPL-3.0-or-later
//! Guarded upstream requests. Every call takes one subrequest from the
//! invocation's budget, one token from the upstream's per-isolate bucket, and
//! is refused outright while that upstream's circuit breaker is open, so a
//! failing or rate-limiting upstream is backed off instead of hammered.

use std::cell::RefCell;
use std::collections::HashMap;

use allthing_core::guard::{Breaker, TokenBucket, parse_retry_after};
use serde::de::DeserializeOwned;

use crate::{FetchError, HttpMethod, HttpRequest, Invocation};

/// Upstreams Allthing reads from, each with its own bucket and breaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Upstream {
    Hypersnap,
    /// Base JSON-RPC, for Neynar scores read onchain.
    BaseRpc,
    /// A chain's public JSON-RPC, for wallet activity.
    Chain(&'static str),
}

impl Upstream {
    pub fn name(self) -> &'static str {
        match self {
            Upstream::Hypersnap => "hypersnap",
            Upstream::BaseRpc => "base-rpc",
            Upstream::Chain(slug) => slug,
        }
    }

    /// (burst, sustained requests per second) per isolate.
    fn rate(self) -> (u32, f64) {
        match self {
            Upstream::Hypersnap => (40, 10.0),
            Upstream::BaseRpc => (10, 2.0),
            Upstream::Chain("solana") => (2, 0.2),
            Upstream::Chain(_) => (4, 0.5),
        }
    }

    fn timeout_ms(self) -> u32 {
        match self {
            Upstream::Hypersnap => 8000,
            _ => 5000,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpstreamError {
    /// Circuit open or bucket empty; retry after this many milliseconds.
    Busy(f64),
    /// This invocation has no subrequests left.
    Budget,
    Timeout,
    Status(u16),
    Network(String),
    Parse(String),
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamError::Busy(ms) => write!(f, "upstream busy, retry in {:.0}s", ms / 1000.0),
            UpstreamError::Budget => write!(f, "request budget exhausted"),
            UpstreamError::Timeout => write!(f, "upstream timed out"),
            UpstreamError::Status(code) => write!(f, "upstream returned HTTP {code}"),
            UpstreamError::Network(e) => write!(f, "upstream unreachable: {e}"),
            UpstreamError::Parse(e) => write!(f, "unexpected upstream response: {e}"),
        }
    }
}

struct Guard {
    bucket: TokenBucket,
    breaker: Breaker,
    requests: u64,
    failures: u64,
}

thread_local! {
    static GUARDS: RefCell<HashMap<&'static str, Guard>> = RefCell::new(HashMap::new());
}

fn with_guard<T>(upstream: Upstream, now: f64, f: impl FnOnce(&mut Guard) -> T) -> T {
    GUARDS.with(|guards| {
        let mut guards = guards.borrow_mut();
        let guard = guards.entry(upstream.name()).or_insert_with(|| {
            let (burst, per_second) = upstream.rate();
            Guard {
                bucket: TokenBucket::new(burst, per_second, now),
                // Open after 3 consecutive failures; back off 5 s doubling to 5 min.
                breaker: Breaker::new(3, 5_000.0, 300_000.0),
                requests: 0,
                failures: 0,
            }
        });
        f(guard)
    })
}

/// A snapshot of each upstream's guard, for `/api/v1/status`.
pub fn guard_status(now: f64) -> Vec<serde_json::Value> {
    GUARDS.with(|guards| {
        let guards = guards.borrow();
        let mut out: Vec<_> = guards
            .iter()
            .map(|(name, g)| {
                serde_json::json!({
                    "upstream": name,
                    "requests": g.requests,
                    "failures": g.failures,
                    "consecutive_failures": g.breaker.failures(),
                    "open_for_ms": g.breaker.retry_after_ms(now),
                })
            })
            .collect();
        out.sort_by(|a, b| a["upstream"].as_str().cmp(&b["upstream"].as_str()));
        out
    })
}

/// Check and take: breaker, then bucket, then the invocation's budget.
fn admit(inv: &Invocation, upstream: Upstream) -> Result<(), UpstreamError> {
    let now = inv.now();
    with_guard(upstream, now, |guard| {
        if !guard.breaker.allow(now) {
            return Err(UpstreamError::Busy(guard.breaker.retry_after_ms(now)));
        }
        if !guard.bucket.try_take(now) {
            return Err(UpstreamError::Busy(guard.bucket.wait_ms(now).max(1000.0)));
        }
        Ok(())
    })?;
    if !inv.take_subrequests(1) {
        return Err(UpstreamError::Budget);
    }
    with_guard(upstream, now, |guard| guard.requests += 1);
    Ok(())
}

fn record(
    inv: &Invocation,
    upstream: Upstream,
    outcome: &Result<Option<Vec<u8>>, UpstreamError>,
    retry_after: Option<f64>,
) {
    let now = inv.now();
    with_guard(upstream, now, |guard| match outcome {
        Ok(_) => guard.breaker.success(),
        // Our own budget, or a client-side 4xx, says nothing about upstream health.
        Err(UpstreamError::Budget | UpstreamError::Parse(_)) => {}
        Err(UpstreamError::Status(code)) if *code < 500 && *code != 429 => {}
        Err(_) => {
            guard.failures += 1;
            guard.breaker.failure(now, retry_after);
        }
    });
}

/// Send a request with a timeout and return the body of a 2xx answer.
/// `Ok(None)` for 404 (and for 400 when `missing_on_400`, since hub-style
/// APIs answer "not found" with 400).
pub async fn send(
    inv: &Invocation,
    upstream: Upstream,
    method: HttpMethod,
    url: &str,
    body: Option<&str>,
    missing_on_400: bool,
) -> Result<Option<Vec<u8>>, UpstreamError> {
    admit(inv, upstream)?;
    let request =
        HttpRequest { method, url: url.to_owned(), body: body.map(str::to_owned), timeout_ms: upstream.timeout_ms() };
    let (outcome, retry_after) = match inv.fetch(request).await {
        Ok(response) => match (response.status, response.body) {
            (_, Some(bytes)) => (Ok(Some(bytes)), None),
            (404, None) => (Ok(None), None),
            (400, None) if missing_on_400 => (Ok(None), None),
            (status, None) => {
                (Err(UpstreamError::Status(status)), response.retry_after.as_deref().and_then(parse_retry_after))
            }
        },
        Err(FetchError::Timeout) => (Err(UpstreamError::Timeout), None),
        Err(FetchError::Network(e)) => (Err(UpstreamError::Network(e)), None),
    };
    record(inv, upstream, &outcome, retry_after);
    outcome
}

pub fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, UpstreamError> {
    serde_json::from_slice(bytes).map_err(|e| UpstreamError::Parse(e.to_string()))
}

/// GET and parse JSON; `Ok(None)` when the upstream has no such thing.
pub async fn get_json<T: DeserializeOwned>(
    inv: &Invocation,
    upstream: Upstream,
    url: &str,
    missing_on_400: bool,
) -> Result<Option<T>, UpstreamError> {
    match send(inv, upstream, HttpMethod::Get, url, None, missing_on_400).await? {
        Some(bytes) => parse(&bytes).map(Some),
        None => Ok(None),
    }
}

/// POST a JSON-RPC body and parse the answer.
pub async fn rpc<T: DeserializeOwned>(
    inv: &Invocation,
    upstream: Upstream,
    url: &str,
    body: &str,
) -> Result<T, UpstreamError> {
    match send(inv, upstream, HttpMethod::Post, url, Some(body), false).await? {
        Some(bytes) => parse(&bytes),
        None => Err(UpstreamError::Status(404)),
    }
}

/// `base/path?k=v&…` with values percent-encoded.
pub fn url(base: &str, path: &str, query: &[(&str, &str)]) -> String {
    let mut out = format!("{base}/{path}");
    for (i, (key, value)) in query.iter().enumerate() {
        out.push(if i == 0 { '?' } else { '&' });
        out.push_str(key);
        out.push('=');
        out.push_str(&allthing_core::params::percent_encode(value));
    }
    out
}
