// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's page payloads, built from Hypersnap and keyless public RPCs.
//!
//! The same builders run in two places: in the Cloudflare Worker behind its
//! edge cache (`allthing-worker`), and in the browser for the static GitHub
//! Pages build (`allthing-web` with the `static-host` feature). The runtime
//! supplies a [`Host`]: the clock, HTTP with a timeout, and background work.

pub mod annotate;
pub mod chain;
pub mod hypersnap;
pub mod miniapps;
pub mod pages;
pub mod upstream;

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

use allthing_core::api::{API_PREFIX, ApiError, CachePolicy, ReactionKind};
use allthing_core::params::{
    channel_key_from_param, hash_from_param, percent_decode, percent_encode, username_from_param,
};
use futures::future::LocalBoxFuture;

use crate::upstream::UpstreamError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    /// A JSON body, for POST.
    pub body: Option<String>,
    pub timeout_ms: u32,
    /// The `Accept` header.
    pub accept: &'static str,
    /// Read at most this many bytes of the body (asked for with `Range`, and
    /// cut if the server sends more).
    pub max_bytes: Option<u32>,
}

pub struct HttpResponse {
    pub status: u16,
    /// The `Retry-After` header, when the runtime can read it.
    pub retry_after: Option<String>,
    /// The body of a 2xx answer.
    pub body: Option<Vec<u8>>,
}

#[derive(Debug)]
pub enum FetchError {
    Timeout,
    Network(String),
}

/// What the runtime provides to the builders.
pub trait Host {
    /// Unix milliseconds.
    fn now(&self) -> f64;
    /// Send a request, giving up after its timeout.
    fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>>;
    /// Run `future` in the background. In a Worker this keeps the invocation
    /// alive until it finishes (`waitUntil`).
    fn spawn(&self, future: LocalBoxFuture<'static, ()>);
}

/// One request's view of the runtime: its host and its subrequest budget.
pub struct Invocation {
    host: Rc<dyn Host>,
    budget: Cell<u32>,
}

impl Invocation {
    pub fn new(host: Rc<dyn Host>, budget: u32) -> Invocation {
        Invocation { host, budget: Cell::new(budget) }
    }

    /// Unix milliseconds.
    pub fn now(&self) -> f64 {
        self.host.now()
    }

    pub fn take_subrequests(&self, n: u32) -> bool {
        let left = self.budget.get();
        if left >= n {
            self.budget.set(left - n);
            true
        } else {
            false
        }
    }

    pub fn remaining_subrequests(&self) -> u32 {
        self.budget.get()
    }

    pub fn wait_until(&self, future: impl Future<Output = ()> + 'static) {
        self.host.spawn(Box::pin(future));
    }

    fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
        self.host.fetch(request)
    }
}

/// A payload ready to serve or cache: JSON body, logical HTTP status and policy.
pub struct Built {
    pub body: Vec<u8>,
    pub status: u16,
    pub policy: CachePolicy,
}

impl Built {
    pub fn json<T: serde::Serialize>(value: &T, policy: CachePolicy) -> Result<Built, Failure> {
        serde_json::to_vec(value)
            .map(|body| Built { body, status: 200, policy })
            .map_err(|e| Failure::internal(&e.to_string()))
    }

    /// A cacheable "no such thing" answer, so bad links don't reach upstream.
    pub fn not_found(message: &str) -> Built {
        let body = serde_json::to_vec(&ApiError { error: message.to_owned(), retry_after: None }).unwrap_or_default();
        Built { body, status: 404, policy: allthing_core::api::MISS_CACHE }
    }
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub status: u16,
    pub message: String,
    pub retry_after: Option<u32>,
}

impl Failure {
    pub fn upstream(error: UpstreamError) -> Failure {
        match error {
            UpstreamError::Busy(ms) => Failure {
                status: 503,
                message: "Farcaster data source is busy; backing off".into(),
                retry_after: Some((ms / 1000.0).ceil().max(1.0) as u32),
            },
            UpstreamError::Budget => Failure { status: 503, message: error.to_string(), retry_after: Some(5) },
            other => Failure { status: 502, message: other.to_string(), retry_after: Some(10) },
        }
    }

    pub fn internal(message: &str) -> Failure {
        Failure { status: 500, message: message.to_owned(), retry_after: None }
    }
}

