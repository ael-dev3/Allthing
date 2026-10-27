// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's page payloads, built from Hypersnap and keyless public RPCs.
//!
//! The same builders run in two places: in the Cloudflare Worker behind its
//! edge cache (`allthing-worker`), and in the browser for the static GitHub
//! Pages build (`allthing-web` with the `static-host` feature). The runtime
//! supplies a [`Host`]: the clock, HTTP with a timeout, and background work.

pub mod activity;
pub mod annotate;
pub mod chain;
pub mod hypersnap;
pub mod indexer;
pub mod miniapps;
pub mod pages;
pub mod scheduled;
pub mod upstream;

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

use allthing_core::api::{
    API_PREFIX, ApiError, CachePolicy, ENGAGEMENT_USAGE, EngagementRequest, FollowsDir, ReactionKind, follows_path,
    home_page_path,
};
use allthing_core::follows::Paging;
use allthing_core::params::{
    channel_key_from_decoded, hash_from_param, percent_decode, percent_encode, username_from_param,
};
use allthing_core::ranking::TrendingWindow;
use futures::future::LocalBoxFuture;

use crate::indexer::Index;
use crate::upstream::UpstreamError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    /// Headers beyond `Accept` and `Content-Type`.
    pub headers: Vec<(&'static str, String)>,
    /// A JSON body, for POST.
    pub body: Option<String>,
    /// Bytes to POST as `application/octet-stream` instead: a signed
    /// Farcaster message, for a node's `submitMessage`.
    pub octets: Option<Vec<u8>>,
    /// Read the body of any answer, not only of a 2xx one: a node says in
    /// its 400 why it refused a message.
    pub any_body: bool,
    pub timeout_ms: u32,
    /// The `Accept` header.
    pub accept: &'static str,
    /// Read at most this many bytes of the body (asked for with `Range`, and
    /// cut if the server sends more).
    pub max_bytes: Option<u32>,
    /// With `max_bytes`: stop reading as soon as this says the body read so
    /// far is enough (a page's `<head>` has ended, say). It is given the body
    /// and how much of it it was shown before.
    pub stop_at: Option<fn(&[u8], usize) -> bool>,
}

pub struct HttpResponse {
    pub status: u16,
    /// The `Retry-After` header, when the runtime can read it.
    pub retry_after: Option<String>,
    /// The body of a 2xx answer (of any answer, with `any_body`).
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
    /// Allthing's index of notable casts, where the runtime has one.
    fn index(&self) -> Option<Rc<dyn Index>> {
        None
    }
    /// A keyed Solana RPC endpoint, where the runtime has one: full history
    /// for wallet counts. The public endpoints can't serve Workers.
    fn solana_rpc(&self) -> Option<String> {
        None
    }
}

/// One request's view of the runtime: its host and its subrequest budget.
pub struct Invocation {
    host: Rc<dyn Host>,
    budget: Cell<u32>,
    link_checks: Cell<usize>,
}

impl Invocation {
    pub fn new(host: Rc<dyn Host>, budget: u32) -> Invocation {
        Invocation { host, budget: Cell::new(budget), link_checks: Cell::new(miniapps::MAX_CHECKS) }
    }

