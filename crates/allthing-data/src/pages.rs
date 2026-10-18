// SPDX-License-Identifier: AGPL-3.0-or-later
//! One payload per page, assembled from upstream reads.

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use allthing_core::api::{
    self, Annotations, CastPayload, ChannelPayload, ChannelsPayload, HomePayload, LatestPayload, Meta, QuotedCast,
    ReactionKind, ReactionsPayload, RepliesPayload, SearchPayload, UserPayload,
};
use allthing_core::channels::{channel_id_for_url, channel_url_for_id, is_channel_id};
use allthing_core::latest::{self, Cursor};
use allthing_core::model::{Cast, Channel};
use allthing_core::normalize::{self, RawAppChannel};
use allthing_core::params::fid_username;
use allthing_core::provenance::Backend;
use allthing_core::quotes;
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
/// Subrequests that reading a listing's quoted casts leaves to the rest of
/// the build: app names and the cache write.
const QUOTES_RESERVE: u32 = 2;

fn meta(inv: &Invocation) -> Meta {
    Meta { backend: Backend::Hypersnap, fetched_at: inv.now() as i64, stale: false }
}

fn targets(casts: &[&Cast]) -> Vec<Target> {
    casts.iter().map(|c| (c.author.fid, c.hash.clone())).collect()
}

/// Trending channels worth showing: named, and not near-empty (fewer than
/// ten accounts have cast there).
fn named_channels(raw: Vec<normalize::RawChannel>) -> Vec<Channel> {
    raw.iter().map(normalize::channel).filter(|c| is_channel_id(&c.id) && c.casters.is_none_or(|n| n >= 10)).collect()
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
                ..Channel::default()
            });
        }
    }
    channels.truncate(limit);
    channels
}

/// Check the casts' links for mini app pages (within the build's budget),
/// then leave out every cast that is a mini app or spam.
async fn drop_hidden(
    inv: &Invocation,
    mut casts: Vec<Cast>,
    notes: &mut api::Annotations,
    known_links: &BTreeMap<String, bool>,
) -> Vec<Cast> {
    notes.links = miniapps::check(inv, &casts, known_links).await;
    mark_mini_apps(&mut casts, &notes.links);
    without_hidden(casts)
}

/// The casts that `casts` quote, by hash, so their cards show them at once:
/// the `known` ones (carried from the previous payload) still quoted, and
/// the rest from one bulk read, made only while the build's budget keeps
/// [`QUOTES_RESERVE`]. A skipped or failed read leaves those quotes out, and
/// their cards link to them instead.
async fn quoted_casts(inv: &Invocation, casts: &[Cast], mut known: BTreeMap<String, Cast>) -> BTreeMap<String, Cast> {
    quotes::retain_quoted(&mut known, casts);
    let missing = quotes::missing(casts, &known, hypersnap::MAX_BULK_CASTS);
    if missing.is_empty() || inv.remaining_subrequests() <= QUOTES_RESERVE {
        return known;
    }
    if let Ok(found) = hypersnap::casts_bulk(inv, &missing).await {
        for cast in found.iter().map(normalize::cast).filter(|c| missing.contains(&c.hash)) {
            known.insert(cast.hash.clone(), quotes::as_quote(cast));
        }
    }
    known
}

/// Check a quote's own links before stripping hidden content. The outer
/// cast stays visible; its quote keeps the author and the hidden reason.
async fn check_quotes<'a>(
    inv: &Invocation,
    casts: impl IntoIterator<Item = &'a mut Cast>,
    known_links: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    let casts: Vec<&mut Cast> = casts.into_iter().collect();
    let links = miniapps::check(inv, casts.iter().map(|c| &**c), known_links).await;
    for cast in casts {
        mark_mini_apps(std::slice::from_mut(cast), &links);
        if cast.hidden() {
            cast.strip_hidden();
        }
    }
    links
}