/// An API endpoint and its parameters, parsed from `/api/v1/<name>?…`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// Trending casts from `offset` (0 for the front page).
    Home(u32),
    Channel(String),
    Cast(String),
    User(String),
    Wallets(u64),
    Channels(Option<String>),
    Search(String),
    Reactions(String, ReactionKind, u32),
    Replies(String),
    Vote(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadRequest {
    pub status: u16,
    pub message: &'static str,
}

impl Endpoint {
    /// `name` is the path after `/api/v1` (`/cast`); `query` returns a
    /// decoded query parameter.
    pub fn parse(name: &str, query: impl Fn(&str) -> Option<String>) -> Result<Endpoint, BadRequest> {
        let bad = |message| BadRequest { status: 400, message };
        match name {
            "/home" => match query("offset").map(|o| o.parse::<u32>()) {
                None => Ok(Endpoint::Home(0)),
                Some(Ok(offset)) if offset % 100 == 0 && offset < 1000 => Ok(Endpoint::Home(offset)),
                Some(_) => Err(bad("Expected ?offset=<multiple of 100 below 1000>")),
            },
            "/reactions" => {
                let hash = query("hash").and_then(|v| hash_from_param(&v)).ok_or(bad("Expected ?hash=<cast hash>"))?;
                let kind =
                    query("type").and_then(|v| ReactionKind::parse(&v)).ok_or(bad("Expected ?type=likes|recasts"))?;
                let limit = match query("limit").as_deref() {
                    None | Some("10") => 10,
                    Some("100") => 100,
                    Some(_) => return Err(bad("Expected ?limit=10|100")),
                };
                Ok(Endpoint::Reactions(hash, kind, limit))
            }
            "/vote" => query("fid")
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|f| *f > 0)
                .map(Endpoint::Vote)
                .ok_or(bad("Expected ?fid=<number>")),
            "/replies" => query("hash")
                .and_then(|v| hash_from_param(&v))
                .map(Endpoint::Replies)
                .ok_or(bad("Expected ?hash=<cast hash>")),
            "/channel" => query("id")
                .and_then(|v| channel_key_from_param(&v))
                .map(Endpoint::Channel)
                .ok_or(bad("Expected ?id=<channel id or parent URL>")),
            "/cast" => query("hash")
                .and_then(|v| hash_from_param(&v))
                .map(Endpoint::Cast)
                .ok_or(bad("Expected ?hash=<40 hex characters>")),
            "/user" => query("name")
                .and_then(|v| username_from_param(&v))
                .map(Endpoint::User)
                .ok_or(bad("Expected ?name=<username>")),
            "/wallets" => query("fid")
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|f| *f > 0)
                .map(Endpoint::Wallets)
                .ok_or(bad("Expected ?fid=<number>")),
            "/channels" => {
                let q = query("q").map(|q| q.trim().to_lowercase()).filter(|q| !q.is_empty());
                if q.as_ref().is_some_and(|q| q.chars().count() > 64) {
                    return Err(bad("Search query too long"));
                }
                Ok(Endpoint::Channels(q))
            }
            "/search" => {
                let q = query("q")
                    .map(|q| q.trim().to_owned())
                    .filter(|q| !q.is_empty())
                    .ok_or(bad("Expected ?q=<search>"))?;
                if q.chars().count() > 100 {
                    return Err(bad("Search query too long"));
                }
                Ok(Endpoint::Search(q))
            }
            _ => Err(BadRequest { status: 404, message: "Unknown endpoint" }),
        }
    }

    /// Parse a full API path such as `/api/v1/cast?hash=0x…`.
    pub fn from_path(path: &str) -> Result<Endpoint, BadRequest> {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let name = path.strip_prefix(API_PREFIX).ok_or(BadRequest { status: 404, message: "Unknown API version" })?;
        let pairs: Vec<(String, String)> = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .filter_map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                Some((percent_decode(key)?, percent_decode(value)?))
            })
            .collect();
        Endpoint::parse(name, |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
    }

    /// The name this payload is cached under, such as `cast?hash=0x…`.
    pub fn cache_name(&self) -> String {
        match self {
            Endpoint::Home(0) => "home".into(),
            Endpoint::Home(offset) => format!("home?offset={offset}"),
            Endpoint::Reactions(hash, kind, limit) => {
                format!("reactions?hash={hash}&type={}&limit={limit}", kind.as_str())
            }
            Endpoint::Replies(hash) => format!("replies?hash={hash}"),
            Endpoint::Vote(fid) => format!("vote?fid={fid}"),
            Endpoint::Channel(id) => format!("channel?id={}", percent_encode(id)),
            Endpoint::Cast(hash) => format!("cast?hash={hash}"),
            Endpoint::User(name) => format!("user?name={}", percent_encode(name)),
            Endpoint::Wallets(fid) => format!("wallets?fid={fid}"),
            Endpoint::Channels(Some(q)) => format!("channels?q={}", percent_encode(q)),
            Endpoint::Channels(None) => "channels".into(),
            Endpoint::Search(q) => format!("search?q={}", percent_encode(&q.to_lowercase())),
        }
    }

    /// Build the payload. `previous` is the last body for the same endpoint,
    /// whose slowly gathered annotations are carried forward.
    pub async fn build(&self, inv: Rc<Invocation>, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
        match self.clone() {
            Endpoint::Home(offset) => pages::home(inv, offset, previous).await,
            Endpoint::Reactions(hash, kind, limit) => pages::reactions(inv, hash, kind, limit).await,
            Endpoint::Replies(hash) => pages::replies(inv, hash).await,
            Endpoint::Vote(fid) => pages::vote(inv, fid).await,
            Endpoint::Channel(id) => pages::channel(inv, id, previous).await,
            Endpoint::Cast(hash) => pages::cast(inv, hash, previous).await,
            Endpoint::User(name) => pages::user(inv, name, previous).await,
            Endpoint::Wallets(fid) => pages::wallets(inv, fid).await,
            Endpoint::Channels(q) => pages::channels(inv, q).await,
            Endpoint::Search(q) => pages::search(inv, q).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_paths() {
        assert_eq!(Endpoint::from_path("/api/v1/home"), Ok(Endpoint::Home(0)));
        assert_eq!(Endpoint::from_path("/api/v1/home?offset=200"), Ok(Endpoint::Home(200)));
        assert_eq!(Endpoint::from_path("/api/v1/home?offset=150").map_err(|e| e.status), Err(400));
        assert_eq!(
            Endpoint::from_path(
                "/api/v1/reactions?hash=0x3b6c0db881a351984ab7844cd56265d492431d79&type=recasts&limit=100"
            ),
            Ok(Endpoint::Reactions("0x3b6c0db881a351984ab7844cd56265d492431d79".into(), ReactionKind::Recasts, 100))
        );
        assert_eq!(Endpoint::from_path("/api/v1/search?q=hyperion"), Ok(Endpoint::Search("hyperion".into())));
        assert_eq!(
            Endpoint::from_path("/api/v1/cast?hash=0x3b6c0db881a351984ab7844cd56265d492431d79"),
            Ok(Endpoint::Cast("0x3b6c0db881a351984ab7844cd56265d492431d79".into()))
        );
        assert_eq!(Endpoint::from_path("/api/v1/user?name=balajis.eth"), Ok(Endpoint::User("balajis.eth".into())));
        assert_eq!(
            Endpoint::from_path("/api/v1/channels?q=Art%20Club"),
            Ok(Endpoint::Channels(Some("art club".into())))
        );
        assert_eq!(Endpoint::from_path("/api/v1/channels?q=%20"), Ok(Endpoint::Channels(None)));
        assert_eq!(Endpoint::from_path("/api/v1/wallets?fid=0").map_err(|e| e.status), Err(400));
        assert_eq!(Endpoint::from_path("/api/v1/nope").map_err(|e| e.status), Err(404));
        assert_eq!(Endpoint::from_path("/api/v2/home").map_err(|e| e.status), Err(404));
    }

    #[test]
    fn cache_names_are_stable() {
        assert_eq!(
            Endpoint::Channel("https://example.com/x".into()).cache_name(),
            "channel?id=https%3A%2F%2Fexample.com%2Fx"
        );
        assert_eq!(Endpoint::Channels(None).cache_name(), "channels");
        assert_eq!(Endpoint::Wallets(3).cache_name(), "wallets?fid=3");
    }
}