    /// The keyed Solana RPC endpoint, if the runtime has one.
    pub fn solana_rpc(&self) -> Option<String> {
        self.host.solana_rpc()
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

    /// All link-checking phases share one quota, including quoted casts.
    pub(crate) fn take_link_checks(&self, wanted: usize) -> usize {
        let taken = wanted.min(self.link_checks.get());
        self.link_checks.set(self.link_checks.get() - taken);
        taken
    }

    pub fn index(&self) -> Option<Rc<dyn Index>> {
        self.host.index()
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
    /// When its data was read (Unix milliseconds), for a stored copy built
    /// earlier; `None` means now.
    pub fetched_at: Option<f64>,
    /// A validated stale shared Day Home copy. The edge may answer it now
    /// and start one normal refresh, retaining its original source age.
    pub revalidate_home: bool,
}

impl Built {
    pub fn json<T: serde::Serialize>(value: &T, policy: CachePolicy) -> Result<Built, Failure> {
        serde_json::to_vec(value)
            .map(|body| Built { body, status: 200, policy, fetched_at: None, revalidate_home: false })
            .map_err(|e| Failure::internal(&e.to_string()))
    }

    /// A cacheable "no such thing" answer, so bad links don't reach upstream.
    pub fn not_found(message: &str) -> Built {
        let body = serde_json::to_vec(&ApiError { error: message.to_owned(), retry_after: None }).unwrap_or_default();
        Built { body, status: 404, policy: allthing_core::api::MISS_CACHE, fetched_at: None, revalidate_home: false }
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
    /// Trending casts over a window, from `offset` (0 for the front page).
    Home(u32, TrendingWindow),
    /// The newest casts, from the event stream.
    Latest,
    Channel(String),
    Cast(String),
    User(String),
    Wallets(u64),
    Channels(Option<String>),
    Search(String),
    /// One page of an account's retained active likes or recasts.
    Library(u64, ReactionKind, Option<String>),
    /// Public cast hydration for an ordered private Library page.
    LibraryCasts(Vec<String>),
    Reactions(String, ReactionKind, u32),
    /// An account's followers or following, by name (`fid:<n>` for later
    /// pages), from the start or from a cursor.
    Follows(String, FollowsDir, Option<String>),
    /// Who liked and recasted a batch of feed cards, and their replies, each
    /// as its own preview (`Reactions` with a limit of 10, `Replies`)
    /// answers it.
    Engagement(EngagementRequest),
    Replies(String),
    /// The likes, recasts and replies of a few casts in view, sorted.
    Counts(Vec<String>),
    Vote(u64),
    /// Farcaster's daily active accounts, as the cron last counted them.
    Activity,
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
            "/home" => {
                let window = match query("window") {
                    None => TrendingWindow::Day,
                    Some(w) => {
                        TrendingWindow::parse(&w).ok_or(bad("Expected ?window=1h|6h|12h|24h|7d|30d|365d|all"))?
                    }
                };
                match query("offset").map(|o| o.parse::<u32>()) {
                    None => Ok(Endpoint::Home(0, window)),
                    Some(Ok(offset)) if offset % 100 == 0 && offset < 1000 => Ok(Endpoint::Home(offset, window)),
                    Some(_) => Err(bad("Expected ?offset=<multiple of 100 below 1000>")),
                }
            }
            "/latest" => Ok(Endpoint::Latest),
            "/activity" => Ok(Endpoint::Activity),
            "/library" => {
                let fid = query("fid")
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|fid| *fid > 0)
                    .ok_or(bad("Expected ?fid=<positive number>"))?;
                let kind =
                    query("kind").and_then(|v| ReactionKind::parse(&v)).ok_or(bad("Expected ?kind=likes|recasts"))?;
                let cursor = match query("cursor") {
                    None => None,
                    Some(cursor) => Some(
                        allthing_core::api::library_cursor(&cursor)
                            .ok_or(bad("Expected ?cursor=<the previous Library page's next>"))?,
                    ),
                };
                Ok(Endpoint::Library(fid, kind, cursor))
            }
            "/library/casts" => allthing_core::api::parse_library_hashes(query("hashes").as_deref())
                .map(Endpoint::LibraryCasts)
                .ok_or(bad("Expected ?hashes=<up to 25 comma-separated cast hashes>")),
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
            "/engagement" => EngagementRequest::parse(
                query("likes").as_deref(),
                query("recasts").as_deref(),
                query("replies").as_deref(),
            )
            .map(Endpoint::Engagement)
            .ok_or(bad(ENGAGEMENT_USAGE)),
            "/counts" => allthing_core::api::parse_counts(query("casts").as_deref())
                .map(Endpoint::Counts)
                .ok_or(bad("Expected ?casts=<up to 12 comma-separated cast hashes>")),
            "/follows" => {
                let name =
                    query("name").and_then(|v| username_from_param(&v)).ok_or(bad("Expected ?name=<username>"))?;
                let dir =
                    query("dir").and_then(|v| FollowsDir::parse(&v)).ok_or(bad("Expected ?dir=followers|following"))?;
                let cursor = match query("cursor") {
                    None => None,
                    Some(cursor) if Paging::decode(&cursor).is_some() => Some(cursor),
                    Some(_) => return Err(bad("Expected ?cursor=<the previous page's next>")),
                };
                Ok(Endpoint::Follows(name, dir, cursor))
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
                .and_then(|v| channel_key_from_decoded(&v))
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
                    return Err(bad(allthing_core::api::SEARCH_TOO_LONG));
                }
                Ok(Endpoint::Channels(q))
            }
            "/search" => {
                let q = query("q")
                    .map(|q| q.trim().to_owned())
                    .filter(|q| !q.is_empty())
                    .ok_or(bad("Expected ?q=<search>"))?;
                if q.chars().count() > 100 {
                    return Err(bad(allthing_core::api::SEARCH_TOO_LONG));
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
                // Match URL query parsing in the Worker: '+' is a space,
                // while an encoded '%2B' remains a literal plus sign.
                Some((percent_decode(&key.replace('+', " "))?, percent_decode(&value.replace('+', " "))?))
            })
            .collect();
        Endpoint::parse(name, |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
    }

    /// The name this payload is cached under, such as `cast?hash=0x…`.
    pub fn cache_name(&self) -> String {
        match self {
            Endpoint::Home(offset, window) => {
                home_page_path(*offset, *window).trim_start_matches(API_PREFIX).trim_start_matches('/').to_owned()
            }
            Endpoint::Latest => "latest".into(),
            Endpoint::Activity => "activity".into(),
            Endpoint::Library(fid, kind, cursor) => allthing_core::api::library_path(*fid, *kind, cursor.as_deref())
                .trim_start_matches(API_PREFIX)
                .trim_start_matches('/')
                .to_owned(),
            Endpoint::LibraryCasts(hashes) => allthing_core::api::library_casts_path(hashes)
                .trim_start_matches(API_PREFIX)
                .trim_start_matches('/')
                .to_owned(),
            Endpoint::Reactions(hash, kind, limit) => {
                format!("reactions?hash={hash}&type={}&limit={limit}", kind.as_str())
            }
            // Canonical: the same lists, in any order or spelling, share one entry.
            Endpoint::Engagement(request) => {
                request.path().trim_start_matches(API_PREFIX).trim_start_matches('/').to_owned()
            }
            Endpoint::Replies(hash) => format!("replies?hash={hash}"),
            Endpoint::Counts(hashes) => format!("counts?casts={}", hashes.join(",")),
            Endpoint::Follows(name, dir, cursor) => follows_path(name, *dir, cursor.as_deref())
                .trim_start_matches(API_PREFIX)
                .trim_start_matches('/')
                .to_owned(),
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
            Endpoint::Home(offset, window) => pages::home(inv, offset, window, previous).await,
            Endpoint::Latest => pages::latest(inv, previous).await,
            Endpoint::Activity => pages::activity(inv).await,
            Endpoint::Library(fid, kind, cursor) => pages::library(inv, fid, kind, cursor, previous).await,
            Endpoint::LibraryCasts(hashes) => pages::library_casts(inv, hashes, previous).await,
            Endpoint::Reactions(hash, kind, limit) => pages::reactions(inv, hash, kind, limit).await,
            Endpoint::Engagement(request) => pages::engagement(inv, request).await,
            Endpoint::Replies(hash) => pages::replies(inv, hash).await,
            Endpoint::Counts(hashes) => pages::counts(inv, hashes).await,
            Endpoint::Follows(name, dir, cursor) => pages::follows(inv, name, dir, cursor).await,
            Endpoint::Vote(fid) => pages::vote(inv, fid).await,
            Endpoint::Channel(id) => pages::channel(inv, id, previous).await,
            Endpoint::Cast(hash) => pages::cast(inv, hash, previous).await,
            Endpoint::User(name) => pages::user(inv, name, previous).await,
            Endpoint::Wallets(fid) => pages::wallets(inv, fid, previous).await,
            Endpoint::Channels(q) => pages::channels(inv, q).await,
            Endpoint::Search(q) => pages::search(inv, q).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_routes_scope_and_escape_pages_and_keep_hydration_order() {
        let page = Endpoint::from_path("/api/v1/library?fid=3&kind=recasts&cursor=%2B%2F8%3D").unwrap();
        assert_eq!(page, Endpoint::Library(3, ReactionKind::Recasts, Some("+/8=".into())));
        assert_eq!(page.cache_name(), "library?fid=3&kind=recasts&cursor=%2B%2F8%3D");
        let (a, b) = (format!("0x{:040x}", 1), format!("0x{:040x}", 2));
        let batch = Endpoint::from_path(&format!("/api/v1/library/casts?hashes={b},{},{b}", a.to_uppercase())).unwrap();
        assert_eq!(batch, Endpoint::LibraryCasts(vec![b.clone(), a.clone()]));
        assert_eq!(batch.cache_name(), format!("library/casts?hashes={b},{a}"));
        for path in [
            "/api/v1/library",
            "/api/v1/library?fid=0&kind=likes",
            "/api/v1/library?fid=3&kind=other",
            "/api/v1/library?fid=3&kind=likes&cursor=bad!",
            "/api/v1/library/casts?hashes=0x12",
        ] {
            assert_eq!(Endpoint::from_path(path).map_err(|error| error.status), Err(400));
        }
    }

    #[test]
    fn counts_share_one_entry_whatever_the_order() {
        let (a, b) = (format!("0x{:040x}", 1), format!("0x{:040x}", 2));
        let endpoint = Endpoint::from_path(&format!("/api/v1/counts?casts={b},{a}")).unwrap();
        assert_eq!(endpoint, Endpoint::Counts(vec![a.clone(), b.clone()]));
        assert_eq!(endpoint.cache_name(), format!("counts?casts={a},{b}"));
        assert_eq!(Endpoint::from_path("/api/v1/counts").map_err(|e| e.status), Err(400));
        assert_eq!(Endpoint::from_path("/api/v1/counts?casts=0x12").map_err(|e| e.status), Err(400));
    }

    #[test]
    fn parses_api_paths() {
        assert_eq!(Endpoint::from_path("/api/v1/home"), Ok(Endpoint::Home(0, TrendingWindow::Day)));
        assert_eq!(Endpoint::from_path("/api/v1/home?offset=200"), Ok(Endpoint::Home(200, TrendingWindow::Day)));
        assert_eq!(Endpoint::from_path("/api/v1/home?offset=150").map_err(|e| e.status), Err(400));
        assert_eq!(
            Endpoint::from_path("/api/v1/home?window=7d&offset=100"),
            Ok(Endpoint::Home(100, TrendingWindow::Week))
        );
        assert_eq!(Endpoint::from_path("/api/v1/home?window=30d"), Ok(Endpoint::Home(0, TrendingWindow::Month)));
        assert_eq!(Endpoint::from_path("/api/v1/home?window=6h"), Ok(Endpoint::Home(0, TrendingWindow::SixHours)));
        assert_eq!(
            Endpoint::Home(0, TrendingWindow::TwelveHours).cache_name(),
            "home?window=12h",
            "a 12-hour copy is its own cache entry"
        );
        assert!(Endpoint::from_path("/api/v1/home?window=90d").is_err());
        assert_eq!(
            Endpoint::from_path("/api/v1/home?window=all&offset=900"),
            Ok(Endpoint::Home(900, TrendingWindow::All))
        );
        assert_eq!(Endpoint::Home(100, TrendingWindow::Year).cache_name(), "home?window=365d&offset=100");
        assert_eq!(Endpoint::from_path("/api/v1/home?window=90d").map_err(|e| e.status), Err(400));
        // An explicit default window is cached with the plain front page.
        assert_eq!(Endpoint::from_path("/api/v1/home?window=24h").unwrap().cache_name(), "home");
        assert_eq!(Endpoint::Home(200, TrendingWindow::Day).cache_name(), "home?offset=200");
        assert_eq!(Endpoint::Home(0, TrendingWindow::Hour).cache_name(), "home?window=1h");
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
        assert_eq!(Endpoint::from_path("/api/v1/activity"), Ok(Endpoint::Activity));
        assert_eq!(Endpoint::Activity.cache_name(), "activity");
        assert_eq!(Endpoint::from_path("/api/v1/nope").map_err(|e| e.status), Err(404));
        assert_eq!(Endpoint::from_path("/api/v2/home").map_err(|e| e.status), Err(404));
    }

    #[test]
    fn parses_engagement_batches_into_one_cache_name_per_set() {
        let (a, b) = (format!("0x{}", "a".repeat(40)), format!("0x{}", "b".repeat(40)));
        let request = EngagementRequest::parse(Some(&format!("{a},{b}")), Some(&b), Some(&a)).unwrap();
        let endpoint = Endpoint::Engagement(request.clone());
        // Any order and spelling of the same lists is one cache entry.
        let messy = format!(
            "/api/v1/engagement?replies={}&recasts={}&likes={b},{}",
            a.to_uppercase().replace("0X", "0x"),
            b.to_uppercase().replace("0X", "0x"),
            &a[2..]
        );
        assert_eq!(Endpoint::from_path(&messy), Ok(endpoint.clone()));
        assert_eq!(Endpoint::from_path(&request.path()), Ok(endpoint.clone()));
        assert_eq!(endpoint.cache_name(), format!("engagement?likes={a},{b}&recasts={b}&replies={a}"));
        // Commas may arrive escaped, and query values as the Worker decodes them.
        assert_eq!(
            Endpoint::from_path(&format!("/api/v1/engagement?likes={a}%2C{b}&recasts={b}&replies={a}")),
            Ok(endpoint.clone())
        );
        let worker = |key: &str| match key {
            "likes" => Some(format!("{b}, {a}")),
            "recasts" => Some(b.clone()),
            "replies" => Some(format!(" {a}")),
            _ => None,
        };
        assert_eq!(Endpoint::parse("/engagement", worker), Ok(endpoint));
        // A batch without replies keeps the cache name it had before them.
        let lists = EngagementRequest::parse(Some(&format!("{a},{b}")), Some(&b), None).unwrap();
        assert_eq!(Endpoint::Engagement(lists).cache_name(), format!("engagement?likes={a},{b}&recasts={b}"));
        // A batch holds a run of cards' lists and replies; one more of
        // either is too many.
        let most = allthing_core::api::ENGAGEMENT_MAX_LISTS;
        let replies = allthing_core::api::ENGAGEMENT_MAX_REPLIES;
        let hashes: Vec<String> = (0..=most).map(|i| format!("0x{i:040x}")).collect();
        let half = most / 2;
        let full = format!(
            "/api/v1/engagement?likes={}&recasts={}&replies={}",
            hashes[..half].join(","),
            hashes[half..most].join(","),
            hashes[..replies].join(",")
        );
        assert!(matches!(Endpoint::from_path(&full), Ok(Endpoint::Engagement(r)) if r.len() == most + replies));
        for bad in [
            "/api/v1/engagement".to_owned(),
            "/api/v1/engagement?likes=".to_owned(),
            "/api/v1/engagement?likes=0x12".to_owned(),
            "/api/v1/engagement?replies=0x12".to_owned(),
            format!("/api/v1/engagement?likes={}&recasts={}", hashes[..=half].join(","), hashes[half + 1..].join(",")),
            format!("/api/v1/engagement?replies={}", hashes[..=replies].join(",")),
        ] {
            let refused = Endpoint::from_path(&bad).map_err(|e| (e.status, e.message));
            assert_eq!(refused, Err((400, ENGAGEMENT_USAGE)), "{bad}");
        }
    }

    #[test]
    fn parses_follows_pages() {
        let cursor = Paging { places: vec![None], waiting: vec![Vec::new()] }.encode();
        assert_eq!(
            Endpoint::from_path("/api/v1/follows?name=Alice&dir=followers"),
            Ok(Endpoint::Follows("alice".into(), FollowsDir::Followers, None))
        );
        let later = format!("/api/v1/follows?name=fid%3A42&dir=following&cursor={cursor}");
        assert_eq!(
            Endpoint::from_path(&later),
            Ok(Endpoint::Follows("fid:42".into(), FollowsDir::Following, Some(cursor.clone())))
        );
        // The cache name is the path the app asks for.
        let first = Endpoint::Follows("vitalik.eth".into(), FollowsDir::Followers, None);
        assert_eq!(first.cache_name(), "follows?name=vitalik.eth&dir=followers");
        assert_eq!(
            Endpoint::Follows("fid:42".into(), FollowsDir::Following, Some(cursor.clone())).cache_name(),
            format!("follows?name=fid%3A42&dir=following&cursor={cursor}")
        );
        assert_eq!(
            Endpoint::from_path(&allthing_core::api::follows_path("fid:42", FollowsDir::Following, Some(&cursor))),
            Ok(Endpoint::Follows("fid:42".into(), FollowsDir::Following, Some(cursor)))
        );
        for bad in [
            "/api/v1/follows?dir=followers",
            "/api/v1/follows?name=alice",
            "/api/v1/follows?name=alice&dir=fans",
            "/api/v1/follows?name=alice&dir=followers&cursor=",
            "/api/v1/follows?name=alice&dir=followers&cursor=not%20a%20cursor",
            "/api/v1/follows?name=alice&dir=followers&cursor=AAAA",
        ] {
            assert_eq!(Endpoint::from_path(bad).map_err(|e| e.status), Err(400), "{bad}");
        }
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

    #[test]
    fn channel_paths_round_trip_parent_url_escapes() {
        for parent in [
            "https://example.com/a%2Fb",
            "https://example.com/100%25?tag=x%26y",
            "https://example.com/%252F",
            "https://example.com/🌴?q=a+b",
        ] {
            let endpoint = Endpoint::Channel(parent.into());
            assert_eq!(Endpoint::from_path(&allthing_core::api::channel_path(parent)), Ok(endpoint.clone()));
            assert_eq!(Endpoint::parse("/channel", |key| (key == "id").then(|| parent.into())), Ok(endpoint));
        }
    }

    #[test]
    fn static_query_parsing_matches_form_encoding() {
        assert_eq!(Endpoint::from_path("/api/v1/search?q=art+club"), Ok(Endpoint::Search("art club".into())));
        assert_eq!(Endpoint::from_path("/api/v1/search?q=C%2B%2B+%2520"), Ok(Endpoint::Search("C++ %20".into())));
        assert_eq!(
            Endpoint::from_path("/api/v1/channels?%71=Art+Club"),
            Ok(Endpoint::Channels(Some("art club".into())))
        );
    }
}
