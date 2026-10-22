// SPDX-License-Identifier: AGPL-3.0-or-later
//! `POST /api/v1/poll`: a vote in Allthing's poll (`allthing_core::poll`),
//! one per network, from Allthing's own pages, signed in or not.
//!
//! The network is the address Cloudflare saw the vote come from
//! (`CF-Connecting-IP`): an IPv4 address, or an IPv6 address's /64. D1
//! keeps only its voter key, the HMAC of the poll and the network keyed
//! with the `SESSION_SECRET` Worker secret ([`voter`]), and the choice. The
//! address is never logged, kept or answered with. A network that has voted
//! is told so, with the counts, and never which way it voted: on a shared
//! address it may have been someone else. The answer is never cached.

use allthing_core::poll::{self, Answer, MAX_BODY, NOT_SET_UP, POLL_PATH, VOTER_PURPOSE, Vote};
use hmac::Mac;
use worker::{Env, Method, Request, Response, Result, console_error};

use crate::auth::{hex, json, mac, session_secret};
use crate::error;
use crate::index::D1Index;

const USAGE: &str = "Expected {\"poll\": \"mobile-client-v1\", \"choice\": \"yes\" or \"no\"}";
/// Short enough for two lines of the poll's card.
const UNAVAILABLE: &str = "Votes can't be counted just now; try again soon";

/// `/api/v1/poll`.
pub async fn handle(mut req: Request, env: &Env) -> Result<Response> {
    if req.method() != Method::Post {
        return error(405, "Votes take POST", None);
    }
    // Browsers send Origin with every POST: another site can't vote for
    // the reader, from their address.
    let origin = req.url()?.origin().ascii_serialization();
    if req.headers().get("origin")?.as_deref() != Some(origin.as_str()) {
        return error(403, "Votes must come from Allthing itself", None);
    }
    // Without the secret, voter keys couldn't be kept apart from addresses.
    let (Some(secret), Some(index)) = (session_secret(env), D1Index::new(env)) else {
        return error(503, NOT_SET_UP, None);
    };
    let Some(vote) = read_vote(&mut req).await else { return error(400, USAGE, None) };
    if vote.poll != poll::MOBILE_CLIENT {
        return error(400, "There's no such poll", None);
    }
    let address = req.headers().get("cf-connecting-ip")?.unwrap_or_default();
    let Some(network) = poll::network(&address) else {
        return error(400, "Where this vote comes from can't be told", None);
    };
    match index.vote(&vote.poll, &voter(secret.as_bytes(), &vote.poll, &network), vote.choice).await {
        Ok((counted, counts)) => json(&Answer { counted, already: !counted, counts }, &[]),
        Err(failure) => {
            console_error!("{POLL_PATH}: {}", failure.0);
            error(503, UNAVAILABLE, Some(30))
        }
    }
}

/// Whether votes are counted here: the secret to key voters with, and D1.
pub fn ready(env: &Env) -> bool {
    session_secret(env).is_some() && D1Index::new(env).is_some()
}

/// The voter key a network's vote in `poll` is kept under: the HMAC-SHA256
/// of [`VOTER_PURPOSE`], a newline, then `<poll>|<network>`, keyed with
/// `secret`, in hex. Without the secret, the keys can't be matched to
/// addresses, nor a network's votes in two polls to each other.
pub(crate) fn voter(secret: &[u8], poll: &str, network: &str) -> String {
    hex(&mac(secret, VOTER_PURPOSE, &poll::voter_value(poll, network)).finalize().into_bytes())
}

/// The request's vote, if it is small and well formed: an unknown choice
/// isn't.
async fn read_vote(req: &mut Request) -> Option<Vote> {
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
    use allthing_core::poll::{MOBILE_CLIENT, network};

    use super::*;

    const SECRET: &[u8] = b"a test secret that is long enough to sign with";

    #[test]
    fn a_voter_key_is_the_secrets_hmac_of_the_poll_and_the_network() {
        let v4 = network("203.0.113.7").unwrap();
        let key = voter(SECRET, MOBILE_CLIENT, &v4);
        // HMAC-SHA256 of "allthing poll v1\nmobile-client-v1|203.0.113.7", as
        // Python's hmac module computes it.
        assert_eq!(key, "f9a79ef7fc7a99831d198c99cb5f333c38d0f9469013061b2b88a211f47aee6d");
        let v6 = network("2001:db8:85a3:8d3::1").unwrap();
        assert_eq!(
            voter(SECRET, MOBILE_CLIENT, &v6),
            "6bccb71a07ab06d91f63f50a0bd4ea5793c10adde62b74ec297772269af9b4b6"
        );
        // The same each time; another for another poll, network or secret.
        assert_eq!(voter(SECRET, MOBILE_CLIENT, &v4), key);
        assert_ne!(voter(SECRET, "another-poll-v1", &v4), key);
        assert_ne!(voter(SECRET, MOBILE_CLIENT, "203.0.113.8"), key);
        assert_ne!(voter(b"another secret that is long enough to sign with", MOBILE_CLIENT, &v4), key);
        // 64 lowercase hex digits.
        assert!(key.len() == 64 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
}
