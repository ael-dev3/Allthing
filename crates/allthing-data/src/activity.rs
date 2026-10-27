// SPDX-License-Identifier: AGPL-3.0-or-later
//! Daily active accounts (see `allthing_core::activity`): the storage the
//! count needs from the runtime, and the cron's run, which reads each
//! shard's hub events on from where the last run stopped and keeps each
//! UTC hour's and day's active fids, and each hour's signing keys. The apps
//! that requested those keys are looked up a few accounts a run and kept
//! (a key's app never changes), and once an hour the past 24 hours' keys
//! are counted by app. The same read keeps the emoji reactions it finds,
//! and drops those taken back, for listings' cards.

use std::collections::{BTreeSet, HashMap};

use allthing_core::activity::{
    self, ActivityPayload, AppsWindow, Batch, ExactKeys, Fids, Follow, KEEP_DAYS, KEEP_HOURS, NAMED_APPS, Reached,
    VERIFICATION_VERSION, WEEK_DAYS, Window, day, day_set, hour, hour_set, key_set, week_set,
};
use allthing_core::api::Meta;
use allthing_core::emoji::ReactionReply;
use allthing_core::latest::{Cursor, SHARDS};
use allthing_core::normalize;
use futures::future::{LocalBoxFuture, join_all};

use crate::Invocation;
use crate::hypersnap;
use crate::indexer::{IndexError, Snapshot};

/// Events a run reads per shard: one page, about 13 minutes of events and
/// 400 KB, parsed in about 2.5 ms of the run's 10 ms of CPU. A run every two
/// minutes reads six times faster than events come, so a backlog clears.
pub const PAGE: u32 = 1000;

/// The name the payload is stored and served under.
pub const SUMMARY: &str = "activity_v2";

/// Reading this close to now (seconds) is keeping up.
const CAUGHT_UP: i64 = 600;

/// Accounts whose keys' apps a run looks up at most, a request each: fewer
/// while it reads a backlog, whose full pages take most of a run's CPU.
const LOOKUPS: usize = 24;
const LOOKUPS_BEHIND: usize = 6;
/// Keys with no known app kept from a count to look up next.
const BACKLOG: usize = 400;
/// Subrequests a run makes after its lookups: the count by app and the save
/// (D1), and with a count, the apps' accounts.
const AFTER: u32 = 2;
/// Subrequests kept for keys removed since, each a read of the account's
/// onchain history.
const HISTORY: u32 = 6;
/// How long emoji reactions are kept for listings' cards, days.
pub const REACTIONS_KEPT_DAYS: i64 = 400;

/// What the follower writes at the end of a run, in one transaction.
pub struct Saving {
    /// Exact state this run read. The atomic store must reject a stale writer.
    pub expected_state: Option<String>,
    /// Sets by name.
    pub sets: Vec<(String, String)>,
    /// Drop the hourly sets (of fids and of keys) before hour `.0`, and the
    /// daily ones before day `.1`.
    pub keep_from: (i64, i64),
    /// Keys' apps learned, as `(signer_identity, app)`; a key's app never changes.
    pub keys: Vec<(String, u64)>,
    /// Emoji reactions read, each with the cast it replies to; then the
    /// casts removed, and reactions revoked or pruned, whose reactions go.
    pub reactions: Vec<(String, ReactionReply)>,
    pub removed: Vec<String>,
    /// Drop reactions from before this, Unix seconds.
    pub reactions_from: i64,
    pub state: String,
    /// The payload served.
    pub summary: Snapshot,
}

/// One app's count among a run of keys: the app (`None` for keys not looked
/// up yet, 0 for keys no app requested), its accounts, and with `None`, the
/// keys.
pub type AppCount = (Option<u64>, u64, Vec<String>);

/// The follower's storage (D1 in the Worker). Each call is one round trip.
pub trait ActivityStore {
    /// The saved state, as JSON.
    fn activity_state(&self) -> LocalBoxFuture<'static, Result<Option<String>, IndexError>>;
    /// The stored sets of those `names` there are.
    fn activity_sets(&self, names: Vec<String>)
    -> LocalBoxFuture<'static, Result<HashMap<String, String>, IndexError>>;
    /// The apps of those exact signer identities that have been looked up.
    fn key_apps(&self, pairs: Vec<String>) -> LocalBoxFuture<'static, Result<HashMap<String, u64>, IndexError>>;
    /// Record exact immutable attribution facts, then count the named
    /// hourly key sets inside the store. Current-run overrides replace
    /// persisted sets. Reject malformed identities or a distinct-account
    /// total that differs from the independently counted FID window. Return
    /// full app counts but at most 400 unresolved identities.
    fn count_apps(
        &self,
        keys: Vec<(String, u64)>,
        names: Vec<String>,
        overrides: Vec<(String, String)>,
        expected_active: u64,
    ) -> LocalBoxFuture<'static, Result<Vec<AppCount>, IndexError>>;
    /// Write it all, and store the payload served.
    fn save_activity(&self, saving: Saving) -> LocalBoxFuture<'static, Result<(), IndexError>>;
}

/// What a run did, for the Worker's log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Events read, per shard.
    pub events: Vec<(u32, usize)>,
    pub actions: usize,
    /// How far behind now reading is, seconds.
    pub behind: Option<i64>,
    /// Shards whose events couldn't be read this run.
    pub failed: Vec<u32>,
    /// Whether the 30-day window was counted again.
    pub counted_30d: bool,
    /// Accounts whose keys' apps were looked up, and keys whose app was
    /// learned.
    pub looked_up: usize,
    pub learned: usize,
    /// Whether accounts by app were counted again.
    pub counted_apps: bool,
}

/// Where a first run starts in `shard`: at the start of yesterday (UTC),
/// estimated from its newest block at about one block a second, so
/// the actual canonical start is then reported. Reading starts after this event.
async fn start(inv: &Invocation, shard: u32, now: i64) -> Result<u64, IndexError> {
    let body = hypersnap::events(inv, shard, None, 1).await.map_err(|e| IndexError(e.to_string()))?;
    let newest = activity::activity_head(&body, shard).ok_or_else(|| IndexError("invalid activity head".into()))?;
    let back = (now - (day(now) - 1) * 86_400).max(0) as u64;
    Ok(((newest >> 14).saturating_sub(back) << 14).saturating_sub(1))
}

/// Match full keys only. A missing, ambiguous, malformed or truncated
/// lookup remains unknown and can be retried; absence never proves no app.
async fn look_up(inv: &Invocation, fid: u64, identities: Vec<String>, after: u32) -> Option<Vec<(String, u64)>> {
    let active = hypersnap::verified_signer_apps(inv, fid).await.ok()?;
    let (mut found, mut removed) = (Vec::new(), Vec::new());
    for identity in identities {
        let key = identity.split_once(':')?.1;
        match active.get(key) {
            Some(app) => found.push((identity, *app)),
            None => removed.push(identity),
        }
    }
    if !removed.is_empty()
        && inv.remaining_subrequests() > after
        && let Ok(history) = hypersnap::verified_onchain_signers(inv, fid).await
    {
        for identity in removed {
            let key = identity.split_once(':')?.1;
            if let Some(app) = history.get(key) {
                found.push((identity, *app));
            }
        }
    }
    Some(found)
}

/// Take a subrequest for a D1 call; the store answers an error without one.
fn d1(inv: &Invocation) -> Result<(), IndexError> {
    if inv.take_subrequests(1) { Ok(()) } else { Err(IndexError("no subrequests left for D1".into())) }
}

