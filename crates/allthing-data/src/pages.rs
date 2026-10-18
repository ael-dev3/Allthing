// SPDX-License-Identifier: AGPL-3.0-or-later
//! One payload per page, assembled from upstream reads.

use std::collections::HashSet;
use std::rc::Rc;

use allthing_core::api::{
    self, CastPayload, ChannelPayload, ChannelsPayload, HomePayload, Meta, QuotedCast, ReactionKind, ReactionsPayload,
    RepliesPayload, SearchPayload, UserPayload,
};
use allthing_core::channels::{channel_url_for_id, is_channel_id};
use allthing_core::model::{Cast, Channel, Embed};
use allthing_core::normalize;
use allthing_core::params::fid_username;
use allthing_core::provenance::Backend;
use allthing_core::thread::{flatten, mark_mini_apps, mark_mini_apps_in_tree, prune_mini_apps, without_mini_apps};
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

fn author_fids(casts: &[&Cast]) -> Vec<u64> {
    let mut seen = HashSet::new();
    casts.iter().map(|c| c.author.fid).filter(|fid| *fid > 0 && seen.insert(*fid)).collect()
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
/// then leave out every cast that is a mini app.
async fn drop_mini_apps(inv: &Invocation, mut casts: Vec<Cast>, notes: &mut api::Annotations) -> Vec<Cast> {
    notes.links = miniapps::check(inv, &casts, &notes.links).await;
    mark_mini_apps(&mut casts, &notes.links);
    without_mini_apps(casts)
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
/// next pages that infinite scroll loads, without the channel list.
pub async fn home(inv: Rc<Invocation>, offset: u32, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    let (casts, channels) = join(hypersnap::trending_casts(&inv, POOL_SIZE, offset), async {
        if offset == 0 { trending_channels(&inv).await } else { Vec::new() }
    })
    .await;
    let casts = casts.map_err(Failure::upstream)?.iter().map(normalize::cast).collect();
    let channels = if offset == 0 { with_defaults(channels, 12) } else { channels };
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let pool = drop_mini_apps(&inv, casts, &mut notes).await;
    let refs: Vec<&Cast> = pool.iter().collect();
    annotate(&inv, &mut notes, &author_fids(&refs), &targets(&refs), ORIGIN_AUTHORS).await;
    Built::json(&HomePayload { pool, channels, notes, meta: meta(&inv) }, api::HOME_CACHE)
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
    let pool = newest_unique(drop_mini_apps(&inv, casts, &mut notes).await, POOL_SIZE as usize);
    let refs: Vec<&Cast> = pool.iter().collect();
    annotate(&inv, &mut notes, &author_fids(&refs), &targets(&refs), ORIGIN_AUTHORS).await;
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
    thread.children = std::mem::take(&mut thread.children).into_iter().filter_map(prune_mini_apps).collect();
    if thread.cast.mini_app {
        thread.cast.strip_mini_app();
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
    annotate(&inv, &mut notes, &author_fids(&refs), &targets(&refs), ORIGIN_AUTHORS).await;
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
    let pool = newest_unique(drop_mini_apps(&inv, casts, &mut notes).await, POOL_SIZE as usize);
    if let Some(score) = raw.score_micros() {
        notes.scores.insert(fid, score);
    }
    let active = signers.ok().map(|apps| annotate::remember_signers(&inv, fid, apps));
    if let Ok(recent) = recent {
        let cast_signers: Vec<(String, String)> =
            pool.iter().filter_map(|c| recent.get(&c.hash).map(|s| (c.hash.clone(), s.clone()))).collect();
        for (hash, origin) in annotate::attribute(&inv, fid, &cast_signers, active).await {
            notes.origins.insert(hash, origin);
        }
    }
    let refs: Vec<&Cast> = pool.iter().collect();
    annotate(&inv, &mut notes, &[fid], &targets(&refs), 2).await;
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
    let casts = drop_mini_apps(&inv, hydrated.iter().map(normalize::cast).collect(), &mut notes).await;
    let users: Vec<_> = users.unwrap_or_default().iter().map(normalize::user).collect();
    let channels: Vec<Channel> = channels.unwrap_or_default().iter().map(normalize::channel).take(10).collect();
    let refs: Vec<&Cast> = casts.iter().collect();
    let mut fids = author_fids(&refs);
    fids.extend(users.iter().map(|u| u.fid));
    notes.scores = annotate::scores(&inv, &fids).await;
    Built::json(&SearchPayload { query, casts, users, channels, notes, meta: meta(&inv) }, api::SEARCH_CACHE)
}

/// Who liked or recasted a cast: a preview (`limit` 10) or the full list
/// (100, which is as many as Hypersnap returns).
pub async fn reactions(inv: Rc<Invocation>, hash: String, kind: ReactionKind, limit: u32) -> Result<Built, Failure> {
    let raw = hypersnap::reactions(&inv, &hash, kind.as_str(), 100).await.map_err(Failure::upstream)?;
    let total = raw.len();
    // Hypersnap lists the oldest first; show the newest.
    let users: Vec<_> = raw.iter().rev().take(limit as usize).map(normalize::user_summary).collect();
    let fids: Vec<u64> = users.iter().map(|u| u.fid).collect();
    let scores = annotate::scores(&inv, &fids).await;
    let more = total > users.len();
    Built::json(&ReactionsPayload { hash, kind, users, more, scores, meta: meta(&inv) }, api::REACTIONS_CACHE)
}

/// Replies to show when hovering a cast's reply count: the ten most engaged
/// direct replies, without mini apps.
pub async fn replies(inv: Rc<Invocation>, hash: String) -> Result<Built, Failure> {
    let Some(root) = hypersnap::conversation(&inv, &hash, 1).await.map_err(Failure::upstream)? else {
        return Ok(Built::not_found("No such cast"));
    };
    let mut notes = api::Annotations::default();
    let replies = root.direct_replies.iter().flatten().map(normalize::cast).collect();
    let mut replies = drop_mini_apps(&inv, replies, &mut notes).await;
    replies.sort_by_key(|c| std::cmp::Reverse(allthing_core::ranking::points(c)));
    replies.truncate(10);
    let refs: Vec<&Cast> = replies.iter().collect();
    notes.scores = annotate::scores(&inv, &author_fids(&refs)).await;
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
