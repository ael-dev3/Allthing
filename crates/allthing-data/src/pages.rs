// SPDX-License-Identifier: AGPL-3.0-or-later
//! One payload per page, assembled from upstream reads.

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use allthing_core::api::{
    self, Annotations, CastPayload, ChannelPayload, ChannelsPayload, EngagementPayload, EngagementRequest, HomePayload,
    IndexCoverage, LatestPayload, Meta, QuotedCast, ReactionKind, ReactionsPayload, RepliesPayload, SearchPayload,
    UserPayload,
};
use allthing_core::channels::{channel_id_for_url, channel_url_for_id, is_channel_id};
use allthing_core::index::{Ranking, Span};
use allthing_core::latest::{self, Cursor};
use allthing_core::model::{Cast, Channel};
use allthing_core::normalize::{self, RawAppChannel, RawCast};
use allthing_core::params::fid_username;
use allthing_core::profile::UserCard;
use allthing_core::provenance::Backend;
use allthing_core::quotes;
use allthing_core::ranking::TrendingWindow;
use allthing_core::thread::{
    flatten, mark_mini_apps, mark_mini_apps_in_tree, prune_hidden, reply_preview, without_hidden,
};
use allthing_core::vote::{AppVote, VotePayload};
use allthing_core::wallet::{self, ChainActivity, ChainKind, WalletActivity};
use futures::future::{join, join_all, join3};
use futures::stream::{self, StreamExt};

use crate::annotate::{self, Target, annotate, carried};
use crate::indexer::Snapshot;
use crate::upstream::{self, Upstream, UpstreamError};
use crate::{Built, Failure, Invocation};
use crate::{chain, hypersnap, miniapps};

/// Casts each listing ranks over; hot and top are computed within this pool.
pub const POOL_SIZE: u32 = 100;
/// Authors whose cast origins may be resolved while building one payload.
const ORIGIN_AUTHORS: usize = 12;
/// Subrequests that reading a listing's quoted casts leaves to the rest of
/// the build: app names and the cache write.
const QUOTES_RESERVE: u32 = 2;
/// How many levels of quotes a listing carries: a quote of a quote of a
/// quote, and no further (its quote shows as a link).
const QUOTE_DEPTH: usize = 3;
/// Preloaded previews never take Hypersnap's per-isolate bucket (40 tokens)
/// below this, so page builds in the same isolate keep their share: two
/// full batches (12 reads each) fit above it.
pub const PREVIEW_RESERVE: u32 = 16;
/// A Worker keeps at most six connections open at once, so a batch's lists
/// are read six at a time: a queued fetch would spend its timeout waiting.
const PARALLEL: usize = 6;

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
/// their cards link to them instead. Alongside: the profile cards of the
/// authors of the quotes read, from the same response.
async fn quoted_casts(
    inv: &Invocation,
    casts: &[Cast],
    mut known: BTreeMap<String, Cast>,
) -> (BTreeMap<String, Cast>, BTreeMap<u64, UserCard>) {
    quotes::retain_quoted(&mut known, casts);
    let mut cards = BTreeMap::new();
    // Level one is the quotes of the listed casts; each later level is the
    // quotes of the one before, so a quote inside a quote shows inline. Each
    // level is one bulk read, made only while the build's budget keeps
    // [`QUOTES_RESERVE`].
    // Each level's sources are the quotes of the level before, read or
    // carried: a carried quote still has quotes of its own to find.
    let mut sources: Vec<Cast> = casts.to_vec();
    for _ in 0..QUOTE_DEPTH {
        let missing = quotes::missing(&sources, &known, hypersnap::MAX_BULK_CASTS);
        if !missing.is_empty() && inv.remaining_subrequests() > QUOTES_RESERVE {
            let Ok(found) = hypersnap::casts_bulk(inv, &missing).await else { break };
            for cast in found.iter().map(normalize::cast).filter(|c| missing.contains(&c.hash)) {
                let quote = quotes::as_quote(cast);
                known.insert(quote.hash.clone(), quote);
            }
            cards.extend(normalize::author_cards(&found));
        }
        let children: Vec<Cast> =
            sources.iter().flat_map(Cast::quoted).filter_map(|hash| known.get(hash).cloned()).collect();
        if children.is_empty() {
            break;
        }
        sources = children;
    }
    (known, cards)
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
/// taken before origins size their batch to the budget that is left. Returns
/// the profile cards of the authors of the quotes it read.
async fn annotate_listing(
    inv: &Invocation,
    notes: &mut api::Annotations,
    pool: &[Cast],
    max_authors: usize,
    known_links: &BTreeMap<String, bool>,
) -> BTreeMap<u64, UserCard> {
    let refs: Vec<&Cast> = pool.iter().collect();
    let known = std::mem::take(&mut notes.quotes);
    let ((quotes, links, cards), ()) = join(
        async {
            let (mut quotes, cards) = quoted_casts(inv, pool, known).await;
            let links = check_quotes(inv, quotes.values_mut(), known_links).await;
            (quotes, links, cards)
        },
        annotate(inv, notes, &targets(&refs), max_authors),
    )
    .await;
    notes.quotes = quotes;
    notes.links.extend(links);
    cards
}

/// The accounts a listing shows by name: the authors of its casts and of
/// the casts they quote.
fn listed_accounts<'a>(casts: impl IntoIterator<Item = &'a Cast>, quotes: &'a BTreeMap<String, Cast>) -> Vec<u64> {
    casts.into_iter().chain(quotes.values()).map(|c| c.author.fid).collect()
}