/// One run: read each shard's next page, add its active accounts to their
/// hours' and days' sets and the keys they signed with to their hours',
/// look up the apps of keys not known yet, count the day so far (and by app
/// once an hour), and store the payload.
pub async fn run(inv: &Invocation, store: &dyn ActivityStore) -> Result<Report, IndexError> {
    d1(inv)?;
    let expected_state = store.activity_state().await?;
    let mut follow: Follow = match expected_state.as_deref() {
        Some(state) => serde_json::from_str(state).map_err(|_| IndexError("invalid verified activity state".into()))?,
        None => Follow { verification_version: VERIFICATION_VERSION, ..Follow::default() },
    };
    let now = (inv.now() / 1000.0) as i64;
    if !follow.valid(now) {
        return Err(IndexError("invalid verified activity progress".into()));
    }
    follow.revision = follow.revision.checked_add(1).ok_or_else(|| IndexError("activity revision exhausted".into()))?;
    let mut afters = Vec::new();
    for shard in SHARDS {
        let after = match follow.cursors.iter().find(|c| c.shard == shard) {
            Some(cursor) => cursor.event_id,
            None => start(inv, shard, now).await?,
        };
        afters.push((shard, after));
    }
    let pages = join_all(afters.iter().map(|&(shard, after)| hypersnap::events(inv, shard, Some(after), PAGE))).await;
    let mut report = Report::default();
    let mut actions = Vec::new();
    let mut learned: HashMap<String, u64> = HashMap::new();

    let (mut reactions, mut removed) = (Vec::new(), Vec::new());
    for ((shard, after), page) in afters.into_iter().zip(pages) {
        let carried = follow.blocks.iter().find(|b| b.shard == shard);
        let Some(verified) = page.ok().and_then(|body| activity::verified_activity(&body, shard, after, carried, now))
        else {
            report.failed.push(shard);
            continue;
        };
        let page = verified.page;
        report.events.push((shard, page.count));
        follow.gaps.extend(verified.gaps);
        if let Some(started) = page.started
            && !follow.starts.iter().any(|s| s.shard == shard)
        {
            follow.starts.push(Reached { shard, at: started + 1 });
        }
        follow.since = SHARDS
            .into_iter()
            .map(|shard| follow.starts.iter().find(|s| s.shard == shard).map(|s| s.at))
            .collect::<Option<Vec<_>>>()
            .and_then(|starts| starts.into_iter().max());
        follow.keys_since = follow.since;
        if let Some(last_id) = page.last_id {
            follow.cursors.retain(|c| c.shard != shard);
            follow.cursors.push(Cursor { shard, event_id: last_id });
        }
        if let Some(block) = verified.block {
            follow.blocks.retain(|b| b.shard != shard);
            follow.blocks.push(block);
        }
        if let Some(at) = page.reached {
            follow.reached.retain(|r| r.shard != shard);
            follow.reached.push(Reached { shard, at });
        }
        learned.extend(page.exact_keys);
        actions.extend(page.actions);
        reactions.extend(page.reactions);
        removed.extend(page.removed);
    }
    report.actions = actions.len();
    report.learned = learned.len();

    let batch = Batch::of(&actions);
    // Until both shards establish coverage, retain the first shard's
    // history from its original start rather than pruning against the wall
    // clock (or that shard's advancing cursor). Bound an exceptionally long
    // wait and explicitly mark any expired prefix as a gap.
    let reached = follow.reached().unwrap_or_else(|| {
        let start = follow.starts.iter().map(|s| s.at).min().unwrap_or(now);
        let anchor = start.max(now - KEEP_DAYS * 86_400);
        let expired_through = (hour(anchor) - KEEP_HOURS) * 3_600;
        if expired_through >= start {
            let to = expired_through + 1;
            if let Some((_, end)) = follow.gaps.iter_mut().find(|(from, _)| *from == start) {
                *end = (*end).max(to);
            } else {
                follow.gaps.push((start, to));
            }
        }
        anchor
    });
    report.behind = follow.reached().map(|at| now - at);
    // The windows end as the hour in progress began.
    let (now_hour, today) = (hour(reached), day(reached));
    let to = activity::rolling_to(reached);
    let keeping_up = follow.reached().is_some() && report.behind.is_some_and(|b| (0..CAUGHT_UP).contains(&b));

    // The apps of the keys that signed, where known.
    let signed: Vec<String> =
        ExactKeys::of(batch.exact_keys.values().flat_map(|k| k.as_slice().iter().cloned())).as_slice().to_vec();
    let mut known = if signed.is_empty() {
        HashMap::new()
    } else {
        d1(inv)?;
        store.key_apps(signed.clone()).await?
    };

    known.extend(learned.iter().map(|(key, &app)| (key.clone(), app)));

    // The 30 days to the end of yesterday, once a day, when keeping up (the
    // day sets of a backlog are still filling) and yesterday was read whole.
    let due_30d = keeping_up
        && follow.since.is_some_and(|since| since <= (today - 1) * 86_400)
        && follow.active_30d.is_none_or(|w| w.day < today - 1);
    let due_apps = follow.apps_due(to, now);
    let mut names: BTreeSet<String> = batch.hours.keys().map(|&h| hour_set(h)).collect();
    names.extend(batch.days.keys().map(|&d| day_set(d)));
    names.extend(batch.exact_keys.keys().map(|&h| key_set(h)));
    names.extend((now_hour - 48..now_hour).map(hour_set));
    if due_30d {
        names.extend((today - 30..today).map(day_set));
    }
    d1(inv)?;
    let stored = store.activity_sets(names.into_iter().collect()).await?;
    // A missing row is an empty set; a present but unreadable row is an
    // integrity failure. Never overwrite it with an apparently exact zero.
    let mut stored_fids = HashMap::new();
    let mut stored_keys = HashMap::new();
    for (name, text) in &stored {
        if name.starts_with("v2k") {
            stored_keys.insert(
                name.clone(),
                ExactKeys::decode(text).ok_or_else(|| IndexError(format!("invalid activity key set {name}")))?,
            );
        } else {
            stored_fids.insert(
                name.clone(),
                Fids::decode(text).ok_or_else(|| IndexError(format!("invalid activity account set {name}")))?,
            );
        }
    }
    let decoded = |name: &str| stored_fids.get(name).cloned().unwrap_or_default();
    let decoded_keys = |name: &str| stored_keys.get(name).cloned().unwrap_or_default();

    // Look up the apps of keys not known yet: this run's first, then those
    // the last count left, an account a request, keeping subrequests for
    // keys removed since and for the D1 calls that follow.
    let after = AFTER + u32::from(due_apps);
    let limit = if keeping_up { LOOKUPS } else { LOOKUPS_BEHIND };
    let limit = limit.min(inv.remaining_subrequests().saturating_sub(after + HISTORY) as usize);
    let mut accounts: Vec<(u64, Vec<String>)> = Vec::new();
    for pair in signed.iter().filter(|p| !known.contains_key(*p)).chain(follow.backlog.iter()) {
        let Some(fid) = activity::identity_fid(pair) else {
            return Err(IndexError("invalid signer identity in activity backlog".into()));
        };
        if let Some((_, pairs)) = accounts.iter_mut().find(|(f, _)| *f == fid) {
            if !pairs.contains(pair) {
                pairs.push(pair.clone());
            }
        } else if accounts.len() < limit {
            accounts.push((fid, vec![pair.clone()]));
        }
    }
    report.looked_up = accounts.len();
    let found = join_all(accounts.into_iter().map(|(fid, pairs)| look_up(inv, fid, pairs, after))).await;
    let mut keys: Vec<(String, u64)> = learned.into_iter().collect();
    for (pair, app) in found.into_iter().flatten().flatten() {
        known.insert(pair.clone(), app);
        keys.push((pair, app));
    }
    follow.backlog.retain(|pair| !known.contains_key(pair));

    let mut writes = Vec::new();
    let mut hours: HashMap<i64, Fids> = (now_hour - 48..now_hour).map(|h| (h, decoded(&hour_set(h)))).collect();
    for (h, fids) in &batch.hours {
        let mut set = hours.remove(h).unwrap_or_else(|| decoded(&hour_set(*h)));
        set.extend(fids);
        writes.push((hour_set(*h), set.encode()));
        hours.insert(*h, set);
    }
    let mut key_overrides = Vec::new();
    for (h, pairs) in &batch.exact_keys {
        let mut set = decoded_keys(&key_set(*h));
        set.extend(pairs);
        let row = (key_set(*h), set.encode());
        if (now_hour - 24..now_hour).contains(h) {
            key_overrides.push(row.clone());
        }
        writes.push(row);
    }
    let mut merged_days = HashMap::new();
    for (d, fids) in &batch.days {
        let mut set = decoded(&day_set(*d));
        set.extend(fids);
        writes.push((day_set(*d), set.encode()));
        merged_days.insert(*d, set.clone());
        let count = follow.day_mut(*d);
        count.active = set.len() as u64;
        for (family, n) in batch.actions.get(d).into_iter().flatten() {
            *count.actions.entry(*family).or_default() += n;
        }
    }
    if due_30d {
        let days: Vec<Fids> =
            (today - 30..today).map(|d| merged_days.get(&d).cloned().unwrap_or_else(|| decoded(&day_set(d)))).collect();
        let complete = follow.covered((today - 30) * 86_400, today * 86_400);
        follow.active_30d = Some(Window { day: today - 1, active: Fids::union_len(&days) as u64, complete });
        report.counted_30d = true;
        // The week to the end of yesterday as one set, for a list to ask
        // who of its people was active (`Index::active_week`): once the
        // week was read in full.
        if follow.covered((today - WEEK_DAYS) * 86_400, today * 86_400) {
            let mut week = Fids::default();
            for set in &days[days.len() - WEEK_DAYS as usize..] {
                week.extend(set);
            }
            writes.push((week_set(today - 1), week.encode()));
        }
    }
    if due_apps && let Some(active) = activity::rolling(&follow, &hours, to) {
        let names = (now_hour - 24..now_hour).map(key_set).collect();
        d1(inv)?;
        let counts = store.count_apps(keys.clone(), names, key_overrides, active).await?;
        follow.backlog = counts.iter().flat_map(|(_, _, pairs)| pairs.iter().cloned()).take(BACKLOG).collect();
        let counts = counts.into_iter().map(|(app, n, _)| (app, n));
        let mut window = AppsWindow::of(to, now, active, counts);
        window.verification_version = VERIFICATION_VERSION;
        // The accounts of the apps the page names: those not read yet, and
        // all of them again each day. Unread, the names read before stay.
        let named: Vec<u64> = window.apps.iter().take(NAMED_APPS).map(|a| a.app).collect();
        follow.app_accounts.retain(|a| named.contains(&a.fid));
        let stale = follow.named != Some(today);
        let missing: Vec<u64> =
            named.iter().copied().filter(|fid| stale || !follow.app_accounts.iter().any(|a| a.fid == *fid)).collect();
        if !missing.is_empty()
            && let Ok(users) = hypersnap::users_bulk(inv, &missing).await
        {
            for user in users.iter().map(normalize::user_summary).filter(|a| named.contains(&a.fid)) {
                follow.app_accounts.retain(|a| a.fid != user.fid);
                follow.app_accounts.push(user);
            }
            follow.named = Some(today);
        }
        follow.apps_24h = Some(window);
        report.counted_apps = true;
    }
    follow.settle();
    let meta = Meta { fetched_at: (inv.now()) as i64, ..Meta::default() };
    let payload: ActivityPayload = activity::summary(&follow, &hours, meta);
    if !payload.is_verified(payload.meta.fetched_at.div_euclid(1_000)) {
        return Err(IndexError("Refusing to publish an incoherent activity verification record".into()));
    }
    let body = serde_json::to_string(&payload).map_err(|e| IndexError(e.to_string()))?;
    let state = serde_json::to_string(&follow).map_err(|e| IndexError(e.to_string()))?;
    let keep_from = (now_hour - KEEP_HOURS, today - KEEP_DAYS);
    d1(inv)?;
    let summary = Snapshot { built: inv.now(), body };
    let reactions_from = now - REACTIONS_KEPT_DAYS * 86_400;
    store
        .save_activity(Saving {
            expected_state,
            sets: writes,
            keep_from,
            keys,
            reactions,
            removed,
            reactions_from,
            state,
            summary,
        })
        .await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use futures::FutureExt;

    use allthing_core::activity::AppActive;

    use super::*;
    use crate::indexer::Index;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// Now: 2027-01-15T08:00:00Z, Unix seconds.
    const NOW: i64 = 1_800_000_000;
    /// The newest block, and where a first run starts: 32 hours earlier,
    /// the start of yesterday.
    const HEAD: u64 = 10_000_000;
    const START: u64 = HEAD - 115_200;

    /// A hub with an event page per (shard, first event id), and accounts'
    /// active keys and onchain key history by fid.
    #[derive(Default)]
    struct Hub {
        events: HashMap<(String, String), String>,
        signers: HashMap<String, String>,
        history: HashMap<String, String>,
        /// `user/bulk` answers by its `fids`.
        users: HashMap<String, String>,
        requests: RefCell<Vec<String>>,
        /// Seconds after `NOW`.
        later: i64,
    }

    fn param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
        url.split(['?', '&']).find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
    }

    impl Host for Hub {
        fn now(&self) -> f64 {
            (NOW + self.later) as f64 * 1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let url = request.url.clone();
            self.requests.borrow_mut().push(url.clone());
            let fid = param(&url, "fid").unwrap_or_default().to_owned();
            let body = if url.contains("/user/bulk") {
                self.users.get(param(&url, "fids").unwrap_or_default())
            } else if url.contains("/signersByFid") {
                self.signers.get(&fid)
            } else if url.contains("/onChainEventsByFid") {
                self.history.get(&fid)
            } else {
                let shard = param(&url, "shard_index").unwrap_or_default().to_owned();
                let from = param(&url, "from_event_id").unwrap_or("newest").to_owned();
                self.events.get(&(shard, from))
            };
            let response = match body {
                Some(body) => {
                    let mut value: serde_json::Value = serde_json::from_str(body).unwrap();
                    if url.contains("/events?")
                        && let Some(events) = value["events"].as_array_mut()
                    {
                        let shard: u32 = param(&url, "shard_index").unwrap().parse().unwrap();
                        for event in events {
                            event["shardIndex"] = shard.into();
                            if let Some(confirmed) = event.get_mut("blockConfirmedBody") {
                                confirmed["shardIndex"] = shard.into();
                            }
                        }
                    }
                    // Short readable fixture keys stand for complete Ed25519 keys.
                    fn full_keys(value: &mut serde_json::Value) {
                        match value {
                            serde_json::Value::Object(fields) => {
                                for (name, value) in fields {
                                    if matches!(name.as_str(), "key" | "signer")
                                        && let Some(key) = value.as_str()
                                    {
                                        *value = format!("0x{:0>64}", key.trim_start_matches("0x")).into();
                                    } else {
                                        full_keys(value);
                                    }
                                }
                            }
                            serde_json::Value::Array(values) => {
                                for value in values {
                                    full_keys(value);
                                }
                            }
                            _ => {}
                        }
                    }
                    full_keys(&mut value);
                    if url.contains("/events?")
                        && let Some(events) = value["events"].as_array_mut()
                    {
                        for event in events {
                            if let Some(body) = event
                                .get_mut("mergeOnChainEventBody")
                                .and_then(|b| b.get_mut("onChainEvent"))
                                .and_then(|e| e.get_mut("signerEventBody"))
                            {
                                body["keyType"] = 1.into();
                            }
                        }
                    }
                    if url.contains("/signersByFid")
                        && let Some(signers) = value["signers"].as_array_mut()
                    {
                        for signer in signers {
                            signer["fid"] = fid.parse::<u64>().unwrap().into();
                            signer["keyType"] = 1.into();
                            if signer.get("requestFid").is_some() {
                                signer["source"] = "SIGNER_SOURCE_OFFCHAIN".into();
                            } else {
                                signer["source"] = "SIGNER_SOURCE_ONCHAIN".into();
                                signer["onchainEvent"] = serde_json::json!({"type":"EVENT_TYPE_SIGNER","fid":fid.parse::<u64>().unwrap(),"signerEventBody":{"key":signer["key"],"keyType":1,"eventType":"SIGNER_EVENT_TYPE_ADD","metadataType":0,"metadata":""}});
                            }
                        }
                    }
                    if url.contains("/onChainEventsByFid")
                        && let Some(events) = value["events"].as_array_mut()
                    {
                        for event in events {
                            event["type"] = "EVENT_TYPE_SIGNER".into();
                            event["fid"] = fid.parse::<u64>().unwrap().into();
                            event["signerEventBody"]["keyType"] = 1.into();
                            event["signerEventBody"]["metadataType"] = 0.into();
                            event["signerEventBody"]["metadata"] = "".into();
                        }
                    }
                    HttpResponse { status: 200, retry_after: None, body: Some(value.to_string().into_bytes()) }
                }
                None => HttpResponse { status: 404, retry_after: None, body: None },
            };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}

        fn index(&self) -> Option<Rc<dyn Index>> {
            None
        }
    }

    /// The follower's storage, in memory.
    #[derive(Default)]
    struct Memory {
        state: RefCell<Option<String>>,
        sets: RefCell<HashMap<String, String>>,
        read_sets: RefCell<Vec<String>>,
        count_overrides: RefCell<Vec<(String, String)>>,
        keys: RefCell<HashMap<String, u64>>,
        summary: RefCell<Option<Snapshot>>,
        /// Kept emoji reactions by reply: the cast replied to, the reply.
        reactions: RefCell<std::collections::BTreeMap<String, (String, ReactionReply)>>,
    }

    impl ActivityStore for Memory {
        fn activity_state(&self) -> LocalBoxFuture<'static, Result<Option<String>, IndexError>> {
            let state = self.state.borrow().clone();
            async move { Ok(state) }.boxed_local()
        }

        fn activity_sets(
            &self,
            names: Vec<String>,
        ) -> LocalBoxFuture<'static, Result<HashMap<String, String>, IndexError>> {
            let sets = self.sets.borrow();
            self.read_sets.borrow_mut().extend(names.iter().cloned());
            let found = names.into_iter().filter_map(|n| Some((n.clone(), sets.get(&n)?.clone()))).collect();
            async move { Ok(found) }.boxed_local()
        }

        fn key_apps(&self, pairs: Vec<String>) -> LocalBoxFuture<'static, Result<HashMap<String, u64>, IndexError>> {
            let keys = self.keys.borrow();
            let found = pairs.into_iter().filter_map(|p| Some((p.clone(), *keys.get(&p)?))).collect();
            async move { Ok(found) }.boxed_local()
        }

        /// Independent in-memory oracle for the SQL's complete window.
        fn count_apps(
            &self,
            keys: Vec<(String, u64)>,
            names: Vec<String>,
            overrides: Vec<(String, String)>,
            expected_active: u64,
        ) -> LocalBoxFuture<'static, Result<Vec<AppCount>, IndexError>> {
            self.count_overrides.borrow_mut().extend(overrides.iter().cloned());
            let overrides: HashMap<_, _> = overrides.into_iter().collect();
            let stored = self.sets.borrow();
            let mut identities = BTreeSet::new();
            for name in names {
                if let Some(body) = overrides.get(&name).or_else(|| stored.get(&name)) {
                    let Some(set) = ExactKeys::decode(body) else {
                        return async { Err(IndexError("invalid stored key window".into())) }.boxed_local();
                    };
                    identities.extend(set.as_slice().iter().cloned());
                }
            }
            let accounts: BTreeSet<_> = identities.iter().map(|key| activity::identity_fid(key).unwrap()).collect();
            if accounts.len() as u64 != expected_active {
                return async { Err(IndexError("key window account count mismatch".into())) }.boxed_local();
            }
            for (key, app) in keys {
                self.keys.borrow_mut().entry(key).or_insert(app);
            }
            let known = self.keys.borrow();
            let mut by_app: std::collections::BTreeMap<Option<u64>, (BTreeSet<u64>, Vec<String>)> =
                std::collections::BTreeMap::new();
            for key in identities {
                let app = known.get(&key).copied();
                let (fids, unknown) = by_app.entry(app).or_default();
                fids.insert(activity::identity_fid(&key).unwrap());
                if app.is_none() && unknown.len() < BACKLOG {
                    unknown.push(key);
                }
            }
            let out = by_app.into_iter().map(|(app, (fids, unknown))| (app, fids.len() as u64, unknown)).collect();
            async move { Ok(out) }.boxed_local()
        }

        fn save_activity(&self, saving: Saving) -> LocalBoxFuture<'static, Result<(), IndexError>> {
            if *self.state.borrow() != saving.expected_state {
                return async { Err(IndexError("activity state changed".into())) }.boxed_local();
            }
            let mut stored = self.sets.borrow_mut();
            stored.extend(saving.sets);
            stored.retain(|name, _| {
                let (kind, n) = name.strip_prefix("v2").unwrap().split_at(1);
                let n: i64 = n.parse().unwrap();
                if kind == "d" || kind == "w" { n >= saving.keep_from.1 } else { n >= saving.keep_from.0 }
            });
            let mut keys = self.keys.borrow_mut();
            for (pair, app) in saving.keys {
                keys.entry(pair).or_insert(app);
            }
            // As the Worker's statements do: add, then drop those taken back
            // and those too old.
            let mut reactions = self.reactions.borrow_mut();
            for (cast, reply) in saving.reactions {
                reactions.entry(reply.hash.clone()).or_insert((cast, reply));
            }
            reactions.retain(|hash, (_, reply)| !saving.removed.contains(hash) && reply.ts >= saving.reactions_from);
            *self.state.borrow_mut() = Some(saving.state);
            *self.summary.borrow_mut() = Some(saving.summary);
            async { Ok(()) }.boxed_local()
        }
    }

    fn farcaster(unix: i64) -> i64 {
        unix - allthing_core::latest::FARCASTER_EPOCH
    }

    fn block(block: u64, at: i64) -> String {
        format!(
            r#"{{"type":"HUB_EVENT_TYPE_BLOCK_CONFIRMED","id":{},"blockNumber":{block},"blockConfirmedBody":{{"blockNumber":{block},"timestamp":{}}}}}"#,
            block << 14,
            farcaster(at)
        )
    }

    fn merge(block: u64, seq: u64, fid: u64, kind: &str) -> String {
        format!(
            r#"{{"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":{},"blockNumber":{block},"mergeMessageBody":{{"message":{{"data":{{"type":"MESSAGE_TYPE_{kind}","fid":{fid},"timestamp":1}},"signer":"0x{seq:064x}"}}}}}}"#,
            (block << 14) | seq
        )
    }

    /// A reply by `fid` with `text` to the cast `parent`.
    fn reply(block: u64, seq: u64, fid: u64, hash: &str, text: &str, parent: &str) -> String {
        format!(
            r#"{{"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":{},"blockNumber":{block},"mergeMessageBody":{{"message":{{"hash":"{hash}","data":{{"type":"MESSAGE_TYPE_CAST_ADD","fid":{fid},"timestamp":1,"castAddBody":{{"text":"{text}","parentCastId":{{"fid":3,"hash":"{parent}"}}}}}},"signer":"0x{seq:064x}"}}}}}}"#,
            (block << 14) | seq
        )
    }

    fn page(events: &[String]) -> String {
        let mut events: Vec<serde_json::Value> = events.iter().map(|e| serde_json::from_str(e).unwrap()).collect();
        let totals: HashMap<u64, u64> = events
            .iter()
            .map(|e| {
                let id = e["id"].as_u64().unwrap();
                (id >> 14, (id & 16_383) + 1)
            })
            .fold(HashMap::new(), |mut out, (block, n)| {
                out.entry(block).and_modify(|prior| *prior = (*prior).max(n)).or_insert(n);
                out
            });
        for event in &mut events {
            let block = event["id"].as_u64().unwrap() >> 14;
            if let Some(body) = event.get_mut("blockConfirmedBody") {
                body["totalEvents"] = totals[&block].into();
            }
        }
        serde_json::json!({ "events": events }).to_string()
    }

    fn verified_state(mut follow: Follow) -> String {
        follow.verification_version = VERIFICATION_VERSION;
        follow.keys_since = follow.since;
        follow.starts = SHARDS.into_iter().map(|shard| Reached { shard, at: follow.since.unwrap() }).collect();
        follow.blocks = follow
            .cursors
            .iter()
            .map(|c| activity::BlockProgress {
                shard: c.shard,
                block: c.event_id >> 14,
                at: follow.reached.iter().find(|r| r.shard == c.shard).unwrap().at + 1,
                total: ((c.event_id & 16_383) + 1) as u32,
                seen: ((c.event_id & 16_383) + 1) as u32,
            })
            .collect();
        serde_json::to_string(&follow).unwrap()
    }

    fn run_once(hub: &Rc<Hub>, store: &Memory) -> Result<Report, IndexError> {
        let inv = Invocation::new(hub.clone(), 45);
        run(&inv, store).now_or_never().unwrap()
    }

    fn state(store: &Memory) -> Follow {
        serde_json::from_str(store.state.borrow().as_deref().unwrap()).unwrap()
    }

    #[test]
    fn a_run_keeps_emoji_reactions_and_drops_those_taken_back_or_too_old() {
        let at = NOW - 90;
        let (cast, mine, theirs) = (
            "0x1111111111111111111111111111111111111111",
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        let mut hub = Hub::default();
        for shard in ["1", "2"] {
            hub.events.insert((shard.into(), "newest".into()), page(&[block(HEAD, NOW)]));
        }
        let from = (START << 14).to_string();
        hub.events.insert(
            ("1".into(), from.clone()),
            page(&[
                block(START, at),
                reply(START, 1, 42, mine, ":pepe_no:", cast),
                reply(START, 2, 7, theirs, ":pepe_heart_hug:", cast),
                reply(START, 3, 9, "0xcccccccccccccccccccccccccccccccccccccccc", "words, not a reaction", cast),
            ]),
        );
        hub.events.insert(("2".into(), from), page(&[block(START, at)]));
        let store = Memory::default();
        // Kept from long ago: too old to keep now.
        let old = ReactionReply { hash: "0xdd".into(), emoji: "pepe_no".into(), fid: 5, ts: NOW - 401 * 86_400 };
        store.reactions.borrow_mut().insert("0xdd".into(), (cast.into(), old));
        run_once(&Rc::new(hub), &store).unwrap();
        let kept = |store: &Memory| {
            store
                .reactions
                .borrow()
                .values()
                .map(|(c, r)| (c.clone(), r.hash.clone(), r.emoji.clone(), r.fid, r.ts))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kept(&store),
            [
                (cast.into(), mine.into(), "pepe_no".into(), 42, at),
                (cast.into(), theirs.into(), "pepe_heart_hug".into(), 7, at),
            ]
        );

        // The next run reads 42 deleting theirs: it's gone.
        let mut hub = Hub::default();
        let after_1 = ((START << 14) | 3) + 1;
        let removal = format!(
            r#"{{"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":{},"blockNumber":{},"mergeMessageBody":{{"message":{{"hash":"0x9999999999999999999999999999999999999999","data":{{"type":"MESSAGE_TYPE_CAST_REMOVE","fid":42,"timestamp":1,"castRemoveBody":{{"targetHash":"qqqqqqqqqqqqqqqqqqqqqqqqqqo="}}}},"signer":"0x01"}}}}}}"#,
            ((START + 1) << 14) | 1,
            START + 1
        );
        hub.events.insert(("1".into(), after_1.to_string()), page(&[block(START + 1, at + 1), removal]));
        hub.events.insert((("2".into()), ((START << 14) + 1).to_string()), page(&[]));
        run_once(&Rc::new(hub), &store).unwrap();
        assert_eq!(kept(&store), [(cast.into(), theirs.into(), "pepe_heart_hug".into(), 7, at)]);
    }

    #[test]
    fn a_first_run_starts_at_yesterday_and_counts_each_account_once() {
        let at = NOW - 90;
        let mut hub = Hub::default();
        for shard in ["1", "2"] {
            hub.events.insert((shard.into(), "newest".into()), page(&[block(HEAD, NOW)]));
        }
        let from = (START << 14).to_string();
        hub.events.insert(
            ("1".into(), from.clone()),
            page(&[
                block(START, at),
                merge(START, 1, 42, "CAST_ADD"),
                merge(START, 2, 7, "REACTION_ADD"),
                merge(START, 3, 42, "LINK_ADD"),
            ]),
        );
        hub.events.insert(
            ("2".into(), from),
            page(&[
                block(START, at + 30),
                merge(START, 1, 42, "REACTION_ADD"),
                merge(START, 2, 9, "CAST_ADD"),
                merge(START, 3, 5, "NONE"),
            ]),
        );
        let hub = Rc::new(hub);
        let store = Memory::default();
        let report = run_once(&hub, &store).unwrap();
        assert_eq!(report.events, [(1, 4), (2, 4)]);
        assert_eq!((report.actions, report.behind, report.failed.len(), report.counted_30d), (5, Some(91), 0, false));
        let follow = state(&store);
        assert_eq!(follow.since, Some(at + 31));
        assert_eq!(follow.reached(), Some(at - 1));
        let today = follow.days.last().unwrap();
        assert_eq!((today.day, today.active), (day(at), 3));
        assert_eq!(today.actions[&allthing_core::activity::Family::Cast], 2);
        assert_eq!(Fids::decode(&store.sets.borrow()[&hour_set(activity::bucket(at))]), Some(Fids::of([7, 9, 42])));
        let payload: ActivityPayload = serde_json::from_str(&store.summary.borrow().as_ref().unwrap().body).unwrap();
        // Counting began 90 seconds ago: no whole window yet.
        assert_eq!((payload.rolling_24h, payload.days.len(), payload.since), (None, 1, Some(at + 31)));

        // The next run reads on from each shard's cursor. Shard 2 can't be
        // read: it keeps its place, and an account seen again isn't
        // counted twice.
        let mut hub = Hub::default();
        let after_1 = ((START << 14) | 3) + 1;
        hub.events.insert(
            ("1".into(), after_1.to_string()),
            page(&[block(START + 1, at + 1), merge(START + 1, 1, 7, "CAST_ADD"), merge(START + 1, 2, 100, "CAST_ADD")]),
        );
        let hub = Rc::new(hub);
        let report = run_once(&hub, &store).unwrap();
        assert_eq!(report.failed, [2]);
        let follow = state(&store);
        assert_eq!(follow.days.last().unwrap().active, 4);
        assert_eq!(follow.cursors.iter().find(|c| c.shard == 2).map(|c| c.event_id), Some((START << 14) | 3));
        assert!(follow.gaps.is_empty());
        assert!(hub.requests.borrow().iter().all(|url| !url.contains("reverse")), "no second start");
    }

    #[test]
    fn blocks_the_node_no_longer_has_are_a_gap() {
        let at = NOW - 3_600;
        let mut follow = Follow {
            cursors: vec![Cursor { shard: 1, event_id: START << 14 }, Cursor { shard: 2, event_id: START << 14 }],
            reached: vec![Reached { shard: 1, at }, Reached { shard: 2, at }],
            since: Some(at - 86_400 * 3),
            ..Follow::default()
        };
        follow.day_mut(day(at)).active = 0;
        let store = Memory::default();
        *store.state.borrow_mut() = Some(verified_state(follow.clone()));
        let mut hub = Hub::default();
        let from = ((START << 14) + 1).to_string();
        // Shard 1 goes on from the next block; shard 2 jumps 100 blocks.
        hub.events.insert(("1".into(), from.clone()), page(&[block(START + 1, at + 1)]));
        hub.events
            .insert(("2".into(), from), page(&[block(START + 100, at + 100), merge(START + 100, 1, 3, "CAST_ADD")]));
        run_once(&Rc::new(hub), &store).unwrap();
        let follow = state(&store);
        assert_eq!(follow.gaps, [(at + 1, at + 101)]);
        assert!(!follow.covered(at - 10, at + 200));
    }

    fn pair(fid: u64, key: &str) -> String {
        activity::signer_identity(fid, &format!("0x{:0>64}", key.trim_start_matches("0x"))).unwrap()
    }

    /// Read to a minute ago from 3 days back, keys kept all along, with
    /// nothing new on either shard.
    fn counting(store: &Memory, hub: &mut Hub) -> i64 {
        let at = NOW - 60;
        let follow = Follow {
            cursors: vec![Cursor { shard: 1, event_id: START << 14 }, Cursor { shard: 2, event_id: START << 14 }],
            reached: vec![Reached { shard: 1, at }, Reached { shard: 2, at }],
            since: Some(at - 3 * 86_400),
            keys_since: Some(at - 3 * 86_400),
            ..Follow::default()
        };
        *store.state.borrow_mut() = Some(verified_state(follow.clone()));
        let from = ((START << 14) + 1).to_string();
        hub.events.insert(("1".into(), from.clone()), page(&[]));
        hub.events.insert(("2".into(), from), page(&[]));
        hour(at)
    }

    #[test]
    fn accounts_are_counted_by_app_and_unknown_keys_looked_up() {
        let store = Memory::default();
        let mut hub = Hub::default();
        let now_hour = counting(&store, &mut hub);
        // In the window: fid 42 signed with a Farcaster key and a key not
        // looked up yet, fid 7 with a key no app requested, fid 9 with a
        // key not looked up yet. The hour in progress isn't in it.
        {
            let mut sets = store.sets.borrow_mut();
            sets.insert(hour_set(now_hour - 1), Fids::of([7, 42]).encode());
            sets.insert(hour_set(now_hour - 20), Fids::of([9, 42]).encode());
            sets.insert(key_set(now_hour - 1), ExactKeys::of([pair(42, "0x01"), pair(7, "0x01")]).encode());
            sets.insert(key_set(now_hour - 20), ExactKeys::of([pair(42, "0x02"), pair(9, "0x03")]).encode());
            sets.insert(key_set(now_hour), ExactKeys::of([pair(5, "0x05")]).encode());
            let mut keys = store.keys.borrow_mut();
            keys.insert(pair(42, "0x01"), 9152);
            keys.insert(pair(7, "0x01"), 0);
        }
        hub.signers.insert("9".into(), r#"{"signers":[{"key":"0x03","requestFid":309857}]}"#.into());
        // The apps' own accounts, for the page that names them.
        hub.users.insert(
            "9152".into(),
            r#"{"users":[{"fid":9152,"username":"warpcast","display_name":"Warpcast"}]}"#.into(),
        );
        hub.users.insert(
            "309857".into(),
            r#"{"users":[{"fid":309857,"username":"baseapp.base.eth","display_name":"Base App"}]}"#.into(),
        );
        // Fid 42's second key has been removed since: its onchain history has it.
        hub.signers.insert("42".into(), r#"{"signers":[{"key":"0x01","requestFid":9152}]}"#.into());
        hub.history.insert(
            "42".into(),
            r#"{"events":[{"signerEventBody":{"key":"0x02","eventType":"SIGNER_EVENT_TYPE_ADD"}}]}"#.into(),
        );
        let hub = Rc::new(hub);
        let report = run_once(&hub, &store).unwrap();
        assert!(report.counted_apps);
        assert_eq!(report.looked_up, 0, "nothing signed this run, and no backlog yet");
        let follow = state(&store);
        let window = follow.apps_24h.clone().unwrap();
        let to = now_hour * 3_600;
        assert_eq!((window.to, window.counted, window.active), (to, NOW, 3));
        assert_eq!(window.apps, [AppActive { app: 9152, active: 1 }]);
        let named: Vec<(u64, String)> = follow.app_accounts.iter().map(|a| (a.fid, a.name().to_owned())).collect();
        assert_eq!(named, [(9152, "Warpcast".to_owned())]);
        assert_eq!(follow.named, Some(day(NOW)));
        assert_eq!((window.none, window.unknown, window.known_percent()), (1, 2, 33));
        assert_eq!(follow.backlog, [pair(9, "0x03"), pair(42, "0x02")]);
        let payload: ActivityPayload = serde_json::from_str(&store.summary.borrow().as_ref().unwrap().body).unwrap();
        assert_eq!(payload.apps_24h.as_ref(), Some(&window));

        // The next run looks up the backlog, but doesn't count again within
        // 10 minutes; then the window is counted with the keys' apps.
        let report = run_once(&hub, &store).unwrap();
        assert_eq!((report.looked_up, report.counted_apps), (2, false));
        assert!(hub.requests.borrow().iter().any(|url| url.contains("/onChainEventsByFid?fid=42&")));
        let mut later = Hub { later: 600, ..Hub::default() };
        counting_keep(&store, &mut later, &hub);
        let later = Rc::new(later);
        let report = run_once(&later, &store).unwrap();
        assert_eq!((report.looked_up, report.counted_apps), (0, true));
        let window = state(&store).apps_24h.unwrap();
        assert_eq!(window.apps, [AppActive { app: 9152, active: 1 }, AppActive { app: 309_857, active: 1 }]);
        assert_eq!((window.none, window.unknown, window.known_percent()), (2, 0, 100));
        // Only the app not named yet is read; the payload names both.
        let payload: ActivityPayload = serde_json::from_str(&store.summary.borrow().as_ref().unwrap().body).unwrap();
        let named: Vec<&str> = payload.app_accounts.iter().map(|a| a.name()).collect();
        assert_eq!(named, ["Warpcast", "Base App"]);
        let reads =
            later.requests.borrow().iter().filter(|url| url.contains("/user/bulk")).cloned().collect::<Vec<_>>();
        assert_eq!(reads.len(), 1);
        assert!(reads[0].ends_with("fids=309857"), "{reads:?}");
        assert_eq!(store.keys.borrow()[&pair(42, "0x02")], 0);
        assert!(state(&store).backlog.is_empty());
        // Known now: an hour later, the window has moved on and is counted
        // again, with nothing to look up.
        let mut hour_on = Hub { later: 3_600, ..Hub::default() };
        counting_keep(&store, &mut hour_on, &later);
        let report = run_once(&Rc::new(hour_on), &store).unwrap();
        assert_eq!((report.looked_up, report.counted_apps), (0, false), "reading hasn't reached the next hour");
    }

    #[test]
    fn lookups_leave_subrequests_for_the_count_and_the_save() {
        let store = Memory::default();
        let mut hub = Hub::default();
        let now_hour = counting(&store, &mut hub);
        store
            .sets
            .borrow_mut()
            .insert(key_set(now_hour - 1), ExactKeys::of((1..=40).map(|fid| pair(fid, "0x01"))).encode());
        store.sets.borrow_mut().insert(hour_set(now_hour - 1), Fids::of(1..=40).encode());
        // Every account has a key removed since, which takes a second read.
        for fid in 1..=40 {
            hub.signers.insert(fid.to_string(), r#"{"signers":[]}"#.into());
            hub.history.insert(fid.to_string(), r#"{"events":[]}"#.into());
        }
        let hub = Rc::new(hub);
        // The first run counts the 40 accounts' keys as not looked up.
        let run = |budget| run(&Invocation::new(hub.clone(), budget), &store).now_or_never().unwrap();
        assert!(run(45).unwrap().counted_apps);
        assert_eq!(state(&store).backlog.len(), 40);
        // 24 accounts a run, and their histories while 8 subrequests are
        // left; the save always has its own.
        let before = hub.requests.borrow().len();
        let report = run(45).unwrap();
        assert_eq!(report.looked_up, 24);
        let requests = hub.requests.borrow()[before..].to_vec();
        let histories = requests.iter().filter(|url| url.contains("/onChainEventsByFid")).count();
        assert!(histories > 0 && histories < 24, "{histories}");
        assert!(store.summary.borrow().is_some());
        // Short of subrequests, a run looks nothing up and still saves; with
        // none for the pages, it reads them again next time.
        assert_eq!(run(7).unwrap().looked_up, 0);
        let short = run(3).unwrap();
        assert_eq!((short.failed, short.looked_up), (vec![1, 2], 0));
    }

    /// The same pages and accounts as `from` has.
    fn counting_keep(_store: &Memory, hub: &mut Hub, from: &Hub) {
        hub.events = from.events.clone();
        hub.signers = from.signers.clone();
        hub.history = from.history.clone();
        hub.users = from.users.clone();
    }

    #[test]
    fn keys_added_onchain_need_no_lookup_and_new_keys_are_looked_up_as_they_sign() {
        let store = Memory::default();
        let mut hub = Hub::default();
        let now_hour = counting(&store, &mut hub);
        let from = ((START << 14) + 1).to_string();
        // Fid 100 adds a Farcaster key and casts with it; fid 8 reacts with
        // a key not known yet.
        let metadata = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAjwAAAAAAAAAAAAAAAAALveQ3XmTo1/YR8BT7drpQNBVWWAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAZkXc9QAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABBkm8PZXmsC5a/2h4CHNOJ+1vP3cEQcvCkzn7pGxnuiV8fz0SfwJzkTYNZO3syUdrWKe2rOJmaBvrTewq+CW2LdRwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let added = format!(
            r#"{{"type":"HUB_EVENT_TYPE_MERGE_ON_CHAIN_EVENT","id":{},"blockNumber":{},"mergeOnChainEventBody":{{"onChainEvent":{{"type":"EVENT_TYPE_SIGNER","fid":100,"signerEventBody":{{"key":"0x01","eventType":"SIGNER_EVENT_TYPE_ADD","metadata":"{metadata}","metadataType":1}}}}}}}}"#,
            ((START + 1) << 14) | 1,
            START + 1
        );
        let at = NOW - 30;
        hub.events.insert(
            ("1".into(), from),
            page(&[
                block(START + 1, at),
                added,
                merge(START + 1, 2, 100, "CAST_ADD").replace(&format!("0x{:064x}", 2), &format!("0x{:064x}", 1)),
                merge(START + 1, 3, 8, "REACTION_ADD"),
            ]),
        );
        hub.signers.insert("8".into(), r#"{"signers":[{"key":"0x03","requestFid":309857}]}"#.into());
        let hub = Rc::new(hub);
        let report = run_once(&hub, &store).unwrap();
        assert_eq!((report.learned, report.looked_up), (1, 1));
        let requests = hub.requests.borrow();
        assert!(requests.iter().any(|url| url.contains("/signersByFid?fid=8&")));
        assert!(!requests.iter().any(|url| url.contains("fid=100")), "{requests:?}");
        let keys = store.keys.borrow();
        assert_eq!((keys[&pair(100, "0x01")], keys[&pair(8, "0x03")]), (9152, 309_857));
        // This run's keys are kept in their hour.
        let set = ExactKeys::decode(&store.sets.borrow()[&key_set(activity::bucket(at))]).unwrap();
        assert_eq!(set, ExactKeys::of([pair(8, "0x03"), pair(100, "0x01")]));
        assert_eq!(activity::bucket(at), now_hour);
    }

    #[test]
    fn the_30_day_window_is_counted_once_a_day_when_keeping_up() {
        let at = NOW - 60;
        let today = day(at);
        let store = Memory::default();
        // Yesterday and the day before were read whole, and their sets kept.
        store.sets.borrow_mut().insert(day_set(today - 1), Fids::of([1, 2, 3]).encode());
        store.sets.borrow_mut().insert(day_set(today - 2), Fids::of([3, 4]).encode());
        store.sets.borrow_mut().insert(day_set(today - 40), Fids::of([9]).encode());
        let follow = Follow {
            cursors: vec![Cursor { shard: 1, event_id: START << 14 }, Cursor { shard: 2, event_id: START << 14 }],
            reached: vec![Reached { shard: 1, at }, Reached { shard: 2, at }],
            since: Some((today - 2) * 86_400),
            ..Follow::default()
        };
        *store.state.borrow_mut() = Some(verified_state(follow.clone()));
        let mut hub = Hub::default();
        let from = ((START << 14) + 1).to_string();
        hub.events.insert(("1".into(), from.clone()), page(&[]));
        hub.events.insert(("2".into(), from), page(&[]));
        let hub = Rc::new(hub);
        assert!(run_once(&hub, &store).unwrap().counted_30d);
        let window = state(&store).active_30d.unwrap();
        // Begun two days ago: counted, but not over 30 whole days.
        assert_eq!((window.day, window.active, window.complete), (today - 1, 4, false));
        assert!(!run_once(&hub, &store).unwrap().counted_30d, "once a day");
        // Sets older than the window are dropped.
        assert!(!store.sets.borrow().contains_key(&day_set(today - 40)));
        // The week isn't kept until it was read in full.
        assert!(!store.sets.borrow().keys().any(|name| name.starts_with("v2w")));
    }

    #[test]
    fn the_week_to_yesterday_is_kept_as_one_set_once_a_day_once_read_in_full() {
        let at = NOW - 60;
        let today = day(at);
        let store = Memory::default();
        // Each of the past 8 days' accounts: the 7 to yesterday, and one before.
        for d in 1..=8 {
            store.sets.borrow_mut().insert(day_set(today - d), Fids::of([d as u64, 100 + d as u64, 500]).encode());
        }
        store.sets.borrow_mut().insert(week_set(today - 40), Fids::of([9]).encode());
        let follow = Follow {
            cursors: vec![Cursor { shard: 1, event_id: START << 14 }, Cursor { shard: 2, event_id: START << 14 }],
            reached: vec![Reached { shard: 1, at }, Reached { shard: 2, at }],
            since: Some((today - 7) * 86_400),
            ..Follow::default()
        };
        *store.state.borrow_mut() = Some(verified_state(follow.clone()));
        let mut hub = Hub::default();
        let from = ((START << 14) + 1).to_string();
        hub.events.insert(("1".into(), from.clone()), page(&[]));
        hub.events.insert(("2".into(), from), page(&[]));
        let hub = Rc::new(hub);
        assert!(run_once(&hub, &store).unwrap().counted_30d);
        let week = Fids::decode(&store.sets.borrow()[&week_set(today - 1)]).unwrap();
        assert_eq!(week, Fids::of([1, 2, 3, 4, 5, 6, 7, 101, 102, 103, 104, 105, 106, 107, 500]));
        // Kept as the days are: the old one goes.
        assert!(!store.sets.borrow().contains_key(&week_set(today - 40)));
        // Once a day: the next run writes it no more.
        store.sets.borrow_mut().remove(&week_set(today - 1));
        assert!(!run_once(&hub, &store).unwrap().counted_30d);
        assert!(!store.sets.borrow().contains_key(&week_set(today - 1)));
        // A gap in the week: not read in full, so not kept.
        let store = Memory::default();
        let gapped = Follow { gaps: vec![((today - 3) * 86_400, (today - 3) * 86_400 + 60)], ..follow };
        *store.state.borrow_mut() = Some(verified_state(gapped));
        assert!(run_once(&hub, &store).unwrap().counted_30d);
        assert!(!store.sets.borrow().keys().any(|name| name.starts_with("v2w")));
    }
    #[test]
    fn unreadable_or_legacy_state_is_never_restarted_or_certified() {
        for raw in ["broken JSON", r#"{"since":1700000000}"#] {
            let store = Memory::default();
            *store.state.borrow_mut() = Some(raw.into());
            let hub = Rc::new(Hub::default());
            assert!(run_once(&hub, &store).is_err());
            assert_eq!(store.state.borrow().as_deref(), Some(raw));
            assert!(store.summary.borrow().is_none());
            assert!(hub.requests.borrow().is_empty());
        }
    }

    #[test]
    fn a_present_corrupt_set_stops_the_run_without_overwriting_it() {
        let store = Memory::default();
        let mut hub = Hub::default();
        let now_hour = counting(&store, &mut hub);
        store.sets.borrow_mut().insert(hour_set(now_hour - 1), "not a set".into());
        let before = store.state.borrow().clone();
        assert!(run_once(&Rc::new(hub), &store).is_err());
        assert_eq!(*store.state.borrow(), before);
        assert_eq!(store.sets.borrow()[&hour_set(now_hour - 1)], "not a set");
        assert!(store.summary.borrow().is_none());
    }

    #[test]
    fn a_missing_first_shard_cannot_produce_a_complete_window() {
        let at = NOW - 60;
        let store = Memory::default();
        let mut hub = Hub::default();
        for shard in ["1", "2"] {
            hub.events.insert((shard.into(), "newest".into()), page(&[block(HEAD, NOW)]));
        }
        hub.events.insert(
            ("1".into(), (START << 14).to_string()),
            page(&[block(START, at), merge(START, 1, 42, "CAST_ADD")]),
        );
        let report = run_once(&Rc::new(hub), &store).unwrap();
        assert_eq!(report.failed, [2]);
        let state = state(&store);
        assert_eq!(state.reached(), None);
        assert_eq!(state.since, None);
        assert!(!state.covered(at - 86_400, at));
        let payload: ActivityPayload = serde_json::from_str(&store.summary.borrow().as_ref().unwrap().body).unwrap();
        assert_eq!(payload.rolling_24h, None);
        assert_eq!(payload.verification_version, VERIFICATION_VERSION);
        assert!(payload.days.iter().all(|day| !day.complete));
    }

    #[test]
    fn the_first_daily_and_weekly_counts_include_the_page_finishing_yesterday() {
        let midnight = day(NOW) * 86_400;
        let store = Memory::default();
        let follow = Follow {
            cursors: SHARDS.into_iter().map(|shard| Cursor { shard, event_id: START << 14 }).collect(),
            reached: SHARDS.into_iter().map(|shard| Reached { shard, at: midnight - 3 }).collect(),
            since: Some(midnight - 30 * 86_400),
            ..Follow::default()
        };
        *store.state.borrow_mut() = Some(verified_state(follow));
        store.sets.borrow_mut().insert(day_set(day(midnight) - 1), Fids::of([7]).encode());
        let mut hub = Hub { later: midnight + 60 - NOW, ..Hub::default() };
        let from = ((START << 14) + 1).to_string();
        hub.events.insert(
            ("1".into(), from.clone()),
            page(&[
                block(START + 1, midnight - 1),
                merge(START + 1, 1, 42, "CAST_ADD"),
                block(START + 2, midnight + 1),
            ]),
        );
        hub.events.insert(("2".into(), from), page(&[block(START + 1, midnight + 1)]));
        let report = run_once(&Rc::new(hub), &store).unwrap();
        assert!(report.counted_30d);
        let state = state(&store);
        assert_eq!(state.active_30d.unwrap().active, 2);
        assert!(state.active_30d.unwrap().complete);
        let week = Fids::decode(&store.sets.borrow()[&week_set(day(midnight) - 1)]).unwrap();
        assert_eq!(week, Fids::of([7, 42]), "the final page must be in both unions before publication");
    }

    #[test]
    fn repeated_or_future_pages_leave_each_shards_cursor_and_counts_unchanged() {
        let store = Memory::default();
        let mut hub = Hub::default();
        counting(&store, &mut hub);
        let before = state(&store);
        let from = ((START << 14) + 1).to_string();
        hub.events.insert(("1".into(), from.clone()), page(&[block(START, NOW - 59)]));
        hub.events.insert(("2".into(), from), page(&[block(START + 1, NOW + 1)]));
        let report = run_once(&Rc::new(hub), &store).unwrap();
        assert_eq!(report.failed, [1, 2]);
        let after = state(&store);
        assert_eq!(after.cursors, before.cursors);
        assert_eq!(after.blocks, before.blocks);
        assert_eq!(after.days, before.days);
    }
    #[test]
    fn counting_apps_keeps_old_hourly_key_sets_inside_the_store() {
        let store = Memory::default();
        let mut hub = Hub::default();
        let now_hour = counting(&store, &mut hub);
        store.sets.borrow_mut().insert(hour_set(now_hour - 1), Fids::of([42]).encode());
        store.sets.borrow_mut().insert(key_set(now_hour - 1), ExactKeys::of([pair(42, "0x01")]).encode());
        assert!(run_once(&Rc::new(hub), &store).unwrap().counted_apps);
        assert!(
            store.read_sets.borrow().iter().all(|name| !name.starts_with("v2k")),
            "the Worker must not read the 24-hour key window"
        );
        assert_eq!(state(&store).apps_24h.unwrap().unknown, 1);
    }

    #[test]
    fn the_store_counts_current_batch_overrides_before_they_are_saved() {
        let midnight = day(NOW) * 86_400;
        let store = Memory::default();
        let follow = Follow {
            cursors: SHARDS.into_iter().map(|shard| Cursor { shard, event_id: START << 14 }).collect(),
            reached: SHARDS.into_iter().map(|shard| Reached { shard, at: midnight - 3 }).collect(),
            since: Some(midnight - 3 * 86_400),
            ..Follow::default()
        };
        *store.state.borrow_mut() = Some(verified_state(follow));
        let prior_hour = hour(midnight) - 1;
        store.sets.borrow_mut().insert(hour_set(prior_hour), Fids::of([7]).encode());
        store.sets.borrow_mut().insert(key_set(prior_hour), ExactKeys::of([pair(7, "0x01")]).encode());
        let mut hub = Hub { later: midnight + 60 - NOW, ..Hub::default() };
        let from = ((START << 14) + 1).to_string();
        hub.events.insert(
            ("1".into(), from.clone()),
            page(&[
                block(START + 1, midnight - 1),
                merge(START + 1, 1, 42, "CAST_ADD"),
                block(START + 2, midnight + 1),
            ]),
        );
        hub.events.insert(("2".into(), from), page(&[block(START + 1, midnight + 1)]));
        assert!(run_once(&Rc::new(hub), &store).unwrap().counted_apps);
        let window = state(&store).apps_24h.unwrap();
        assert_eq!((window.active, window.unknown), (2, 2));
        assert_eq!(store.count_overrides.borrow().len(), 1);
        assert_eq!(
            ExactKeys::decode(&store.count_overrides.borrow()[0].1).unwrap(),
            ExactKeys::of([pair(7, "0x01"), pair(42, "0x01")])
        );
    }

    #[test]
    fn absent_or_malformed_key_windows_cannot_claim_all_accounts_are_known() {
        for corrupt in [None, Some("broken JSON"), Some(r#"["0000000042:0x01"]"#)] {
            let store = Memory::default();
            let mut hub = Hub::default();
            let now_hour = counting(&store, &mut hub);
            store.sets.borrow_mut().insert(hour_set(now_hour - 1), Fids::of([42]).encode());
            if let Some(body) = corrupt {
                store.sets.borrow_mut().insert(key_set(now_hour - 1), body.into());
            }
            let before = store.state.borrow().clone();
            assert!(run_once(&Rc::new(hub), &store).is_err());
            assert_eq!(*store.state.borrow(), before);
            assert!(store.summary.borrow().is_none());
            assert!(store.read_sets.borrow().iter().all(|name| !name.starts_with("v2k")));
        }
    }
    #[test]
    fn malformed_derived_state_is_rejected_before_any_upstream_read_or_cursor_advance() {
        let seed = Memory::default();
        let mut hub = Hub::default();
        counting(&seed, &mut hub);
        let base = state(&seed);
        let mut cases = Vec::new();
        let mut bad = base.clone();
        bad.days.push(activity::DayCount { day: i64::MIN, ..activity::DayCount::default() });
        cases.push(bad);
        let mut bad = base.clone();
        bad.days.push(activity::DayCount { day: day(NOW) + 1, ..activity::DayCount::default() });
        cases.push(bad);
        let mut bad = base.clone();
        bad.reached[0].at = 0;
        cases.push(bad);
        let mut bad = base.clone();
        bad.reached.remove(0);
        cases.push(bad);
        let mut bad = base.clone();
        bad.reached[0].at -= 1;
        cases.push(bad);
        let mut bad = base.clone();
        bad.active_30d = Some(Window { day: i64::MAX, active: 1, complete: false });
        cases.push(bad);
        let mut bad = base.clone();
        bad.active_30d = Some(Window { day: day(NOW) - 1, active: 1, complete: true });
        cases.push(bad);
        let mut bad = base.clone();
        bad.days.push(activity::DayCount {
            day: day(NOW) - 4,
            active: 1,
            complete: true,
            ..activity::DayCount::default()
        });
        cases.push(bad);
        let mut bad = base.clone();
        let mut window = AppsWindow::of(activity::rolling_to(NOW) - 4 * 86_400, NOW, 1, [(Some(9152), 1)]);
        window.verification_version = VERIFICATION_VERSION;
        bad.apps_24h = Some(window);
        cases.push(bad);
        let mut bad = base.clone();
        bad.active_30d = Some(Window { day: day(NOW) - 1, active: u64::from(u32::MAX) + 1, complete: false });
        cases.push(bad);
        let mut bad = base.clone();
        bad.gaps.push((i64::MIN, 0));
        cases.push(bad);
        for (index, bad) in cases.into_iter().enumerate() {
            let store = Memory::default();
            let raw = serde_json::to_string(&bad).unwrap();
            *store.state.borrow_mut() = Some(raw.clone());
            let hub = Rc::new(Hub::default());
            assert!(run_once(&hub, &store).is_err(), "corrupt-state case{index}");
            assert!(hub.requests.borrow().is_empty(), "case{index} reached the upstream");
            assert_eq!(store.state.borrow().as_deref(), Some(raw.as_str()));
            assert!(store.summary.borrow().is_none());
            assert!(store.sets.borrow().is_empty());
        }
    }
    fn one_shard_state(first: i64, reached: i64) -> Follow {
        Follow {
            verification_version: VERIFICATION_VERSION,
            starts: vec![Reached { shard: 1, at: first + 1 }],
            cursors: vec![Cursor { shard: 1, event_id: START << 14 }],
            blocks: vec![activity::BlockProgress { shard: 1, block: START, at: reached + 1, total: 1, seen: 1 }],
            reached: vec![Reached { shard: 1, at: reached }],
            ..Follow::default()
        }
    }

    #[test]
    fn missing_first_shard_preserves_old_hours_until_common_coverage_can_count_them() {
        let first = NOW - 73 * 3_600;
        let reached = NOW - 24 * 3_600;
        let active_at = NOW - 60 * 3_600;
        let store = Memory::default();
        *store.state.borrow_mut() = Some(serde_json::to_string(&one_shard_state(first, reached)).unwrap());
        let old_hour = hour_set(activity::bucket(active_at));
        store.sets.borrow_mut().insert(old_hour.clone(), Fids::of([42]).encode());
        let mut waiting = Hub::default();
        waiting.events.insert(("1".into(), ((START << 14) + 1).to_string()), page(&[]));
        waiting.events.insert(("2".into(), "newest".into()), page(&[block(HEAD, NOW)]));
        let report = run_once(&Rc::new(waiting), &store).unwrap();
        assert_eq!(report.failed, [2]);
        assert!(store.sets.borrow().contains_key(&old_hour), "a 60-hour-old set must survive missing common coverage");
        assert_eq!(state(&store).reached(), None);

        let mut joined = Hub::default();
        joined.events.insert(("1".into(), ((START << 14) + 1).to_string()), page(&[]));
        joined.events.insert(("2".into(), "newest".into()), page(&[block(HEAD, NOW)]));
        joined.events.insert(
            ("2".into(), (START << 14).to_string()),
            page(&[block(START, first), block(START + 1, reached + 1)]),
        );
        run_once(&Rc::new(joined), &store).unwrap();
        let payload: ActivityPayload = serde_json::from_str(&store.summary.borrow().as_ref().unwrap().body).unwrap();
        assert!(payload.is_verified(NOW));
        assert_eq!(payload.rolling_24h, Some(0));
        assert_eq!(
            payload.previous_24h,
            Some(1),
            "joining the shards must recover the retained account, not a certified zero"
        );
    }

    #[test]
    fn an_extended_missing_shard_wait_bounds_history_with_an_explicit_expired_gap() {
        let first = NOW - 100 * 86_400;
        let active_at = NOW - 90 * 86_400;
        let store = Memory::default();
        *store.state.borrow_mut() = Some(serde_json::to_string(&one_shard_state(first, NOW - 60)).unwrap());
        let old_hour = hour_set(activity::bucket(active_at));
        store.sets.borrow_mut().insert(old_hour.clone(), Fids::of([42]).encode());
        let mut hub = Hub::default();
        hub.events.insert(("1".into(), ((START << 14) + 1).to_string()), page(&[]));
        hub.events.insert(("2".into(), "newest".into()), page(&[block(HEAD, NOW)]));
        run_once(&Rc::new(hub), &store).unwrap();
        assert!(!store.sets.borrow().contains_key(&old_hour));
        let retained = state(&store);
        assert!(retained.gaps.iter().any(|&(from, to)| from <= active_at && to > active_at));
        assert_eq!(retained.reached(), None);
        assert!(retained.valid(NOW));
    }
}
