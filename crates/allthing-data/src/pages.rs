// SPDX-License-Identifier: AGPL-3.0-or-later
//! One payload per page, assembled from upstream reads.

use std::collections::HashSet;
use std::rc::Rc;

use allthing_core::api::{
    self, Annotations, CastPayload, ChannelPayload, ChannelsPayload, HomePayload, LatestPayload, Meta, QuotedCast,
    ReactionKind, ReactionsPayload, RepliesPayload, SearchPayload, UserPayload,
};
use allthing_core::channels::{channel_url_for_id, is_channel_id};
use allthing_core::latest::{self, Cursor};
use allthing_core::model::{Cast, Channel, Embed};
use allthing_core::normalize;
use allthing_core::params::fid_username;
use allthing_core::provenance::Backend;
use allthing_core::ranking::TrendingWindow;
use allthing_core::thread::{flatten, mark_mini_apps, mark_mini_apps_in_tree, prune_hidden, without_hidden};
use allthing_core::vote::{AppVote, VotePayload};
use allthing_core::wallet::{self, ChainActivity, ChainKind, WalletActivity};
use futures::future::{join, join_all, join3};

use crate::annotate::{self, Target, annotate, carried};
use crate::{Built, Failure, Invocation};
use crate::{chain, hypersnap, miniapps};

/// Casts each listing ranks over; hot and top are computed within this pool.
pub const POOL_SIZE: u32 = 100;
/// Authors whose cast origins may be resolved while building one payload.
const ORIGIN_AUTHORS: usize = 12;

fn meta(inv: &Invocation) -> Meta {
    Meta { backend: Backend::Hypersnap, fetched_at: inv.now() as i64, stale: false }
}

fn targets(casts: &[&Cast]) -> Vec<Target> {
    casts.iter().map(|c| (c.author.fid, c.hash.clone())).collect()
}

/// Trending channels worth showing: named, and not near-empty.
fn named_channels(raw: Vec<normalize::RawChannel>) -> Vec<Channel> {
    raw.iter()
        .map(normalize::channel)
        .filter(|c| is_channel_id(&c.id) && c.member_count.is_none_or(|m| m >= 10))
        .collect()
}

/// Top up a short trending list with well-known channels.
fn with_defaults(mut channels: Vec<Channel>, limit: usize) -> Vec<Channel> {
    for id in allthing_core::channels::DEFAULT_CHANNELS {
        if channels.len() >= limit {
            break;
        }
        if !channels.iter().any(|c| c.id == *id) {
            channels.push(Channel {
                id: (*id).to_owned(),
                name: (*id).to_owned(),
                parent_url: channel_url_for_id(id),
                description: None,
                image_url: None,
                member_count: None,
                created_at: None,
            });
        }
    }
    channels.truncate(limit);
    channels
}

/// Check the casts' links for mini app pages (within the build's budget),
/// then leave out every cast that is a mini app or spam.
async fn drop_hidden(inv: &Invocation, mut casts: Vec<Cast>, notes: &mut api::Annotations) -> Vec<Cast> {
    notes.links = miniapps::check(inv, &casts, &notes.links).await;
    mark_mini_apps(&mut casts, &notes.links);
    without_hidden(casts)
}

fn newest_unique(mut casts: Vec<Cast>, limit: usize) -> Vec<Cast> {
    casts.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| a.hash.cmp(&b.hash)));
    let mut seen = HashSet::new();
    casts.retain(|c| seen.insert(c.hash.clone()));
    casts.truncate(limit);
    casts
}

thread_local! {
    /// Trending channels change slowly; keep them for ten minutes per isolate.
    static TRENDING_CHANNELS: std::cell::RefCell<Option<(f64, Vec<Channel>)>> = const { std::cell::RefCell::new(None) };
}
const TRENDING_CHANNELS_TTL_MS: f64 = 600_000.0;