/// Origins and app names for a listing's casts, with the casts they quote
/// read meanwhile. The quotes read starts first, so its one subrequest is
/// taken before origins size their batch to the budget that is left.
async fn annotate_listing(
    inv: &Invocation,
    notes: &mut api::Annotations,
    pool: &[Cast],
    max_authors: usize,
    known_links: &BTreeMap<String, bool>,
) {
    let refs: Vec<&Cast> = pool.iter().collect();
    let known = std::mem::take(&mut notes.quotes);
    let ((quotes, links), ()) = join(
        async {
            let mut quotes = quoted_casts(inv, pool, known).await;
            let links = check_quotes(inv, quotes.values_mut(), known_links).await;
            (quotes, links)
        },
        annotate(inv, notes, &targets(&refs), max_authors),
    )
    .await;
    notes.quotes = quotes;
    notes.links.extend(links);
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
    let known_links = std::mem::take(&mut notes.links);
    let pool = drop_hidden(&inv, casts, &mut notes, &known_links).await;
    annotate_listing(&inv, &mut notes, &pool, ORIGIN_AUTHORS, &known_links).await;
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
    let mut readable = 0;
    for (shard, page) in latest::SHARDS.into_iter().zip(pages) {
        match page.map(|body| latest::cast_events(&body)) {
            Ok(Some(events)) => {
                readable += 1;
                if let Some(event_id) = events.last_id.or(cursor(shard)) {
                    next.push(Cursor { shard, event_id });
                }
                added.extend(events.added);
                removed.extend(events.removed);
            }
            // Unreadable: this shard continues from where it was next time.
            other => {
                failure = Some(
                    other
                        .err()
                        .unwrap_or_else(|| crate::upstream::UpstreamError::Parse("unreadable event stream".into())),
                );
                next.extend(cursor(shard).map(|event_id| Cursor { shard, event_id }));
            }
        }
    }
    if readable == 0 {
        return Err(failure.map(Failure::upstream).unwrap_or_else(|| Failure::internal("unreadable event stream")));
    }
    let hashes = latest::to_hydrate(&added, &listed, &removed, 100);
    let fresh: Vec<Cast> = match hypersnap::casts_bulk(&inv, &hashes).await {
        Ok(raw) => raw.iter().map(normalize::cast).filter(|c| c.parent_hash.is_none()).collect(),
        Err(e) if !added.is_empty() => return Err(Failure::upstream(e)),
        Err(_) => Vec::new(),
    };
    let known_links = std::mem::take(&mut notes.links);
    let fresh = drop_hidden(&inv, fresh, &mut notes, &known_links).await;
    let pool = latest::newest(listed, fresh, &removed, POOL_SIZE as usize);
    annotate_listing(&inv, &mut notes, &pool, ORIGIN_AUTHORS, &known_links).await;
    Built::json(&LatestPayload { pool, notes, cursors: next, meta: meta(&inv) }, api::LATEST_CACHE)
}

/// The Farcaster app's record of a channel, when it has an id there and the
/// app can be read (it can't from a browser: no CORS).
async fn app_channel(inv: &Invocation, id: Option<&str>) -> Option<RawAppChannel> {
    hypersnap::app_channel(inv, id?).await.ok().flatten()
}

