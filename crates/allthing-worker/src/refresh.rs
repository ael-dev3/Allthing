// SPDX-License-Identifier: AGPL-3.0-or-later
//! `POST /api/v1/refresh`: a signed-in reader just wrote from Allthing with
//! their key (a cast, a reply, a like, a recast, a delete), and the pages it
//! changes are rebuilt on their next read instead of when their copies age
//! out: their own profile, the cast they replied to or reacted to, with its
//! replies or its people, the post they wrote it on, the channel they cast
//! in ([`Wrote`]). Each is expired in the edge cache (`cache::expire`):
//! kept, so a rebuild that fails still serves it, but due.
//!
//! It needs the reader's session, must come from Allthing itself (its
//! `Origin`), and asks nothing upstream: a cache read, and a write, for each
//! page. A profile is expired only when it is the session's own, and a
//! reader may refresh a few times a minute in an isolate. Nothing is cached.

use std::cell::RefCell;
use std::collections::HashMap;

use allthing_core::api::Wrote;
use allthing_data::{Endpoint, Invocation};
use serde::Deserialize;
use worker::{Env, Method, Request, Response, Result, Url};

use crate::auth::{json, signed_in_fid};
use crate::{cache, cache_key, error};

/// The largest request body: a name, three hashes and a channel, in JSON.
const MAX_BODY: usize = 1024;
/// Refreshes a reader may ask for in a minute, in this isolate: a few
/// writes, each tried again once or twice.
const PER_MINUTE: u32 = 20;
/// Readers kept count of before old minutes are forgotten.
const READERS: usize = 1000;

thread_local! {
    /// Each reader's refreshes: when their minute began (Unix seconds), and
    /// how many they asked for in it.
    static RECENT: RefCell<HashMap<u64, (i64, u32)>> = RefCell::new(HashMap::new());
}

/// Whether `fid` may refresh at `now` (Unix seconds), counting it.
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

#[derive(Deserialize)]
struct Profile {
    user: Account,
}

#[derive(Deserialize)]
struct Account {
    fid: u64,
}

/// Whose profile a `user` payload is.
fn profile_fid(body: &[u8]) -> Option<u64> {
    serde_json::from_slice::<Profile>(body).ok().map(|profile| profile.user.fid)
}

/// `POST /api/v1/refresh` with a [`Wrote`], at `now` (Unix seconds): how
/// many pages were expired.
pub async fn handle(mut req: Request, env: &Env, url: &Url, inv: &Invocation, now: i64) -> Result<Response> {
    if req.method() != Method::Post {
        return error(405, "Refreshing takes POST", None);
    }
    // Browsers send Origin with every POST: another site can't make a
    // reader's pages rebuild.
    if req.headers().get("origin")?.as_deref() != Some(url.origin().ascii_serialization().as_str()) {
        return error(403, "Refreshing must come from Allthing itself", None);
    }
    let Some(fid) = signed_in_fid(&req, env, now)? else { return error(401, "Sign in to refresh", None) };
    if !allowed(fid, now) {
        return error(429, "Too many refreshes; slow down", Some(60));
    }
    let Some(wrote) = read_body(&mut req).await else {
        return error(
            400,
            "Expected what was written: {\"username\", \"reply_to\", \"reacted_to\", \"page\", \"channel\"}",
            None,
        );
    };
    let mut expired = 0;
    for path in wrote.paths() {
        let Ok(endpoint) = Endpoint::from_path(&path) else { continue };
        let profile = matches!(endpoint, Endpoint::User(_));
        let key = cache_key(url, &endpoint.cache_name());
        // A profile only when it is the reader's own.
        if cache::expire(inv, &key, |entry| !profile || profile_fid(&entry.body) == Some(fid)).await {
            expired += 1;
        }
    }
    json(&serde_json::json!({ "expired": expired }), &[])
}

/// The request's JSON body, if it is small and well formed.
async fn read_body(req: &mut Request) -> Option<Wrote> {
    let declared = req.headers().get("content-length").ok().flatten()?.parse::<usize>().ok()?;
    if declared > MAX_BODY {
        return None;
    }
    let body = req.text().await.ok()?;
    if body.len() > MAX_BODY {
        return None;
    }
    serde_json::from_str(&body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_refreshes_twenty_times_a_minute() {
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
    fn a_profile_says_whose_it_is() {
        let body =
            br#"{"user":{"fid":3356318,"username":"allthingapp","display_name":"Allthing"},"pool":[],"meta":{}}"#;
        assert_eq!(profile_fid(body), Some(3356318));
        assert_eq!(profile_fid(br#"{"error":"No such user"}"#), None);
        assert_eq!(profile_fid(b"not json"), None);
    }
}