async fn trending_channels(inv: &Invocation) -> Vec<Channel> {
    let now = inv.now();
    if let Some(channels) = TRENDING_CHANNELS
        .with(|t| t.borrow().as_ref().filter(|(at, _)| now - at < TRENDING_CHANNELS_TTL_MS).map(|(_, c)| c.clone()))
    {
        return channels;
    }
    match hypersnap::trending_channels(inv, 100).await {
        Ok(raw) => {
            let channels = named_channels(raw);
            TRENDING_CHANNELS.with(|t| *t.borrow_mut() = Some((now, channels.clone())));
            channels
        }
        Err(_) => Vec::new(),
    }
}

/// Trending casts. `offset` 0 is the front page; later offsets are the
/// next pages that infinite scroll loads.
pub async fn home(
    inv: Rc<Invocation>,
    offset: u32,
    window: TrendingWindow,
    previous: Option<Rc<Vec<u8>>>,
) -> Result<Built, Failure> {
    let casts = hypersnap::trending_casts(&inv, POOL_SIZE, offset, window).await;
    let casts = casts.map_err(Failure::upstream)?.iter().map(normalize::cast).collect();
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let pool = drop_hidden(&inv, casts, &mut notes).await;
    let refs: Vec<&Cast> = pool.iter().collect();
    annotate(&inv, &mut notes, &targets(&refs), ORIGIN_AUTHORS).await;
    Built::json(&HomePayload { pool, notes, meta: meta(&inv) }, api::HOME_CACHE)
}

/// Events read per shard in one build: about 14 minutes of Farcaster, most
/// of them block confirmations.
const EVENTS_PAGE: u32 = 1000;
/// A buffer older than this starts over from the newest events: continuing
/// from its cursors would leave a gap one page can't cover.
const WARM_MS: f64 = 600_000.0;

/// The newest casts on Farcaster, from Snapchain's event stream. Each build
/// continues from the previous one's cursors, so it reads only the events
/// since then: it adds the new top-level casts, read in one bulk call that
/// also refreshes the listed casts' counts, and drops removed ones. New casts
/// are never skipped: if they can't be read now, the build fails, the
/// previous copy is served, and the next build reads the same events again.
pub async fn latest(inv: Rc<Invocation>, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    let warm = previous
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<LatestPayload>(bytes).ok())
        .filter(|p| inv.now() - (p.meta.fetched_at as f64) < WARM_MS);
    let (listed, cursors, mut notes) = match warm {
        Some(p) => (p.pool, p.cursors, p.notes),
        None => (Vec::new(), Vec::new(), Annotations::default()),
    };
    let cursor = |shard: u32| cursors.iter().find(|c| c.shard == shard).map(|c| c.event_id);
    let pages = join_all(latest::SHARDS.map(|shard| hypersnap::events(&inv, shard, cursor(shard), EVENTS_PAGE))).await;
    let (mut added, mut removed, mut next) = (Vec::new(), Vec::new(), Vec::new());
    let mut failure = None;
    for (shard, page) in latest::SHARDS.into_iter().zip(pages) {
        match page.map(|body| latest::cast_events(&body)) {
            Ok(Some(events)) => {
                if let Some(event_id) = events.last_id.or(cursor(shard)) {
                    next.push(Cursor { shard, event_id });
                }
                added.extend(events.added);
                removed.extend(events.removed);
            }
            // Unreadable: this shard continues from where it was next time.
            other => {
                failure = other.err().or(failure);
                next.extend(cursor(shard).map(|event_id| Cursor { shard, event_id }));
            }
        }
    }
    if next.is_empty() {
        return Err(failure.map(Failure::upstream).unwrap_or_else(|| Failure::internal("unreadable event stream")));
    }
    let hashes = latest::to_hydrate(&added, &listed, &removed, 100);
    let fresh: Vec<Cast> = match hypersnap::casts_bulk(&inv, &hashes).await {
        Ok(raw) => raw.iter().map(normalize::cast).filter(|c| c.parent_hash.is_none()).collect(),
        Err(e) if !added.is_empty() => return Err(Failure::upstream(e)),
        Err(_) => Vec::new(),
    };
    let fresh = drop_hidden(&inv, fresh, &mut notes).await;
    let pool = latest::newest(listed, fresh, &removed, POOL_SIZE as usize);
    let refs: Vec<&Cast> = pool.iter().collect();
    annotate(&inv, &mut notes, &targets(&refs), ORIGIN_AUTHORS).await;
    Built::json(&LatestPayload { pool, notes, cursors: next, meta: meta(&inv) }, api::LATEST_CACHE)
}