pub async fn channel(inv: Rc<Invocation>, key: String, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    // The feed URL is almost always predictable from the key, and so is the
    // id the Farcaster app knows the channel by: fetch the feed and the app's
    // record alongside the channel lookup instead of after it.
    let guess = if is_channel_id(&key) { channel_url_for_id(&key) } else { key.clone() };
    let app_id = if is_channel_id(&key) { Some(key.clone()) } else { channel_id_for_url(&key) };
    let (channel, guessed, app) = join3(
        hypersnap::channel(&inv, &key),
        hypersnap::channel_casts(&inv, &guess, POOL_SIZE / 2),
        app_channel(&inv, app_id.as_deref()),
    )
    .await;
    let Some(mut channel) = channel.map_err(Failure::upstream)? else {
        return Ok(Built::not_found("No such channel"));
    };
    // Then, alongside the feed when the guess was wrong, the accounts the
    // app's record names (one `user/bulk`) and its pinned cast. Both are
    // best-effort: what can't be read is left out of the page.
    let fids = app.as_ref().map(RawAppChannel::fids).unwrap_or_default();
    let pinned_hash = app.as_ref().and_then(RawAppChannel::pinned_hash);
    let (raw, users, pinned) = join3(
        async {
            match guessed {
                Ok(casts) if channel.parent_url == guess => Ok(casts),
                _ => hypersnap::channel_casts(&inv, &channel.parent_url, POOL_SIZE / 2).await,
            }
        },
        async { hypersnap::users_bulk(&inv, &fids).await.unwrap_or_default() },
        async {
            match &pinned_hash {
                Some(hash) => hypersnap::cast(&inv, hash).await.ok().flatten(),
                None => None,
            }
        },
    )
    .await;
    let raw = raw.map_err(Failure::upstream)?;
    if let Some(app) = &app {
        normalize::enrich_channel(&mut channel, app, &users);
    }
    // Hypersnap merges several shards per page, so order isn't guaranteed.
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let known_links = std::mem::take(&mut notes.links);
    let mut casts: Vec<Cast> = raw.iter().map(normalize::cast).collect();
    let mut pinned: Vec<Cast> = pinned.iter().map(normalize::cast).collect();
    // The pinned cast's links are checked first, with the feed's, and it is
    // left out like any listed cast if it is a mini app or spam.
    notes.links = miniapps::check(&inv, pinned.iter().chain(&casts), &known_links).await;
    mark_mini_apps(&mut pinned, &notes.links);
    mark_mini_apps(&mut casts, &notes.links);
    channel.pinned = without_hidden(pinned).pop();
    let pool = newest_unique(without_hidden(casts), POOL_SIZE as usize);
    let listed: Vec<Cast> = channel.pinned.iter().chain(&pool).cloned().collect();
    annotate_listing(&inv, &mut notes, &listed, ORIGIN_AUTHORS, &known_links).await;
    Built::json(&ChannelPayload { channel, pool, notes, meta: meta(&inv) }, api::CHANNEL_CACHE)
}

