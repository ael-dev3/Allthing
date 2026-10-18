// SPDX-License-Identifier: AGPL-3.0-or-later
//! One payload per page, assembled from upstream reads.

use std::collections::HashSet;
use std::rc::Rc;

use allthing_core::api::{
    self, CastPayload, ChannelPayload, ChannelsPayload, EngagementPayload, EngagementRequest, HomePayload, Meta,
    QuotedCast, ReactionKind, ReactionPreview, ReactionsPayload, RepliesPayload, SearchPayload, UserPayload,
};
use allthing_core::channels::{channel_url_for_id, is_channel_id};
use allthing_core::model::{Cast, Channel, Embed};
use allthing_core::normalize;
use allthing_core::params::fid_username;
use allthing_core::provenance::Backend;
use allthing_core::ranking::TrendingWindow;
use allthing_core::thread::{
    flatten, mark_mini_apps, mark_mini_apps_in_tree, prune_hidden, reply_preview, without_hidden,
};
use allthing_core::vote::{AppVote, VotePayload};
use allthing_core::wallet::{self, ChainActivity, ChainKind, WalletActivity};
use futures::future::{join, join_all, join3};
use futures::stream::{self, StreamExt};

use crate::annotate::{self, Target, annotate, carried};
use crate::upstream::{self, Upstream, UpstreamError};
use crate::{Built, Failure, Invocation};
use crate::{chain, hypersnap, miniapps};

/// Casts each listing ranks over; hot and top are computed within this pool.
pub const POOL_SIZE: u32 = 100;
/// Authors whose cast origins may be resolved while building one payload.
const ORIGIN_AUTHORS: usize = 12;
/// Preloaded previews never take Hypersnap's per-isolate bucket (40 tokens)
/// below this, so page builds in the same isolate keep their share.
pub const PREVIEW_RESERVE: u32 = 16;
/// A Worker keeps at most six connections open at once; a queued fetch
/// would spend its timeout waiting.
const PARALLEL: usize = 6;

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

/// Who reacted to a cast, newest first: at most `limit` people, out of the
/// 100 that Hypersnap lists.
async fn reactors(
    inv: &Invocation,
    hash: &str,
    kind: ReactionKind,
    limit: u32,
) -> Result<ReactionPreview, UpstreamError> {
    let raw = hypersnap::reactions(inv, hash, kind.as_str(), 100).await?;
    let (users, more) = normalize::reactors(&raw, limit as usize);
    Ok(ReactionPreview { users, more })
}

/// Who liked or recasted a cast: a preview (`limit` 10) or the full list
/// (100, which is as many as Hypersnap returns).
pub async fn reactions(inv: Rc<Invocation>, hash: String, kind: ReactionKind, limit: u32) -> Result<Built, Failure> {
    let ReactionPreview { users, more } = reactors(&inv, &hash, kind, limit).await.map_err(Failure::upstream)?;
    Built::json(&ReactionsPayload { hash, kind, users, more, meta: meta(&inv) }, api::REACTIONS_CACHE)
}

/// The like and recast previews of a batch of feed cards, each list as its
/// own preview shows it ([`reactions`] with a limit of 10), read six at a
/// time. A list that can't be read is left out without failing the others,
/// and the batch is then kept only briefly. Lists never take Hypersnap's
/// bucket below [`PREVIEW_RESERVE`]: with less room, the batch reads what
/// fits (likes first) and leaves the rest to its rebuild.
pub async fn engagement(inv: Rc<Invocation>, request: EngagementRequest) -> Result<Built, Failure> {
    let lists: Vec<(&str, ReactionKind)> = request.lists().collect();
    // One subrequest stays for the cache write.
    let room = upstream::spare(&inv, Upstream::Hypersnap, PREVIEW_RESERVE)
        .min(inv.remaining_subrequests().saturating_sub(1)) as usize;
    let read: Vec<_> = stream::iter(lists.iter().take(room).map(|&(hash, kind)| {
        let inv = &inv;
        async move { (hash, kind, reactors(inv, hash, kind, api::REACTIONS_PREVIEW).await) }
    }))
    .buffered(PARALLEL)
    .collect()
    .await;
    let mut payload = EngagementPayload { meta: meta(&inv), ..Default::default() };
    let mut failure = None;
    for (hash, kind, result) in read {
        match result {
            Ok(preview) => payload.insert(hash.to_owned(), kind, preview),
            Err(error) => failure = failure.or(Some(error)),
        }
    }
    if payload.is_empty() {
        // Nothing read: the bucket was at its reserve, or every read failed.
        return Err(Failure::upstream(failure.unwrap_or(UpstreamError::Busy(1000.0))));
    }
    let policy = if payload.len() == lists.len() { api::ENGAGEMENT_CACHE } else { api::ENGAGEMENT_PARTIAL_CACHE };
    Built::json(&payload, policy)
}