pub async fn channel(inv: Rc<Invocation>, key: String, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    // The feed URL is almost always predictable from the key, so fetch it
    // alongside the channel lookup instead of after it.
    let guess = if is_channel_id(&key) { channel_url_for_id(&key) } else { key.clone() };
    let (channel, guessed) =
        join(hypersnap::channel(&inv, &key), hypersnap::channel_casts(&inv, &guess, POOL_SIZE / 2)).await;
    let Some(channel) = channel.map_err(Failure::upstream)? else {
        return Ok(Built::not_found("No such channel"));
    };
    let raw = match guessed {
        Ok(casts) if channel.parent_url == guess => casts,
        _ => hypersnap::channel_casts(&inv, &channel.parent_url, POOL_SIZE / 2).await.map_err(Failure::upstream)?,
    };
    // Hypersnap merges several shards per page, so order isn't guaranteed.
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let casts = raw.iter().map(normalize::cast).collect();
    let pool = newest_unique(drop_hidden(&inv, casts, &mut notes).await, POOL_SIZE as usize);
    let refs: Vec<&Cast> = pool.iter().collect();
    annotate(&inv, &mut notes, &targets(&refs), ORIGIN_AUTHORS).await;
    Built::json(&ChannelPayload { channel, pool, notes, meta: meta(&inv) }, api::CHANNEL_CACHE)
}

pub async fn cast(inv: Rc<Invocation>, hash: String, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    let Some(root) = hypersnap::conversation(&inv, &hash, 5).await.map_err(Failure::upstream)? else {
        return Ok(Built::not_found("No such cast"));
    };
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let mut thread = normalize::thread(&root);
    notes.links = miniapps::check(&inv, flatten(&thread), &notes.links).await;
    mark_mini_apps_in_tree(&mut thread, &notes.links);
    thread.children = std::mem::take(&mut thread.children).into_iter().filter_map(prune_hidden).collect();
    if thread.cast.hidden() {
        thread.cast.strip_hidden();
    }
    let first = &thread.cast;
    let quote_hashes: Vec<String> = first
        .embeds
        .iter()
        .filter_map(|e| match e {
            Embed::Cast { hash, .. } => Some(hash.clone()),
            _ => None,
        })
        .take(3)
        .collect();

    // Context around the cast is best-effort; the thread is what matters.
    let channel_key = first.channel.as_ref().map(|c| c.id.clone());
    let parent_hash = first.parent_hash.clone();
    let (channel, parent, quotes) = join3(
        async {
            match &channel_key {
                Some(key) => hypersnap::channel(&inv, key).await.ok().flatten(),
                None => None,
            }
        },
        async {
            match &parent_hash {
                Some(hash) => hypersnap::cast(&inv, hash).await.ok().flatten().map(|c| normalize::cast(&c)),
                None => None,
            }
        },
        join_all(quote_hashes.iter().map(|hash| {
            let inv = &inv;
            async move {
                QuotedCast {
                    hash: hash.clone(),
                    cast: hypersnap::cast(inv, hash).await.ok().flatten().map(|c| normalize::cast(&c)),
                }
            }
        })),
    )
    .await;

    // Attribute the cast itself first, then its context, then the replies.
    let mut refs: Vec<&Cast> = vec![first];
    refs.extend(parent.iter());
    refs.extend(quotes.iter().filter_map(|q| q.cast.as_ref()));
    refs.extend(flatten(&thread).into_iter().skip(1));
    annotate(&inv, &mut notes, &targets(&refs), ORIGIN_AUTHORS).await;
    Built::json(&CastPayload { thread, parent, quotes, channel, notes, meta: meta(&inv) }, api::CAST_CACHE)
}