/// Profile cards from the casts a build read, with those of the quotes'
/// authors (`quoted`). The user objects come with the casts: no request.
fn read_cards<'a>(
    raw: impl IntoIterator<Item = &'a RawCast>,
    quoted: BTreeMap<u64, UserCard>,
) -> BTreeMap<u64, UserCard> {
    let mut cards = normalize::author_cards(raw);
    cards.extend(quoted);
    cards
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
/// next pages that infinite scroll loads. Past month, Past year and All
/// time are ranked from Allthing's index; until it can rank them (or where
/// there is none, as on the static host), they show the past week's
/// trending casts, and say so by leaving out `index`.
pub async fn home(
    inv: Rc<Invocation>,
    offset: u32,
    window: TrendingWindow,
    previous: Option<Rc<Vec<u8>>>,
) -> Result<Built, Failure> {
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let known_links = std::mem::take(&mut notes.links);
    if let Some(span) = Span::of(window)
        && let Some(ranking) = ranking(&inv, span).await
    {
        let hashes = ranking.page(offset as usize, POOL_SIZE as usize);
        let (raw, casts) = ranked_casts(&inv, &hashes).await?;
        inv.mark("ranked");
        let pool = drop_hidden(&inv, casts, &mut notes, &known_links).await;
        inv.mark("hidden");
        let quoted = annotate_listing(&inv, &mut notes, &pool, ORIGIN_AUTHORS, &known_links).await;
        inv.mark("annotate");
        notes.keep_users(read_cards(&raw, quoted), listed_accounts(&pool, &notes.quotes));
        let index = IndexCoverage { casts: ranking.total, since: ranking.oldest, ranked_at: ranking.built };
        let payload = HomePayload { pool, notes, meta: meta(&inv), index: Some(index) };
        return Built::json(&payload, api::INDEXED_HOME_CACHE);
    }
    let window = if window.is_indexed() { TrendingWindow::Week } else { window };
    let raw = hypersnap::trending_casts(&inv, POOL_SIZE, offset, window).await.map_err(Failure::upstream)?;
    let casts = raw.iter().map(normalize::cast).collect();
    let pool = drop_hidden(&inv, casts, &mut notes, &known_links).await;
    let quoted = annotate_listing(&inv, &mut notes, &pool, ORIGIN_AUTHORS, &known_links).await;
    notes.keep_users(read_cards(&raw, quoted), listed_accounts(&pool, &notes.quotes));
    Built::json(&HomePayload { pool, notes, meta: meta(&inv), index: None }, api::HOME_CACHE)
}

/// A span's ranking from the index, if there is an index, it answers, and
/// the ranking has casts. Reading it takes one of the build's subrequests.
async fn ranking(inv: &Invocation, span: Span) -> Option<Ranking> {
    let index = inv.index()?;
    if !inv.take_subrequests(1) {
        return None;
    }
    index.ranking(span).await.ok().flatten().filter(|r| !r.entries.is_empty())
}

/// Ranked casts, freshly read in one bulk call, in rank order. Casts deleted
/// since they were indexed are left out. With them, the raw casts read.
async fn ranked_casts(inv: &Invocation, hashes: &[String]) -> Result<(Vec<RawCast>, Vec<Cast>), Failure> {
    if hashes.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let found = hypersnap::casts_bulk(inv, hashes).await.map_err(Failure::upstream)?;
    let mut by_hash: std::collections::HashMap<String, Cast> =
        found.iter().map(normalize::cast).map(|c| (c.hash.clone(), c)).collect();
    let ranked = hashes.iter().filter_map(|hash| by_hash.remove(hash)).collect();
    Ok((found, ranked))
}

/// Events read per shard in one build: about 14 minutes of Farcaster, most
/// of them block confirmations.
const EVENTS_PAGE: u32 = 1000;
/// A buffer older than this starts over from the newest events: continuing
/// from its cursors would leave a gap one page can't cover.
const WARM_MS: f64 = 600_000.0;
/// What the cron's copy of the newest casts is stored as.
pub const LATEST_SNAPSHOT: &str = "latest";
/// A data center whose own list of the newest casts is older than this
/// reads the cron's stored copy, and serves it as it is if it's younger.
const SNAPSHOT_SERVE_MS: f64 = 90_000.0;

/// The newest casts on Farcaster, from Snapchain's event stream. Each build
/// continues from the previous one's cursors, so it reads only the events
/// since then: it adds the new top-level casts, read in one bulk call that
/// also refreshes the listed casts' counts, and drops removed ones. New casts
/// are never skipped: if they can't be read now, the build fails, the
/// previous copy is served, and the next build reads the same events again.
pub async fn latest(inv: Rc<Invocation>, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    let mut warm = previous.as_deref().and_then(|bytes| serde_json::from_slice::<LatestPayload>(bytes).ok());
    let built_at = |p: &LatestPayload| p.meta.fetched_at as f64;
    // A data center that hasn't listed the newest casts lately (or ever)
    // starts from the copy the cron stores every minute: served as it is
    // while it's young, for one read and no upstream calls, or built on.
    if warm.as_ref().is_none_or(|p| inv.now() - built_at(p) >= SNAPSHOT_SERVE_MS)
        && let Some(stored) = stored(&inv, LATEST_SNAPSHOT).await
        && warm.as_ref().is_none_or(|p| stored.built > built_at(p))
    {
        if inv.now() - stored.built < SNAPSHOT_SERVE_MS {
            let body = stored.body.into_bytes();
            return Ok(Built { body, status: 200, policy: api::LATEST_CACHE, fetched_at: Some(stored.built) });
        }
        warm = serde_json::from_str(&stored.body).ok();
    }
    build_latest(&inv, warm).await
}

/// The cron's turn: the newest casts, built on the stored copy and stored
/// again, so that every data center has a copy at most about a minute old
/// without a reader waiting for a build.
pub async fn store_latest(inv: Rc<Invocation>) -> Result<(), Failure> {
    let index = inv.index().ok_or_else(|| Failure::internal("no index to store the newest casts in"))?;
    let warm = stored(&inv, LATEST_SNAPSHOT).await.and_then(|s| serde_json::from_str(&s.body).ok());
    let built = build_latest(&inv, warm).await?;
    let body = String::from_utf8(built.body).map_err(|e| Failure::internal(&e.to_string()))?;
    if !inv.take_subrequests(1) {
        return Err(Failure::internal("no subrequest left to store the newest casts"));
    }
    let snapshot = Snapshot { built: inv.now(), body };
    index.save_snapshot(LATEST_SNAPSHOT, snapshot).await.map_err(|e| Failure::internal(&e.to_string()))
}

/// The copy of `name` the cron stores, if there is an index and it has one.
/// Reading it takes one of the build's subrequests.
async fn stored(inv: &Invocation, name: &'static str) -> Option<Snapshot> {
    let index = inv.index()?;
    if !inv.take_subrequests(1) {
        return None;
    }
    index.snapshot(name).await.ok().flatten()
}

async fn build_latest(inv: &Invocation, warm: Option<LatestPayload>) -> Result<Built, Failure> {
    let warm = warm.filter(|p| inv.now() - (p.meta.fetched_at as f64) < WARM_MS);
    let (listed, cursors, mut notes) = match warm {
        Some(p) => (p.pool, p.cursors, p.notes),
        None => (Vec::new(), Vec::new(), Annotations::default()),
    };
    let cursor = |shard: u32| cursors.iter().find(|c| c.shard == shard).map(|c| c.event_id);
    let pages = join_all(latest::SHARDS.map(|shard| hypersnap::events(inv, shard, cursor(shard), EVENTS_PAGE))).await;
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
    let raw = match hypersnap::casts_bulk(inv, &hashes).await {
        Ok(raw) => raw,
        Err(e) if !added.is_empty() => return Err(Failure::upstream(e)),
        Err(_) => Vec::new(),
    };
    let fresh = raw.iter().map(normalize::cast).filter(|c| c.parent_hash.is_none()).collect();
    let known_links = std::mem::take(&mut notes.links);
    let fresh = drop_hidden(inv, fresh, &mut notes, &known_links).await;
    let pool = latest::newest(listed, fresh, &removed, POOL_SIZE as usize);
    let quoted = annotate_listing(inv, &mut notes, &pool, ORIGIN_AUTHORS, &known_links).await;
    notes.keep_users(read_cards(&raw, quoted), listed_accounts(&pool, &notes.quotes));
    Built::json(&LatestPayload { pool, notes, cursors: next, meta: meta(inv) }, api::LATEST_CACHE)
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
    let mut pinned_cast: Vec<Cast> = pinned.iter().map(normalize::cast).collect();
    // The pinned cast's links are checked first, with the feed's, and it is
    // left out like any listed cast if it is a mini app or spam.
    notes.links = miniapps::check(&inv, pinned_cast.iter().chain(&casts), &known_links).await;
    mark_mini_apps(&mut pinned_cast, &notes.links);
    mark_mini_apps(&mut casts, &notes.links);
    channel.pinned = without_hidden(pinned_cast).pop();
    let pool = newest_unique(without_hidden(casts), POOL_SIZE as usize);
    let listed: Vec<Cast> = channel.pinned.iter().chain(&pool).cloned().collect();
    let quoted = annotate_listing(&inv, &mut notes, &listed, ORIGIN_AUTHORS, &known_links).await;
    // Cards for the casts' authors and for the hosts, whose user objects
    // came with the app's record.
    let mut cards = read_cards(raw.iter().chain(&pinned), quoted);
    cards.extend(normalize::user_cards(&users));
    let hosts = channel.lead.iter().chain(&channel.moderators).map(|host| host.fid);
    notes.keep_users(cards, listed_accounts(&listed, &notes.quotes).into_iter().chain(hosts));
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
    let (channel, parent, raw_quotes) = join3(
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
            async move { hypersnap::cast(inv, hash).await.ok().flatten() }
        })),
    )
    .await;
    let mut quotes: Vec<QuotedCast> = quote_hashes
        .iter()
        .zip(&raw_quotes)
        .map(|(hash, raw)| QuotedCast {
            hash: hash.clone(),
            cast: raw.as_ref().map(|c| quotes::as_quote(normalize::cast(c))),
        })
        .collect();
    notes.links.extend(check_quotes(&inv, quotes.iter_mut().filter_map(|q| q.cast.as_mut()), &known_links).await);

    // Attribute the cast itself first, then its context, then the replies.
    let mut refs: Vec<&Cast> = vec![first];
    refs.extend(parent.iter());
    refs.extend(quotes.iter().filter_map(|q| q.cast.as_ref()));
    refs.extend(flatten(&thread).into_iter().skip(1));
    annotate(&inv, &mut notes, &targets(&refs), ORIGIN_AUTHORS).await;
    // Cards for the accounts the page names: the thread's and the quotes'
    // authors (the parent shows as a link, without one).
    let named: Vec<u64> = flatten(&thread)
        .into_iter()
        .chain(quotes.iter().filter_map(|q| q.cast.as_ref()))
        .map(|c| c.author.fid)
        .collect();
    notes.keep_users(normalize::author_cards(std::iter::once(&root).chain(raw_quotes.iter().flatten())), named);
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
    inv.mark("lookup");
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
    inv.mark("reads");
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
    inv.mark("hidden");
    let active = signers.ok().map(|apps| annotate::remember_signers(&inv, fid, apps));
    if let Ok(recent) = recent {
        let cast_signers: Vec<(String, String)> =
            pool.iter().filter_map(|c| recent.get(&c.hash).map(|s| (c.hash.clone(), s.clone()))).collect();
        for (hash, origin) in annotate::attribute(&inv, fid, &cast_signers, active).await {
            notes.origins.insert(hash, origin);
        }
    }
    inv.mark("attribute");
    let quoted = annotate_listing(&inv, &mut notes, &pool, 2, &known_links).await;
    inv.mark("annotate");
    // The casts are the account's own, whose card is the payload's `user`:
    // the notes carry cards for the authors of the casts they quote.
    let quoting: Vec<u64> = notes.quotes.values().map(|c| c.author.fid).collect();
    notes.keep_users(quoted, quoting);
    let policy = if retry { api::USER_RETRY_CACHE } else { api::USER_CACHE };
    Built::json(&UserPayload { user, pool, notes, meta: meta(&inv) }, policy)
}

