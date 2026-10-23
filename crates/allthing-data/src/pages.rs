// SPDX-License-Identifier: AGPL-3.0-or-later
//! One payload per page, assembled from upstream reads.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use allthing_core::activity::AppsWindow;
use allthing_core::api::{
    self, Annotations, CastPayload, ChannelPayload, ChannelsPayload, EngagementPayload, EngagementRequest, FollowsDir,
    FollowsPayload, HomePayload, IndexCoverage, LatestPayload, Meta, QuotedCast, ReactionKind, ReactionsPayload,
    RepliesPayload, SearchPayload, UserPayload,
};
use allthing_core::channels::{channel_id_for_url, channel_url_for_id, is_channel_id};
use allthing_core::embeds::link_key;
use allthing_core::emoji::ReactionReply;
use allthing_core::following::{self, FollowingPayload};
use allthing_core::follows;
use allthing_core::index::{Ranking, Span};
use allthing_core::latest::{self, Cursor};
use allthing_core::model::{Cast, Channel};
use allthing_core::normalize::{self, RawAppChannel, RawCast, RawFeed};
use allthing_core::notifications::{self, Event, Kind, NotificationsPayload, Recasts};
use allthing_core::params::fid_username;
use allthing_core::poll::{self, Counts};
use allthing_core::profile::UserCard;
use allthing_core::provenance::Backend;
use allthing_core::quotes::{self, QUOTE_DEPTH};
use allthing_core::ranking::TrendingWindow;
use allthing_core::thread::{
    chain_in_order, flatten, mark_mini_apps, mark_mini_apps_in_tree, prune_spam, reply_preview, without_hidden,
    without_spam,
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
/// Preloaded previews never take Hypersnap's per-isolate bucket (40 tokens)
/// below this, so page builds in the same isolate keep their share: a full
/// batch (18 reads) fits above it.
pub const PREVIEW_RESERVE: u32 = 16;
/// A Worker keeps at most six connections open at once, so a batch's lists
/// are read six at a time: a queued fetch would spend its timeout waiting.
const PARALLEL: usize = 6;
/// Upstream JSON a batch of previews parses at most, in bytes: about what
/// its twelve lists come as when every list is full (1.4 MB on 21 October
/// 2026), which takes about 3 ms of CPU natively (docs/BUDGET.md). Its
/// replies take what its lists leave, the smallest conversations first,
/// each byte counting [`CONVERSATION_WEIGHT`]; a card whose replies don't
/// fit is left out, and its popover reads them when it opens. So a batch
/// with replies costs no more CPU than one of full lists did.
pub const BATCH_PARSE_BYTES: usize = 1536 * 1024;
/// What a byte of conversation costs to parse, in bytes of reaction list:
/// its replies are whole casts, normalized and checked for spam and mini
/// apps, where a list keeps names and cards (about 5.3 µs a KB natively,
/// against 2.1).
pub const CONVERSATION_WEIGHT: usize = 3;

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
/// mark the mini app casts, and leave out spam. A feed (`keep_apps`) keeps
/// the mini app casts, marked, for the browser to show to the readers who
/// chose to see them and leave out for the others; search leaves them out.
async fn drop_hidden(
    inv: &Invocation,
    mut casts: Vec<Cast>,
    notes: &mut api::Annotations,
    known_links: &BTreeMap<String, bool>,
    keep_apps: bool,
) -> Vec<Cast> {
    notes.links = miniapps::check(inv, &casts, known_links).await;
    mark_mini_apps(&mut casts, &notes.links);
    if keep_apps { without_spam(casts) } else { without_hidden(casts) }
}

/// Keep what a listing's notes need for the casts it shows, once the hidden
/// ones are left out: the quotes those casts reach (at any depth), their
/// origins, and the apps of those origins.
fn keep_shown(notes: &mut api::Annotations, shown: &[Cast]) {
    let mut reached: HashSet<String> = HashSet::new();
    let mut level: Vec<&Cast> = shown.iter().collect();
    while !level.is_empty() {
        level = level
            .iter()
            .flat_map(|c| c.quoted())
            .filter(|hash| reached.insert((*hash).to_owned()))
            .filter_map(|hash| notes.quotes.get(hash))
            .collect();
    }
    notes.quotes.retain(|hash, _| reached.contains(hash));
    let listed: HashSet<&str> = shown.iter().map(|c| c.hash.as_str()).collect();
    notes.origins.retain(|hash, _| listed.contains(hash.as_str()));
    let apps: HashSet<u64> = notes.origins.values().filter_map(|o| o.app_fid).collect();
    notes.apps.retain(|fid, _| apps.contains(fid));
    notes.pending = shown.iter().filter(|c| !notes.origins.contains_key(&c.hash)).count() as u32;
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

/// Check a quote's own links before stripping spam. The outer cast stays
/// visible; a spam quote keeps the author and the hidden reason, a mini app
/// quote its content, marked, for the browser to show or not.
async fn check_quotes<'a>(
    inv: &Invocation,
    casts: impl IntoIterator<Item = &'a mut Cast>,
    known_links: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    let casts: Vec<&mut Cast> = casts.into_iter().collect();
    let links = miniapps::check(inv, casts.iter().map(|c| &**c), known_links).await;
    for cast in casts {
        mark_mini_apps(std::slice::from_mut(cast), &links);
        if cast.spam {
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
    let reactions = read_reactions(inv, pool, std::mem::take(&mut notes.reactions));
    let ((quotes, links, cards), (), reactions) = join3(
        async {
            let (mut quotes, cards) = quoted_casts(inv, pool, known).await;
            let links = check_quotes(inv, quotes.values_mut(), known_links).await;
            (quotes, links, cards)
        },
        annotate(inv, notes, &targets(&refs), max_authors),
        reactions,
    )
    .await;
    notes.quotes = quotes;
    notes.links.extend(links);
    notes.reactions = reactions;
    cards
}

/// The emoji reactions the cron keeps to those of `pool`'s casts that have
/// replies, by cast, oldest first: one D1 read, its subrequest taken at
/// once, so origins size their batch to what is left. A read that can't be
/// made or fails leaves those carried from the last build (`carried`) for
/// the casts still listed. None without an index: the static copy.
fn read_reactions(
    inv: &Invocation,
    pool: &[Cast],
    carried: BTreeMap<String, Vec<ReactionReply>>,
) -> impl Future<Output = BTreeMap<String, Vec<ReactionReply>>> + 'static {
    let casts: Vec<String> = pool.iter().filter(|c| c.replies > 0).map(|c| c.hash.clone()).collect();
    let index = match inv.index() {
        Some(index) if !casts.is_empty() && inv.take_subrequests(1) => Some(index),
        _ => None,
    };
    async move {
        let read = match index {
            Some(index) => index.reactions(casts.clone()).await.ok(),
            None => None,
        };
        match read {
            Some(rows) => {
                let mut by_cast: BTreeMap<String, Vec<ReactionReply>> = BTreeMap::new();
                for (cast, reply) in rows {
                    by_cast.entry(cast).or_default().push(reply);
                }
                by_cast
            }
            None => carried.into_iter().filter(|(hash, _)| casts.contains(hash)).collect(),
        }
    }
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

/// The time now in seconds since the Unix epoch, as the casts' times count.
fn now_secs(inv: &Invocation) -> i64 {
    (inv.now() / 1000.0) as i64
}

/// The casts posted within `window` at `now`; all time still excludes the future.
fn within(window: TrendingWindow, now: i64, casts: Vec<Cast>) -> Vec<Cast> {
    casts.into_iter().filter(|cast| window.holds(cast.ts, now)).collect()
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
    // Only the first page heads the list with the client bar, and carries
    // the poll's counts.
    let counted = async { if offset == 0 { apps_24h(&inv).await } else { None } };
    let polled = async { if offset == 0 { poll_counts(&inv).await } else { None } };
    if let Some(span) = Span::of(window)
        && let Some(ranking) = ranking(&inv, span).await
    {
        let hashes = ranking.page(offset as usize, POOL_SIZE as usize);
        let (ranked, counted, poll) = join3(ranked_casts(&inv, &hashes), counted, polled).await;
        let (raw, casts) = ranked?;
        let casts = within(window, now_secs(&inv), casts);
        let pool = drop_hidden(&inv, casts, &mut notes, &known_links, true).await;
        let quoted = annotate_listing(&inv, &mut notes, &pool, ORIGIN_AUTHORS, &known_links).await;
        notes.keep_users(read_cards(&raw, quoted), listed_accounts(&pool, &notes.quotes));
        let index = IndexCoverage { casts: ranking.total, since: ranking.oldest, ranked_at: ranking.built };
        let apps_24h = counted.map(|w| w.only(|app| notes.apps.contains_key(&app)));
        let payload = HomePayload { pool, notes, meta: meta(&inv), index: Some(index), apps_24h, poll };
        return Built::json(&payload, api::INDEXED_HOME_CACHE);
    }
    let shown = if window.is_indexed() { TrendingWindow::Week } else { window };
    // Hypersnap's trending lists run hours behind, so the first page also
    // takes the newest casts the cron stores each minute: one read.
    let newest = async {
        if offset > 0 || window.is_indexed() {
            return None;
        }
        let stored = stored(&inv, LATEST_SNAPSHOT).await?;
        serde_json::from_str::<LatestPayload>(&stored.body).ok()
    };
    let ((trending, newest), counted, poll) =
        join3(join(hypersnap::trending_casts(&inv, POOL_SIZE, offset, shown), newest), counted, polled).await;
    let raw = trending.map_err(Failure::upstream)?;
    // A source read can include casts posted after this request began.
    // Judge both edges at its completion, not before awaiting the response.
    let now = now_secs(&inv);
    // Hypersnap's list can hold casts older than the window (its 12-hour
    // list is its 24-hour one), so only those posted within it are kept,
    // before the page is cut. A page short of casts is not topped up with
    // older ones: the list ends, and the page says how many there are.
    let (raw, casts): (Vec<RawCast>, Vec<Cast>) = raw
        .into_iter()
        .map(|raw| {
            let cast = normalize::cast(&raw);
            (raw, cast)
        })
        .filter(|(_, cast)| shown.holds(cast.ts, now))
        .unzip();
    let pool = drop_hidden(&inv, casts, &mut notes, &known_links, true).await;
    // The stored newest casts are spam-filtered, with mini apps marked for the reader's preference.
    let pool = match newest {
        Some(newest) => {
            notes.merge(&newest.notes);
            latest::with_newest(pool, newest.pool, shown, now)
        }
        None => pool,
    };
    let quoted = annotate_listing(&inv, &mut notes, &pool, ORIGIN_AUTHORS, &known_links).await;
    notes.keep_users(read_cards(&raw, quoted), listed_accounts(&pool, &notes.quotes));
    let apps_24h = counted.map(|w| w.only(|app| notes.apps.contains_key(&app)));
    Built::json(&HomePayload { pool, notes, meta: meta(&inv), index: None, apps_24h, poll }, api::home_cache(window))
}

/// A signed-in reader's front page (`allthing_core::following`): the
/// top-level casts of the accounts `fid` follows, newest first, for the app to
/// rank. A first page with few reads Hypersnap's next page too. Only their
/// casts: no trending casts top it up. Mini apps and spam are left out.
pub async fn following(
    inv: Rc<Invocation>,
    fid: u64,
    cursor: Option<String>,
    previous: Option<Rc<Vec<u8>>>,
) -> Result<Built, Failure> {
    let first = cursor.is_none();
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let known_links = std::mem::take(&mut notes.links);
    let top_level = |raw: &[RawCast]| raw.iter().map(normalize::cast).filter(|c| !c.is_reply()).collect::<Vec<_>>();
    let (mut raw, mut next) =
        hypersnap::following_feed(&inv, fid, cursor.as_deref()).await.map_err(Failure::upstream)?;
    let mut theirs = top_level(&raw);
    if following::wants_second_page(first, theirs.len(), next.as_deref())
        && let Ok((more, after)) = hypersnap::following_feed(&inv, fid, next.as_deref()).await
    {
        theirs.extend(top_level(&more));
        raw.extend(more);
        next = after;
    }
    // Keep the existing page-read decision: validation must not fetch another
    // upstream page merely to replace rows with invalid future timestamps.
    // Use the current clock after both reads: otherwise an actually present
    // cast created during I/O could be dropped while its cursor advances.
    let theirs = within(TrendingWindow::All, now_secs(&inv), theirs);
    let pool = drop_hidden(&inv, theirs, &mut notes, &known_links, true).await;
    let quoted = annotate_listing(&inv, &mut notes, &pool, ORIGIN_AUTHORS, &known_links).await;
    notes.keep_users(read_cards(&raw, quoted), listed_accounts(&pool, &notes.quotes));
    let payload = FollowingPayload { pool, notes, meta: meta(&inv), next };
    Built::json(&payload, following::FOLLOWING_CACHE)
}

/// How many voted each way in Allthing's poll, so a reader who voted sees
/// them with no request of their own: one D1 read of the two counts its
/// votes keep, given an index and a subrequest for it. `None` before anyone
/// has voted, or when the read can't be made.
async fn poll_counts(inv: &Invocation) -> Option<Counts> {
    let index = inv.index()?;
    if !inv.take_subrequests(1) {
        return None;
    }
    index.poll(poll::MOBILE_CLIENT).await.ok().filter(|counts| counts.total() > 0)
}

/// Accounts active by app in the past 24 hours, as the cron last counted
/// them: one D1 read, given an index and a subrequest for it.
async fn apps_24h(inv: &Invocation) -> Option<AppsWindow> {
    #[derive(serde::Deserialize)]
    struct Counted {
        apps_24h: Option<AppsWindow>,
    }
    let index = inv.index()?;
    if !inv.take_subrequests(1) {
        return None;
    }
    let stored = index.snapshot(crate::activity::SUMMARY).await.ok().flatten()?;
    serde_json::from_str::<Counted>(&stored.body).ok()?.apps_24h
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
            let mut payload: LatestPayload = serde_json::from_str(&stored.body)
                .map_err(|e| Failure::internal(&format!("invalid newest-casts snapshot: {e}")))?;
            let before = payload.pool.len();
            payload.pool.retain(|cast| TrendingWindow::All.holds(cast.ts, now_secs(&inv)));
            // Preserve the exact stored bytes on the common unchanged path.
            let body = if payload.pool.len() == before {
                stored.body.into_bytes()
            } else {
                serde_json::to_vec(&payload).map_err(|e| Failure::internal(&e.to_string()))?
            };
            return Ok(Built { body, status: 200, policy: api::LATEST_CACHE, fetched_at: Some(stored.built) });
        }
        warm = serde_json::from_str(&stored.body).ok();
    }
    let mut payload = build_latest(&inv, warm).await?;
    payload.pool.retain(|cast| TrendingWindow::All.holds(cast.ts, now_secs(&inv)));
    Built::json(&payload, api::LATEST_CACHE)
}

/// The cron's turn: the newest casts, built on the stored copy and stored
/// again, so that every data center has a copy at most about a minute old
/// without a reader waiting for a build.
pub async fn store_latest(inv: Rc<Invocation>) -> Result<(), Failure> {
    let index = inv.index().ok_or_else(|| Failure::internal("no index to store the newest casts in"))?;
    let warm = stored(&inv, LATEST_SNAPSHOT).await.and_then(|s| serde_json::from_str(&s.body).ok());
    // Keep the bounded ingest buffer for the cron; the public `latest` view
    // excludes every future timestamp, without mutating this stored source.
    let payload = build_latest(&inv, warm).await?;
    let body = serde_json::to_string(&payload).map_err(|e| Failure::internal(&e.to_string()))?;
    if !inv.take_subrequests(1) {
        return Err(Failure::internal("no subrequest left to store the newest casts"));
    }
    let snapshot = Snapshot { built: inv.now(), body };
    index.save_snapshot(LATEST_SNAPSHOT, snapshot).await.map_err(|e| Failure::internal(&e.to_string()))
}

/// Farcaster's daily active accounts: the copy the cron stores every two
/// minutes (`activity::run`), as it is. One read; before the first count,
/// or without an index (the static copy), 503.
pub async fn activity(inv: Rc<Invocation>) -> Result<Built, Failure> {
    let Some(stored) = stored(&inv, crate::activity::SUMMARY).await else {
        return Err(Failure {
            status: 503,
            message: "Active accounts aren't counted yet".into(),
            retry_after: Some(120),
        });
    };
    let body = stored.body.into_bytes();
    Ok(Built { body, status: 200, policy: api::ACTIVITY_CACHE, fetched_at: Some(stored.built) })
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

async fn build_latest(inv: &Invocation, warm: Option<LatestPayload>) -> Result<LatestPayload, Failure> {
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
    let fresh = drop_hidden(inv, fresh, &mut notes, &known_links, true).await;
    let pool = latest::newest(listed, fresh, &removed, POOL_SIZE as usize, now_secs(inv));
    let quoted = annotate_listing(inv, &mut notes, &pool, ORIGIN_AUTHORS, &known_links).await;
    notes.keep_users(read_cards(&raw, quoted), listed_accounts(&pool, &notes.quotes));
    Ok(LatestPayload { pool, notes, cursors: next, meta: meta(inv) })
}

/// The pages with no casts but a cursor for more that a channel page follows
/// to the channel's newest casts: Hypersnap sends /veg's after two.
const EMPTY_PAGES_FOLLOWED: usize = 3;

/// The casts of the first page with any, from `page` on, following the
/// cursor of up to `EMPTY_PAGES_FOLLOWED` pages with none. Still none, the
/// read fails, and the cache keeps serving the page's last good copy.
async fn newest_casts(
    inv: &Invocation,
    parent_url: &str,
    mut page: Result<RawFeed, crate::upstream::UpstreamError>,
) -> Result<Vec<RawCast>, crate::upstream::UpstreamError> {
    for _ in 0..EMPTY_PAGES_FOLLOWED {
        let feed = match page {
            Ok(feed) if feed.is_empty_with_more() => feed,
            other => return other.map(|feed| feed.casts),
        };
        page = hypersnap::channel_page(inv, parent_url, POOL_SIZE / 2, feed.cursor()).await;
    }
    match page? {
        feed if feed.is_empty_with_more() => {
            Err(crate::upstream::UpstreamError::Parse("channel pages with no casts but a cursor for more".into()))
        }
        feed => Ok(feed.casts),
    }
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
        hypersnap::channel_page(&inv, &guess, POOL_SIZE / 2, None),
        app_channel(&inv, app_id.as_deref()),
    )
    .await;
    let Some(mut channel) = channel.map_err(Failure::upstream)? else {
        return Ok(Built::not_found(allthing_core::api::NO_SUCH_CHANNEL));
    };
    // Then, alongside the feed when the guess was wrong, the accounts the
    // app's record names (one `user/bulk`) and its pinned cast. Both are
    // best-effort: what can't be read is left out of the page.
    let fids = app.as_ref().map(RawAppChannel::fids).unwrap_or_default();
    let pinned_hash = app.as_ref().and_then(RawAppChannel::pinned_hash);
    let (raw, users, pinned) = join3(
        async {
            let first = match guessed {
                Ok(page) if channel.parent_url == guess => Ok(page),
                _ => hypersnap::channel_page(&inv, &channel.parent_url, POOL_SIZE / 2, None).await,
            };
            newest_casts(&inv, &channel.parent_url, first).await
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
    // left out like any listed cast if it is spam, and kept marked if it is
    // a mini app.
    notes.links = miniapps::check(&inv, pinned_cast.iter().chain(&casts), &known_links).await;
    mark_mini_apps(&mut pinned_cast, &notes.links);
    mark_mini_apps(&mut casts, &notes.links);
    channel.pinned = without_spam(pinned_cast).pop();
    let pool = newest_unique(without_spam(casts), POOL_SIZE as usize);
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

/// The most casts above a reply that a post page reads: one call each, in
/// turn, from the reply's parent up.
const MAX_ANCESTORS: usize = 8;

/// The casts above `start`, nearest first, and whether casts above them
/// couldn't be read: a call failed or found no cast, or the limit held.
async fn walk_ancestors(inv: &Invocation, start: Option<String>) -> (Vec<RawCast>, bool) {
    let mut found = Vec::new();
    let mut next = start;
    while let Some(hash) = next.take() {
        if found.len() == MAX_ANCESTORS {
            return (found, true);
        }
        match hypersnap::cast(inv, &hash).await {
            Ok(Some(raw)) => {
                next = normalize::cast(&raw).parent_hash;
                found.push(raw);
            }
            _ => return (found, true),
        }
    }
    (found, false)
}

pub async fn cast(inv: Rc<Invocation>, hash: String, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    let Some(root) = hypersnap::conversation(&inv, &hash, 5).await.map_err(Failure::upstream)? else {
        return Ok(Built::not_found(allthing_core::api::NO_SUCH_CAST));
    };
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let known_links = std::mem::take(&mut notes.links);
    let mut thread = normalize::thread(&root);
    let parent_hash = thread.cast.parent_hash.clone();
    let channel_key = thread.cast.channel.as_ref().map(|c| c.id.clone());
    // Context around the cast is best-effort; the thread is what matters.
    // The chain above is read in turn, and the link checks over the thread
    // and the chain start as it's read, so they take their share first.
    let (channel, (raw_ancestors, mut above, earlier, links)) = join(
        async {
            match &channel_key {
                Some(key) => hypersnap::channel(&inv, key).await.ok().flatten(),
                None => None,
            }
        },
        async {
            let (raw, earlier) = walk_ancestors(&inv, parent_hash).await;
            let above: Vec<Cast> = raw.iter().map(normalize::cast).collect();
            let links = miniapps::check(&inv, flatten(&thread).into_iter().chain(&above), &known_links).await;
            (raw, above, earlier, links)
        },
    )
    .await;
    notes.links = links;
    mark_mini_apps_in_tree(&mut thread, &notes.links);
    mark_mini_apps(&mut above, &notes.links);
    // Spam is pruned here; mini app replies stay whole, marked, for the
    // browser to prune as the reader chose (`thread::prune_hidden_for`).
    thread.children = std::mem::take(&mut thread.children).into_iter().filter_map(prune_spam).collect();
    if thread.cast.spam {
        thread.cast.strip_hidden();
    }
    let ancestors = chain_in_order(above);
    let first = &thread.cast;
    let quote_hashes: Vec<String> = first.quoted().take(3).map(str::to_owned).collect();
    let raw_quotes = join_all(quote_hashes.iter().map(|hash| {
        let inv = &inv;
        async move { hypersnap::cast(inv, hash).await.ok().flatten() }
    }))
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

    // Attribute the cast itself first, then the casts above it, then the
    // quotes and the replies.
    let mut refs: Vec<&Cast> = vec![first];
    refs.extend(ancestors.iter());
    refs.extend(quotes.iter().filter_map(|q| q.cast.as_ref()));
    refs.extend(flatten(&thread).into_iter().skip(1));
    annotate(&inv, &mut notes, &targets(&refs), ORIGIN_AUTHORS).await;
    // Cards for the accounts the page names: the thread's, the chain's and
    // the quotes' authors.
    let named: Vec<u64> = flatten(&thread)
        .into_iter()
        .chain(ancestors.iter())
        .chain(quotes.iter().filter_map(|q| q.cast.as_ref()))
        .map(|c| c.author.fid)
        .collect();
    let cards = std::iter::once(&root).chain(raw_quotes.iter().flatten()).chain(raw_ancestors.iter());
    notes.keep_users(normalize::author_cards(cards), named);
    Built::json(&CastPayload { thread, ancestors, earlier, quotes, channel, notes, meta: meta(&inv) }, api::CAST_CACHE)
}

pub async fn user(inv: Rc<Invocation>, username: String, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    let raw = match fid_username(&username) {
        Some(fid) => hypersnap::users_bulk(&inv, &[fid]).await.map_err(Failure::upstream)?.into_iter().next(),
        None => hypersnap::user_by_username(&inv, &username).await.map_err(Failure::upstream)?,
    };
    let Some(raw) = raw else {
        return Ok(Built::not_found(allthing_core::api::NO_SUCH_USER));
    };
    let mut user = normalize::user(&raw);
    let fid = user.fid;

    // The casts, the account's signers and its casts' signers in parallel:
    // two requests attribute every cast on the page. Alongside, the
    // account's website, and for an account registered before the ID
    // registry's migration (which reports the migration's day), when it got
    // its first username.
    let before_migration = user.registered_before_migration();
    let ((casts, signers, recent), (first_username_at, website)) = join(
        join3(
            hypersnap::user_casts(&inv, fid, POOL_SIZE),
            hypersnap::active_signers(&inv, fid),
            hypersnap::recent_cast_signers(&inv, fid, POOL_SIZE),
        ),
        join(
            async { if before_migration { Some(hypersnap::first_username_at(&inv, fid).await) } else { None } },
            hypersnap::website(&inv, fid),
        ),
    )
    .await;
    // A failed read keeps what the previous copy knew, and this copy is
    // refreshed soon instead of in five minutes. Neither fails the profile.
    let retry = matches!(first_username_at, Some(Err(_))) || website.is_err();
    user.first_username_at = match first_username_at {
        Some(Ok(ts)) => ts,
        Some(Err(_)) => previous.as_deref().and_then(|p| api::first_username_at_in(p, fid)),
        None => None,
    };
    user.website = website.unwrap_or_else(|_| previous.as_deref().and_then(|p| api::website_in(p, fid)));
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let known_links = std::mem::take(&mut notes.links);
    let mut casts =
        newest_unique(casts.map_err(Failure::upstream)?.iter().map(normalize::cast).collect(), POOL_SIZE as usize);
    let active = signers.ok().map(|apps| annotate::remember_signers(&inv, fid, apps));
    if let Ok(recent) = recent {
        let cast_signers: Vec<(String, String)> =
            casts.iter().filter_map(|c| recent.get(&c.hash).map(|s| (c.hash.clone(), s.clone()))).collect();
        for (hash, origin) in annotate::attribute(&inv, fid, &cast_signers, active).await {
            notes.origins.insert(hash, origin);
        }
    }
    // The link checks wait on the slowest link page: the quotes and app
    // names are read meanwhile, for every cast read, and what only the
    // hidden casts needed is dropped after. The checks are polled first, so
    // they take their share of the budget before the quotes and origins
    // size theirs, as when they ran first.
    let (links, quoted) =
        join(miniapps::check(&inv, &casts, &known_links), annotate_listing(&inv, &mut notes, &casts, 2, &known_links))
            .await;
    mark_mini_apps(&mut casts, &links);
    let pool = without_spam(casts);
    let quote_links = std::mem::replace(&mut notes.links, links);
    notes.links.extend(quote_links);
    keep_shown(&mut notes, &pool);
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
    let casts =
        drop_hidden(&inv, hydrated.iter().map(normalize::cast).collect(), &mut notes, &BTreeMap::new(), false).await;
    let users: Vec<_> = users.unwrap_or_default().iter().map(normalize::user).collect();
    let channels: Vec<Channel> = channels.unwrap_or_default().iter().map(searched_channel).take(10).collect();
    let (quotes, quoted) = quoted_casts(&inv, &casts, BTreeMap::new()).await;
    notes.quotes = quotes;
    notes.links.extend(check_quotes(&inv, notes.quotes.values_mut(), &BTreeMap::new()).await);
    // The people found are whole profiles already; the casts' authors get cards.
    notes.keep_users(read_cards(&hydrated, quoted), listed_accounts(&casts, &notes.quotes));
    Built::json(&SearchPayload { query, casts, users, channels, notes, meta: meta(&inv) }, api::SEARCH_CACHE)
}

/// Links a followers read brings from each shard: two shards, so up to 100
/// people a page. Whom an account follows is in one shard: 100 a page.
pub const FOLLOWERS_PAGE: usize = 50;
pub const FOLLOWING_PAGE: usize = 100;

/// A page of `name`'s followers or following, the newest follow first: the
/// first, or the one `cursor` names (the previous page's `next`). Its links
/// come from the node ([`hypersnap::follow_links`]), put in order across the
/// node's shards ([`follows::merge`]), and its people from one `user/bulk`
/// read, which also brings the account itself on a first page asked for by
/// fid. A first page asked for by name looks the account up first; later
/// pages name it `fid:<n>`.
pub async fn follows(
    inv: Rc<Invocation>,
    name: String,
    dir: FollowsDir,
    cursor: Option<String>,
) -> Result<Built, Failure> {
    let before = match cursor.as_deref() {
        Some(cursor) => Some(follows::Paging::decode(cursor).ok_or_else(|| Failure {
            status: 400,
            message: "Expected ?cursor=<the previous page's next>".into(),
            retry_after: None,
        })?),
        None => None,
    };
    let (fid, raw_owner) = match fid_username(&name) {
        Some(fid) => (fid, None),
        None => match hypersnap::user_by_username(&inv, &name).await.map_err(Failure::upstream)? {
            Some(raw) => (raw.fid, Some(raw)),
            None => return Ok(Built::not_found("No such user")),
        },
    };
    let page_size = match dir {
        FollowsDir::Followers => FOLLOWERS_PAGE,
        FollowsDir::Following => FOLLOWING_PAGE,
    };
    let ask = before.as_ref().map(|paging| paging.ask(page_size));
    // Nothing left to read, only links waiting to be listed.
    let read = match &ask {
        Some(ask) if ask.iter().all(Option::is_none) => {
            follows::Read { links: Vec::new(), next: vec![None; ask.len()] }
        }
        _ => hypersnap::follow_links(&inv, dir, fid, page_size, ask.as_deref()).await.map_err(Failure::upstream)?.0,
    };
    let merged = follows::merge(before.as_ref(), read, page_size);
    // An account may follow itself (the oldest do): not listed.
    let listed: Vec<follows::Link> = merged.listed.into_iter().filter(|link| link.fid != fid).collect();
    let first = before.is_none();
    let mut fids: Vec<u64> = listed.iter().map(|link| link.fid).collect();
    if first && raw_owner.is_none() {
        fids.push(fid);
    }
    let users = hypersnap::users_bulk(&inv, &fids).await.map_err(Failure::upstream)?;
    let by_fid: HashMap<u64, &normalize::RawUser> = users.iter().map(|user| (user.fid, user)).collect();
    let owner = match first {
        true => match raw_owner.as_ref().or_else(|| by_fid.get(&fid).copied()) {
            Some(raw) => Some(normalize::follows_owner(raw)),
            None => return Ok(Built::not_found("No such user")),
        },
        false => None,
    };
    // An account the node lists but `user/bulk` doesn't know is left out.
    let people = listed
        .iter()
        .filter_map(|link| by_fid.get(&link.fid).map(|raw| normalize::person(raw, follows::unix(link.ts))))
        .collect();
    let next = merged.paging.map(|paging| paging.encode());
    Built::json(&FollowsPayload { dir, owner, people, next, meta: meta(&inv) }, api::FOLLOWS_CACHE)
}

/// Who reacted to a cast, newest first: at most `limit` people out of the
/// 100 that Hypersnap lists, as `/api/v1/reactions` answers; and the bytes
/// the list came as.
async fn reactors(
    inv: &Invocation,
    hash: &str,
    kind: ReactionKind,
    limit: u32,
) -> Result<(ReactionsPayload, usize), UpstreamError> {
    let (raw, bytes) = hypersnap::reactions(inv, hash, kind.as_str(), 100).await?;
    let (users, more) = normalize::reactors(&raw, limit as usize);
    let cards = normalize::reactor_cards(&raw, &users);
    Ok((ReactionsPayload { hash: hash.to_owned(), kind, users, more, cards, meta: meta(inv) }, bytes))
}

/// Who liked or recasted a cast: a preview (`limit` 10) or the full list
/// (100, which is as many as Hypersnap returns).
pub async fn reactions(inv: Rc<Invocation>, hash: String, kind: ReactionKind, limit: u32) -> Result<Built, Failure> {
    let (payload, _) = reactors(&inv, &hash, kind, limit).await.map_err(Failure::upstream)?;
    Built::json(&payload, api::REACTIONS_CACHE)
}

/// The likes, recasts and replies of a few casts (`/api/v1/counts`): one
/// bulk read of the node's casts, the source the reader's notifications
/// come from, keeping only their counts.
pub async fn counts(inv: Rc<Invocation>, hashes: Vec<String>) -> Result<Built, Failure> {
    let casts = hypersnap::casts_bulk(&inv, &hashes).await.map_err(Failure::upstream)?;
    let counts: BTreeMap<_, _> = casts
        .iter()
        .map(|raw| (allthing_core::params::normalize_hash(&raw.hash), normalize::counts(raw)))
        .filter(|(hash, _)| hashes.contains(hash))
        .collect();
    Built::json(&api::CountsPayload { counts, meta: meta(&inv) }, api::COUNTS_CACHE)
}

/// Whether a preloaded preview may read now: Hypersnap's bucket holds more
/// than [`PREVIEW_RESERVE`] tokens, and the build keeps a subrequest for its
/// cache write. Checked as each read starts, so batches built side by side
/// in an isolate don't both count the same tokens.
fn preview_may_read(inv: &Invocation) -> bool {
    inv.remaining_subrequests() > 1 && upstream::spare(inv, Upstream::Hypersnap, PREVIEW_RESERVE) > 0
}

/// What one of a batch's reads asks for.
enum Ask<'a> {
    List(&'a str, ReactionKind),
    Replies(&'a str),
}

/// What one of a batch's reads brought.
enum Read {
    /// A like or recast list, and the bytes it came as.
    List(ReactionsPayload, usize),
    /// A cast's conversation, unparsed; `None` when there is no such cast.
    Replies(String, Option<Vec<u8>>),
}

/// Who liked and recasted a batch of feed cards, and their replies: each
/// list exactly as its own preview answers it ([`reactions`] with a limit
/// of 10), one `reactions/cast` read each, and each replies preview as
/// [`replies`] answers it, one `cast/conversation` read each, six reads at a
/// time. A list or replies preview that can't be read is left out without
/// failing the others, and the batch is then kept only briefly. Reads never
/// take Hypersnap's bucket below [`PREVIEW_RESERVE`]: with less room, a
/// batch reads what fits (likes first, then recasts, then replies) and
/// leaves the rest to the popovers, which load what they lack when they
/// open. So do replies that don't fit in the batch's [`BATCH_PARSE_BYTES`].
/// Replies are filtered as [`replies`] filters them: spam and mini apps,
/// by their address or their page, are left out.
pub async fn engagement(inv: Rc<Invocation>, request: EngagementRequest) -> Result<Built, Failure> {
    let asks = request.lists().map(|(hash, kind)| Ask::List(hash, kind));
    let asks = asks.chain(request.replies().iter().map(|hash| Ask::Replies(hash)));
    let read: Vec<Result<Read, UpstreamError>> = stream::iter(asks.map(|ask| {
        let inv = &inv;
        async move {
            if !preview_may_read(inv) {
                return Err(UpstreamError::Busy(1000.0));
            }
            match ask {
                Ask::List(hash, kind) => {
                    let (preview, bytes) = reactors(inv, hash, kind, api::REACTIONS_PREVIEW).await?;
                    Ok(Read::List(preview, bytes))
                }
                Ask::Replies(hash) => {
                    let body = hypersnap::conversation_body(inv, hash, 1).await?;
                    Ok(Read::Replies(hash.to_owned(), body))
                }
            }
        }
    }))
    .buffered(PARALLEL)
    .collect()
    .await;
    let mut previews = Vec::with_capacity(read.len());
    let mut parsed = 0;
    let mut conversations = Vec::new();
    let mut failure = None;
    for result in read {
        match result {
            Ok(Read::List(preview, bytes)) => {
                previews.push(preview);
                parsed += bytes;
            }
            Ok(Read::Replies(hash, Some(body))) => conversations.push((hash, body)),
            // No such cast: its popover says so if it opens.
            Ok(Read::Replies(_, None)) => {}
            Err(error) => failure = failure.or(Some(error)),
        }
    }
    let room = BATCH_PARSE_BYTES.saturating_sub(parsed) / CONVERSATION_WEIGHT;
    let (replies, unreadable) = batch_replies(&inv, conversations, room).await;
    let payload = EngagementPayload { previews, replies, meta: meta(&inv) };
    match failure.or(unreadable) {
        None => Built::json(&payload, api::ENGAGEMENT_CACHE),
        // Nothing read: every read failed, or the bucket was at its reserve.
        Some(error) if payload.previews.is_empty() && payload.replies.is_empty() => Err(Failure::upstream(error)),
        Some(_) => Built::json(&payload, api::ENGAGEMENT_PARTIAL_CACHE),
    }
}

/// Whether a failed `/engagement` build answers as [`engagement_skipped`]:
/// Hypersnap busy or failing, or its reads left to page builds (502 to
/// 504), not a fault of Allthing's own.
pub fn skips_engagement(failure: &Failure) -> bool {
    matches!(failure.status, 502..=504)
}

/// What `/engagement` answers when its batch couldn't be read and no copy
/// is kept ([`skips_engagement`]): an empty batch, as a 200, read at
/// `fetched_at` (Unix milliseconds). A batch is a background preload, and a
/// popover it would have filled asks for itself as it opens, so nothing is
/// lost; an error status would only be an error in the reader's console.
pub fn engagement_skipped(fetched_at: i64) -> Vec<u8> {
    let meta = Meta { backend: Backend::Hypersnap, fetched_at, stale: false };
    serde_json::to_vec(&EngagementPayload { previews: Vec::new(), replies: Vec::new(), meta }).unwrap_or_default()
}

/// The replies previews of a batch's conversations (by hash, unparsed), in
/// hash order: the smallest parsed first, while they fit in `room` bytes.
/// One that doesn't fit is left out, as it would be again next time; one
/// that doesn't parse is left out too, and reported.
async fn batch_replies(
    inv: &Invocation,
    mut conversations: Vec<(String, Vec<u8>)>,
    mut room: usize,
) -> (Vec<RepliesPayload>, Option<UpstreamError>) {
    // Stable: the same sizes keep their hash order.
    conversations.sort_by_key(|(_, body)| body.len());
    let mut roots = Vec::new();
    let mut unreadable = None;
    for (hash, body) in conversations {
        let Some(left) = room.checked_sub(body.len()) else { break };
        room = left;
        match hypersnap::parse_conversation(&body) {
            Ok(Some(root)) => roots.push((hash, root)),
            Ok(None) => {}
            Err(error) => unreadable = unreadable.or(Some(error)),
        }
    }
    let mut previews = replies_previews(inv, roots).await;
    previews.sort_by(|a, b| a.hash.cmp(&b.hash));
    (previews, unreadable)
}

/// Replies previews, each as `/api/v1/replies` answers it, of the casts
/// whose conversations were read (`roots`, by hash): the ten most engaged
/// direct replies, without mini apps or spam, with the verdicts on their
/// links and their authors' profile cards. The replies' links are checked
/// together, in one batch of the invocation's link checks.
async fn replies_previews(inv: &Invocation, roots: Vec<(String, RawCast)>) -> Vec<RepliesPayload> {
    let replies: Vec<Vec<Cast>> =
        roots.iter().map(|(_, root)| root.direct_replies.iter().flatten().map(normalize::cast).collect()).collect();
    let verdicts = miniapps::check(inv, replies.iter().flatten(), &BTreeMap::new()).await;
    roots
        .into_iter()
        .zip(replies)
        .map(|((hash, root), mut replies)| {
            // The verdicts on this cast's replies' links, as a check of its
            // own would answer.
            let links: BTreeMap<String, bool> = replies
                .iter()
                .filter(|reply| !reply.hidden())
                .flat_map(Cast::link_urls)
                .filter_map(|url| Some((link_key(url).to_owned(), *verdicts.get(link_key(url))?)))
                .collect();
            mark_mini_apps(&mut replies, &links);
            let replies = reply_preview(replies);
            // The authors' profile cards, from the user objects that come
            // with the replies: no request.
            let authors: HashSet<u64> = replies.iter().map(|reply| reply.author.fid).collect();
            let shown = root
                .direct_replies
                .iter()
                .flatten()
                .filter(|raw| raw.author.as_ref().is_some_and(|a| authors.contains(&a.fid)));
            let mut notes = api::Annotations { links, ..Default::default() };
            notes.keep_users(normalize::author_cards(shown), authors);
            RepliesPayload { hash, replies, notes, meta: meta(inv) }
        })
        .collect()
}

/// Replies to show when hovering a cast's reply count: the ten most engaged
/// direct replies, without mini apps or spam ([`replies_previews`]).
pub async fn replies(inv: Rc<Invocation>, hash: String) -> Result<Built, Failure> {
    let Some(root) = hypersnap::conversation(&inv, &hash, 1).await.map_err(Failure::upstream)? else {
        return Ok(Built::not_found(allthing_core::api::NO_SUCH_CAST));
    };
    let preview = replies_previews(&inv, vec![(hash, root)]).await.pop();
    preview.map_or_else(|| Err(Failure::internal("No replies preview")), |p| Built::json(&p, api::REPLIES_CACHE))
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

/// Notifications a page reads from Hypersnap. Each repeats its person and
/// its cast whole, about 3 KB, so thirty are about 100 KB.
const NOTIFICATIONS_PAGE: u32 = 30;
/// The reader's latest casts, read with the first page: their counts of
/// recasts, and whose quotes to read.
const RECENT_CASTS: u32 = 10;
/// Whose quotes are read: the reader's newest casts, replies last.
const QUOTED_CASTS: usize = 4;
/// Quotes read of each, at most: few casts are quoted more often.
const QUOTES_PER_CAST: u32 = 10;
/// Casts whose recasts are read to tell them from likes, at most: the most
/// recently liked first. With the quotes, one batch of reads.
const RECAST_READS: usize = 6;
/// Recasts read of each of the hub's shards, newest first.
const RECASTS_PER_SHARD: u32 = 10;

/// Whether a notification of this kind is a cast someone wrote: a reply, a
/// mention or a quote, which shows its text.
fn is_cast(kind: Kind) -> bool {
    matches!(kind, Kind::Reply | Kind::Mention | Kind::Quote)
}

/// The cast a reply, a mention or a quote is.
fn shown_cast(event: &Event) -> Option<&Cast> {
    event.cast.as_ref().filter(|_| is_cast(event.kind))
}

/// What a batch of the notifications' reads brought.
enum NotificationRead {
    Recasts(String, Option<Recasts>),
    Quotes(Vec<RawCast>),
}

/// A signed-in reader's notifications (`fid` is the session's): a page of
/// Hypersnap's from `cursor` (the newest without one), grouped as the
/// Farcaster app groups them.
///
/// Hypersnap lists a recast as one more like: the hub's newest recasts of
/// the casts liked on the page tell them apart (one read each, for those
/// that may have some). With the first page come the reader's latest casts,
/// whose counts say which those are, and the quotes of the newest of them,
/// which Hypersnap doesn't list (those as new as the page's oldest item, or
/// all on a last page). Replies, mentions and quotes that are spam or mini
/// apps are left out, as listings leave them out, after their links are
/// checked; never for who wrote them. Reads beyond the page are
/// best-effort: one that fails leaves its likes as they came, or its quotes
/// out. Upper bound: 2 reads, then 10 in one batch, then link pages.
pub async fn notifications(
    inv: Rc<Invocation>,
    fid: u64,
    cursor: Option<String>,
    previous: Option<Rc<Vec<u8>>>,
) -> Result<Built, Failure> {
    let first = cursor.is_none();
    let (page, recent) = join(hypersnap::notifications(&inv, fid, NOTIFICATIONS_PAGE, cursor.as_deref()), async {
        if first { hypersnap::user_casts(&inv, fid, RECENT_CASTS).await.unwrap_or_default() } else { Vec::new() }
    })
    .await;
    let page = page.map_err(Failure::upstream)?;
    let mut events = normalize::notifications(&page, fid);
    let recent: Vec<Cast> = recent.iter().map(normalize::cast).filter(|c| c.author.fid == fid).collect();
    let counts: HashMap<String, u64> = recent.iter().map(|c| (c.hash.clone(), c.recasts)).collect();
    let reads = notifications::recasts_to_read(&events, &counts, RECAST_READS)
        .into_iter()
        .map(|hash| (true, hash))
        .chain(notifications::quoted_casts(&recent, QUOTED_CASTS).into_iter().map(|c| (false, c.hash.clone())));
    let done: Vec<NotificationRead> = stream::iter(reads.map(|(recasts, hash)| {
        let inv = &inv;
        async move {
            if recasts {
                let read = hypersnap::newest_recasts(inv, fid, &hash, RECASTS_PER_SHARD).await;
                NotificationRead::Recasts(hash, read.ok().map(|by| Recasts::read(by, RECASTS_PER_SHARD as usize)))
            } else {
                NotificationRead::Quotes(hypersnap::quotes(inv, &hash, QUOTES_PER_CAST).await.unwrap_or_default())
            }
        }
    }))
    .buffered(PARALLEL)
    .collect()
    .await;
    let mut recasts = HashMap::new();
    let mut quoting: Vec<RawCast> = Vec::new();
    for read in done {
        match read {
            NotificationRead::Recasts(hash, Some(read)) => {
                recasts.insert(hash, read);
            }
            NotificationRead::Recasts(_, None) => {}
            NotificationRead::Quotes(found) => quoting.extend(found),
        }
    }
    notifications::sort_reactions(&mut events, &recasts);
    let quotes: Vec<Event> = quoting
        .iter()
        .map(normalize::cast)
        .filter(|cast| notifications::quotes(cast, fid))
        .map(|cast| Event {
            kind: Kind::Quote,
            actor: cast.author.clone(),
            ts: cast.ts,
            mentions_you: notifications::mentions(&cast, fid),
            cast: Some(cast),
        })
        .collect();
    let oldest = events.iter().map(|event| event.ts).min();
    events.extend(notifications::within_page(quotes, oldest, page.cursor().is_some()));

    // Replies, mentions and quotes as listings show casts: without spam or
    // mini apps, their links checked within the build's budget.
    let mut notes = carried(previous.as_deref().map(Vec::as_slice));
    let known_links = std::mem::take(&mut notes.links);
    notes.links = miniapps::check(&inv, events.iter().filter_map(shown_cast), &known_links).await;
    for event in &mut events {
        if is_cast(event.kind)
            && let Some(cast) = event.cast.as_mut()
        {
            mark_mini_apps(std::slice::from_mut(cast), &notes.links);
        }
    }
    events.retain(|event| !is_cast(event.kind) || shown_cast(event).is_some_and(|cast| !cast.hidden()));

    let groups = notifications::group(events, fid);
    // The profile cards of the people shown, from the user objects that came
    // with them: no request.
    let mut cards: BTreeMap<u64, UserCard> = BTreeMap::new();
    for user in page.notifications.iter().filter_map(|item| item.user.as_ref()) {
        if let Some(card) = normalize::summary_card(user) {
            cards.entry(user.fid).or_insert(card);
        }
    }
    cards.extend(normalize::author_cards(&quoting));
    notes.keep_users(cards, groups.iter().flat_map(|g| g.actors.iter().map(|a| a.fid)));
    let newest = groups.first().map_or(0, |group| group.ts);
    let next = page.cursor().map(str::to_owned);
    Built::json(&NotificationsPayload { groups, next, newest, notes, meta: meta(&inv) }, api::NOTIFICATIONS_CACHE)
}

/// At most this many addresses of each kind are measured per account.
const MAX_EVM_ADDRESSES: usize = 5;
const MAX_SOLANA_ADDRESSES: usize = 2;

pub async fn wallets(inv: Rc<Invocation>, fid: u64, previous: Option<Rc<Vec<u8>>>) -> Result<Built, Failure> {
    let Some(raw) = hypersnap::users_bulk(&inv, &[fid]).await.map_err(Failure::upstream)?.into_iter().next() else {
        return Ok(Built::not_found(allthing_core::api::NO_SUCH_USER));
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

/// A wallet response at the public boundary, including older cache entries.
/// Successful payloads keep the byte-copy path. Failed chains are checked
/// so private Solana diagnostics cannot survive a deploy in a cached body.
pub fn public_wallet_body(body: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, Failure> {
    use serde_json::Value;
    use std::borrow::Cow;

    if !body.windows(b"\"error\"".len()).any(|word| word == b"\"error\"") {
        return Ok(Cow::Borrowed(body));
    }
    let invalid = || Failure::internal("Cached wallet data could not be read");
    let mut payload: Value = serde_json::from_slice(body).map_err(|_| invalid())?;
    let chains = payload.get_mut("chains").and_then(Value::as_array_mut).ok_or_else(invalid)?;
    let mut changed = false;
    for chain in chains {
        if chain.get("kind").and_then(Value::as_str) != Some("solana") {
            continue;
        }
        let Some(error) = chain.get_mut("error").filter(|error| !error.is_null()) else { continue };
        let public = chain::public_solana_error(error.as_str().unwrap_or_default());
        if error.as_str() != Some(public.as_ref()) {
            let public = public.into_owned();
            *error = Value::String(public);
            changed = true;
        }
    }
    if changed { serde_json::to_vec(&payload).map(Cow::Owned).map_err(|_| invalid()) } else { Ok(Cow::Borrowed(body)) }
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
        now_ms: Option<f64>,
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
            self.now_ms.unwrap_or(1_800_000_000_000.0)
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
    fn a_fresh_stored_latest_copy_excludes_future_rows_without_more_reads() {
        let stored = stored_copy(40_000.0);
        let before;
        {
            let mut copy = stored.copy.borrow_mut();
            let snapshot = copy.as_mut().unwrap();
            let mut payload: LatestPayload = serde_json::from_str(&snapshot.body).unwrap();
            let present: Cast = serde_json::from_value(serde_json::json!({
                "hash": "0xpresent", "author": { "fid": 7, "username": "seven" }, "text": "present",
                "ts": (NOW / 1000.0) as i64, "likes": 1
            }))
            .unwrap();
            payload.pool.push(present.clone());
            before = payload.pool.len();
            let mut future = present;
            future.hash = "0xfuture".into();
            future.ts = (NOW / 1000.0) as i64 + 5;
            payload.pool.insert(0, future);
            snapshot.body = serde_json::to_string(&payload).unwrap();
        }
        let hub = Rc::new(Hub { stored: Some(stored.clone()), ..Default::default() });
        let inv = Rc::new(Invocation::new(hub.clone(), 45));
        let built = latest(inv, None).now_or_never().unwrap().unwrap();
        let payload: LatestPayload = serde_json::from_slice(&built.body).unwrap();
        assert_eq!(payload.pool.len(), before);
        assert!(payload.pool.iter().all(|cast| cast.ts <= (NOW / 1000.0) as i64));
        assert_eq!(built.fetched_at, Some(NOW - 40_000.0));
        assert_eq!(stored.reads.get(), 1);
        assert!(hub.requests.borrow().is_empty());
        // The source copy still holds the small skew. A later public read can
        // expose it, if retained, without re-reading the event source or aging it anew.
        let later = Rc::new(Hub { stored: Some(stored.clone()), now_ms: Some(NOW + 5_000.0), ..Default::default() });
        let inv = Rc::new(Invocation::new(later.clone(), 45));
        let built = latest(inv, None).now_or_never().unwrap().unwrap();
        let payload: LatestPayload = serde_json::from_slice(&built.body).unwrap();
        assert_eq!(payload.pool.len(), before + 1);
        assert!(payload.pool.iter().any(|cast| cast.hash == "0xfuture"));
        assert_eq!(built.fetched_at, Some(NOW - 40_000.0));
        assert_eq!(stored.reads.get(), 2, "one existing snapshot read for each public request");
        assert!(later.requests.borrow().is_empty());
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

    #[test]
    fn only_the_crons_existing_buffer_retains_a_small_source_clock_skew() {
        let now = allthing_core::time::parse_rfc3339("2026-10-02T00:00:00Z").unwrap() as f64 * 1000.0;
        let stored = Rc::new(Stored::default());
        let mut hub = Hub { stored: Some(stored.clone()), now_ms: Some(now), ..Default::default() };
        hub.events.insert(
            ("1".into(), "newest".into()),
            page(&[
                add(12, "0xnow", 100, false),
                add(13, "0xskew", 105, false),
                add(14, "0xfuture", 106, false),
                add(15, "0xfar", 107, false),
            ]),
        );
        hub.events.insert(("2".into(), "newest".into()), page(&[]));
        for (hash, timestamp) in [
            ("0xnow", "2026-10-02T00:00:00Z"),
            ("0xskew", "2026-10-02T00:00:05Z"),
            ("0xfuture", "2026-10-02T00:00:06Z"),
            ("0xfar", "2061-09-12T00:03:58Z"),
        ] {
            hub.casts.insert(hash.into(), raw_cast(hash, timestamp, 1));
        }
        let public = build(
            Rc::new(Hub {
                events: hub.events.clone(),
                casts: hub.casts.clone(),
                now_ms: Some(now),
                ..Default::default()
            }),
            None,
        )
        .unwrap();
        assert_eq!(public.pool.iter().map(|cast| cast.hash.as_str()).collect::<Vec<_>>(), ["0xnow"]);

        let hub = Rc::new(hub);
        let inv = Rc::new(Invocation::new(hub.clone(), 45));
        store_latest(inv).now_or_never().unwrap().unwrap();
        let saved = stored.copy.borrow().clone().unwrap();
        let payload: LatestPayload = serde_json::from_str(&saved.body).unwrap();
        assert_eq!(payload.pool.iter().map(|cast| cast.hash.as_str()).collect::<Vec<_>>(), ["0xskew", "0xnow"]);
        assert_eq!(payload.cursors, [Cursor { shard: 1, event_id: 15 }]);
        assert_eq!(stored.reads.get(), 1);
        assert_eq!(hub.requests.borrow().iter().filter(|url| url.contains("/v1/events?")).count(), 2);
        assert_eq!(hub.requests.borrow().iter().filter(|url| url.contains("/v2/farcaster/casts?")).count(), 1);
        assert_eq!(saved.built, now);
    }
}

#[cfg(test)]
mod channel_tests {
    use std::cell::{Cell, RefCell};

    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    const PINNED: &str = "0x50184b62b4c52fd76628713562e1eda3dccaf63d";

    /// Hypersnap and the Farcaster app as /c/farcaster finds them. Without
    /// `app`, the app can't be read, as from a browser (it sends no CORS
    /// headers). Everything else (signers, scores) is missing. The first
    /// `hollow` pages of the feed have no casts, each with a cursor for the
    /// next.
    struct Upstreams {
        app: bool,
        pinned_text: &'static str,
        hollow: Cell<u32>,
        requests: RefCell<Vec<String>>,
    }

    impl Upstreams {
        fn new(app: bool, pinned_text: &'static str) -> Rc<Upstreams> {
            Rc::new(Upstreams { app, pinned_text, hollow: Cell::new(0), requests: RefCell::new(Vec::new()) })
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
            } else if v2("feed/parent_urls?") && self.hollow.get() > 0 {
                self.hollow.set(self.hollow.get() - 1);
                json(format!(r#"{{"casts":[],"next":{{"cursor":"page{}"}}}}"#, self.hollow.get()))
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

    fn feed_reads(host: &Upstreams) -> Vec<String> {
        let feed = format!("{}/v2/farcaster/feed/parent_urls", hypersnap::BASE);
        host.requests.borrow().iter().filter(|url| url.starts_with(&feed)).cloned().collect()
    }

    #[test]
    fn the_newest_casts_after_pages_with_none_are_found_by_their_cursors() {
        let host = Upstreams::new(true, "gm");
        host.hollow.set(2);
        let (payload, spent) = build(&host);
        assert_eq!(payload.pool.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), ["0x01"]);
        // The first page, then each empty page's cursor.
        let reads = feed_reads(&host);
        assert_eq!(reads.len(), 3);
        assert!(!reads[0].contains("cursor="));
        assert!(reads[1].ends_with("&cursor=page1") && reads[2].ends_with("&cursor=page0"), "{reads:?}");
        assert_eq!(spent as usize, host.requests.borrow().len());
    }

    #[test]
    fn a_feed_still_without_casts_fails_the_build_so_the_last_good_copy_stays() {
        let host = Upstreams::new(true, "gm");
        host.hollow.set(10);
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let failure = channel(inv, "farcaster".into(), None).now_or_never().unwrap().err().unwrap();
        assert_eq!(failure.status, 502);
        // The first page and three more, no further: the cache serves the
        // last good copy instead (stale-if-error), or the page says the data
        // source didn't answer and offers to try again.
        assert_eq!(feed_reads(&host).len(), 1 + EMPTY_PAGES_FOLLOWED);
    }

    #[test]
    fn a_pinned_mini_app_is_kept_marked() {
        let host = Upstreams::new(true, "play https://farcaster.xyz/miniapps/abc/game");
        let (payload, _) = build(&host);
        // Kept, marked, for the browser to show or not as the reader chose.
        assert!(payload.channel.pinned.as_ref().is_some_and(|c| c.mini_app && !c.text.is_empty()));
        assert_eq!(payload.channel.member_count, Some(1));
        assert!(payload.notes.users.contains_key(&37));
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

    fn nested_carried() -> BTreeMap<String, Cast> {
        BTreeMap::from([
            ("0xb1".to_owned(), cast("0xb1", &["0xc1"])),
            ("0xc1".to_owned(), cast("0xc1", &["0xd1"])),
            ("0xd1".to_owned(), cast("0xd1", &["0xe1"])),
            ("0xe1".to_owned(), cast("0xe1", &[])),
            ("0xold".to_owned(), cast("0xold", &[])),
        ])
    }

    #[test]
    fn all_carried_quote_levels_survive_an_unavailable_provider_without_reads() {
        let host = Bulk::new(true);
        let inv = Invocation::new(host.clone(), 45);
        let pool = [cast("0x01", &["0xb1"])];
        let (quotes, cards) = quoted_casts(&inv, &pool, nested_carried()).now_or_never().unwrap();
        assert_eq!(quotes.keys().collect::<Vec<_>>(), ["0xb1", "0xc1", "0xd1"]);
        assert!(cards.is_empty());
        assert!(host.requests.borrow().is_empty(), "retained quotes need no bulk read");
        assert_eq!(inv.remaining_subrequests(), 45);
    }

    #[test]
    fn all_carried_quote_levels_survive_a_tight_budget() {
        let host = Bulk::new(false);
        let inv = Invocation::new(host.clone(), QUOTES_RESERVE);
        let pool = [cast("0x01", &["0xb1"])];
        let (quotes, _) = quoted_casts(&inv, &pool, nested_carried()).now_or_never().unwrap();
        assert_eq!(quotes.keys().collect::<Vec<_>>(), ["0xb1", "0xc1", "0xd1"]);
        assert!(host.requests.borrow().is_empty());
    }

    #[test]
    fn a_refresh_reuses_all_carried_quote_levels_without_bulk_reads() {
        let host = Rc::new(Chain {
            next: BTreeMap::from([("0xb1", "0xc1"), ("0xc1", "0xd1"), ("0xd1", "0xe1")]),
            requests: RefCell::new(0),
        });
        let inv = Invocation::new(host.clone(), 45);
        let pool = [cast("0x01", &["0xb1"])];
        let (quotes, _) = quoted_casts(&inv, &pool, nested_carried()).now_or_never().unwrap();
        assert_eq!(quotes.keys().collect::<Vec<_>>(), ["0xb1", "0xc1", "0xd1"]);
        assert_eq!(*host.requests.borrow(), 0, "a refresh must reuse the two nested levels too");
    }

    #[test]
    fn a_missing_sibling_does_not_discard_carried_nested_quotes() {
        let host = Bulk::new(true);
        let inv = Invocation::new(host.clone(), 45);
        let pool = [cast("0x01", &["0xb1", "0xmissing"])];
        let (quotes, _) = quoted_casts(&inv, &pool, nested_carried()).now_or_never().unwrap();
        assert_eq!(quotes.keys().collect::<Vec<_>>(), ["0xb1", "0xc1", "0xd1"]);
        assert_eq!(host.asked(), [["0xmissing"]]);
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
    fn a_hidden_cast_leaves_nothing_behind_in_the_notes() {
        use allthing_core::provenance::Origin;
        let app =
            |fid: u64| UserSummary { fid, username: format!("app{fid}"), display_name: String::new(), pfp_url: None };
        let origin = |app_fid: u64| Origin { signer: format!("0x{app_fid:064x}"), app_fid: Some(app_fid) };
        // 0x01 is shown and quotes 0xb1, which quotes 0xc1. 0x02 was read
        // with it, then hidden: its quote, origin and app go.
        let shown = [cast("0x01", &["0xb1"])];
        let mut notes = api::Annotations {
            quotes: BTreeMap::from([
                ("0xb1".to_owned(), cast("0xb1", &["0xc1"])),
                ("0xc1".to_owned(), cast("0xc1", &[])),
                ("0xb2".to_owned(), cast("0xb2", &[])),
            ]),
            origins: BTreeMap::from([("0x01".to_owned(), origin(9152)), ("0x02".to_owned(), origin(309_857))]),
            apps: BTreeMap::from([(9152, app(9152)), (309_857, app(309_857))]),
            pending: 1,
            ..Default::default()
        };
        keep_shown(&mut notes, &shown);
        assert_eq!(notes.quotes.keys().collect::<Vec<_>>(), ["0xb1", "0xc1"]);
        assert_eq!(notes.origins.keys().collect::<Vec<_>>(), ["0x01"]);
        assert_eq!(notes.apps.keys().collect::<Vec<_>>(), [&9152]);
        assert_eq!(notes.pending, 0);
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
                    // 47 minutes before `NOW_MS`: inside every window.
                    "timestamp": "2026-10-14T17:00:00Z",
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

    fn build<H: Host + 'static>(host: &Rc<H>, offset: u32, window: TrendingWindow) -> HomePayload {
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let built = home(inv, offset, window, None).now_or_never().unwrap().unwrap();
        serde_json::from_slice(&built.body).unwrap()
    }

    fn filled() -> Rc<Memory> {
        fill(Memory::default())
    }

    /// `index` with casts indexed and ranked.
    fn fill(index: Memory) -> Rc<Memory> {
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

    /// Hypersnap's trending list of casts posted at fixed times, and no index.
    struct Trending {
        casts: Vec<(&'static str, &'static str)>,
        next: Option<&'static str>,
        requests: RefCell<Vec<String>>,
    }

    impl Host for Trending {
        fn now(&self) -> f64 {
            NOW_MS
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let url = request.url.clone();
            self.requests.borrow_mut().push(url.clone());
            let body = if url.contains("/feed/trending?") || url.contains("/feed/following?") {
                let casts: Vec<_> = self
                    .casts
                    .iter()
                    .map(|(hash, timestamp)| {
                        json!({ "hash": hash, "author": { "fid": 2, "username": "two" }, "text": "words", "timestamp": timestamp })
                    })
                    .collect();
                json!({ "casts": casts, "next": { "cursor": self.next } })
            } else {
                json!({})
            };
            let response =
                HttpResponse { status: 200, retry_after: None, body: Some(serde_json::to_vec(&body).unwrap()) };
            async move { Ok(response) }.boxed_local()
        }

        fn index(&self) -> Option<Rc<dyn Index>> {
            None
        }
    }

    /// Two seconds pass while the source response arrives, including a cast
    /// written during the read and one still a second ahead at its end.
    struct Advancing {
        feed: Trending,
        now: Rc<std::cell::Cell<f64>>,
    }

    impl Advancing {
        fn new() -> Rc<Self> {
            Rc::new(Self {
                feed: Trending {
                    casts: vec![
                        ("0xduring", "2026-10-14T17:46:41Z"),
                        ("0xfuture", "2026-10-14T17:46:43Z"),
                        ("0xaged", "2026-10-14T16:46:41Z"),
                    ],
                    next: Some("42"),
                    requests: RefCell::new(Vec::new()),
                },
                now: Rc::new(std::cell::Cell::new(NOW_MS)),
            })
        }
    }

    impl Host for Advancing {
        fn now(&self) -> f64 {
            self.now.get()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let read = self.feed.fetch(request);
            let now = self.now.clone();
            async move {
                let result = read.await;
                now.set(NOW_MS + 2_000.0);
                result
            }
            .boxed_local()
        }
    }

    #[test]
    fn home_filters_at_the_time_the_source_read_completes() {
        let host = Advancing::new();
        // The one-hour window advances too: 0xaged was inside it when the
        // request began, but is outside when the source response arrives.
        assert_eq!(hashes(&build(&host, 0, TrendingWindow::Hour)), ["0xduring"]);
        assert_eq!(host.feed.requests.borrow().iter().filter(|url| url.contains("/feed/trending?")).count(), 1);
    }

    #[test]
    fn following_keeps_casts_created_while_its_source_read_is_in_flight() {
        let host = Advancing::new();
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let built = following(inv, 7, Some("later".into()), None).now_or_never().unwrap().unwrap();
        let payload: FollowingPayload = serde_json::from_slice(&built.body).unwrap();
        assert_eq!(payload.pool.iter().map(|cast| cast.hash.as_str()).collect::<Vec<_>>(), ["0xduring", "0xaged"]);
        assert_eq!(payload.next.as_deref(), Some("42"));
        assert_eq!(host.feed.requests.borrow().iter().filter(|url| url.contains("/feed/following?")).count(), 1);
    }

    #[test]
    fn each_window_keeps_only_the_casts_posted_within_it() {
        // NOW_MS is 14 October 2026, 17:46:40 UTC: 30 minutes, 2 hours, 5 hours and 3 days before.
        let host = Rc::new(Trending {
            casts: vec![
                ("0xa1", "2026-10-14T17:16:40Z"),
                ("0xa2", "2026-10-14T15:46:40Z"),
                ("0xa3", "2026-10-14T12:46:40Z"),
                ("0xa4", "2026-10-11T17:46:40Z"),
                ("0xf1", "2026-10-14T17:46:41Z"),
                ("0xf2", "2061-09-12T00:03:58Z"),
            ],
            next: None,
            requests: RefCell::new(Vec::new()),
        });
        let shown = |window| build(&host, 0, window).pool.into_iter().map(|cast| cast.hash).collect::<Vec<_>>();
        assert_eq!(shown(TrendingWindow::Hour), ["0xa1"]);
        assert_eq!(shown(TrendingWindow::SixHours), ["0xa1", "0xa2", "0xa3"]);
        // Hypersnap has no 12-hour window: its 24-hour list, cut to 12 hours.
        assert_eq!(shown(TrendingWindow::TwelveHours), ["0xa1", "0xa2", "0xa3"]);
        assert_eq!(shown(TrendingWindow::Day), ["0xa1", "0xa2", "0xa3"]);
        assert_eq!(shown(TrendingWindow::Week), ["0xa1", "0xa2", "0xa3", "0xa4"]);
        // Without an index, all time shows the past week, as before.
        assert_eq!(shown(TrendingWindow::All), ["0xa1", "0xa2", "0xa3", "0xa4"]);
        let asked = host.requests.borrow();
        assert!(asked.iter().any(|url| url.contains("time_window=1h")));
        assert!(asked.iter().any(|url| url.contains("time_window=6h")));
        assert!(asked.iter().any(|url| url.contains("time_window=24h")));
        assert!(!asked.iter().any(|url| url.contains("time_window=12h")), "{asked:?}");
    }

    #[test]
    fn following_payloads_exclude_future_casts_without_changing_page_reads() {
        let host = Rc::new(Trending {
            casts: vec![
                ("0xpresent", "2026-10-14T17:46:40Z"),
                ("0xfuture", "2026-10-14T17:46:41Z"),
                ("0xfar", "2061-09-12T00:03:58Z"),
            ],
            next: None,
            requests: RefCell::new(Vec::new()),
        });
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let built = following(inv, 7, Some("later".into()), None).now_or_never().unwrap().unwrap();
        let payload: FollowingPayload = serde_json::from_slice(&built.body).unwrap();
        assert_eq!(payload.pool.iter().map(|cast| cast.hash.as_str()).collect::<Vec<_>>(), ["0xpresent"]);
        assert_eq!(host.requests.borrow().iter().filter(|url| url.contains("/feed/following?")).count(), 1);
    }

    #[test]
    fn the_first_page_carries_the_polls_counts_once_someone_has_voted() {
        let counts = Counts { yes: 3, no: 1 };
        let voted = fill(Memory { poll: counts, ..Memory::default() });
        let host = Rc::new(Indexed { index: Some(voted), requests: RefCell::new(Vec::new()) });
        // Trending, and ranked from the index.
        assert_eq!(build(&host, 0, TrendingWindow::Day).poll, Some(counts));
        assert_eq!(build(&host, 0, TrendingWindow::Month).poll, Some(counts));
        // Not the later pages; nor before anyone has voted, nor where the index fails or there is none.
        assert_eq!(build(&host, 100, TrendingWindow::Day).poll, None);
        for index in [Some(filled()), Some(Rc::new(Memory { fail: true, ..Memory::default() })), None] {
            let host = Rc::new(Indexed { index, requests: RefCell::new(Vec::new()) });
            assert_eq!(build(&host, 0, TrendingWindow::Day).poll, None);
        }
        // Like any read, it takes one of the build's subrequests: with only
        // the trending read's left, the counts are left out.
        let voted = fill(Memory { poll: counts, ..Memory::default() });
        let host = Rc::new(Indexed { index: Some(voted), requests: RefCell::new(Vec::new()) });
        let built =
            home(Rc::new(Invocation::new(host, 1)), 0, TrendingWindow::Day, None).now_or_never().unwrap().unwrap();
        assert_eq!(serde_json::from_slice::<HomePayload>(&built.body).unwrap().poll, None);
    }
}

#[cfg(test)]
mod engagement_tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use futures::future::{FutureExt, LocalBoxFuture};
    use serde_json::{Value, json};

    use super::*;
    use crate::{Endpoint, FetchError, Host, HttpRequest, HttpResponse};

    /// Hypersnap, and the pages casts link to. Its `reactions/cast` for any
    /// cast: twelve people a second apart (or as many as `people` says),
    /// listed as Hypersnap lists them, shard by shard and each shard oldest
    /// first (the odd minutes, then the even ones). Its `cast/conversation`:
    /// the bodies in `conversations`, by hash, and no such cast for any
    /// other. HTTP 500 for the reads in `failing` (a hash and `likes`,
    /// `recasts` or `replies`). Link pages: the first of `pages` whose
    /// address starts the link's, or a 404. The clock stands still, so
    /// buckets don't refill.
    struct Node {
        failing: Vec<(String, &'static str)>,
        conversations: HashMap<String, String>,
        pages: Vec<(String, &'static str)>,
        /// People a reactions list names, and how long each one's bio is.
        people: (u32, usize),
        requests: RefCell<Vec<String>>,
    }

    /// A `reactions/cast` answer naming `people`, each with a bio `bio` long.
    fn list_body(kind: &str, (people, bio): (u32, usize)) -> String {
        let bio = "b".repeat(bio);
        let people: Vec<String> = (1..=people)
            .filter(|i| i % 2 == 1)
            .chain((1..=people).filter(|i| i % 2 == 0))
            .map(|i| {
                format!(
                    r#"{{"reaction_timestamp":"2026-10-17T10:{:02}:{:02}.000Z","user":{{"fid":{i},"username":"{kind}-{i}","profile":{{"bio":{{"text":"skipped{bio}"}}}}}}}}"#,
                    i / 60,
                    i % 60
                )
            })
            .collect();
        format!(r#"{{"reactions":[{}],"next":{{}}}}"#, people.join(","))
    }

    impl Node {
        fn new(failing: Vec<(String, &'static str)>) -> Rc<Node> {
            Node::with(failing, HashMap::new(), vec![])
        }

        fn with(
            failing: Vec<(String, &'static str)>,
            conversations: HashMap<String, String>,
            pages: Vec<(String, &'static str)>,
        ) -> Rc<Node> {
            Node::listing(failing, conversations, pages, (12, 0))
        }

        fn listing(
            failing: Vec<(String, &'static str)>,
            conversations: HashMap<String, String>,
            pages: Vec<(String, &'static str)>,
            people: (u32, usize),
        ) -> Rc<Node> {
            Rc::new(Node { failing, conversations, pages, people, requests: RefCell::new(Vec::new()) })
        }

        fn sent(&self) -> usize {
            self.requests.borrow().len()
        }

        /// Requests whose path or address starts with `prefix`.
        fn sent_to(&self, prefix: &str) -> usize {
            self.requests.borrow().iter().filter(|r| r.starts_with(prefix)).count()
        }
    }

    impl Host for Node {
        fn now(&self) -> f64 {
            1_800_000_000_000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let path = request.url.trim_start_matches(hypersnap::BASE).to_owned();
            self.requests.borrow_mut().push(path.clone());
            let param = |key: &str| {
                path.split(['?', '&']).find_map(|pair| pair.strip_prefix(key)?.strip_prefix('=')).unwrap_or_default()
            };
            let status = |status: u16| HttpResponse { status, retry_after: None, body: None };
            let ok = |body: String| HttpResponse { status: 200, retry_after: None, body: Some(body.into_bytes()) };
            let failing = |hash: &str, kind: &str| self.failing.iter().any(|(h, k)| h == hash && *k == kind);
            let response = if request.accept == "text/html" {
                match self.pages.iter().find(|(url, _)| request.url.starts_with(url.as_str())) {
                    Some((_, html)) => ok((*html).to_owned()),
                    None => status(404),
                }
            } else if path.starts_with("/v2/farcaster/cast/conversation?") {
                let hash = param("identifier");
                match self.conversations.get(hash) {
                    _ if failing(hash, "replies") => status(500),
                    Some(body) if param("reply_depth") == "1" => ok(body.clone()),
                    _ => status(404),
                }
            } else if !path.starts_with("/v2/farcaster/reactions/cast?") || param("limit") != "100" {
                status(404)
            } else if failing(param("hash"), param("types")) {
                status(500)
            } else {
                ok(list_body(param("types"), self.people))
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
        EngagementRequest::for_casts(hashes.iter().map(|h| (h.as_str(), 3, 1, 0)))
    }

    /// A batch for `hashes`, each with likes, recasts and replies.
    fn all(hashes: &[String]) -> EngagementRequest {
        EngagementRequest::for_casts(hashes.iter().map(|h| (h.as_str(), 3, 1, 2)))
    }

    /// A direct reply to `parent` as Hypersnap sends it: by `fid`, with
    /// `likes`, its `text` and links to `embeds`.
    fn reply(n: u32, parent: &str, fid: u64, likes: u64, text: &str, embeds: &[&str]) -> Value {
        json!({
            "hash": format!("0x{n:040x}"), "parent_hash": parent, "text": text,
            "timestamp": "2026-10-17T10:00:00.000Z",
            "author": {
                "fid": fid, "username": format!("replier{fid}"), "display_name": format!("Replier {fid}"),
                "profile": { "bio": { "text": format!("Bio of {fid}.") } }, "follower_count": fid * 10,
                "verified_addresses": { "eth_addresses": ["0x0000000000000000000000000000000000000001"] },
            },
            "embeds": embeds.iter().map(|url| json!({ "url": url })).collect::<Vec<_>>(),
            "reactions": { "likes_count": likes, "recasts_count": 0 }, "replies": { "count": 0 },
        })
    }

    /// `cast/conversation` for `hash` with its direct replies.
    fn conversation(hash: &str, replies: Vec<Value>) -> String {
        json!({ "conversation": { "cast": {
            "hash": hash, "text": "The cast", "author": { "fid": 1, "username": "author" },
            "replies": { "count": replies.len() }, "direct_replies": replies,
        } } })
        .to_string()
    }

    /// Each of `hashes` with `n` ordinary replies, the first the most liked.
    fn talked(hashes: &[String], n: u32) -> HashMap<String, String> {
        hashes
            .iter()
            .enumerate()
            .map(|(i, hash)| {
                let replies = (0..n)
                    .map(|r| reply(1000 * (i as u32 + 1) + r, hash, 200 + u64::from(r), u64::from(n - r), "gm", &[]))
                    .collect();
                (hash.clone(), conversation(hash, replies))
            })
            .collect()
    }

    /// Build a batch the way the Worker and the static copy route it.
    fn build(inv: &Rc<Invocation>, request: &EngagementRequest) -> Result<Built, Failure> {
        let endpoint = Endpoint::from_path(&request.path()).unwrap();
        assert_eq!(endpoint, Endpoint::Engagement(request.clone()));
        endpoint.build(inv.clone(), None).now_or_never().expect("the mock host answers at once")
    }

    #[test]
    fn a_batch_hypersnap_can_not_read_is_empty_not_an_error() {
        let busy =
            [UpstreamError::Busy(1000.0), UpstreamError::Budget, UpstreamError::Timeout, UpstreamError::Status(500)];
        for error in busy {
            assert!(skips_engagement(&Failure::upstream(error)));
        }
        // A fault of Allthing's own still says so.
        assert!(!skips_engagement(&Failure::internal("unexpected")));
        let batch: EngagementPayload = serde_json::from_slice(&engagement_skipped(1_000)).unwrap();
        assert!(batch.previews.is_empty() && batch.replies.is_empty());
        assert_eq!(batch.meta.fetched_at, 1_000);
    }

    fn payload(built: &Built) -> EngagementPayload {
        serde_json::from_slice(&built.body).unwrap()
    }

    fn preview<'a>(batch: &'a EngagementPayload, hash: &str, kind: ReactionKind) -> Option<&'a ReactionsPayload> {
        batch.previews.iter().find(|p| p.hash == hash && p.kind == kind)
    }

    fn replies_of<'a>(batch: &'a EngagementPayload, hash: &str) -> Option<&'a RepliesPayload> {
        batch.replies.iter().find(|p| p.hash == hash)
    }

    /// What `/api/v1/replies?hash=…` answers for `hash`, built alone.
    fn single_replies(host: &Rc<Node>, hash: &str) -> RepliesPayload {
        let endpoint = Endpoint::from_path(&api::replies_path(hash)).unwrap();
        let built = endpoint.build(Rc::new(Invocation::new(host.clone(), 45)), None).now_or_never().unwrap().unwrap();
        assert_eq!(built.policy, api::REPLIES_CACHE);
        serde_json::from_slice(&built.body).unwrap()
    }

    #[test]
    fn a_full_batch_reads_each_list_once_well_within_the_subrequest_budget() {
        let host = Node::new(vec![]);
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
        assert!(batch.replies.is_empty());
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
    fn a_full_batch_with_replies_reads_each_conversation_once_within_the_budget() {
        let hashes = cards(api::ENGAGEMENT_CHUNK as u32);
        let mut conversations = talked(&hashes, 12);
        // One cast's replies link to two pages no one has checked yet.
        let linked = vec![
            reply(1, &hashes[2], 301, 50, "a blog post", &["https://blog.full-batch.example/post"]),
            reply(2, &hashes[2], 302, 40, "a game", &["https://game.full-batch.example/play"]),
        ];
        conversations.insert(hashes[2].clone(), conversation(&hashes[2], linked));
        let pages = vec![
            ("https://blog.full-batch.example".to_owned(), "<html><head><title>A post</title></head></html>"),
            (
                "https://game.full-batch.example".to_owned(),
                r#"<html><head><meta name="fc:miniapp" content="{}"></head>"#,
            ),
        ];
        let host = Node::with(vec![], conversations, pages);
        let inv = Rc::new(Invocation::new(host.clone(), 45));
        let request = all(&hashes);
        assert_eq!(request.len(), api::ENGAGEMENT_MAX_LISTS + api::ENGAGEMENT_MAX_REPLIES);
        assert!(inv.take_subrequests(1));
        let built = build(&inv, &request).unwrap();
        assert!(inv.take_subrequests(1));
        // A read per list and per conversation, and the two pages: with the
        // cache read and write, 22 of the 45 subrequests; at most 26 with
        // the six pages a build may check.
        assert_eq!(host.sent_to("/v2/farcaster/reactions/cast?"), 12);
        assert_eq!(host.sent_to("/v2/farcaster/cast/conversation?"), 6);
        assert_eq!(host.sent_to("https://"), 2);
        assert_eq!(45 - inv.remaining_subrequests(), 22);
        assert_eq!((built.status, built.policy), (200, api::ENGAGEMENT_CACHE));

        let batch = payload(&built);
        assert_eq!((batch.previews.len(), batch.replies.len()), (12, 6));
        // In hash order, each with the ten most engaged replies, and each
        // exactly what /api/v1/replies answers for its cast.
        let order: Vec<&str> = batch.replies.iter().map(|p| p.hash.as_str()).collect();
        assert_eq!(order, hashes.iter().map(String::as_str).collect::<Vec<_>>());
        let first = replies_of(&batch, &hashes[0]).unwrap();
        assert_eq!(first.replies.len(), 10);
        assert_eq!(first.replies[0].likes, 12);
        assert_eq!(first.notes.users[&200].bio, "Bio of 200.");
        assert_eq!(first.notes.users.len(), 10);
        // The game's page declares a mini app: its reply is left out.
        let linked = replies_of(&batch, &hashes[2]).unwrap();
        assert_eq!(linked.replies.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["a blog post"]);
        assert_eq!(
            linked.notes.links,
            BTreeMap::from([
                ("https://blog.full-batch.example/post".to_owned(), false),
                ("https://game.full-batch.example/play".to_owned(), true),
            ])
        );
        for hash in &hashes {
            assert_eq!(replies_of(&batch, hash), Some(&single_replies(&host, hash)), "{hash}");
        }
    }

    #[test]
    fn hidden_replies_are_left_out_of_a_batch_as_replies_leaves_them_out() {
        let hashes = cards(2);
        let cast = &hashes[0];
        // The most liked replies are spam, mini apps by their address or by
        // their page, and a reply that links to a spam site; then the
        // replies to show.
        let replies = vec![
            reply(1, cast, 401, 90, "Just unlocked my DEGEN rewards, claim yours", &[]),
            reply(2, cast, 402, 80, "play", &["https://farcaster.xyz/miniapps/abc/hidden-game"]),
            reply(3, cast, 403, 70, "a frame", &["https://frame.hidden-replies.example/"]),
            reply(4, cast, 404, 60, "free", &["https://claim.vercel.app/"]),
            reply(5, cast, 405, 2, "A thoughtful reply", &[]),
            reply(6, cast, 406, 9, "The most liked reply shown", &["https://news.hidden-replies.example/story"]),
            reply(7, cast, 407, 1, "Thanks @gig-bot", &[]),
        ];
        let conversations = HashMap::from([(cast.clone(), conversation(cast, replies))]);
        let pages = vec![
            ("https://frame.hidden-replies.example".to_owned(), r#"<head><meta property="fc:frame" content="vNext">"#),
            ("https://news.hidden-replies.example".to_owned(), "<head><title>News</title></head>"),
        ];
        let host = Node::with(vec![], conversations, pages);
        let built = build(&Rc::new(Invocation::new(host.clone(), 45)), &all(&hashes)).unwrap();
        let batch = payload(&built);
        let shown = replies_of(&batch, cast).unwrap();
        let texts: Vec<&str> = shown.replies.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["The most liked reply shown", "A thoughtful reply"]);
        assert!(shown.replies.iter().all(|r| !r.hidden()));
        // Only the authors shown have their profile cards.
        assert_eq!(shown.notes.users.keys().copied().collect::<Vec<_>>(), [405, 406]);
        // Exactly what /api/v1/replies answers.
        assert_eq!(shown, &single_replies(&host, cast));
        // The other card's cast is gone: no preview, and the batch is kept.
        assert!(replies_of(&batch, &hashes[1]).is_none());
        assert_eq!(built.policy, api::ENGAGEMENT_CACHE);
    }

    #[test]
    fn replies_that_dont_fit_a_batchs_parsing_are_left_out_the_smallest_kept() {
        let hashes = cards(4);
        // What the lists leave, in conversation bytes: the batch parses the
        // smallest conversations while they fit in it.
        let lists = 4 * (list_body("likes", (12, 0)).len() + list_body("recasts", (12, 0)).len());
        let room = (BATCH_PARSE_BYTES - lists) / CONVERSATION_WEIGHT;
        let sizes = [35, 110, 30, 40].map(|percent| room * percent / 100);
        let conversations: HashMap<String, String> = hashes
            .iter()
            .zip(sizes)
            .map(|(hash, size)| {
                let body = conversation(hash, vec![reply(1, hash, 500, 1, &"x".repeat(size), &[])]);
                assert!(body.len() - size < 1024);
                (hash.clone(), body)
            })
            .collect();
        let host = Node::with(vec![], conversations, vec![]);
        let built = build(&Rc::new(Invocation::new(host.clone(), 45)), &all(&hashes)).unwrap();
        let batch = payload(&built);
        let kept: Vec<&str> = batch.replies.iter().map(|p| p.hash.as_str()).collect();
        assert_eq!(kept, [hashes[0].as_str(), hashes[2].as_str()]);
        // Every conversation was read, and all their lists are in. Leaving
        // the rest to their popovers isn't a failure: the batch is kept as
        // long as one without them, as it would be the same next time.
        assert_eq!(host.sent_to("/v2/farcaster/cast/conversation?"), 4);
        assert_eq!(batch.previews.len(), 8);
        assert_eq!(built.policy, api::ENGAGEMENT_CACHE);
        // A preview left out still loads on its own, as before the batches.
        assert_eq!(single_replies(&host, &hashes[1]).replies.len(), 1);
    }

    #[test]
    fn full_lists_leave_their_cards_replies_to_the_popovers() {
        // Twelve lists of 100 people with long bios, about 1.6 MB: as much
        // as a batch parses, so even short replies are left out.
        let hashes = cards(api::ENGAGEMENT_CHUNK as u32);
        let full = (100, 1200);
        assert!(6 * (list_body("likes", full).len() + list_body("recasts", full).len()) >= BATCH_PARSE_BYTES);
        let host = Node::listing(vec![], talked(&hashes, 2), vec![], full);
        let built = build(&Rc::new(Invocation::new(host.clone(), 45)), &all(&hashes)).unwrap();
        let batch = payload(&built);
        assert_eq!((batch.previews.len(), batch.replies.len()), (12, 0));
        assert!(batch.previews.iter().all(|p| p.users.len() == 10 && p.more));
        assert_eq!(built.policy, api::ENGAGEMENT_CACHE);
    }

    #[test]
    fn lists_that_leave_room_let_the_replies_in() {
        // Twelve lists of 100 people with shorter bios, about 0.9 MB: the
        // short replies of every card fit beside them.
        let hashes = cards(api::ENGAGEMENT_CHUNK as u32);
        let host = Node::listing(vec![], talked(&hashes, 2), vec![], (100, 600));
        let batch = payload(&build(&Rc::new(Invocation::new(host, 45)), &all(&hashes)).unwrap());
        assert_eq!((batch.previews.len(), batch.replies.len()), (12, 6));
    }

    #[test]
    fn a_conversation_that_fails_leaves_the_rest_and_the_batch_is_kept_briefly() {
        let hashes = cards(3);
        let host = Node::with(vec![(hashes[1].clone(), "replies")], talked(&hashes, 3), vec![]);
        let built = build(&Rc::new(Invocation::new(host.clone(), 45)), &all(&hashes)).unwrap();
        assert_eq!((built.status, built.policy), (200, api::ENGAGEMENT_PARTIAL_CACHE));
        let batch = payload(&built);
        assert_eq!(batch.previews.len(), 6);
        assert!(replies_of(&batch, &hashes[1]).is_none());
        assert_eq!(replies_of(&batch, &hashes[0]).unwrap().replies.len(), 3);
        assert_eq!(replies_of(&batch, &hashes[2]).unwrap().replies.len(), 3);

        // So does one that isn't JSON.
        let mut conversations = talked(&hashes, 3);
        conversations.insert(hashes[2].clone(), "<html>Bad gateway</html>".to_owned());
        let host = Node::with(vec![], conversations, vec![]);
        let built = build(&Rc::new(Invocation::new(host, 45)), &all(&hashes)).unwrap();
        assert_eq!(built.policy, api::ENGAGEMENT_PARTIAL_CACHE);
        assert_eq!(payload(&built).replies.len(), 2);
    }

    #[test]
    fn the_static_copy_builds_a_full_batch_within_its_budget() {
        // In the browser there is no edge cache, and 30 upstream calls a page.
        let host = Node::new(vec![]);
        let inv = Rc::new(Invocation::new(host.clone(), 30));
        let built = build(&inv, &both(&cards(api::ENGAGEMENT_CHUNK as u32))).unwrap();
        assert_eq!((built.policy, payload(&built).previews.len()), (api::ENGAGEMENT_CACHE, 12));
        assert_eq!(inv.remaining_subrequests(), 18);
    }

    #[test]
    fn the_static_copy_builds_a_full_batch_with_replies_within_its_budget() {
        // 18 reads, and the replies' link checks leave the last ten.
        let hashes = cards(api::ENGAGEMENT_CHUNK as u32);
        let mut conversations = talked(&hashes, 2);
        let links: Vec<String> = (0..4).map(|i| format!("https://page{i}.static-copy.example/")).collect();
        let linking =
            links.iter().enumerate().map(|(i, url)| reply(i as u32, &hashes[0], 600, 1, "see", &[url])).collect();
        conversations.insert(hashes[0].clone(), conversation(&hashes[0], linking));
        let host = Node::with(vec![], conversations, vec![]);
        let inv = Rc::new(Invocation::new(host.clone(), 30));
        let built = build(&inv, &all(&hashes)).unwrap();
        let batch = payload(&built);
        assert_eq!((built.policy, batch.previews.len(), batch.replies.len()), (api::ENGAGEMENT_CACHE, 12, 6));
        assert_eq!(host.sent_to("https://"), 2);
        assert_eq!(inv.remaining_subrequests(), 10);
    }

    #[test]
    fn a_list_that_fails_leaves_the_others_and_the_batch_is_kept_briefly() {
        let hashes = cards(3);
        let host = Node::new(vec![(hashes[1].clone(), "likes")]);
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
        let host = Node::new(vec![(hashes[0].clone(), "likes")]);
        let inv = Rc::new(Invocation::new(host, 45));
        let only = EngagementRequest::for_casts([(hashes[0].as_str(), 1, 0, 0)]);
        assert_eq!(build(&inv, &only).err().map(|f| f.status), Some(502));
    }

    #[test]
    fn previews_leave_hypersnaps_reserve_to_page_builds() {
        // The isolate's bucket holds 40 Hypersnap requests, and the clock
        // stands still: 12 for a full batch, then 4 for two cards.
        let host = Node::new(vec![]);
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

    #[test]
    fn replies_come_after_the_lists_when_the_reserve_is_near() {
        // A full batch with replies takes 18 of the 40, leaving 6 above the
        // reserve: the next batch reads its likes, and leaves its recasts
        // and replies to their popovers.
        let hashes = cards(12);
        let host = Node::with(vec![], talked(&hashes, 2), vec![]);
        let batch =
            |range: std::ops::Range<usize>| build(&Rc::new(Invocation::new(host.clone(), 45)), &all(&hashes[range]));
        let full = batch(0..6).unwrap();
        assert_eq!((host.sent(), full.policy, payload(&full).replies.len()), (18, api::ENGAGEMENT_CACHE, 6));
        let partial = batch(6..12).unwrap();
        assert_eq!((host.sent(), partial.policy), (24, api::ENGAGEMENT_PARTIAL_CACHE));
        let partial = payload(&partial);
        assert!(partial.previews.iter().all(|p| p.kind == ReactionKind::Likes) && partial.previews.len() == 6);
        assert!(partial.replies.is_empty());
        assert_eq!(upstream::spare(&Invocation::new(host.clone(), 45), Upstream::Hypersnap, 0), PREVIEW_RESERVE);
    }
}

#[cfg(test)]
mod profile_tests {
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::task::{Context, Poll};

    use futures::future::{FutureExt, LocalBoxFuture, poll_fn};
    use serde_json::json;

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// Alice's newest cast links a mini app and quotes 0xb2; her other cast
    /// quotes 0xb1. The mini app's page answers only once the quotes have
    /// been asked for (or, after 1000 polls, not at all): a build that read
    /// the quotes after its link checks would never learn it is a mini app.
    #[derive(Default)]
    struct Hub {
        quotes_asked: Rc<Cell<bool>>,
        requests: RefCell<Vec<String>>,
    }

    impl Host for Hub {
        fn now(&self) -> f64 {
            1_800_000_000_000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let url = request.url.clone();
            self.requests.borrow_mut().push(url.clone());
            if url.starts_with("https://app.example/") {
                let asked = self.quotes_asked.clone();
                let mut polls = 0;
                return poll_fn(move |_| {
                    polls += 1;
                    if asked.get() {
                        let page = r#"<html><head><meta name="fc:miniapp" content="{}"></head></html>"#;
                        Poll::Ready(Ok(HttpResponse { status: 200, retry_after: None, body: Some(page.into()) }))
                    } else if polls > 1000 {
                        Poll::Ready(Err(FetchError::Network("the quotes were never asked for".into())))
                    } else {
                        Poll::Pending
                    }
                })
                .boxed_local();
            }
            let alice = json!({ "fid": 42, "username": "alice" });
            let body = if url.contains("/v2/farcaster/user/by_username?") {
                Some(json!({ "user": alice }))
            } else if url.contains("/v2/farcaster/feed/user/casts?") {
                Some(json!({ "casts": [
                    { "hash": "0x02", "author": alice, "text": "Play my game", "timestamp": "2026-10-18T10:00:00Z",
                      "embeds": [{ "url": "https://app.example/game" }, { "cast_id": { "hash": "0xb2", "fid": 2 } }] },
                    { "hash": "0x01", "author": alice, "text": "Worth reading", "timestamp": "2026-10-18T09:00:00Z",
                      "embeds": [{ "cast_id": { "hash": "0xb1", "fid": 2 } }] },
                ] }))
            } else if let Some((_, list)) = url.split_once("/v2/farcaster/casts?casts=") {
                self.quotes_asked.set(true);
                let casts: Vec<serde_json::Value> = list
                    .split("%2C")
                    .map(|hash| {
                        json!({ "hash": hash, "author": { "fid": 2, "username": "quoted" }, "text": format!("quoted {hash}"),
                                "timestamp": "2026-10-17T00:00:00Z" })
                    })
                    .collect();
                Some(json!({ "casts": casts }))
            } else {
                None
            };
            let response = match body {
                Some(body) => {
                    HttpResponse { status: 200, retry_after: None, body: Some(serde_json::to_vec(&body).unwrap()) }
                }
                None => HttpResponse { status: 404, retry_after: None, body: None },
            };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    #[test]
    fn quotes_are_read_while_the_link_checks_wait_and_a_mini_app_cast_is_kept_marked() {
        let hub = Rc::new(Hub::default());
        let inv = Rc::new(Invocation::new(hub.clone(), 45));
        let mut build = Box::pin(user(inv, "alice".into(), None));
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let built = loop {
            if let Poll::Ready(built) = build.as_mut().poll(&mut cx) {
                break built.unwrap();
            }
        };
        let payload: UserPayload = serde_json::from_slice(&built.body).unwrap();
        // The mini app's cast is kept, marked, with the quote it needs.
        assert_eq!(
            payload.pool.iter().map(|c| (c.hash.as_str(), c.mini_app)).collect::<Vec<_>>(),
            [("0x02", true), ("0x01", false)]
        );
        assert_eq!(payload.notes.links, BTreeMap::from([("https://app.example/game".to_owned(), true)]));
        assert_eq!(payload.notes.quotes.keys().collect::<Vec<_>>(), ["0xb1", "0xb2"]);
        assert_eq!(payload.notes.quotes["0xb1"].text, "quoted 0xb1");
        // One bulk read brought both quotes.
        assert_eq!(hub.requests.borrow().iter().filter(|url| url.contains("/v2/farcaster/casts?")).count(), 1);
    }

    /// Bob's profile. His casts answer only once his user data has been
    /// asked for (or, after 1000 polls, not at all): a build that read his
    /// website after his casts would never finish. His user data answers
    /// `user_data`, or 503 without it.
    struct Bob {
        user_data: Option<serde_json::Value>,
        asked: Rc<Cell<bool>>,
    }

    impl Host for Bob {
        fn now(&self) -> f64 {
            1_800_000_000_000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let ok = |body: serde_json::Value| HttpResponse {
                status: 200,
                retry_after: None,
                body: Some(serde_json::to_vec(&body).unwrap()),
            };
            let bob = json!({ "fid": 43, "username": "bob" });
            if request.url.contains("/v2/farcaster/feed/user/casts?") {
                let asked = self.asked.clone();
                let mut casts = Some(ok(
                    json!({ "casts": [{ "hash": "0x01", "author": bob, "text": "gm", "timestamp": "2026-10-18T09:00:00Z" }] }),
                ));
                let mut polls = 0;
                return poll_fn(move |_| {
                    polls += 1;
                    if asked.get() {
                        Poll::Ready(Ok(casts.take().unwrap()))
                    } else if polls > 1000 {
                        Poll::Ready(Err(FetchError::Network("the website was never asked for".into())))
                    } else {
                        Poll::Pending
                    }
                })
                .boxed_local();
            }
            let response = if request.url.contains("/v2/farcaster/user/by_username?") {
                ok(json!({ "user": bob }))
            } else if request.url.ends_with("/v1/userDataByFid?fid=43") {
                self.asked.set(true);
                match &self.user_data {
                    Some(body) => ok(body.clone()),
                    None => HttpResponse { status: 503, retry_after: None, body: None },
                }
            } else {
                HttpResponse { status: 404, retry_after: None, body: None }
            };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn bob(user_data: Option<serde_json::Value>, previous: Option<&Built>) -> Built {
        let host = Rc::new(Bob { user_data, asked: Rc::default() });
        let inv = Rc::new(Invocation::new(host, 45));
        let mut build = Box::pin(user(inv, "bob".into(), previous.map(|p| Rc::new(p.body.clone()))));
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        loop {
            if let Poll::Ready(built) = build.as_mut().poll(&mut cx) {
                break built.unwrap();
            }
        }
    }

    fn website_of(built: &Built) -> Option<String> {
        serde_json::from_slice::<UserPayload>(&built.body).unwrap().user.website.map(|w| w.href)
    }

    #[test]
    fn the_website_is_read_alongside_the_casts_and_a_failed_read_never_fails_the_profile() {
        let url = |value: &str| {
            json!({ "messages": [{ "data": { "type": "MESSAGE_TYPE_USER_DATA_ADD", "fid": 43,
                "userDataBody": { "type": "USER_DATA_TYPE_URL", "value": value } } }], "nextPageToken": "" })
        };
        let read = bob(Some(url("bob.example")), None);
        assert_eq!((website_of(&read).as_deref(), read.policy), (Some("https://bob.example"), api::USER_CACHE));
        let payload: UserPayload = serde_json::from_slice(&read.body).unwrap();
        assert_eq!(payload.user.website.unwrap().label, "bob.example");
        assert_eq!(payload.pool.len(), 1);
        // Cleared, or not a web address: none, as read.
        for value in ["", "javascript:alert(1)"] {
            let cleared = bob(Some(url(value)), Some(&read));
            assert_eq!((website_of(&cleared), cleared.policy), (None, api::USER_CACHE), "{value:?}");
        }
        // A read that fails keeps the previous copy's, or has none, and the
        // profile is refreshed soon.
        let failed = bob(None, Some(&read));
        assert_eq!(
            (website_of(&failed).as_deref(), failed.policy),
            (Some("https://bob.example"), api::USER_RETRY_CACHE)
        );
        let first = bob(None, None);
        assert_eq!((website_of(&first), first.policy), (None, api::USER_RETRY_CACHE));
        assert_eq!(serde_json::from_slice::<UserPayload>(&first.body).unwrap().pool.len(), 1);
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

#[cfg(test)]
mod reaction_tests {
    use std::cell::RefCell;

    use allthing_core::index::Entry;
    use allthing_core::model::UserSummary;
    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::indexer::{Index, IndexError, Saved};
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// The reactions the cron keeps, and which casts were asked for.
    #[derive(Default)]
    struct Kept {
        rows: Vec<(String, ReactionReply)>,
        fails: bool,
        asked: RefCell<Vec<Vec<String>>>,
    }

    impl Index for Kept {
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

        fn reactions(
            &self,
            casts: Vec<String>,
        ) -> LocalBoxFuture<'static, Result<Vec<(String, ReactionReply)>, IndexError>> {
            let rows = self.rows.iter().filter(|(cast, _)| casts.contains(cast)).cloned().collect();
            self.asked.borrow_mut().push(casts);
            let result = if self.fails { Err(IndexError("D1 is down".into())) } else { Ok(rows) };
            async move { result }.boxed_local()
        }
    }

    struct Runtime(Rc<Kept>);

    impl Host for Runtime {
        fn now(&self) -> f64 {
            0.0
        }

        fn fetch(&self, _: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            panic!("reactions are read from the index alone")
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}

        fn index(&self) -> Option<Rc<dyn Index>> {
            Some(self.0.clone() as Rc<dyn Index>)
        }
    }

    fn listed(hash: &str, replies: u64) -> Cast {
        Cast {
            hash: hash.into(),
            author: UserSummary { fid: 1, username: "a".into(), display_name: String::new(), pfp_url: None },
            text: String::new(),
            mentions: vec![],
            ts: 0,
            parent_hash: None,
            parent_url: None,
            channel: None,
            embeds: vec![],
            likes: 0,
            recasts: 0,
            replies,
            mini_app: false,
            spam: false,
        }
    }

    fn reply(hash: &str, emoji: &str, fid: u64) -> ReactionReply {
        ReactionReply { hash: hash.into(), emoji: emoji.into(), fid, ts: 100 }
    }

    #[test]
    fn a_listing_reads_its_casts_reactions_in_one_query_and_keeps_the_last_good_ones() {
        let pool = [listed("0xa", 2), listed("0xb", 0), listed("0xc", 1)];
        let kept = Rc::new(Kept {
            rows: vec![
                ("0xa".into(), reply("0xa1", "pepe_no", 7)),
                ("0xc".into(), reply("0xc1", "pepe_heart_hug", 8)),
                ("0xa".into(), reply("0xa2", "pepe_no", 9)),
            ],
            ..Kept::default()
        });
        let inv = Invocation::new(Rc::new(Runtime(kept.clone())), 45);
        let read = read_reactions(&inv, &pool, BTreeMap::new()).now_or_never().unwrap();
        // One read, for the casts with replies, its subrequest taken at once.
        assert_eq!(*kept.asked.borrow(), [vec!["0xa".to_owned(), "0xc".to_owned()]]);
        assert_eq!(inv.remaining_subrequests(), 44);
        assert_eq!(read["0xa"], [reply("0xa1", "pepe_no", 7), reply("0xa2", "pepe_no", 9)]);
        assert_eq!(read["0xc"], [reply("0xc1", "pepe_heart_hug", 8)]);
        assert_eq!(read.len(), 2);

        // A read that fails, or can't be made, keeps the last build's for
        // the casts still listed.
        let carried = BTreeMap::from([
            ("0xa".to_owned(), vec![reply("0xa1", "pepe_no", 7)]),
            ("0xgone".to_owned(), vec![reply("0xg1", "pepe_no", 7)]),
        ]);
        let failing = Rc::new(Kept { fails: true, ..Kept::default() });
        let inv = Invocation::new(Rc::new(Runtime(failing)), 45);
        let read = read_reactions(&inv, &pool, carried.clone()).now_or_never().unwrap();
        assert_eq!(read.keys().collect::<Vec<_>>(), ["0xa"]);
        let spent = Rc::new(Kept::default());
        let inv = Invocation::new(Rc::new(Runtime(spent.clone())), 0);
        let read = read_reactions(&inv, &pool, carried).now_or_never().unwrap();
        assert_eq!(read.keys().collect::<Vec<_>>(), ["0xa"]);
        assert!(spent.asked.borrow().is_empty());

        // No cast with replies: nothing to read.
        let quiet = Rc::new(Kept::default());
        let inv = Invocation::new(Rc::new(Runtime(quiet.clone())), 45);
        assert!(read_reactions(&inv, &[listed("0xb", 0)], BTreeMap::new()).now_or_never().unwrap().is_empty());
        assert!(quiet.asked.borrow().is_empty());
        assert_eq!(inv.remaining_subrequests(), 45);
    }
}

#[cfg(test)]
mod notification_tests {
    use std::cell::RefCell;

    use allthing_core::notifications::{Group, Opens};
    use allthing_core::time::parse_rfc3339;
    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    const ME: u64 = 539_854;
    const D506: &str = "0xd506281357e47c2bf94a770116c74c3d33eb322c";
    const SIX: &str = "0x6fe6f775495c0fb1ed11dcdf53a17105b028da34";
    const OLD: &str = "0x281dd7fc80dda450f6ab3f23dff0ec24ada9c1a4";
    const NONE: &str = r#"{"messages":[],"nextPageToken":""}"#;

    /// Hypersnap as it answered for @0xael.eth (fid 539854) on 21 October
    /// 2026: a page of notifications, the latest casts, and the newest
    /// recasts of three of them: 0xd506… by warpcast-com and 0x6fe6… by
    /// eth3, who liked them too, and 0x281d… (none of them on the page).
    /// The quotes of 0xd506… (with `quoted`) are made up: one by another
    /// account, one from before the page, one that is spam, and the
    /// reader's own. Link pages are plain pages, or a mini app's at
    /// `game.example`.
    struct Node {
        page: String,
        quotes: serde_json::Value,
        requests: RefCell<Vec<String>>,
    }

    impl Node {
        fn new(page: &str, quoted: bool) -> Rc<Node> {
            let quote = |fid: u64, name: &str, at: &str, text: &str| {
                serde_json::json!({"hash": format!("0x{fid:040x}"), "author": {"fid": fid, "username": name,
                    "profile": {"bio": {"text": format!("{name} quotes")}}, "follower_count": 7},
                    "text": text, "timestamp": at, "embeds": [{"cast_id": {"fid": ME, "hash": D506}}]})
            };
            let own: serde_json::Value =
                serde_json::from_str(include_str!("../../allthing-core/tests/fixtures/hypersnap-cast-quotes.json"))
                    .unwrap();
            let quotes = if quoted {
                serde_json::json!({ "casts": [
                    quote(7, "seven", "2026-10-21T07:00:00Z", "Favorite and Show less, at last"),
                    quote(8, "eight", "2026-10-19T00:00:00Z", "From before the page"),
                    quote(9, "nine", "2026-10-21T06:00:00Z", "Just unlocked my DEGEN rewards"),
                    own["casts"][0].clone(),
                ]})
            } else {
                serde_json::json!({ "casts": [] })
            };
            Rc::new(Node { page: page.to_owned(), quotes, requests: RefCell::new(Vec::new()) })
        }

        fn asked(&self) -> Vec<String> {
            self.requests.borrow().clone()
        }
    }

    impl Host for Node {
        fn now(&self) -> f64 {
            1_792_580_000_000.0
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let path = request.url.strip_prefix(hypersnap::BASE).map(str::to_owned);
            self.requests.borrow_mut().push(path.clone().unwrap_or_else(|| request.url.clone()));
            let body: Option<String> = match path.as_deref() {
                Some(p) if p.starts_with("/v2/farcaster/notifications?") => Some(self.page.clone()),
                Some(p) if p.starts_with("/v2/farcaster/feed/user/casts?") => {
                    Some(include_str!("../../allthing-core/tests/fixtures/hypersnap-notifications-casts.json").into())
                }
                Some(p) if p.starts_with("/v1/reactionsByCast?") && p.contains(SIX) => {
                    Some(r#"{"messages":[{"data":{"type":"MESSAGE_TYPE_REACTION_ADD","fid":635889,"timestamp":183035898}}],"nextPageToken":""}"#.into())
                }
                Some(p) if p.starts_with("/v1/reactionsByCast?") && p.contains(D506) => {
                    Some(r#"{"messages":[{"data":{"type":"MESSAGE_TYPE_REACTION_ADD","fid":434449,"timestamp":183114069}}],"nextPageToken":""}"#.into())
                }
                Some(p) if p.starts_with("/v1/reactionsByCast?") && p.contains(OLD) => {
                    Some(include_str!("../../allthing-core/tests/fixtures/hub-reactions-recasts.json").into())
                }
                Some(p) if p.starts_with("/v1/reactionsByCast?") => Some(NONE.into()),
                Some(p) if p.starts_with("/v2/farcaster/cast/quotes?") && p.contains(D506) => {
                    Some(self.quotes.to_string())
                }
                Some(p) if p.starts_with("/v2/farcaster/cast/quotes?") => Some(r#"{"casts":[],"next":{}}"#.into()),
                Some(_) => None,
                None if request.url.starts_with("https://game.example") => {
                    Some(r#"<html><head><meta name="fc:miniapp" content="{}"></head></html>"#.into())
                }
                None => Some("<html><head><title>A page</title></head><body></body></html>".into()),
            };
            let response = HttpResponse {
                status: if body.is_some() { 200 } else { 404 },
                retry_after: None,
                body: body.map(String::into_bytes),
            };
            async move { Ok(response) }.boxed_local()
        }
    }

    fn build(node: &Rc<Node>, cursor: Option<&str>) -> (NotificationsPayload, u32) {
        let inv = Rc::new(Invocation::new(node.clone(), 45));
        let built = notifications(inv.clone(), ME, cursor.map(str::to_owned), None).now_or_never().unwrap().unwrap();
        assert_eq!((built.status, built.policy), (200, api::NOTIFICATIONS_CACHE));
        (serde_json::from_slice(&built.body).unwrap(), 45 - inv.remaining_subrequests())
    }

    fn shape(groups: &[Group]) -> Vec<(Kind, String, u32, String)> {
        groups
            .iter()
            .map(|g| {
                let opens = match g.opens() {
                    Some(Opens::Cast(hash)) => hash[..8].to_owned(),
                    Some(Opens::Profile(name)) => format!("@{name}"),
                    None => String::new(),
                };
                (g.kind, g.first().map(|u| u.username.clone()).unwrap_or_default(), g.count, opens)
            })
            .collect()
    }

    #[test]
    fn builds_the_first_page_from_hypersnap_and_the_hub() {
        let node = Node::new(include_str!("../../allthing-core/tests/fixtures/hypersnap-notifications.json"), true);
        let (payload, spent) = build(&node, None);
        let at = |iso: &str| parse_rfc3339(iso).unwrap();
        let row = |kind, first: &str, count, opens: &str| (kind, first.to_owned(), count, opens.to_owned());
        assert_eq!(
            shape(&payload.groups),
            [
                row(Kind::Reply, "ivc", 1, "0x83c2e8"),
                row(Kind::Like, "ivc", 3, "0xd50628"),
                row(Kind::Follow, "experttrader", 2, "@experttrader"),
                // warpcast-com liked and recasted it: the later is the recast.
                row(Kind::Recast, "warpcast-com", 1, "0xd50628"),
                row(Kind::Like, "chengsengchee", 2, "0x45555a"),
                // The quote by another account; the reader's own, the one
                // before the page and the spam one are left out.
                row(Kind::Quote, "seven", 1, "0x000000"),
                row(Kind::Mention, "nounishprof", 1, "0xafa73c"),
                // eth3's like stays a like, their recast is one (the hub's).
                row(Kind::Like, "onedreamoneshot", 11, "0x6fe6f7"),
                row(Kind::Like, "cryptopoorboyeth.base.eth", 1, "0x281dd7"),
                row(Kind::Reply, "vrn.eth", 1, "0x3991be"),
                row(Kind::Like, "vrn.eth", 1, "0x849ba4"),
                row(Kind::Reply, "vrn.eth", 1, "0x57ff90"),
                row(Kind::Recast, "eth3", 1, "0x6fe6f7"),
                row(Kind::Like, "hazelramon", 2, "0xba9d2d"),
                row(Kind::Follow, "klaassen", 1, "@klaassen"),
                row(Kind::Reply, "4050", 1, "0x861675"),
            ]
        );
        let six = &payload.groups[7];
        assert_eq!(six.actors.len(), 5);
        assert!(six.cast.as_ref().unwrap().excerpt.starts_with("Always wanted to see custom emojis for Farcaster"));
        assert_eq!(payload.groups[1].sentence(), "ivc and 2 others liked your cast");
        assert_eq!(payload.newest, at("2026-10-21T10:26:56Z"));
        assert!(payload.next.as_deref().is_some_and(|c| c.starts_with("eyJ")));
        // The cards of the people shown came with them; a quote's too.
        assert!(payload.notes.users[&1_469_572].bio.starts_with("No bots allowed"));
        assert_eq!(payload.notes.users[&7].bio, "seven quotes");
        let shown: HashSet<u64> = payload.groups.iter().flat_map(|g| g.actors.iter().map(|a| a.fid)).collect();
        assert!(payload.notes.users.keys().all(|fid| shown.contains(fid)));

        // The page and the latest casts, then one batch: the recasts of the
        // two liked casts whose counts its pairs don't account for, and the
        // quotes of the four newest casts. The mention's link page, once.
        let asked = node.asked();
        let hub: Vec<&str> = asked.iter().filter(|p| p.starts_with('/')).map(String::as_str).collect();
        assert_eq!(
            hub[..2],
            [
                "/v2/farcaster/notifications?fid=539854&limit=30",
                "/v2/farcaster/feed/user/casts?fid=539854&limit=10&include_replies=true",
            ]
        );
        assert_eq!(
            hub[2],
            format!(
                "/v1/reactionsByCast?target_fid=539854&target_hash={SIX}&reaction_type=Recast&pageSize=10&reverse=true"
            )
        );
        assert!(hub[3].contains(OLD));
        let quoted: Vec<&str> = hub[4..].iter().map(|p| &p[p.find("0x").unwrap()..][..10]).collect();
        assert_eq!(quoted, ["0xd5062813", "0x45555a30", "0xba9d2d95", "0x6fe6f775"]);
        let pages: Vec<&String> = asked.iter().filter(|p| !p.starts_with('/')).collect();
        assert_eq!(pages, ["https://www.gmfarcaster.com/episodes/ep397"]);
        assert_eq!(spent as usize, asked.len());
        assert_eq!(spent, 9);
    }

    #[test]
    fn a_later_page_reads_its_likes_recasts_and_no_quotes() {
        let node = Node::new(include_str!("../../allthing-core/tests/fixtures/hypersnap-notifications.json"), true);
        let (payload, spent) = build(&node, Some("eyJiZWZvcmVfdGltZXN0YW1wIjoxODMwMjk1MDJ9"));
        let asked = node.asked();
        assert!(asked[0].ends_with("&cursor=eyJiZWZvcmVfdGltZXN0YW1wIjoxODMwMjk1MDJ9"));
        // No latest casts to say which have none: every liked cast's recasts
        // (six), and no quotes.
        assert_eq!(asked.iter().filter(|p| p.starts_with("/v1/reactionsByCast?")).count(), 6);
        assert!(!asked.iter().any(|p| p.contains("feed/user/casts") || p.contains("cast/quotes")));
        assert_eq!(spent, 8);
        // The same recasts as on the first page, both found by the hub's
        // reads this time.
        let kinds: Vec<(Kind, &str)> = payload
            .groups
            .iter()
            .filter(|g| g.kind == Kind::Recast)
            .map(|g| (g.kind, g.first().unwrap().username.as_str()))
            .collect();
        assert_eq!(kinds, [(Kind::Recast, "warpcast-com"), (Kind::Recast, "eth3")]);
        assert!(!payload.groups.iter().any(|g| g.kind == Kind::Quote));
    }

    #[test]
    fn replies_that_are_spam_or_mini_apps_are_left_out() {
        let reply = |n: u32, text: &str, embeds: serde_json::Value| {
            serde_json::json!({"type": "reply", "timestamp": "2026-10-21T10:00:00Z",
                "user": {"fid": 7, "username": "seven"},
                "cast": {"hash": format!("{n:040x}"), "author": {"fid": 7}, "text": text, "parent_author": {"fid": ME},
                    "timestamp": "2026-10-21T10:00:00Z", "embeds": embeds}})
        };
        let page = serde_json::json!({"notifications": [
            reply(1, "Play with me", serde_json::json!([{"url": "https://game.example/play"}])),
            reply(2, "Stake your DEGEN at yeetr.fun", serde_json::json!([])),
            reply(3, "Thanks for this", serde_json::json!([{"url": "https://blog.example/post"}])),
        ], "next": {}});
        let node = Node::new(&page.to_string(), false);
        let (payload, _) = build(&node, None);
        assert_eq!(shape(&payload.groups), [(Kind::Reply, "seven".to_owned(), 1, "0x000000".to_owned())]);
        assert_eq!(payload.groups[0].cast.as_ref().unwrap().hash, format!("0x{:040x}", 3));
        assert_eq!(payload.groups[0].cast.as_ref().unwrap().excerpt, "Thanks for this");
        // The verdicts come with the payload, for its next build.
        assert_eq!(payload.notes.links.get("https://game.example/play"), Some(&true));
        assert_eq!(payload.notes.links.get("https://blog.example/post"), Some(&false));
        let previous = Rc::new(serde_json::to_vec(&payload).unwrap());
        let again = Node::new(&page.to_string(), false);
        let inv = Rc::new(Invocation::new(again.clone(), 45));
        notifications(inv, ME, None, Some(previous)).now_or_never().unwrap().unwrap();
        assert!(again.asked().iter().all(|p| p.starts_with('/')));
        assert!(payload.next.is_none());
    }

    #[test]
    fn a_page_hypersnap_cant_give_is_a_failure() {
        let node = Node::new("", false);
        let inv = Rc::new(Invocation::new(node, 45));
        let failure = notifications(inv, ME, None, None).now_or_never().unwrap().map(|_| ()).unwrap_err();
        assert_eq!(failure.status, 502);
    }
}
