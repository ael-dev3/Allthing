// SPDX-License-Identifier: AGPL-3.0-or-later
//! `/api/v1/scheduled`: a signed-in reader's scheduled casts (see
//! `allthing_core::scheduled`). GET lists them; POST schedules one, or puts
//! one in the place of another (`replaces`); POST to `cancel` takes one off,
//! and says what it did (`Cancelled`): taken off (as it says again when
//! asked twice), or too late, as the cast went out or is going; one that is
//! someone else's it refuses. Every call needs the reader's session, every
//! POST must come from Allthing itself (its `Origin`), and no answer is
//! cached.
//!
//! The Worker keeps the message the reader's browser signed, after checking
//! that it is a cast of theirs for a time ahead, and the cron gives it to
//! the node at that time (`allthing_data::scheduled`). It never has the
//! reader's key, and signs nothing.

use allthing_core::scheduled::{
    self, Cancel, MAX_DRAFT_BYTES, MAX_WAITING, SCHEDULED_PATH, Schedule, ScheduledCast, ScheduledList, State,
};
use serde::de::DeserializeOwned;
use worker::{Env, Method, Request, Response, Result, console_error};

use crate::auth::{json, signed_in_fid};
use crate::error;
use crate::index::D1Index;

/// The largest request body: a signed message and its draft, in JSON.
const MAX_BODY: usize = 16 * 1024;
const SIGN_IN: &str = "Sign in to schedule casts";
const NOT_YOURS: &str = "That scheduled cast isn't yours to cancel. Reload the page to see your own.";
const UNAVAILABLE: &str = "Scheduled casts can't be read or kept just now; try again shortly";

/// `/api/v1/scheduled<rest>`, at `now` (Unix seconds).
pub async fn handle(rest: &str, mut req: Request, env: &Env, now: i64) -> Result<Response> {
    let post = match (rest, req.method()) {
        ("", Method::Get | Method::Head) => false,
        ("" | "/cancel", Method::Post) => true,
        ("" | "/cancel", _) => return error(405, "Scheduled casts take GET, and POST", None),
        _ => return error(404, "Unknown endpoint", None),
    };
    if post {
        let origin = req.url()?.origin().ascii_serialization();
        // Browsers send Origin with every POST: another site can't schedule
        // or cancel a reader's casts.
        if req.headers().get("origin")?.as_deref() != Some(origin.as_str()) {
            return error(403, "Scheduling must come from Allthing itself", None);
        }
    }
    let Some(fid) = signed_in_fid(&req, env, now)? else { return error(401, SIGN_IN, None) };
    let Some(index) = D1Index::new(env) else {
        return error(503, "Scheduling casts isn't set up on this server", None);
    };
    match (rest, post) {
        ("", false) => match index.scheduled(fid, now).await {
            Ok(casts) => json(&ScheduledList { casts }, &[]),
            Err(failure) => unavailable(&failure.0),
        },
        ("", true) => schedule(&index, &mut req, fid, now).await,
        _ => match read_body::<Cancel>(&mut req).await {
            Some(Cancel { id }) => match index.cancel(fid, id).await {
                Ok(Some(done)) => json(&done, &[]),
                Ok(None) => error(403, NOT_YOURS, None),
                Err(failure) => unavailable(&failure.0),
            },
            None => error(400, "Expected {\"id\": a scheduled cast's id}", None),
        },
    }
}

/// Keep the reader's signed cast for its time, or in the place of one of
/// theirs.
async fn schedule(index: &D1Index, req: &mut Request, fid: u64, now: i64) -> Result<Response> {
    let Some(asked) = read_body::<Schedule>(req).await else {
        return error(400, "Expected {\"message\": a signed cast in base64, \"draft\": its draft}", None);
    };
    if asked.draft.len() > MAX_DRAFT_BYTES {
        return error(400, "That draft is too long to keep", None);
    }
    let checked = match scheduled::check(&asked.message, fid, now) {
        Ok(checked) => checked,
        Err(why) => return error(400, why, None),
    };
    match index.schedule(fid, &checked, &asked.message, &asked.draft, asked.replaces, MAX_WAITING).await {
        Ok(Some(id)) => json(
            &ScheduledCast {
                id,
                at: checked.at,
                hash: checked.hash,
                state: State::Waiting,
                error: None,
                draft: asked.draft,
            },
            &[],
        ),
        Ok(None) if asked.replaces.is_some() => error(404, "That cast has gone out, or isn't yours to change", None),
        Ok(None) => error(409, "You have 25 casts waiting already: cancel one, or wait for one to go out", None),
        Err(failure) => unavailable(&failure.0),
    }
}

fn unavailable(why: &str) -> Result<Response> {
    console_error!("{SCHEDULED_PATH}: {why}");
    error(503, UNAVAILABLE, Some(30))
}

/// The request's JSON body, if it is small and well formed.
async fn read_body<T: DeserializeOwned>(req: &mut Request) -> Option<T> {
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