/// A channel as Hypersnap's channel search returns it, without its count:
/// search can return a different record of the same channel than the
/// channel page reads (/farcaster: 23 against 94,568 on 19 October 2026), so
/// its count would contradict the page it links to.
fn searched_channel(raw: &normalize::RawChannel) -> Channel {
    Channel { casters: None, ..normalize::channel(raw) }
}

pub async fn channels(inv: Rc<Invocation>, query: Option<String>) -> Result<Built, Failure> {
    let channels = match &query {
        Some(q) => hypersnap::search_channels(&inv, q, 50)
            .await
            .map_err(Failure::upstream)?
            .iter()
            .map(searched_channel)
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
    let channels: Vec<Channel> = channels.unwrap_or_default().iter().map(searched_channel).take(10).collect();
    let (quotes, quoted) = quoted_casts(&inv, &casts, BTreeMap::new()).await;
    notes.quotes = quotes;
    notes.links.extend(check_quotes(&inv, notes.quotes.values_mut(), &BTreeMap::new()).await);
    // The people found are whole profiles already; the casts' authors get cards.
    notes.keep_users(read_cards(&hydrated, quoted), listed_accounts(&casts, &notes.quotes));
    Built::json(&SearchPayload { query, casts, users, channels, notes, meta: meta(&inv) }, api::SEARCH_CACHE)
}

/// Who reacted to a cast, newest first: at most `limit` people out of the
/// 100 that Hypersnap lists, as `/api/v1/reactions` answers.
async fn reactors(
    inv: &Invocation,
    hash: &str,
    kind: ReactionKind,
    limit: u32,
) -> Result<ReactionsPayload, UpstreamError> {
    let raw = hypersnap::reactions(inv, hash, kind.as_str(), 100).await?;
    let (users, more) = normalize::reactors(&raw, limit as usize);
    Ok(ReactionsPayload { hash: hash.to_owned(), kind, users, more, meta: meta(inv) })
}

/// Who liked or recasted a cast: a preview (`limit` 10) or the full list
/// (100, which is as many as Hypersnap returns).
pub async fn reactions(inv: Rc<Invocation>, hash: String, kind: ReactionKind, limit: u32) -> Result<Built, Failure> {
    let payload = reactors(&inv, &hash, kind, limit).await.map_err(Failure::upstream)?;
    Built::json(&payload, api::REACTIONS_CACHE)
}

/// Whether a preloaded preview may read now: Hypersnap's bucket holds more
/// than [`PREVIEW_RESERVE`] tokens, and the build keeps a subrequest for its
/// cache write. Checked as each read starts, so batches built side by side
/// in an isolate don't both count the same tokens.
fn preview_may_read(inv: &Invocation) -> bool {
    inv.remaining_subrequests() > 1 && upstream::spare(inv, Upstream::Hypersnap, PREVIEW_RESERVE) > 0
}

/// Who liked and recasted a batch of feed cards: each list exactly as its
/// own preview answers it ([`reactions`] with a limit of 10), one
/// `reactions/cast` read each, six at a time. Previews name people, not
/// casts, so there are no mini apps or spam to leave out. A list that can't
/// be read is left out without failing the others, and the batch is then
/// kept only briefly. Reads never take Hypersnap's bucket below
/// [`PREVIEW_RESERVE`]: with less room, a batch reads what fits (likes
/// first) and leaves the rest to the popovers, which load a list they lack
/// when they open.
pub async fn engagement(inv: Rc<Invocation>, request: EngagementRequest) -> Result<Built, Failure> {
    let lists: Vec<(&str, ReactionKind)> = request.lists().collect();
    let read: Vec<Result<ReactionsPayload, UpstreamError>> = stream::iter(lists.iter().map(|&(hash, kind)| {
        let inv = &inv;
        async move {
            if !preview_may_read(inv) {
                return Err(UpstreamError::Busy(1000.0));
            }
            reactors(inv, hash, kind, api::REACTIONS_PREVIEW).await
        }
    }))
    .buffered(PARALLEL)
    .collect()
    .await;
    let mut previews = Vec::with_capacity(read.len());
    let mut failure = None;
    for result in read {
        match result {
            Ok(preview) => previews.push(preview),
            Err(error) => failure = failure.or(Some(error)),
        }
    }
    let Some(error) = failure else {
        return Built::json(&EngagementPayload { previews, meta: meta(&inv) }, api::ENGAGEMENT_CACHE);
    };
    if previews.is_empty() {
        // Nothing read: every read failed, or the bucket was at its reserve.
        return Err(Failure::upstream(error));
    }
    Built::json(&EngagementPayload { previews, meta: meta(&inv) }, api::ENGAGEMENT_PARTIAL_CACHE)
}

/// Replies to show when hovering a cast's reply count: the ten most engaged
/// direct replies, without mini apps or spam.
pub async fn replies(inv: Rc<Invocation>, hash: String) -> Result<Built, Failure> {
    let Some(root) = hypersnap::conversation(&inv, &hash, 1).await.map_err(Failure::upstream)? else {
        return Ok(Built::not_found("No such cast"));
    };
    let mut notes = api::Annotations::default();
    let replies = root.direct_replies.iter().flatten().map(normalize::cast).collect();
    let replies = reply_preview(drop_hidden(&inv, replies, &mut notes, &BTreeMap::new()).await);
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

pub async fn wallets(inv: Rc<Invocation>, fid: u64, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
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
                let (inv, evm, solana) = (&inv, &evm, &solana);
                async move {
                    let result = match chain.kind {
                        ChainKind::Evm => chain::evm_sent_count(inv, chain, evm).await.map(|n| (n, false)),
                        // Its public RPC may refuse Cloudflare's addresses: say so.
                        ChainKind::Solana => chain::solana_tx_count(inv, chain, solana)
                            .await
                            .map_err(|e| UpstreamError::Network(format!("not measured ({e})"))),
                    }
                    .map_err(|e| e.to_string());
                    let (count, capped) = *result.as_ref().unwrap_or(&(0, false));
                    ChainActivity {
                        chain: chain.slug.to_owned(),
                        name: chain.name.to_owned(),
                        kind: chain.kind,
                        count,
                        capped,
                        percent: 0,
                        error: result.err(),
                        carried: false,
                    }
                }
            }),
    )
    .await;
    let mut chains = measured;
    // A chain that can't be read now keeps its last good count, from the
    // previous copy, instead of leaving the others to share its part.
    if let Some(last) = previous.as_deref().and_then(|p| serde_json::from_slice::<WalletActivity>(p).ok()) {
        wallet::carry_unread(&mut chains, &addresses, &last);
    }
    let policy = if wallet::evm_unread(&chains) { api::WALLET_RETRY_CACHE } else { api::WALLET_CACHE };
    let total = wallet::apportion(&mut chains);
    let sources = wallet_sources(!evm.is_empty(), !solana.is_empty(), inv.solana_rpc().is_some(), &chains);
    let activity = WalletActivity { fid, addresses, chains, total, sources, fetched_at: inv.now() as i64 };
    Built::json(&activity, policy)
}