pub async fn user(inv: Rc<Invocation>, username: String, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    let raw = match fid_username(&username) {
        Some(fid) => hypersnap::users_bulk(&inv, &[fid]).await.map_err(Failure::upstream)?.into_iter().next(),
        None => hypersnap::user_by_username(&inv, &username).await.map_err(Failure::upstream)?,
    };
    let Some(raw) = raw else {
        return Ok(Built::not_found("No such user"));
    };
    let user = normalize::user(&raw);
    let fid = user.fid;

    // The casts, the account's signers and its casts' signers in parallel:
    // two requests attribute every cast on the page.
    let (casts, signers, recent) = join3(
        hypersnap::user_casts(&inv, fid, POOL_SIZE),
        hypersnap::active_signers(&inv, fid),
        hypersnap::recent_cast_signers(&inv, fid, POOL_SIZE),
    )
    .await;
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let casts = casts.map_err(Failure::upstream)?.iter().map(normalize::cast).collect();
    let pool = newest_unique(drop_hidden(&inv, casts, &mut notes).await, POOL_SIZE as usize);
    let active = signers.ok().map(|apps| annotate::remember_signers(&inv, fid, apps));
    if let Ok(recent) = recent {
        let cast_signers: Vec<(String, String)> =
            pool.iter().filter_map(|c| recent.get(&c.hash).map(|s| (c.hash.clone(), s.clone()))).collect();
        for (hash, origin) in annotate::attribute(&inv, fid, &cast_signers, active).await {
            notes.origins.insert(hash, origin);
        }
    }
    let refs: Vec<&Cast> = pool.iter().collect();
    annotate(&inv, &mut notes, &targets(&refs), 2).await;
    Built::json(&UserPayload { user, pool, notes, meta: meta(&inv) }, api::USER_CACHE)
}

pub async fn channels(inv: Rc<Invocation>, query: Option<String>) -> Result<Built, Failure> {
    let channels = match &query {
        Some(q) => hypersnap::search_channels(&inv, q, 50)
            .await
            .map_err(Failure::upstream)?
            .iter()
            .map(normalize::channel)
            .take(50)
            .collect(),
        None => with_defaults(trending_channels(&inv).await, 50),
    };
    Built::json(&ChannelsPayload { channels, meta: meta(&inv) }, api::CHANNELS_CACHE)
}

/// Casts, people and channels matching `query`, searched in parallel. A
/// failing part leaves its group empty rather than failing the page.
pub async fn search(inv: Rc<Invocation>, query: String) -> Result<Built, Failure> {
    let terms = hypersnap::search_terms(&query);
    let (casts, users, channels) = join3(
        hypersnap::search_casts(&inv, &terms, 25),
        hypersnap::search_users(&inv, &terms, 10),
        hypersnap::search_channels(&inv, &terms, 10),
    )
    .await;
    if let (Err(e), Err(_), Err(_)) = (&casts, &users, &channels) {
        return Err(Failure::upstream(e.clone()));
    }
    // Search hits come without embeds or counts: fetch the full casts.
    let hashes: Vec<String> =
        casts.unwrap_or_default().iter().map(|c| allthing_core::params::normalize_hash(&c.hash)).collect();
    let hydrated = hypersnap::casts_bulk(&inv, &hashes).await.unwrap_or_default();
    let mut notes = api::Annotations::default();
    let casts = drop_hidden(&inv, hydrated.iter().map(normalize::cast).collect(), &mut notes).await;
    let users: Vec<_> = users.unwrap_or_default().iter().map(normalize::user).collect();
    let channels: Vec<Channel> = channels.unwrap_or_default().iter().map(normalize::channel).take(10).collect();
    Built::json(&SearchPayload { query, casts, users, channels, notes, meta: meta(&inv) }, api::SEARCH_CACHE)
}

