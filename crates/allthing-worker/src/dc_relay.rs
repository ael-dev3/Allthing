// SPDX-License-Identifier: AGPL-3.0-or-later
//! `/api/v1/dc/<endpoint>`: the Direct Cast API (`allthing_core::dc`),
//! passed on for a signed-in reader whose browser can't call it itself,
//! since the API answers other sites' pages only for sending
//! (docs/DIRECT_CASTS.md). The reader's own API key comes with each call,
//! in the `x-dc-key` header, and goes on as `Authorization: Bearer`.
//!
//! This is the one place a reader's secret passes through Allthing's
//! server, so it is off unless Allthing's owner turns it on (`DC_RELAY=1`,
//! docs/DEPLOY.md). Until then every path under it answers 404, and the
//! browser sends the key nowhere but Farcaster's API. On, it:
//!
//! - passes on only the five endpoints Allthing uses, each with its one
//!   method, with only the query keys each takes ([`dc::relay_query`]);
//! - takes calls only from Allthing's own pages, with the reader's session,
//!   and counts them: [`PER_MINUTE`] a reader in an isolate;
//! - takes a body of [`dc::MAX_BODY`] at most, and sends on the message as
//!   it checked it, not the bytes it was given ([`dc::check_send`]);
//! - keeps nothing: the key and the bodies pass through memory once, and
//!   are never cached (`Cache-Control: no-store`, outside `cache::serve`),
//!   stored or logged. Cloudflare's own logs redact a header whose name
//!   has "key" or "auth" in it, as `x-dc-key` and `authorization` do;
//! - spends one subrequest a call, through `upstream::relay`'s guard.

use std::cell::RefCell;
use std::collections::HashMap;

use allthing_core::dc;
use allthing_core::message::is_local_stand_in;
use allthing_data::{HttpMethod, HttpResponse, Invocation, upstream};
use worker::{Env, Headers, Method, Request, Response, Result, Url};

use crate::auth::{json, signed_in_fid};
use crate::error;

/// Calls a reader may make in a minute, in this isolate: the Messages page
/// asks for the list and the open conversation every 20 seconds, and
/// there's room to page back and send.
const PER_MINUTE: u32 = 30;
/// Readers kept count of before old minutes are forgotten.
const READERS: usize = 1000;

thread_local! {
    /// Each reader's calls: when their minute began (Unix seconds), and how
    /// many they made in it.
    static RECENT: RefCell<HashMap<u64, (i64, u32)>> = RefCell::new(HashMap::new());
}

/// Whether Allthing's owner turned the relay on: the `DC_RELAY` variable
/// is `1`.
pub fn enabled(env: &Env) -> bool {
    env.var("DC_RELAY").is_ok_and(|on| on.to_string() == "1")
}

/// Where the API is: the Farcaster app's, or, for the end-to-end tests, a
/// stand-in on this machine (`DC_ORIGIN`). Anything but a local port is
/// ignored, so it can't send keys elsewhere where Allthing is deployed.
fn api_origin(env: &Env) -> String {
    env.var("DC_ORIGIN")
        .ok()
        .map(|origin| origin.to_string())
        .filter(|origin| is_local_stand_in(origin))
        .unwrap_or_else(|| dc::API.to_owned())
}

/// Whether `fid` may make a call at `now` (Unix seconds), counting it.
fn allowed(fid: u64, now: i64) -> bool {
    RECENT.with(|recent| {
        let mut recent = recent.borrow_mut();
        if recent.len() >= READERS {
            recent.retain(|_, (began, _)| now - *began < 60);
        }
        let (began, count) = recent.entry(fid).or_insert((now, 0));
        if now - *began >= 60 {
            *began = now;
            *count = 0;
        }
        *count += 1;
        *count <= PER_MINUTE
    })
}

/// Whether a call comes from Allthing's own pages, `ours`. A browser says
/// where a request comes from, and a page elsewhere can't make it lie: in
/// `Sec-Fetch-Site`, and in `Origin`, which it sends with all but a
/// same-origin GET.
fn from_allthing(get: bool, origin: Option<&str>, site: Option<&str>, ours: &str) -> bool {
    let origin = match origin {
        Some(origin) => origin == ours,
        None => get,
    };
    origin && site.is_none_or(|site| site == "same-origin")
}

/// `/api/v1/dc` and what's under it (`rest`), at `now` (Unix seconds).
pub async fn handle(
    rest: &str,
    mut req: Request,
    env: &Env,
    url: &Url,
    inv: &Invocation,
    now: i64,
) -> Result<Response> {
    if !enabled(env) {
        return error(404, "Not found", None);
    }
    // `GET /api/v1/dc`: on. The browser asks before it tries the relay.
    if rest.is_empty() {
        return match req.method() {
            Method::Get | Method::Head => json(&serde_json::json!({ "relay": true }), &[]),
            _ => wrong_method("GET"),
        };
    }
    let Some(endpoint) = rest.strip_prefix('/').and_then(dc::Endpoint::named) else {
        return error(404, "No such Direct Cast endpoint", None);
    };
    let (method, upstream_method) = match endpoint.method() {
        dc::Method::Get => (Method::Get, HttpMethod::Get),
        dc::Method::Put => (Method::Put, HttpMethod::Put),
    };
    if req.method() != method {
        return wrong_method(endpoint.method().as_str());
    }
    let ours = url.origin().ascii_serialization();
    let (origin, site) = (req.headers().get("origin")?, req.headers().get("sec-fetch-site")?);
    if !from_allthing(method == Method::Get, origin.as_deref(), site.as_deref(), &ours) {
        return error(403, "Direct Casts are relayed only for Allthing's own pages", None);
    }
    let Some(fid) = signed_in_fid(&req, env, now)? else {
        return error(401, "Sign in to read your Direct Casts here", None);
    };
    if !allowed(fid, now) {
        return error(429, "Too many Direct Cast requests; slow down", Some(60));
    }
    let Some(key) = req.headers().get(dc::KEY_HEADER)?.and_then(|key| dc::check_key(&key).ok()) else {
        return error(400, "Expected your Farcaster API key (wc_secret_…) in the x-dc-key header", None);
    };
    let mut headers = vec![("authorization", format!("Bearer {key}"))];
    let body = if method == Method::Put {
        if let Some(idempotency) = req.headers().get("idempotency-key")? {
            if !dc::check_idempotency(&idempotency) {
                return error(400, "An idempotency-key is 1 to 64 letters, digits, - or _", None);
            }
            headers.push(("idempotency-key", idempotency));
        }
        match read_send(&mut req).await? {
            Ok(body) => Some(body),
            Err((status, why)) => return error(status, &why, None),
        }
    } else {
        None
    };
    let asked: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let call = dc::Call { endpoint, query: dc::relay_query(endpoint, &asked), body: None };
    match upstream::relay(inv, upstream_method, &call.url(&api_origin(env)), headers, body).await {
        Ok(answer) => passed_back(answer),
        Err(failure) => unanswered(&failure),
    }
}