/// Where each kind of count was read, or that Solana's couldn't be.
fn wallet_sources(evm: bool, solana: bool, keyed: bool, chains: &[ChainActivity]) -> Vec<String> {
    let mut sources = Vec::new();
    if evm {
        sources.push(
            "EVM chains: transactions sent by each verified address (its account nonce), read from keyless public RPCs \
             (mainnet.base.org, publicnode.com, the chains' own endpoints)."
                .to_owned(),
        );
    }
    if solana {
        let unread = chains.iter().any(|c| c.kind == ChainKind::Solana && c.error.is_some());
        sources.push(if unread {
            "Solana: couldn't be read just now, so its wallets aren't counted. The read failed or its RPC endpoint \
             isn't configured."
                .to_owned()
        } else {
            if keyed {
                format!(
                    "Solana: transactions involving each verified address (its signatures, paged through its history \
                     up to {} each, a count at the cap shows as more), read from the keyed Solana RPC endpoint.",
                    chain::SOLANA_KEYED_PAGES as u64 * chain::SOLANA_PAGE
                )
            } else {
                format!(
                    "Solana: transactions involving each verified address (its signatures, up to {} each), read from \
                     Solana's public RPC (api.mainnet-beta.solana.com).",
                    chain::SOLANA_PAGE
                )
            }
        });
    }
    sources
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use futures::future::{FutureExt, LocalBoxFuture};

    use allthing_core::index::Entry;

    use super::*;
    use crate::indexer::{Index, IndexError, Saved};
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// A hub with an event page per (shard, first event), the casts it can
    /// read in bulk, and whether bulk reads fail.
    #[derive(Default)]
    struct Hub {
        events: HashMap<(String, String), String>,
        casts: HashMap<String, String>,
        bulk_fails: bool,
        requests: RefCell<Vec<String>>,
        stored: Option<Rc<Stored>>,
    }

    /// The part of the index the newest casts use: the cron's stored copy.
    #[derive(Default)]
    struct Stored {
        copy: RefCell<Option<Snapshot>>,
        reads: std::cell::Cell<u32>,
    }

    impl Index for Stored {
        fn ranking(&self, _: Span) -> LocalBoxFuture<'static, Result<Option<Ranking>, IndexError>> {
            async { Ok(None) }.boxed_local()
        }

        fn load(&self) -> LocalBoxFuture<'static, Result<Saved, IndexError>> {
            async { Ok(Saved::default()) }.boxed_local()
        }

        fn save(
            &self,
            _: Vec<Entry>,
            _: Option<Span>,
            _: String,
            _: f64,
        ) -> LocalBoxFuture<'static, Result<(), IndexError>> {
            async { Ok(()) }.boxed_local()
        }

        fn snapshot(&self, name: &'static str) -> LocalBoxFuture<'static, Result<Option<Snapshot>, IndexError>> {
            assert_eq!(name, LATEST_SNAPSHOT);
            self.reads.set(self.reads.get() + 1);
            let stored = self.copy.borrow().clone();
            async move { Ok(stored) }.boxed_local()
        }

        fn save_snapshot(
            &self,
            name: &'static str,
            snapshot: Snapshot,
        ) -> LocalBoxFuture<'static, Result<(), IndexError>> {
            assert_eq!(name, LATEST_SNAPSHOT);
            *self.copy.borrow_mut() = Some(snapshot);
            async { Ok(()) }.boxed_local()
        }
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

        fn index(&self) -> Option<Rc<dyn Index>> {
            self.stored.clone().map(|stored| stored as Rc<dyn Index>)
        }
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

    /// The cron's copy, built `age_ms` ago, with cursors at events 12 and 22.
    fn stored_copy(age_ms: f64) -> Rc<Stored> {
        let mut payload: LatestPayload = serde_json::from_slice(&previous_latest()).unwrap();
        payload.meta.fetched_at = (NOW - age_ms) as i64;
        let body = serde_json::to_string(&payload).unwrap();
        Rc::new(Stored { copy: RefCell::new(Some(Snapshot { built: NOW - age_ms, body })), ..Default::default() })
    }

    const NOW: f64 = 1_800_000_000_000.0;

    #[test]
    fn a_data_center_with_no_fresh_list_serves_the_crons_copy_without_building() {
        let stored = stored_copy(40_000.0);
        let hub = Rc::new(Hub { stored: Some(stored.clone()), ..Default::default() });
        let inv = Rc::new(Invocation::new(hub.clone(), 45));
        let built = latest(inv, None).now_or_never().unwrap().unwrap();
        assert_eq!(built.body, stored.copy.borrow().as_ref().unwrap().body.as_bytes());
        // It is as old as the copy, so the edge refreshes it soon.
        assert_eq!(built.fetched_at, Some(NOW - 40_000.0));
        assert!(hub.requests.borrow().is_empty(), "no upstream calls: {:?}", hub.requests.borrow());
    }

    #[test]
    fn an_older_copy_of_the_crons_is_built_on_from_its_cursors() {
        let mut hub = Hub { stored: Some(stored_copy(120_000.0)), ..Default::default() };
        hub.events.insert(("1".into(), "13".into()), page(&[add(13, "0xd3", 110, false)]));
        hub.events.insert(("2".into(), "23".into()), page(&[]));
        hub.casts.insert("0xd3".into(), raw_cast("0xd3", "2026-01-01T00:01:50Z", 0));
        // This data center's own list is older still.
        let mut own: LatestPayload = serde_json::from_slice(&previous_latest()).unwrap();
        own.meta.fetched_at = (NOW - 300_000.0) as i64;
        own.cursors = vec![Cursor { shard: 1, event_id: 5 }, Cursor { shard: 2, event_id: 5 }];
        let payload = build(Rc::new(hub), Some(serde_json::to_vec(&own).unwrap())).unwrap();
        assert_eq!(payload.pool.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), ["0xd3"]);
        assert_eq!(payload.cursors, [Cursor { shard: 1, event_id: 13 }, Cursor { shard: 2, event_id: 22 }]);
    }

    #[test]
    fn a_fresh_list_of_its_own_needs_no_stored_copy() {
        let stored = stored_copy(10_000.0);
        let mut hub = Hub { stored: Some(stored.clone()), ..Default::default() };
        hub.events.insert(("1".into(), "13".into()), page(&[]));
        hub.events.insert(("2".into(), "23".into()), page(&[]));
        // Its own list is 30 s old: built on, and the stored copy isn't read.
        let inv = Rc::new(Invocation::new(Rc::new(hub), 45));
        let built = latest(inv, Some(Rc::new(previous_latest()))).now_or_never().unwrap().unwrap();
        assert_eq!(built.fetched_at, None);
        assert_eq!(stored.reads.get(), 0);
    }

    #[test]
    fn the_cron_builds_on_its_stored_copy_and_stores_the_result() {
        let stored = stored_copy(60_000.0);
        let mut hub = Hub { stored: Some(stored.clone()), ..Default::default() };
        hub.events.insert(("1".into(), "13".into()), page(&[add(13, "0xd3", 110, false)]));
        hub.events.insert(("2".into(), "23".into()), page(&[]));
        hub.casts.insert("0xd3".into(), raw_cast("0xd3", "2026-01-01T00:01:50Z", 0));
        let inv = Rc::new(Invocation::new(Rc::new(hub), 45));
        store_latest(inv).now_or_never().unwrap().unwrap();
        let saved = stored.copy.borrow().clone().unwrap();
        assert_eq!(saved.built, NOW);
        let payload: LatestPayload = serde_json::from_str(&saved.body).unwrap();
        assert_eq!(payload.pool.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), ["0xd3"]);
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
                json(r#"{"casts":[{"hash":"0x01","text":"gm","timestamp":"2026-10-18T10:00:00Z","author":{"fid":2,"username":"v","profile":{"bio":{"text":"gm"}},"follower_count":5}}]}"#.into())
            } else if v2("user/bulk?fids=1") {
                json(r#"{"users":[{"fid":1,"username":"farcaster","display_name":"Farcaster","follower_count":445632,"registered_at":"2023-11-07T19:42:51.000Z"}]}"#.into())
            } else if v2(&format!("cast?identifier={PINNED}")) {
                json(format!(
                    r#"{{"cast":{{"hash":"{}","text":"{}","timestamp":"2026-10-01T00:00:00Z","author":{{"fid":37,"username":"balajis.eth","follower_count":575918}}}}}}"#,
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
        // Profile cards for the feed's author, the pinned cast's and the lead,
        // from the user objects those reads brought: no read of their own.
        let users = &payload.notes.users;
        assert_eq!(users.keys().copied().collect::<Vec<_>>(), [1, 2, 37]);
        assert_eq!((users[&1].followers, users[&1].joined()), (445_632, Some(allthing_core::Joined::BeforeMigration)));
        assert_eq!((users[&2].bio.as_str(), users[&2].followers), ("gm", 5));
        assert_eq!(users[&37].followers, 575_918);
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
        assert_eq!(payload.notes.users.keys().copied().collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn a_pinned_mini_app_is_left_out() {
        let host = Upstreams::new(true, "play https://farcaster.xyz/miniapps/abc/game");
        let (payload, _) = build(&host);
        assert!(payload.channel.pinned.is_none());
        assert_eq!(payload.channel.member_count, Some(1));
        // So is its author's card.
        assert!(!payload.notes.users.contains_key(&37));
    }
}

#[cfg(test)]
mod search_channel_tests {
    use super::*;

    /// Hypersnap's channel search for "farcaster" on 19 October 2026: a
    /// record with 23 "members", while the channel page reads 94,568.
    #[test]
    fn a_searched_channel_carries_no_count() {
        let raw: normalize::RawChannel = serde_json::from_value(serde_json::json!({
            "id": "farcaster", "name": "farcaster", "member_count": 23,
            "parent_url": "https://warpcast.com/~/channel/farcaster"
        }))
        .unwrap();
        assert_eq!(normalize::channel(&raw).casters, Some(23));
        let searched = searched_channel(&raw);
        assert_eq!(searched.casters, None);
        assert!(searched.counts().is_empty());
        assert_eq!(searched.id, "farcaster");
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
                            "author": { "fid": 2, "username": "quoted", "follower_count": 9 },
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
        let (quotes, cards) = quoted_casts(&inv, &pool, carried).now_or_never().unwrap();
        assert_eq!(host.asked(), [["0xb1", SPAM]]);
        assert_eq!(inv.remaining_subrequests(), 44);
        assert_eq!(quotes.keys().collect::<Vec<_>>(), ["0xb1", SPAM, "0xb3"]);
        assert_eq!((quotes["0xb1"].text.as_str(), quotes["0xb1"].author.username.as_str()), ("quoted words", "quoted"));
        assert!(quotes[SPAM].spam && quotes[SPAM].text.is_empty());
        // The quoted author's profile card comes with the same read.
        assert_eq!(cards, BTreeMap::from([(2, UserCard { followers: 9, ..Default::default() })]));

        // The next build carries them all and reads nothing.
        let (again, cards) = quoted_casts(&inv, &pool, quotes.clone()).now_or_never().unwrap();
        assert_eq!((again, cards), (quotes, BTreeMap::new()));
        assert_eq!(host.requests.borrow().len(), 1);
    }

    /// Serves casts whose embeds quote the next cast of a chain: each hash
    /// quotes the one in `next`, if any.
    struct Chain {
        next: BTreeMap<&'static str, &'static str>,
        requests: RefCell<usize>,
    }

    impl Host for Chain {
        fn now(&self) -> f64 {
            1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            *self.requests.borrow_mut() += 1;
            let casts: Vec<serde_json::Value> = requested(&request.url)
                .into_iter()
                .map(|hash| {
                    let embeds: Vec<serde_json::Value> = self
                        .next
                        .get(hash)
                        .map(|quoted| vec![json!({ "cast_id": { "hash": quoted, "fid": 2 } })])
                        .unwrap_or_default();
                    json!({
                        "hash": hash,
                        "text": format!("cast {hash}"),
                        "timestamp": "2026-10-17T00:00:00Z",
                        "author": { "fid": 2, "username": "quoted" },
                        "embeds": embeds,
                    })
                })
                .collect();
            let body = serde_json::to_vec(&json!({ "casts": casts })).unwrap();
            let response = Ok(HttpResponse { status: 200, retry_after: None, body: Some(body) });
            async move { response }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    #[test]
    fn a_quote_of_a_quote_comes_along_to_three_levels_and_no_further() {
        // 0x01 quotes 0xb1, which quotes 0xc1, which quotes 0xd1, which quotes 0xe1.
        let host = Rc::new(Chain {
            next: BTreeMap::from([("0xb1", "0xc1"), ("0xc1", "0xd1"), ("0xd1", "0xe1")]),
            requests: RefCell::new(0),
        });
        let inv = Invocation::new(host.clone(), 45);
        let pool = [cast("0x01", &["0xb1"])];
        let (quotes, _) = quoted_casts(&inv, &pool, BTreeMap::new()).now_or_never().unwrap();
        assert_eq!(quotes.keys().collect::<Vec<_>>(), ["0xb1", "0xc1", "0xd1"]);
        assert_eq!(*host.requests.borrow(), 3, "one bulk read per level");
    }

    #[test]
    fn a_carried_quote_still_has_its_quotes_read() {
        // 0xb1 is carried from the previous payload, so nothing is missing at
        // level one; its quote 0xc1 and then 0xd1 must still be read.
        let host = Rc::new(Chain {
            next: BTreeMap::from([("0xb1", "0xc1"), ("0xc1", "0xd1"), ("0xd1", "0xe1")]),
            requests: RefCell::new(0),
        });
        let inv = Invocation::new(host.clone(), 45);
        let pool = [cast("0x01", &["0xb1"])];
        let carried = BTreeMap::from([("0xb1".to_owned(), cast("0xb1", &["0xc1"]))]);
        let (quotes, _) = quoted_casts(&inv, &pool, carried).now_or_never().unwrap();
        assert_eq!(quotes.keys().collect::<Vec<_>>(), ["0xb1", "0xc1", "0xd1"]);
        assert_eq!(*host.requests.borrow(), 2, "0xc1 and 0xd1, one read each");
    }

    #[test]
    fn a_short_budget_or_a_failed_read_leaves_quotes_out() {
        let pool = [cast("0x01", &["0xb1", "0xb3"])];
        let carried = BTreeMap::from([("0xb3".to_owned(), cast("0xb3", &[]))]);

        let host = Bulk::new(false);
        let tight = Invocation::new(host.clone(), QUOTES_RESERVE);
        assert_eq!(
            quoted_casts(&tight, &pool, carried.clone()).now_or_never().unwrap(),
            (carried.clone(), BTreeMap::new())
        );
        assert!(host.requests.borrow().is_empty());

        let host = Bulk::new(true);
        let inv = Invocation::new(host.clone(), 45);
        assert_eq!(quoted_casts(&inv, &pool, carried.clone()).now_or_never().unwrap(), (carried, BTreeMap::new()));
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
        let (quotes, _) = quoted_casts(&inv, &pool, BTreeMap::new()).now_or_never().unwrap();
        assert_eq!(host.asked()[0], hashes[..hypersnap::MAX_BULK_CASTS]);
        assert_eq!(quotes.len(), hypersnap::MAX_BULK_CASTS);
    }
}

#[cfg(test)]
mod indexed_home_tests {
    use std::cell::RefCell;

    use allthing_core::index::Entry;
    use futures::future::{FutureExt, LocalBoxFuture};
    use serde_json::json;

    use super::*;
    use crate::indexer::Index;
    use crate::indexer::tests::Memory;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    const NOW_MS: f64 = 1_792_000_000_000.0;

    /// Hypersnap's trending feed and bulk cast reads, and an index if any.
    struct Indexed {
        index: Option<Rc<Memory>>,
        requests: RefCell<Vec<String>>,
    }

    impl Host for Indexed {
        fn now(&self) -> f64 {
            NOW_MS
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let url = request.url.clone();
            self.requests.borrow_mut().push(url.clone());
            let cast = |hash: &str| {
                json!({
                    "hash": hash,
                    "author": { "fid": 2, "username": "two" },
                    "text": if hash == "0xc3" { "Just unlocked my DEGEN rewards" } else { "words" },
                    "timestamp": "2026-10-01T00:00:00Z",
                })
            };
            let body = if url.contains("/v2/farcaster/casts?") {
                let list = url.split_once("casts=").map_or("", |(_, l)| l).replace("%2C", ",");
                // 0xc9 was deleted after it was indexed.
                json!({ "casts": list.split(',').filter(|h| *h != "0xc9").map(cast).collect::<Vec<_>>() })
            } else if url.contains("/feed/trending?") {
                json!({ "casts": [cast("0xd1")] })
            } else {
                json!({})
            };
            let response =
                HttpResponse { status: 200, retry_after: None, body: Some(serde_json::to_vec(&body).unwrap()) };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}

        fn index(&self) -> Option<Rc<dyn Index>> {
            self.index.clone().map(|index| index as Rc<dyn Index>)
        }
    }

    fn build(host: &Rc<Indexed>, offset: u32, window: TrendingWindow) -> HomePayload {
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let built = home(inv, offset, window, None).now_or_never().unwrap().unwrap();
        serde_json::from_slice(&built.body).unwrap()
    }

    fn filled() -> Rc<Memory> {
        let index = Memory::default();
        let now = (NOW_MS / 1000.0) as i64;
        let day = 86_400;
        let entries = [("0xc1", now - day, 900), ("0xc2", now - 3 * day, 400), ("0xc3", now - day, 300)];
        let mut casts = vec![Entry("0xc9".into(), now - day, 950)];
        casts.extend(entries.iter().map(|(h, ts, p)| Entry((*h).into(), *ts, *p)));
        casts.push(Entry("0xc4".into(), now - 400 * day, 5000));
        index.save(casts, None, String::new(), NOW_MS).now_or_never().unwrap().unwrap();
        for span in Span::ALL {
            index.rerank(span, NOW_MS);
        }
        Rc::new(index)
    }

    fn hashes(payload: &HomePayload) -> Vec<&str> {
        payload.pool.iter().map(|c| c.hash.as_str()).collect()
    }

    #[test]
    fn month_year_and_all_time_come_from_the_index_in_rank_order() {
        let host = Rc::new(Indexed { index: Some(filled()), requests: RefCell::new(Vec::new()) });
        let month = build(&host, 0, TrendingWindow::Month);
        // Deleted (0xc9) and spam (0xc3) casts are left out; the rest keep their rank.
        assert_eq!(hashes(&month), ["0xc1", "0xc2"]);
        let coverage = month.index.unwrap();
        assert_eq!((coverage.casts, coverage.ranked_at), (4, NOW_MS as i64));
        assert_eq!(hashes(&build(&host, 0, TrendingWindow::All)), ["0xc4", "0xc1", "0xc2"]);
        assert_eq!(hashes(&build(&host, 0, TrendingWindow::Year)), ["0xc1", "0xc2"]);
        // Past the end of a ranking: an empty page, which ends the list.
        assert!(build(&host, 100, TrendingWindow::All).pool.is_empty());
        assert!(host.requests.borrow().iter().all(|url| !url.contains("/feed/trending")));
    }

    #[test]
    fn without_a_ranking_the_long_ranges_show_the_past_week_and_say_so() {
        for index in [None, Some(Rc::new(Memory::default())), Some(Rc::new(Memory { fail: true, ..Memory::default() }))]
        {
            let host = Rc::new(Indexed { index, requests: RefCell::new(Vec::new()) });
            let payload = build(&host, 0, TrendingWindow::Year);
            assert_eq!(hashes(&payload), ["0xd1"]);
            assert_eq!(payload.index, None);
            assert!(host.requests.borrow()[0].contains("time_window=7d"), "{:?}", host.requests.borrow());
        }
        // The week itself never asks the index.
        let host = Rc::new(Indexed { index: Some(filled()), requests: RefCell::new(Vec::new()) });
        assert_eq!(build(&host, 0, TrendingWindow::Week).index, None);
    }
}

#[cfg(test)]
mod engagement_tests {
    use std::cell::RefCell;

    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::{Endpoint, FetchError, Host, HttpRequest, HttpResponse};

    /// Hypersnap's `reactions/cast` for any cast: twelve people a minute
    /// apart, listed as Hypersnap lists them, shard by shard and each shard
    /// oldest first (the odd minutes, then the even ones). HTTP 500 for the
    /// lists in `failing`. The clock stands still, so buckets don't refill.
    struct Reactions {
        failing: Vec<(String, &'static str)>,
        requests: RefCell<Vec<String>>,
    }

    impl Reactions {
        fn new(failing: Vec<(String, &'static str)>) -> Rc<Reactions> {
            Rc::new(Reactions { failing, requests: RefCell::new(Vec::new()) })
        }

        fn sent(&self) -> usize {
            self.requests.borrow().len()
        }
    }

    impl Host for Reactions {
        fn now(&self) -> f64 {
            1_800_000_000_000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let path = request.url.trim_start_matches(hypersnap::BASE).to_owned();
            self.requests.borrow_mut().push(path.clone());
            let param = |key: &str| {
                path.split(['?', '&']).find_map(|pair| pair.strip_prefix(key)?.strip_prefix('=')).unwrap_or_default()
            };
            let (hash, kind) = (param("hash"), param("types"));
            let status = |status: u16| HttpResponse { status, retry_after: None, body: None };
            let response = if !path.starts_with("/v2/farcaster/reactions/cast?") || param("limit") != "100" {
                status(404)
            } else if self.failing.iter().any(|(h, k)| h == hash && *k == kind) {
                status(500)
            } else {
                let people: Vec<String> = (1..=12)
                    .filter(|i| i % 2 == 1)
                    .chain((1..=12).filter(|i| i % 2 == 0))
                    .map(|i| {
                        format!(
                            r#"{{"reaction_timestamp":"2026-10-17T10:{i:02}:00.000Z","user":{{"fid":{i},"username":"{kind}-{i}","profile":{{"bio":{{"text":"skipped"}}}}}}}}"#
                        )
                    })
                    .collect();
                let body = format!(r#"{{"reactions":[{}],"next":{{}}}}"#, people.join(","));
                HttpResponse { status: 200, retry_after: None, body: Some(body.into_bytes()) }
            };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn cards(n: u32) -> Vec<String> {
        (1..=n).map(|i| format!("0x{i:040x}")).collect()
    }

    /// A batch for `hashes`, each with likes and recasts.
    fn both(hashes: &[String]) -> EngagementRequest {
        EngagementRequest::for_casts(hashes.iter().map(|h| (h.as_str(), 3, 1)))
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

    fn preview<'a>(batch: &'a EngagementPayload, hash: &str, kind: ReactionKind) -> Option<&'a ReactionsPayload> {
        batch.previews.iter().find(|p| p.hash == hash && p.kind == kind)
    }

    #[test]
    fn a_full_batch_reads_each_list_once_well_within_the_subrequest_budget() {
        let host = Reactions::new(vec![]);
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let hashes = cards(api::ENGAGEMENT_CHUNK as u32);
        let request = both(&hashes);
        assert_eq!(request.len(), api::ENGAGEMENT_MAX_LISTS);
        assert_eq!(api::ENGAGEMENT_MAX_LISTS, 12);
        // The Worker reads the edge cache first, and writes it last.
        assert!(inv.take_subrequests(1));
        let built = build(&inv, &request).unwrap();
        assert!(inv.take_subrequests(1));
        // One Hypersnap read per list, and nothing else: with the cache read
        // and write, a full batch takes 14 of the 45 subrequests.
        assert_eq!(host.sent(), 12);
        assert_eq!(45 - inv.remaining_subrequests(), 14);
        let mut asked = host.requests.borrow().clone();
        asked.sort();
        asked.dedup();
        assert_eq!(asked.len(), 12, "each list is read once");
        assert_eq!((built.status, built.policy), (200, api::ENGAGEMENT_CACHE));

        let batch = payload(&built);
        assert_eq!(batch.previews.len(), 12);
        // In the request's order, each preview as the single endpoint
        // answers it: the ten newest across shards, and that there are more.
        let order: Vec<(&str, ReactionKind)> = batch.previews.iter().map(|p| (p.hash.as_str(), p.kind)).collect();
        assert_eq!(order, request.lists().collect::<Vec<_>>());
        let likes = preview(&batch, &hashes[0], ReactionKind::Likes).unwrap();
        assert_eq!(likes.users.iter().map(|u| u.fid).collect::<Vec<_>>(), [12, 11, 10, 9, 8, 7, 6, 5, 4, 3]);
        assert!(likes.more);
        assert_eq!(likes.users[0].username, "likes-12");
        assert_eq!(preview(&batch, &hashes[5], ReactionKind::Recasts).unwrap().users[0].username, "recasts-12");
        let single = reactions(Rc::new(Invocation::new(host.clone(), 45)), hashes[0].clone(), ReactionKind::Likes, 10)
            .now_or_never()
            .unwrap()
            .unwrap();
        let single: ReactionsPayload = serde_json::from_slice(&single.body).unwrap();
        assert_eq!(&single, likes);
    }

    #[test]
    fn the_static_copy_builds_a_full_batch_within_its_budget() {
        // In the browser there is no edge cache, and 30 upstream calls a page.
        let host = Reactions::new(vec![]);
        let inv = Rc::new(Invocation::new(host.clone(), 30));
        let built = build(&inv, &both(&cards(api::ENGAGEMENT_CHUNK as u32))).unwrap();
        assert_eq!((built.policy, payload(&built).previews.len()), (api::ENGAGEMENT_CACHE, 12));
        assert_eq!(inv.remaining_subrequests(), 18);
    }

    #[test]
    fn a_list_that_fails_leaves_the_others_and_the_batch_is_kept_briefly() {
        let hashes = cards(3);
        let host = Reactions::new(vec![(hashes[1].clone(), "likes")]);
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let built = build(&inv, &both(&hashes)).unwrap();
        assert_eq!(host.sent(), 6);
        assert_eq!((built.status, built.policy), (200, api::ENGAGEMENT_PARTIAL_CACHE));
        let batch = payload(&built);
        assert_eq!(batch.previews.len(), 5);
        assert!(preview(&batch, &hashes[1], ReactionKind::Likes).is_none());
        assert!(preview(&batch, &hashes[1], ReactionKind::Recasts).is_some());
        for hash in [&hashes[0], &hashes[2]] {
            assert_eq!(preview(&batch, hash, ReactionKind::Likes).unwrap().users.len(), 10);
        }

        // Nothing read at all is an error: the last good copy is served, or
        // the popovers load their lists when they open.
        let host = Reactions::new(vec![(hashes[0].clone(), "likes")]);
        let inv = Rc::new(Invocation::new(host, 45));
        let only = EngagementRequest::for_casts([(hashes[0].as_str(), 1, 0)]);
        assert_eq!(build(&inv, &only).err().map(|f| f.status), Some(502));
    }

    #[test]
    fn previews_leave_hypersnaps_reserve_to_page_builds() {
        // The isolate's bucket holds 40 Hypersnap requests, and the clock
        // stands still: 12 for a full batch, then 4 for two cards.
        let host = Reactions::new(vec![]);
        let hashes = cards(20);
        let batch =
            |range: std::ops::Range<usize>| build(&Rc::new(Invocation::new(host.clone(), 45)), &both(&hashes[range]));
        assert_eq!(batch(0..6).unwrap().policy, api::ENGAGEMENT_CACHE);
        assert_eq!(batch(6..8).unwrap().policy, api::ENGAGEMENT_CACHE);
        assert_eq!(host.sent(), 16);

        // 24 left, 8 above the reserve: the likes come first.
        let partial = batch(8..14).unwrap();
        assert_eq!((host.sent(), partial.policy), (24, api::ENGAGEMENT_PARTIAL_CACHE));
        let kinds: Vec<ReactionKind> = payload(&partial).previews.iter().map(|p| p.kind).collect();
        assert_eq!(kinds, [[ReactionKind::Likes; 6].as_slice(), &[ReactionKind::Recasts; 2]].concat());

        // At the reserve: refused without a request, as busy.
        let refused = batch(14..20);
        assert_eq!(refused.err().map(|f| (f.status, f.retry_after)), Some((503, Some(1))));
        assert_eq!(host.sent(), 24);
        // Page builds still have the reserve.
        assert_eq!(upstream::spare(&Invocation::new(host.clone(), 45), Upstream::Hypersnap, 0), PREVIEW_RESERVE);
    }
}

#[cfg(test)]
mod wallet_tests {
    use super::*;

    #[test]
    fn wallet_sources_say_when_solana_went_uncounted() {
        let solana = |error: Option<&str>| ChainActivity {
            chain: "solana".into(),
            name: "Solana".into(),
            kind: ChainKind::Solana,
            count: 0,
            capped: false,
            percent: 0,
            error: error.map(str::to_owned),
            carried: false,
        };
        let unread = wallet_sources(true, true, false, &[solana(Some("upstream returned HTTP 403"))]);
        assert_eq!(unread.len(), 2);
        assert!(unread[1].starts_with("Solana: couldn't be read just now"), "{}", unread[1]);
        let read = wallet_sources(false, true, false, &[solana(None)]);
        assert!(read[0].contains("read from Solana's public RPC"), "{}", read[0]);
        let keyed = wallet_sources(false, true, true, &[solana(None)]);
        assert!(keyed[0].contains("keyed Solana RPC endpoint"), "{}", keyed[0]);
        assert!(wallet_sources(false, false, false, &[]).is_empty());
    }
}