/// Who liked or recasted a cast: a preview (`limit` 10) or the full list
/// (100, which is as many as Hypersnap returns).
pub async fn reactions(inv: Rc<Invocation>, hash: String, kind: ReactionKind, limit: u32) -> Result<Built, Failure> {
    let raw = hypersnap::reactions(&inv, &hash, kind.as_str(), 100).await.map_err(Failure::upstream)?;
    let total = raw.len();
    // Hypersnap lists the oldest first; show the newest.
    let users: Vec<_> = raw.iter().rev().take(limit as usize).map(normalize::user_summary).collect();
    let more = total > users.len();
    Built::json(&ReactionsPayload { hash, kind, users, more, meta: meta(&inv) }, api::REACTIONS_CACHE)
}

/// Replies to show when hovering a cast's reply count: the ten most engaged
/// direct replies, without mini apps.
pub async fn replies(inv: Rc<Invocation>, hash: String) -> Result<Built, Failure> {
    let Some(root) = hypersnap::conversation(&inv, &hash, 1).await.map_err(Failure::upstream)? else {
        return Ok(Built::not_found("No such cast"));
    };
    let mut notes = api::Annotations::default();
    let replies = root.direct_replies.iter().flatten().map(normalize::cast).collect();
    let mut replies = drop_hidden(&inv, replies, &mut notes).await;
    replies.sort_by_key(|c| std::cmp::Reverse(allthing_core::ranking::points(c)));
    replies.truncate(10);
    Built::json(&RepliesPayload { hash, replies, notes, meta: meta(&inv) }, api::REPLIES_CACHE)
}

/// How an FID voted on staying with farcaster.xyz: Base first, the app for
/// what Base cannot know. Either part may be missing.
pub async fn vote(inv: Rc<Invocation>, fid: u64) -> Result<Built, Failure> {
    let (onchain, app) = join(chain::vote(&inv, fid), hypersnap::app_vote(&inv, fid)).await;
    if let (Err(e), Err(_)) = (&onchain, &app) {
        return Err(Failure::upstream(e.clone()));
    }
    let payload = VotePayload {
        fid,
        onchain: onchain.ok(),
        app: app.ok().flatten().as_deref().and_then(AppVote::parse),
        meta: meta(&inv),
    };
    Built::json(&payload, api::VOTE_CACHE)
}

/// At most this many addresses of each kind are measured per account.
const MAX_EVM_ADDRESSES: usize = 5;
const MAX_SOLANA_ADDRESSES: usize = 2;

