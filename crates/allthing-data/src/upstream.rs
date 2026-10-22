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
    /// Base JSON-RPC, for the NS vote read onchain.
    BaseRpc,
    /// OP Mainnet JSON-RPC, for Farcaster's registries when someone signs in.
    OptimismRpc,
    /// A chain's public JSON-RPC, for wallet activity.
    Chain(&'static str),
    /// The farcaster.xyz app's public read API (vote status, channel details).
    FarcasterApp,
    /// The fname server's public transfer log (when accounts got usernames).
    Fnames,
    /// Pages that casts link to, read for mini app meta tags. Many sites,
    /// so no breaker: one broken site says nothing about the others.
    LinkPage,
}

impl Upstream {
    pub fn name(self) -> &'static str {
        match self {
            Upstream::Hypersnap => "hypersnap",
            Upstream::BaseRpc => "base-rpc",
            Upstream::OptimismRpc => "optimism-rpc",
            Upstream::Chain(slug) => slug,
            Upstream::FarcasterApp => "farcaster-app",
            Upstream::Fnames => "fnames",
            Upstream::LinkPage => "link-pages",
        }
    }

    /// (burst, sustained requests per second) per isolate.
    fn rate(self) -> (u32, f64) {
        match self {
            Upstream::Hypersnap => (40, 10.0),
            Upstream::BaseRpc | Upstream::OptimismRpc => (10, 2.0),
            Upstream::Chain("solana") => (4, 0.4),
            // A keyed endpoint serves more requests a second than the public one.
            Upstream::Chain("solana-keyed") => (10, 5.0),
            Upstream::FarcasterApp => (6, 1.0),
            Upstream::Fnames => (6, 1.0),
            Upstream::LinkPage => (12, 2.0),
            Upstream::Chain(_) => (4, 0.5),
        }
    }

    fn timeout_ms(self) -> u32 {
        match self {
            Upstream::Hypersnap => 8000,
            Upstream::LinkPage => 2500,
            _ => 5000,
        }
    }

    fn accept(self) -> &'static str {
        match self {
            Upstream::LinkPage => "text/html",
            _ => "application/json",
        }
    }

    /// Only a page's `<head>` is needed, which is near its start.
    fn max_bytes(self) -> Option<u32> {
        match self {
            Upstream::LinkPage => Some(131_072),
            _ => None,
        }
    }

    /// A link page is read up to the end of its `<head>`, not the rest of
    /// the 128 KB: a large page from a slow server spent the read's timeout
    /// on the body.
    fn stop_at(self) -> Option<fn(&[u8], usize) -> bool> {
        match self {
            Upstream::LinkPage => Some(allthing_core::embeds::head_ended),
            _ => None,
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

/// Requests `upstream`'s per-isolate bucket allows now beyond `reserve`,
/// without taking any (0 while its breaker is open): for work that must
/// not crowd out page builds, such as preloaded popover previews.
pub fn spare(inv: &Invocation, upstream: Upstream, reserve: u32) -> u32 {
    let now = inv.now();
    with_guard(upstream, now, |guard| {
        if upstream != Upstream::LinkPage && !guard.breaker.allow(now) {
            return 0;
        }
        guard.bucket.available(now).saturating_sub(reserve)
    })
}

/// Check and take: breaker, then bucket, then the invocation's budget.
fn admit(inv: &Invocation, upstream: Upstream) -> Result<(), UpstreamError> {
    let now = inv.now();
    with_guard(upstream, now, |guard| {
        if upstream != Upstream::LinkPage && !guard.breaker.allow(now) {
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
        Ok(_) if upstream == Upstream::LinkPage => {}
        Ok(_) => guard.breaker.success(),
        Err(_) if upstream == Upstream::LinkPage => guard.failures += 1,
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
    let request = HttpRequest {
        method,
        url: url.to_owned(),
        headers: Vec::new(),
        body: body.map(str::to_owned),
        octets: None,
        any_body: false,
        timeout_ms: upstream.timeout_ms(),
        accept: upstream.accept(),
        max_bytes: upstream.max_bytes(),
        stop_at: upstream.stop_at(),
    };
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

/// Give a node a signed Farcaster message (its protobuf bytes, as the
/// browser would), through Hypersnap's guard: the node's answer, its status
/// and body, for `allthing_core::message::submitted` to read. An answer that
/// says the node is busy or down (429, 5xx) counts against its breaker, as
/// any other read's does, and comes back as an error.
pub async fn submit(inv: &Invocation, url: &str, message: Vec<u8>) -> Result<(u16, String), UpstreamError> {
    let upstream = Upstream::Hypersnap;
    admit(inv, upstream)?;
    let request = HttpRequest {
        method: HttpMethod::Post,
        url: url.to_owned(),
        headers: Vec::new(),
        body: None,
        octets: Some(message),
        any_body: true,
        timeout_ms: upstream.timeout_ms(),
        accept: upstream.accept(),
        max_bytes: None,
        stop_at: None,
    };
    let (outcome, retry_after) = match inv.fetch(request).await {
        Ok(response) if response.status == 429 || response.status >= 500 => {
            (Err(UpstreamError::Status(response.status)), response.retry_after.as_deref().and_then(parse_retry_after))
        }
        Ok(response) => {
            let body = String::from_utf8_lossy(&response.body.unwrap_or_default()).into_owned();
            (Ok((response.status, body)), None)
        }
        Err(FetchError::Timeout) => (Err(UpstreamError::Timeout), None),
        Err(FetchError::Network(e)) => (Err(UpstreamError::Network(e)), None),
    };
    record(inv, upstream, &outcome.as_ref().map(|_| None).map_err(Clone::clone), retry_after);
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