/// Replies to show when hovering a cast's reply count: the ten most engaged
/// direct replies, without mini apps.
pub async fn replies(inv: Rc<Invocation>, hash: String) -> Result<Built, Failure> {
    let Some(root) = hypersnap::conversation(&inv, &hash, 1).await.map_err(Failure::upstream)? else {
        return Ok(Built::not_found("No such cast"));
    };
    let mut notes = api::Annotations::default();
    let replies = root.direct_replies.iter().flatten().map(normalize::cast).collect();
    let replies = reply_preview(drop_hidden(&inv, replies, &mut notes).await);
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

    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::{Endpoint, FetchError, Host, HttpRequest, HttpResponse};

    /// Hypersnap's `reactions/cast`: twelve people per list, a minute apart,
    /// oldest first; HTTP 500 for the lists in `failing`.
    struct Reactions {
        requests: RefCell<Vec<String>>,
        failing: Vec<(String, &'static str)>,
    }

    impl Reactions {
        fn new(failing: Vec<(String, &'static str)>) -> Rc<Reactions> {
            Rc::new(Reactions { requests: RefCell::new(vec![]), failing })
        }
    }

    impl Host for Reactions {
        fn now(&self) -> f64 {
            1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let path = request.url.trim_start_matches(hypersnap::BASE).to_owned();
            self.requests.borrow_mut().push(path.clone());
            let param = |key: &str| {
                path.split(['?', '&'])
                    .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
                    .unwrap_or_default()
                    .to_owned()
            };
            let (hash, kind) = (param("hash"), param("types"));
            let status = |status: u16| HttpResponse { status, retry_after: None, body: None };
            let response = if !path.starts_with("/v2/farcaster/reactions/cast?") || param("limit") != "100" {
                status(404)
            } else if self.failing.iter().any(|(h, k)| *h == hash && *k == kind) {
                status(500)
            } else {
                let people: Vec<String> = (1..=12)
                    .map(|i| {
                        format!(
                            r#"{{"reaction_timestamp":"2026-10-17T10:{i:02}:00.000Z","user":{{"fid":{i},"username":"{kind}-{i}","profile":{{"bio":{{"text":"skipped"}}}}}}}}"#
                        )
                    })
                    .collect();
                let body = format!(r#"{{"reactions":[{}]}}"#, people.join(","));
                HttpResponse { status: 200, retry_after: None, body: Some(body.into_bytes()) }
            };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn cards(n: u32) -> Vec<String> {
        (1..=n).map(|i| format!("0x{i:040x}")).collect()
    }

    /// Build a batch the way the Worker and the static copy route it.
    fn build(inv: &Rc<Invocation>, request: &EngagementRequest) -> Result<Built, Failure> {
        let endpoint = Endpoint::from_path(&request.path()).unwrap();
        assert_eq!(endpoint, Endpoint::Engagement(request.clone()));
        endpoint.build(inv.clone(), None).now_or_never().expect("the mock host answers at once")
    }

    fn payload(built: &Built) -> EngagementPayload {
        serde_json::from_slice(&built.body).unwrap()
    }

    #[test]
    fn a_full_batch_reads_each_list_once_within_the_subrequest_budget() {
        let host = Reactions::new(vec![]);
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let hashes = cards(8);
        let request = EngagementRequest::for_casts(hashes.iter().map(|h| (h.as_str(), 3, 1)));
        assert_eq!(request.len(), api::ENGAGEMENT_MAX_LISTS);
        // The Worker reads the edge cache first.
        assert!(inv.take_subrequests(1));
        let built = build(&inv, &request).unwrap();
        // One Hypersnap read per list: with the cache read and the cache
        // write, a full batch takes 18 of the 45 subrequests.
        assert_eq!(host.requests.borrow().len(), 16);
        assert_eq!(45 - inv.remaining_subrequests() + 1, 18);
        assert_eq!((built.status, built.policy), (200, api::ENGAGEMENT_CACHE));

        let batch = payload(&built);
        assert_eq!((batch.likes.len(), batch.recasts.len()), (8, 8));
        // Each preview is the single endpoint's: the ten newest, and more.
        let likes = batch.reactions(&hashes[0], ReactionKind::Likes).unwrap();
        let fids: Vec<u64> = likes.users.iter().map(|u| u.fid).collect();
        assert_eq!(fids, [12, 11, 10, 9, 8, 7, 6, 5, 4, 3]);
        assert!(likes.more);
        assert_eq!(likes.users[0].username, "likes-12");
        assert_eq!(batch.recasts[&hashes[7]].users[0].username, "recasts-12");
    }

    #[test]
    fn a_list_that_fails_leaves_the_others_and_the_batch_is_kept_briefly() {
        let hashes = cards(3);
        let host = Reactions::new(vec![(hashes[1].clone(), "likes")]);
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let request = EngagementRequest::for_casts(hashes.iter().map(|h| (h.as_str(), 1, 1)));
        let built = build(&inv, &request).unwrap();
        assert_eq!(host.requests.borrow().len(), 6);
        assert_eq!((built.status, built.policy), (200, api::ENGAGEMENT_PARTIAL_CACHE));
        let batch = payload(&built);
        assert!(batch.likes.contains_key(&hashes[0]) && batch.likes.contains_key(&hashes[2]));
        assert!(!batch.likes.contains_key(&hashes[1]));
        assert_eq!(batch.recasts.len(), 3);

        // Nothing read at all is an error, served stale or retried later.
        let host = Reactions::new(vec![(hashes[0].clone(), "likes")]);
        let inv = Rc::new(Invocation::new(host, 45));
        let only = EngagementRequest::for_casts([(hashes[0].as_str(), 1, 0)]);
        assert_eq!(build(&inv, &only).err().map(|f| f.status), Some(502));
    }

    #[test]
    fn previews_leave_hypersnaps_reserve_to_page_builds() {
        // The isolate's bucket holds 40 Hypersnap requests, and the mock's
        // clock stands still, so it does not refill.
        let host = Reactions::new(vec![]);
        let hashes = cards(24);
        let batch = |range: std::ops::Range<usize>| {
            EngagementRequest::for_casts(hashes[range].iter().map(|h| (h.as_str(), 1, 1)))
        };
        let first = build(&Rc::new(Invocation::new(host.clone(), 45)), &batch(0..8)).unwrap();
        assert_eq!((host.requests.borrow().len(), first.policy), (16, api::ENGAGEMENT_CACHE));

        // 24 left: 8 above the reserve. The likes come first.
        let second = build(&Rc::new(Invocation::new(host.clone(), 45)), &batch(8..16)).unwrap();
        assert_eq!((host.requests.borrow().len(), second.policy), (24, api::ENGAGEMENT_PARTIAL_CACHE));
        let partial = payload(&second);
        assert_eq!((partial.likes.len(), partial.recasts.len()), (8, 0));

        // At the reserve: refused without a request, as busy.
        let third = build(&Rc::new(Invocation::new(host.clone(), 45)), &batch(16..24));
        assert_eq!(third.err().map(|f| (f.status, f.retry_after)), Some((503, Some(1))));
        assert_eq!(host.requests.borrow().len(), 24);
        assert_eq!(upstream::spare(&Invocation::new(host, 45), Upstream::Hypersnap, 0), PREVIEW_RESERVE);
    }

    #[test]
    fn single_previews_list_the_newest_first() {
        let host = Reactions::new(vec![]);
        let inv = Rc::new(Invocation::new(host, 45));
        let hash = cards(1).remove(0);
        let built = reactions(inv, hash.clone(), ReactionKind::Recasts, 10).now_or_never().unwrap().unwrap();
        let single: ReactionsPayload = serde_json::from_slice(&built.body).unwrap();
        assert_eq!(single.users.len(), 10);
        assert_eq!(single.users[0].username, "recasts-12");
        assert!(single.more);
    }
}
