// SPDX-License-Identifier: AGPL-3.0-or-later
//! `POST /api/v1/presence`: a browser's beat, and who is reading Allthing
//! now (`allthing_core::presence`).
//!
//! A beat records its visitor in D1: a signed-in reader who shows themselves
//! by their account, from the sealed session cookie (never a fid, name or
//! picture the browser sends), anyone else by an HMAC of their browser's
//! random id for the day, under `SESSION_SECRET`. A reader who hides is
//! stored as one signed out, and their row by name goes at once. Nothing
//! else of theirs is kept or logged: no IP, no user agent, no raw id. The
//! answer (how many are here, up to twelve shown by name, and when to beat
//! again) is kept for 30 seconds in each Cloudflare location, so a location
//! recounts at most twice a minute. A beat makes at most four subrequests:
//! the cached answer, one D1 batch, on a recount the names and pictures of
//! those shown (Hypersnap's `user/bulk`, through `upstream`) and the new
//! answer's write; and once per isolate, the schema. `PRESENCE = "on"` in
//! wrangler.toml turns it on; anything else answers 404, and the header's
//! chip never shows (docs/DEPLOY.md#presence).

use std::rc::Rc;

use allthing_core::api::{API_PREFIX, CachePolicy};
use allthing_core::normalize;
use allthing_core::presence::{
    Answer, Beat, ID_PURPOSE, KEEP, MAX_NEXT, SHOWN, anonymous_visitor, is_id, named_visitor, next_interval,
    online_window,
};
use allthing_data::{Invocation, hypersnap};
use hmac::Mac;
use serde::Deserialize;
use worker::{Context, Env, Headers, Method, Request, Response, Result};

use crate::auth::{hex, mac, session_secret, signed_in_fid};
use crate::cache::{self, Entry};
use crate::error;
use crate::host::{Lifetime, WorkerHost};
use crate::index::D1Index;

/// A beat's body at most: `{"id":"<32 hex>","hidden":false}` and room.
const MAX_BODY: usize = 256;

/// How long a location keeps the answer it counted, in milliseconds.
const FRESH_MS: f64 = 30_000.0;

/// The subrequests a beat may make: see the module notes.
const BUDGET: u32 = 4;

const OFF: &str = "Presence is off here";
const UNSET: &str = "Presence isn't set up here";

/// Whether presence is on: `PRESENCE = "on"` in wrangler.toml's `[vars]`.
pub fn ready(env: &Env) -> bool {
    env.var("PRESENCE").is_ok_and(|on| on.to_string() == "on")
}

/// A browser's beat at `now_ms` (Unix milliseconds): see the module notes.
pub async fn handle(mut req: Request, env: &Env, ctx: Context, now_ms: f64) -> Result<Response> {
    if !ready(env) {
        return error(404, OFF, None);
    }
    if req.method() != Method::Post {
        return error(405, "A beat is a POST", None);
    }
    let url = req.url()?;
    let origin = url.origin().ascii_serialization();
    // Browsers send Origin with every POST: another site can't beat for a reader.
    if req.headers().get("origin")?.as_deref() != Some(origin.as_str()) {
        return error(403, "Beats must come from Allthing itself", None);
    }
    let (Some(secret), Some(index)) = (session_secret(env), D1Index::new(env)) else { return error(503, UNSET, None) };
    let Some(beat) = read_beat(&mut req).await else {
        return error(400, "A beat is {\"id\": 32 lowercase hex, \"hidden\": bool}", None);
    };
    let now = (now_ms / 1000.0) as i64;
    // Shown by name only as the session's own account, and only if they
    // don't hide; anyone else as a hash of the day's id. A reader who hides
    // is stored as one signed out, and their row by name goes at once.
    let signed_in = signed_in_fid(&req, env, now)?;
    let (fid, forget) = if beat.hidden { (None, signed_in) } else { (signed_in, None) };
    let visitor = match fid {
        Some(fid) => named_visitor(fid),
        None => anonymous_visitor(&hex(&mac(secret.as_bytes(), ID_PURPOSE, &beat.id).finalize().into_bytes())),
    };
    let host = WorkerHost::new(Lifetime::Request(ctx), None, None, crate::node_stand_in(env));
    let inv = Invocation::new(Rc::new(host), BUDGET);
    let key = format!("{origin}/__cache{API_PREFIX}/presence");
    let cached = cache::peek(&inv, &key).await.filter(|entry| entry.status == 200);
    // Counted in the last 30 s: this beat is written, and gets that answer,
    // as it was kept.
    if let Some(entry) = cached.as_ref().filter(|entry| now_ms - entry.fetched_at < FRESH_MS) {
        return match index.presence(&visitor, fid, forget, now, None, SHOWN).await {
            Ok(_) => answer_with(entry.body.to_vec()),
            Err(failure) => unavailable(&failure.0),
        };
    }
    // Recount over the window of the interval browsers were last given.
    let given = cached.and_then(|entry| serde_json::from_slice::<Given>(&entry.body).ok());
    let window = online_window(given.map_or(MAX_NEXT, |given| given.next));
    let counted = index.presence(&visitor, fid, forget, now, Some((now - i64::from(window), now - KEEP)), SHOWN).await;
    let (online, fids) = match counted {
        Ok(Some(counted)) => counted,
        Ok(None) => (0, Vec::new()),
        Err(failure) => return unavailable(&failure.0),
    };
    // Names and pictures for those shown by name, in the order they were
    // seen; one the node doesn't know is counted, not shown.
    let users = if fids.is_empty() { Vec::new() } else { hypersnap::users_bulk(&inv, &fids).await.unwrap_or_default() };
    let readers =
        fids.iter().filter_map(|fid| users.iter().find(|user| user.fid == *fid).map(normalize::user_summary)).collect();
    let body = serde_json::to_vec(&Answer { online, readers, next: next_interval(online) })?;
    let policy = CachePolicy { fresh: 30, stale: 30, keep: 60, browser: 0 };
    let entry = Entry { body: Rc::new(body), status: 200, fetched_at: now_ms, policy };
    cache::put(&inv, &key, &entry, policy.keep).await;
    answer_with(entry.body.to_vec())
}

/// The interval an answer gave, all a recount needs of the one before.
#[derive(Deserialize)]
struct Given {
    next: u32,
}

/// The beat in the request's body, if it is small, well formed and its id
/// is 32 lowercase hex characters.
async fn read_beat(req: &mut Request) -> Option<Beat> {
    let declared = req.headers().get("content-length").ok().flatten()?.parse::<usize>().ok()?;
    if declared > MAX_BODY {
        return None;
    }
    let body = req.text().await.ok()?;
    if body.len() > MAX_BODY {
        return None;
    }
    serde_json::from_str::<Beat>(&body).ok().filter(|beat| is_id(&beat.id))
}

/// The answer, an `Answer` as JSON, which no cache keeps.
fn answer_with(body: Vec<u8>) -> Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", "application/json; charset=utf-8")?;
    headers.set("cache-control", "no-store")?;
    Ok(Response::from_bytes(body)?.with_headers(headers))
}

fn unavailable(why: &str) -> Result<Response> {
    worker::console_error!("presence: {why}");
    error(503, "Presence is unavailable just now", Some(60))
}