pub async fn cast(inv: Rc<Invocation>, hash: String, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    let Some(root) = hypersnap::conversation(&inv, &hash, 5).await.map_err(Failure::upstream)? else {
        return Ok(Built::not_found("No such cast"));
    };
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let known_links = std::mem::take(&mut notes.links);
    let mut thread = normalize::thread(&root);
    notes.links = miniapps::check(&inv, flatten(&thread), &known_links).await;
    mark_mini_apps_in_tree(&mut thread, &notes.links);
    thread.children = std::mem::take(&mut thread.children).into_iter().filter_map(prune_hidden).collect();
    if thread.cast.hidden() {
        thread.cast.strip_hidden();
    }
    let first = &thread.cast;
    let quote_hashes: Vec<String> = first.quoted().take(3).map(str::to_owned).collect();

    // Context around the cast is best-effort; the thread is what matters.
    let channel_key = first.channel.as_ref().map(|c| c.id.clone());
    let parent_hash = first.parent_hash.clone();
    let (channel, parent, mut quotes) = join3(
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
                    cast: hypersnap::cast(inv, hash)
                        .await
                        .ok()
                        .flatten()
                        .map(|c| quotes::as_quote(normalize::cast(&c))),
                }
            }
        })),
    )
    .await;
    notes.links.extend(check_quotes(&inv, quotes.iter_mut().filter_map(|q| q.cast.as_mut()), &known_links).await);

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
    let mut user = normalize::user(&raw);
    let fid = user.fid;

    // The casts, the account's signers and its casts' signers in parallel:
    // two requests attribute every cast on the page. Alongside, for an
    // account registered before the ID registry's migration (which reports
    // the migration's day), when it got its first username.
    let before_migration = user.registered_before_migration();
    let ((casts, signers, recent), first_username_at) = join(
        join3(
            hypersnap::user_casts(&inv, fid, POOL_SIZE),
            hypersnap::active_signers(&inv, fid),
            hypersnap::recent_cast_signers(&inv, fid, POOL_SIZE),
        ),
        async { if before_migration { Some(hypersnap::first_username_at(&inv, fid).await) } else { None } },
    )
    .await;
    // A failed read keeps what the previous copy knew, and this copy is
    // refreshed soon instead of in five minutes.
    let retry = matches!(first_username_at, Some(Err(_)));
    user.first_username_at = match first_username_at {
        Some(Ok(ts)) => ts,
        Some(Err(_)) => previous.as_deref().and_then(|p| api::first_username_at_in(p, fid)),
        None => None,
    };
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let known_links = std::mem::take(&mut notes.links);
    let casts = casts.map_err(Failure::upstream)?.iter().map(normalize::cast).collect();
    let pool = newest_unique(drop_hidden(&inv, casts, &mut notes, &known_links).await, POOL_SIZE as usize);
    let active = signers.ok().map(|apps| annotate::remember_signers(&inv, fid, apps));
    if let Ok(recent) = recent {
        let cast_signers: Vec<(String, String)> =
            pool.iter().filter_map(|c| recent.get(&c.hash).map(|s| (c.hash.clone(), s.clone()))).collect();
        for (hash, origin) in annotate::attribute(&inv, fid, &cast_signers, active).await {
            notes.origins.insert(hash, origin);
        }
    }
    annotate_listing(&inv, &mut notes, &pool, 2, &known_links).await;
    let policy = if retry { api::USER_RETRY_CACHE } else { api::USER_CACHE };
    Built::json(&UserPayload { user, pool, notes, meta: meta(&inv) }, policy)
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
    let casts = drop_hidden(&inv, hydrated.iter().map(normalize::cast).collect(), &mut notes, &BTreeMap::new()).await;
    let users: Vec<_> = users.unwrap_or_default().iter().map(normalize::user).collect();
    let channels: Vec<Channel> = channels.unwrap_or_default().iter().map(normalize::channel).take(10).collect();
    notes.quotes = quoted_casts(&inv, &casts, BTreeMap::new()).await;
    notes.links.extend(check_quotes(&inv, notes.quotes.values_mut(), &BTreeMap::new()).await);
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
    let mut replies = drop_hidden(&inv, replies, &mut notes, &BTreeMap::new()).await;
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

    fn previous_latest() -> Vec<u8> {
        serde_json::to_vec(&LatestPayload {
            pool: Vec::new(),
            notes: Annotations::default(),
            cursors: vec![Cursor { shard: 1, event_id: 12 }, Cursor { shard: 2, event_id: 22 }],
            meta: Meta { backend: Backend::Hypersnap, fetched_at: 1_800_000_000_000 - 30_000, stale: false },
        })
        .unwrap()
    }

    #[test]
    fn unavailable_shards_do_not_turn_a_cached_list_into_a_fresh_success() {
        let hub = Rc::new(Hub::default());
        assert!(build(hub.clone(), Some(previous_latest())).is_err());
        assert_eq!(hub.requests.borrow().iter().filter(|u| u.contains("/v1/events?")).count(), 2);
        assert!(!hub.requests.borrow().iter().any(|u| u.contains("/v2/farcaster/casts?")));
    }

    #[test]
    fn empty_readable_shards_are_a_valid_empty_list() {
        let mut hub = Hub::default();
        for shard in ["1", "2"] {
            hub.events.insert((shard.into(), "newest".into()), page(&[]));
        }
        let payload = build(Rc::new(hub), None).unwrap();
        assert!(payload.pool.is_empty() && payload.cursors.is_empty());
    }

    #[test]
    fn a_readable_shard_advances_while_the_failed_shard_keeps_its_cursor() {
        let mut hub = Hub::default();
        hub.events.insert(("1".into(), "13".into()), page(&[r#"{"id":13}"#.into()]));
        let payload = build(Rc::new(hub), Some(previous_latest())).unwrap();
        assert_eq!(payload.cursors, [Cursor { shard: 1, event_id: 13 }, Cursor { shard: 2, event_id: 22 }]);
    }
}

#[cfg(test)]
mod channel_tests {
    use std::cell::RefCell;

    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    const PINNED: &str = "0x50184b62b4c52fd76628713562e1eda3dccaf63d";

    /// Hypersnap and the Farcaster app as /c/farcaster finds them. Without
    /// `app`, the app can't be read, as from a browser (it sends no CORS
    /// headers). Everything else (signers, scores) is missing.
    struct Upstreams {
        app: bool,
        pinned_text: &'static str,
        requests: RefCell<Vec<String>>,
    }

    impl Upstreams {
        fn new(app: bool, pinned_text: &'static str) -> Rc<Upstreams> {
            Rc::new(Upstreams { app, pinned_text, requests: RefCell::new(Vec::new()) })
        }

        fn sent(&self, prefix: &str) -> usize {
            self.requests.borrow().iter().filter(|url| url.starts_with(prefix)).count()
        }
    }

    impl Host for Upstreams {
        fn now(&self) -> f64 {
            1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            self.requests.borrow_mut().push(request.url.clone());
            let json =
                |body: String| Ok(HttpResponse { status: 200, retry_after: None, body: Some(body.into_bytes()) });
            let v2 = |path: &str| request.url.starts_with(&format!("{}/v2/farcaster/{path}", hypersnap::BASE));
            let response = if request.url.starts_with("https://api.farcaster.xyz/v1/channel?channelId=farcaster") {
                if self.app {
                    json(include_str!("../../allthing-core/tests/fixtures/farcaster-channel-farcaster.json").into())
                } else {
                    Err(FetchError::Network("blocked by CORS".into()))
                }
            } else if v2("channel?") {
                json(include_str!("../../allthing-core/tests/fixtures/hypersnap-channel-legacy.json").into())
            } else if v2("feed/parent_urls?") {
                json(r#"{"casts":[{"hash":"0x01","text":"gm","timestamp":"2026-10-18T10:00:00Z","author":{"fid":2,"username":"v"}}]}"#.into())
            } else if v2("user/bulk?fids=1") {
                json(r#"{"users":[{"fid":1,"username":"farcaster","display_name":"Farcaster"}]}"#.into())
            } else if v2(&format!("cast?identifier={PINNED}")) {
                json(format!(
                    r#"{{"cast":{{"hash":"{}","text":"{}","timestamp":"2026-10-01T00:00:00Z","author":{{"fid":37,"username":"balajis.eth"}}}}}}"#,
                    &PINNED[2..],
                    self.pinned_text
                ))
            } else {
                Ok(HttpResponse { status: 404, retry_after: None, body: None })
            };
            async move { response }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn build(host: &Rc<Upstreams>) -> (ChannelPayload, u32) {
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let built = channel(inv.clone(), "farcaster".into(), None).now_or_never().unwrap().ok().unwrap();
        assert_eq!(built.status, 200);
        (serde_json::from_slice(&built.body).unwrap(), 45 - inv.remaining_subrequests())
    }

    #[test]
    fn channel_pages_add_the_apps_details_for_three_more_reads() {
        let host = Upstreams::new(true, "$2048 FOR FARCASTER CLIENTS");
        let (payload, spent) = build(&host);
        let channel = &payload.channel;
        assert_eq!((channel.name.as_str(), channel.id.as_str()), ("Farcaster", "farcaster"));
        assert_eq!(
            (channel.follower_count, channel.member_count, channel.casters),
            (Some(445_632), Some(1), Some(94_568))
        );
        assert!(channel.header_image_url.is_some() && channel.image_url.is_some());
        assert_eq!(channel.lead.as_ref().map(|u| u.username.as_str()), Some("farcaster"));
        assert!(channel.moderators.is_empty(), "the only moderator is the lead");
        assert_eq!(channel.pinned.as_ref().map(|c| c.hash.as_str()), Some(PINNED));
        assert_eq!(payload.pool.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), ["0x01"]);
        // One read of the app, then the accounts it names and its pinned cast.
        assert_eq!(host.sent("https://api.farcaster.xyz/"), 1);
        assert_eq!(host.sent(&format!("{}/v2/farcaster/user/bulk", hypersnap::BASE)), 1);
        assert_eq!(host.sent(&format!("{}/v2/farcaster/cast?", hypersnap::BASE)), 1);
        assert_eq!(spent as usize, host.requests.borrow().len());
    }

    #[test]
    fn without_the_app_a_channel_page_shows_hypersnaps_data_and_no_member_count() {
        let host = Upstreams::new(false, "unused");
        let (payload, _) = build(&host);
        let channel = &payload.channel;
        assert_eq!(channel.name, "farcaster");
        assert_eq!((channel.follower_count, channel.member_count, channel.casters), (None, None, Some(94_568)));
        assert_eq!(channel.counts(), [allthing_core::model::ChannelCount::Casters(94_568)]);
        assert!(channel.lead.is_none() && channel.pinned.is_none() && channel.header_image_url.is_none());
        assert_eq!(payload.pool.len(), 1);
        // Nothing else is read for it.
        assert_eq!(host.sent(&format!("{}/v2/farcaster/user/bulk", hypersnap::BASE)), 0);
        assert_eq!(host.sent(&format!("{}/v2/farcaster/cast?", hypersnap::BASE)), 0);
    }

    #[test]
    fn a_pinned_mini_app_is_left_out() {
        let host = Upstreams::new(true, "play https://farcaster.xyz/miniapps/abc/game");
        let (payload, _) = build(&host);
        assert!(payload.channel.pinned.is_none());
        assert_eq!(payload.channel.member_count, Some(1));
    }
}

#[cfg(test)]
mod quote_tests {
    use std::cell::RefCell;

    use allthing_core::model::{Embed, UserSummary};
    use futures::future::{FutureExt, LocalBoxFuture};
    use serde_json::json;

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// A quoted cast whose text is spam.
    const SPAM: &str = "0xb2";

    /// Answers Hypersnap's bulk `casts?casts=…` with the casts asked for, or
    /// fails every request when `down`.
    struct Bulk {
        down: bool,
        requests: RefCell<Vec<String>>,
    }

    impl Bulk {
        fn new(down: bool) -> Rc<Bulk> {
            Rc::new(Bulk { down, requests: RefCell::new(Vec::new()) })
        }

        /// The hashes each request asked for.
        fn asked(&self) -> Vec<Vec<String>> {
            self.requests.borrow().iter().map(|url| requested(url).into_iter().map(str::to_owned).collect()).collect()
        }
    }

    fn requested(url: &str) -> Vec<&str> {
        url.split_once("casts=").map_or("", |(_, list)| list).split("%2C").collect()
    }

    impl Host for Bulk {
        fn now(&self) -> f64 {
            1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            assert!(request.url.starts_with(&format!("{}/v2/farcaster/casts?", hypersnap::BASE)), "{}", request.url);
            self.requests.borrow_mut().push(request.url.clone());
            let response = if self.down {
                Err(FetchError::Network("unreachable".into()))
            } else {
                let casts: Vec<serde_json::Value> = requested(&request.url)
                    .into_iter()
                    .map(|hash| {
                        json!({
                            "hash": hash,
                            "text": if hash == SPAM { "Just unlocked my DEGEN rewards" } else { "quoted words" },
                            "timestamp": "2026-10-17T00:00:00Z",
                            "author": { "fid": 2, "username": "quoted" },
                        })
                    })
                    .collect();
                let body = serde_json::to_vec(&json!({ "casts": casts })).unwrap();
                Ok(HttpResponse { status: 200, retry_after: None, body: Some(body) })
            };
            async move { response }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn cast(hash: &str, quotes: &[&str]) -> Cast {
        Cast {
            hash: hash.into(),
            author: UserSummary { fid: 1, username: "a".into(), display_name: String::new(), pfp_url: None },
            text: String::new(),
            mentions: vec![],
            ts: 0,
            parent_hash: None,
            parent_url: None,
            channel: None,
            embeds: quotes.iter().map(|q| Embed::Cast { hash: (*q).into(), fid: Some(2) }).collect(),
            likes: 0,
            recasts: 0,
            replies: 0,
            mini_app: false,
            spam: false,
        }
    }

    #[test]
    fn one_read_brings_the_quotes_not_carried_and_hidden_ones_come_stripped() {
        let host = Bulk::new(false);
        let inv = Invocation::new(host.clone(), 45);
        let pool = [cast("0x01", &["0xb1", SPAM]), cast("0x02", &["0xb1", "0xb3"]), cast("0x03", &[])];
        let carried =
            BTreeMap::from([("0xb3".to_owned(), cast("0xb3", &[])), ("0xold".to_owned(), cast("0xold", &[]))]);
        let quotes = quoted_casts(&inv, &pool, carried).now_or_never().unwrap();
        assert_eq!(host.asked(), [["0xb1", SPAM]]);
        assert_eq!(inv.remaining_subrequests(), 44);
        assert_eq!(quotes.keys().collect::<Vec<_>>(), ["0xb1", SPAM, "0xb3"]);
        assert_eq!((quotes["0xb1"].text.as_str(), quotes["0xb1"].author.username.as_str()), ("quoted words", "quoted"));
        assert!(quotes[SPAM].spam && quotes[SPAM].text.is_empty());

        // The next build carries them all and reads nothing.
        let again = quoted_casts(&inv, &pool, quotes.clone()).now_or_never().unwrap();
        assert_eq!(again, quotes);
        assert_eq!(host.requests.borrow().len(), 1);
    }

    #[test]
    fn a_short_budget_or_a_failed_read_leaves_quotes_out() {
        let pool = [cast("0x01", &["0xb1", "0xb3"])];
        let carried = BTreeMap::from([("0xb3".to_owned(), cast("0xb3", &[]))]);

        let host = Bulk::new(false);
        let tight = Invocation::new(host.clone(), QUOTES_RESERVE);
        assert_eq!(quoted_casts(&tight, &pool, carried.clone()).now_or_never().unwrap(), carried);
        assert!(host.requests.borrow().is_empty());

        let host = Bulk::new(true);
        let inv = Invocation::new(host.clone(), 45);
        assert_eq!(quoted_casts(&inv, &pool, carried.clone()).now_or_never().unwrap(), carried);
        assert_eq!(host.requests.borrow().len(), 1);
    }

    #[test]
    fn quotes_beyond_one_bulk_read_wait_for_a_later_build() {
        let host = Bulk::new(false);
        let inv = Invocation::new(host.clone(), 45);
        let hashes: Vec<String> = (0..150).map(|i| format!("0x{i:04x}")).collect();
        let pool: Vec<Cast> = hashes
            .chunks(2)
            .enumerate()
            .map(|(i, pair)| cast(&format!("0xc{i}"), &pair.iter().map(String::as_str).collect::<Vec<_>>()))
            .collect();
        let quotes = quoted_casts(&inv, &pool, BTreeMap::new()).now_or_never().unwrap();
        assert_eq!(host.asked()[0], hashes[..hypersnap::MAX_BULK_CASTS]);
        assert_eq!(quotes.len(), hypersnap::MAX_BULK_CASTS);
    }
}