pub async fn wallets(inv: Rc<Invocation>, fid: u64) -> Result<Built, Failure> {
    let Some(raw) = hypersnap::users_bulk(&inv, &[fid]).await.map_err(Failure::upstream)?.into_iter().next() else {
        return Ok(Built::not_found("No such user"));
    };
    let user = normalize::user(&raw);
    let addresses = wallet::addresses(
        &user.eth_addresses,
        &user.sol_addresses,
        user.primary_eth.as_deref(),
        user.primary_sol.as_deref(),
    );
    let evm: Vec<String> = addresses
        .iter()
        .filter(|a| a.kind == ChainKind::Evm)
        .take(MAX_EVM_ADDRESSES)
        .map(|a| a.address.clone())
        .collect();
    let solana: Vec<String> = addresses
        .iter()
        .filter(|a| a.kind == ChainKind::Solana)
        .take(MAX_SOLANA_ADDRESSES)
        .map(|a| a.address.clone())
        .collect();

    let measured = join_all(
        wallet::CHAINS
            .iter()
            .filter(|c| match c.kind {
                ChainKind::Evm => !evm.is_empty(),
                ChainKind::Solana => !solana.is_empty(),
            })
            .map(|chain| {
                let (inv, evm) = (&inv, &evm);
                async move {
                    let result = match chain.kind {
                        ChainKind::Evm => chain::evm_sent_count(inv, chain, evm).await.map_err(|e| e.to_string()),
                        ChainKind::Solana => Err(wallet::SOLANA_UNMEASURED.to_owned()),
                    };
                    ChainActivity {
                        chain: chain.slug.to_owned(),
                        name: chain.name.to_owned(),
                        kind: chain.kind,
                        count: *result.as_ref().unwrap_or(&0),
                        capped: false,
                        percent: 0,
                        error: result.err(),
                    }
                }
            }),
    )
    .await;
    let mut chains = measured;
    let total = wallet::apportion(&mut chains);
    let mut sources = Vec::new();
    if !evm.is_empty() {
        sources.push(
            "EVM chains: transactions sent by each verified address (its account nonce), read from keyless public RPCs \
             (mainnet.base.org, publicnode.com, the chains' own endpoints)."
                .to_owned(),
        );
    }
    if !solana.is_empty() {
        sources.push(format!("Solana: {}", wallet::SOLANA_UNMEASURED));
    }
    let activity = WalletActivity { fid, addresses, chains, total, sources, fetched_at: inv.now() as i64 };
    Built::json(&activity, api::WALLET_CACHE)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// A hub with an event page per (shard, first event), the casts it can
    /// read in bulk, and whether bulk reads fail.
    #[derive(Default)]
    struct Hub {
        events: HashMap<(String, String), String>,
        casts: HashMap<String, String>,
        bulk_fails: bool,
        requests: RefCell<Vec<String>>,
    }

    fn param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
        url.split(['?', '&']).find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
    }

    impl Host for Hub {
        fn now(&self) -> f64 {
            1_800_000_000_000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let url = request.url.clone();
            self.requests.borrow_mut().push(url.clone());
            let ok = |body: String| HttpResponse { status: 200, retry_after: None, body: Some(body.into_bytes()) };
            let missing = HttpResponse { status: 404, retry_after: None, body: None };
            let response = if url.contains("/v1/events?") {
                let shard = param(&url, "shard_index").unwrap_or_default().to_owned();
                let from = param(&url, "from_event_id").unwrap_or("newest").to_owned();
                self.events.get(&(shard, from)).cloned().map_or(missing, ok)
            } else if url.contains("/v2/farcaster/casts?") {
                if self.bulk_fails {
                    HttpResponse { status: 500, retry_after: None, body: None }
                } else {
                    let hashes = param(&url, "casts").unwrap_or_default().replace("%2C", ",");
                    let found: Vec<String> = hashes.split(',').filter_map(|h| self.casts.get(h).cloned()).collect();
                    ok(format!(r#"{{"casts":[{}]}}"#, found.join(",")))
                }
            } else {
                missing
            };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn add(id: u64, hash: &str, ts: i64, reply: bool) -> String {
        let parent = if reply { r#"{"fid":1,"hash":"0x01"}"# } else { "null" };
        format!(
            r#"{{"id":{id},"mergeMessageBody":{{"message":{{"hash":"{hash}","data":{{"type":"MESSAGE_TYPE_CAST_ADD","fid":7,"timestamp":{ts},"castAddBody":{{"parentCastId":{parent}}}}}}}}}}}"#
        )
    }

    fn remove(id: u64, target_base64: &str) -> String {
        format!(
            r#"{{"id":{id},"mergeMessageBody":{{"message":{{"hash":"0x09","data":{{"type":"MESSAGE_TYPE_CAST_REMOVE","fid":7,"timestamp":1,"castRemoveBody":{{"targetHash":"{target_base64}"}}}}}}}}}}"#
        )
    }

    fn page(events: &[String]) -> String {
        format!(r#"{{"events":[{}]}}"#, events.join(","))
    }

    fn raw_cast(hash: &str, ts: &str, likes: u32) -> String {
        format!(
            r#"{{"hash":"{hash}","author":{{"fid":7,"username":"seven"}},"text":"cast {hash}","timestamp":"{ts}","reactions":{{"likes_count":{likes},"recasts_count":0}},"replies":{{"count":0}}}}"#
        )
    }

    /// 20 bytes of 0xaa..., in base64 and hex.
    const GONE_B64: &str = "qqqqqqqqqqqqqqqqqqqqqqqqqqo=";
    const GONE: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn build(hub: Rc<Hub>, previous: Option<Vec<u8>>) -> Result<LatestPayload, Failure> {
        let inv = Rc::new(Invocation::new(hub, 45));
        let built = latest(inv, previous.map(Rc::new)).now_or_never().unwrap()?;
        Ok(serde_json::from_slice(&built.body).unwrap())
    }

    #[test]
    fn keeps_the_newest_top_level_casts_and_continues_from_its_cursors() {
        let mut hub = Hub::default();
        hub.events.insert(("1".into(), "newest".into()), page(&[add(12, GONE, 100, false), add(11, "0xb1", 90, true)]));
        hub.events.insert(("2".into(), "newest".into()), page(&[add(22, "0xc2", 95, false)]));
        hub.casts.insert(GONE.into(), raw_cast(GONE, "2026-01-01T00:01:40Z", 1));
        hub.casts.insert("0xc2".into(), raw_cast("0xc2", "2026-01-01T00:01:35Z", 2));
        let first = build(Rc::new(hub), None).unwrap();
        // Replies are left out; newest first.
        assert_eq!(first.pool.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), [GONE, "0xc2"]);
        assert_eq!(first.cursors, [Cursor { shard: 1, event_id: 12 }, Cursor { shard: 2, event_id: 22 }]);

        // Next build: only the events after the cursors. A new cast, and the
        // first one removed; the listed cast's counts are read again.
        let mut hub = Hub::default();
        hub.events.insert(("1".into(), "13".into()), page(&[add(13, "0xd3", 110, false), remove(14, GONE_B64)]));
        hub.events.insert(("2".into(), "23".into()), page(&[]));
        hub.casts.insert("0xd3".into(), raw_cast("0xd3", "2026-01-01T00:01:50Z", 0));
        hub.casts.insert("0xc2".into(), raw_cast("0xc2", "2026-01-01T00:01:35Z", 9));
        let hub = Rc::new(hub);
        let mut previous = first;
        previous.meta.fetched_at = 1_800_000_000_000 - 30_000;
        let second = build(hub.clone(), Some(serde_json::to_vec(&previous).unwrap())).unwrap();
        let listed: Vec<(&str, u64)> = second.pool.iter().map(|c| (c.hash.as_str(), c.likes)).collect();
        assert_eq!(listed, [("0xd3", 0), ("0xc2", 9)]);
        assert_eq!(second.cursors, [Cursor { shard: 1, event_id: 14 }, Cursor { shard: 2, event_id: 22 }]);
        let events: Vec<String> = hub.requests.borrow().iter().filter(|u| u.contains("/v1/events?")).cloned().collect();
        assert!(events.iter().all(|url| url.contains("from_event_id=") && !url.contains("reverse=true")));
    }

    #[test]
    fn new_casts_that_cant_be_read_are_not_skipped() {
        let mut hub = Hub { bulk_fails: true, ..Default::default() };
        hub.events.insert(("1".into(), "newest".into()), page(&[add(12, "0xe1", 100, false)]));
        hub.events.insert(("2".into(), "newest".into()), page(&[]));
        // The build fails, so the previous copy stays (with its cursors).
        assert!(build(Rc::new(hub), None).is_err());
    }
}