/// A message's body, checked: the JSON to send on, or the status and words
/// to refuse it with.
async fn read_send(req: &mut Request) -> Result<std::result::Result<String, (u16, String)>> {
    let too_large = || (413, format!("A message's body is {} bytes at most", dc::MAX_BODY));
    match req.headers().get("content-length")?.and_then(|length| length.parse::<usize>().ok()) {
        None => return Ok(Err((411, "A message's body needs its Content-Length".into()))),
        Some(length) if length > dc::MAX_BODY => return Ok(Err(too_large())),
        Some(_) => {}
    }
    let bytes = req.bytes().await?;
    if bytes.len() > dc::MAX_BODY {
        return Ok(Err(too_large()));
    }
    let Ok(body) = String::from_utf8(bytes) else { return Ok(Err((400, dc::SendProblem::Shape.message()))) };
    Ok(dc::check_send(&body).map_err(|problem| (400, problem.message())))
}

/// The API's answer, as it gave it, marked as the API's: never cached.
fn passed_back(answer: HttpResponse) -> Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", "application/json; charset=utf-8")?;
    headers.set("cache-control", "no-store")?;
    headers.set(dc::RELAY_HEADER, "upstream")?;
    if let Some(seconds) =
        answer.retry_after.filter(|s| !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit()))
    {
        headers.set("retry-after", &seconds)?;
    }
    let response = match answer.status {
        // Statuses an answer can't have a body with.
        204 | 205 | 304 => Response::empty()?,
        200..=599 => Response::from_bytes(answer.body.unwrap_or_default())?,
        _ => return unanswered(&upstream::UpstreamError::Status(answer.status)),
    };
    Ok(response.with_status(answer.status).with_headers(headers))
}

/// No answer from the API, or none in time, or Allthing's guard held the
/// call back: said as the API says why, and marked as about it, so the
/// browser tells the reader the API isn't answering, and tries again.
fn unanswered(failure: &upstream::UpstreamError) -> Result<Response> {
    use upstream::UpstreamError;
    let (status, retry_after) = match failure {
        UpstreamError::Busy(ms) => (503, Some((ms / 1000.0).ceil().max(1.0) as u32)),
        UpstreamError::Budget => (503, Some(5)),
        UpstreamError::Timeout => (504, None),
        _ => (502, None),
    };
    let headers = Headers::new();
    headers.set("content-type", "application/json; charset=utf-8")?;
    headers.set("cache-control", "no-store")?;
    headers.set(dc::RELAY_HEADER, "upstream")?;
    if let Some(seconds) = retry_after {
        headers.set("retry-after", &seconds.to_string())?;
    }
    let body = serde_json::json!({ "errors": [{ "message": failure.to_string() }] });
    Ok(Response::from_json(&body)?.with_status(status).with_headers(headers))
}

/// 405, with the one method the path takes.
fn wrong_method(allowed: &str) -> Result<Response> {
    let mut response = error(405, &format!("This takes {allowed}"), None)?;
    response.headers_mut().set("allow", allowed)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_makes_thirty_calls_a_minute() {
        for _ in 0..PER_MINUTE {
            assert!(allowed(7, 1_000));
        }
        assert!(!allowed(7, 1_030));
        // Someone else's count is their own.
        assert!(allowed(8, 1_030));
        // A new minute starts the count again.
        assert!(allowed(7, 1_060));
    }

    #[test]
    fn calls_come_from_allthings_own_pages() {
        let ours = "https://allthing.social";
        // A same-origin GET has no Origin; a PUT names it.
        assert!(from_allthing(true, None, Some("same-origin"), ours));
        assert!(from_allthing(false, Some(ours), Some("same-origin"), ours));
        assert!(from_allthing(true, None, None, ours));
        // Another site, or a PUT that doesn't say where it's from.
        assert!(!from_allthing(true, Some("https://elsewhere.example"), Some("cross-site"), ours));
        assert!(!from_allthing(true, None, Some("cross-site"), ours));
        assert!(!from_allthing(true, None, Some("same-site"), ours));
        assert!(!from_allthing(false, None, Some("same-origin"), ours));
        assert!(!from_allthing(false, Some("https://elsewhere.example"), Some("same-origin"), ours));
        assert!(!from_allthing(false, Some("null"), None, ours));
    }
}
