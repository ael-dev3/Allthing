// SPDX-License-Identifier: AGPL-3.0-or-later
//! Daily active accounts (see `allthing_core::activity`): the storage the
//! count needs from the runtime, and the cron's run, which reads each
//! shard's hub events on from where the last run stopped and keeps each
//! UTC hour's and day's active fids, and each hour's signing keys. The apps
//! that requested those keys are looked up a few accounts a run and kept
//! (a key's app never changes), and once an hour the past 24 hours' keys
//! are counted by app.

use std::collections::{BTreeSet, HashMap};

use allthing_core::activity::{
    self, ActivityPayload, AppsWindow, Batch, Fids, Follow, KEEP_DAYS, KEEP_HOURS, Keys, NAMED_APPS, Reached, Window,
    day, day_set, hour, hour_set, key_pair, key_set,
};
use allthing_core::api::Meta;
use allthing_core::latest::{Cursor, SHARDS};
use allthing_core::normalize;
use futures::future::{LocalBoxFuture, join_all};

use crate::Invocation;
use crate::annotate;
use crate::hypersnap;
use crate::indexer::{IndexError, Snapshot};

/// Events a run reads per shard: one page, about 13 minutes of events and
/// 400 KB, parsed in about 2.5 ms of the run's 10 ms of CPU. A run every two
/// minutes reads six times faster than events come, so a backlog clears.
pub const PAGE: u32 = 1000;

/// The name the payload is stored and served under.
pub const SUMMARY: &str = "activity";

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
/// Keys counted by app in one statement, about 30 KB of JSON.
const COUNT_CHUNK: usize = 2_000;

/// What the follower writes at the end of a run, in one transaction.
pub struct Saving {
    /// Sets by name.
    pub sets: Vec<(String, String)>,
    /// Drop the hourly sets (of fids and of keys) before hour `.0`, and the
    /// daily ones before day `.1`.
    pub keep_from: (i64, i64),
    /// Keys' apps learned, as `(key_pair, app)`; a key's app never changes.
    pub keys: Vec<(u64, u64)>,
    pub state: String,
    /// The payload served.
    pub summary: Snapshot,
}

/// One app's count among a run of keys: the app (`None` for keys not looked
/// up yet, 0 for keys no app requested), its accounts, and with `None`, the
/// keys.
pub type AppCount = (Option<u64>, u64, Vec<u64>);

/// The follower's storage (D1 in the Worker). Each call is one round trip.
pub trait ActivityStore {
    /// The saved state, as JSON.
    fn activity_state(&self) -> LocalBoxFuture<'static, Result<Option<String>, IndexError>>;
    /// The stored sets of those `names` there are.
    fn activity_sets(&self, names: Vec<String>)
    -> LocalBoxFuture<'static, Result<HashMap<String, String>, IndexError>>;
    /// The apps of those keys (`key_pair`) that have been looked up.
    fn key_apps(&self, pairs: Vec<u64>) -> LocalBoxFuture<'static, Result<HashMap<u64, u64>, IndexError>>;
    /// Record `keys`' apps (as `save_activity` does, so the count knows
    /// them), then count accounts active by app among each run of keys
    /// (`Keys::chunks`).
    fn count_apps(
        &self,
        keys: Vec<(u64, u64)>,
        chunks: Vec<Vec<u64>>,
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
/// yesterday is counted whole. Reading starts after the returned event.
async fn start(inv: &Invocation, shard: u32, now: i64) -> Result<u64, IndexError> {
    let body = hypersnap::events(inv, shard, None, 1).await.map_err(|e| IndexError(e.to_string()))?;
    let newest = activity::activity(&body).and_then(|p| p.last_id).ok_or_else(|| IndexError("no events".into()))?;
    let back = (now - (day(now) - 1) * 86_400).max(0) as u64;
    Ok(((newest >> 14).saturating_sub(back) << 14).saturating_sub(1))
}

/// The app of `pair` among an account's keys, 0 for one no app requested.
fn app_among(keys: &HashMap<String, Option<u64>>, fid: u64, pair: u64) -> Option<u64> {
    keys.iter().find(|(key, _)| key_pair(fid, key) == Some(pair)).map(|(_, app)| app.unwrap_or(0))
}

/// The apps of an account's keys `pairs`, from its active keys (cached in
/// the isolate for an hour, as casts' origins use them), or for a key
/// removed since, its onchain history. Keys whose app couldn't be read are
/// left out, to be looked up again later.
async fn look_up(inv: &Invocation, fid: u64, pairs: Vec<u64>, after: u32) -> Option<Vec<(u64, u64)>> {
    let active = annotate::signer_apps(inv, fid).await?;
    let (mut found, mut removed) = (Vec::new(), Vec::new());
    for pair in pairs {
        match app_among(&active, fid, pair) {
            Some(app) => found.push((pair, app)),
            None => removed.push(pair),
        }
    }
    // Not read (no subrequest to spare, say): those keys are looked up again.
    if !removed.is_empty()
        && inv.remaining_subrequests() > after
        && let Ok(history) = hypersnap::all_onchain_signers(inv, fid).await
    {
        found.extend(removed.into_iter().map(|pair| (pair, app_among(&history, fid, pair).unwrap_or(0))));
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
    let mut follow: Follow =
        store.activity_state().await?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let now = (inv.now() / 1000.0) as i64;
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
    let mut learned: HashMap<u64, u64> = HashMap::new();
    for ((shard, after), page) in afters.into_iter().zip(pages) {
        let Some(page) = page.ok().and_then(|body| activity::activity(&body)) else {
            // Read again from the same place next run.
            report.failed.push(shard);
            continue;
        };
        report.events.push((shard, page.count));
        let previous = follow.reached.iter().find(|r| r.shard == shard).map(|r| r.at);
        if let (Some(previous), Some(first), Some(started)) = (previous, page.first_block, page.started)
            && first > (after >> 14) + 1
        {
            // The node no longer had the blocks between: they are a gap.
            follow.gaps.push((previous, started));
        }
        if let Some(started) = page.started {
            follow.since = Some(follow.since.map_or(started, |s| s.min(started)));
            follow.keys_since = Some(follow.keys_since.map_or(started, |s| s.min(started)));
        }
        if let Some(last_id) = page.last_id {
            follow.cursors.retain(|c| c.shard != shard);
            follow.cursors.push(Cursor { shard, event_id: last_id });
        }
        if let Some(at) = page.reached {
            follow.reached.retain(|r| r.shard != shard);
            follow.reached.push(Reached { shard, at });
        }
        actions.extend(page.actions);
        learned.extend(page.keys);
    }
    report.actions = actions.len();
    report.learned = learned.len();
    let batch = Batch::of(&actions);
    let reached = follow.reached().unwrap_or(now);
    report.behind = Some(now - reached);
    // The windows end as the hour in progress began.
    let (now_hour, today) = (hour(reached), day(reached));
    let to = activity::rolling_to(reached);
    let keeping_up = report.behind.is_some_and(|b| b < CAUGHT_UP);

    // The apps of the keys that signed, where known.
    let signed: Vec<u64> = Keys::of(batch.keys.values().flat_map(|k| k.as_slice().iter().copied())).as_slice().to_vec();
    let mut known = if signed.is_empty() {
        HashMap::new()
    } else {
        d1(inv)?;
        store.key_apps(signed.clone()).await?
    };
    known.extend(learned.iter().map(|(&pair, &app)| (pair, app)));

    // The 30 days to the end of yesterday, once a day, when keeping up (the
    // day sets of a backlog are still filling) and yesterday was read whole.
    let due_30d = keeping_up
        && follow.since.is_some_and(|since| since <= (today - 1) * 86_400)
        && follow.active_30d.is_none_or(|w| w.day < today - 1);
    let due_apps = follow.apps_due(to, now);
    let mut names: BTreeSet<String> = batch.hours.keys().map(|&h| hour_set(h)).collect();
    names.extend(batch.days.keys().map(|&d| day_set(d)));
    names.extend(batch.keys.keys().map(|&h| key_set(h)));
    names.extend((now_hour - 48..now_hour).map(hour_set));
    if due_30d {
        names.extend((today - 30..today).map(day_set));
    }
    if due_apps {
        names.extend((now_hour - 24..now_hour).map(key_set));
    }
    d1(inv)?;
    let stored = store.activity_sets(names.into_iter().collect()).await?;
    let decoded = |name: &str| stored.get(name).and_then(|text| Fids::decode(text)).unwrap_or_default();
    let decoded_keys = |name: &str| stored.get(name).and_then(|text| Keys::decode(text)).unwrap_or_default();

    // Look up the apps of keys not known yet: this run's first, then those
    // the last count left, an account a request, keeping subrequests for
    // keys removed since and for the D1 calls that follow.
    let after = AFTER + u32::from(due_apps);
    let limit = if keeping_up { LOOKUPS } else { LOOKUPS_BEHIND };
    let limit = limit.min(inv.remaining_subrequests().saturating_sub(after + HISTORY) as usize);
    let mut accounts: Vec<(u64, Vec<u64>)> = Vec::new();
    for &pair in signed.iter().filter(|p| !known.contains_key(p)).chain(follow.backlog.iter()) {
        let fid = activity::pair_fid(pair);
        if let Some((_, pairs)) = accounts.iter_mut().find(|(f, _)| *f == fid) {
            if !pairs.contains(&pair) {
                pairs.push(pair);
            }
        } else if accounts.len() < limit {
            accounts.push((fid, vec![pair]));
        }
    }
    report.looked_up = accounts.len();
    let found = join_all(accounts.into_iter().map(|(fid, pairs)| look_up(inv, fid, pairs, after))).await;
    let mut keys: Vec<(u64, u64)> = learned.into_iter().collect();
    for (pair, app) in found.into_iter().flatten().flatten() {
        known.insert(pair, app);
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
    let mut key_sets: HashMap<i64, Keys> = HashMap::new();
    for (h, pairs) in &batch.keys {
        let mut set = decoded_keys(&key_set(*h));
        set.extend(pairs);
        writes.push((key_set(*h), set.encode()));
        key_sets.insert(*h, set);
    }
    for (d, fids) in &batch.days {
        let mut set = decoded(&day_set(*d));
        set.extend(fids);
        writes.push((day_set(*d), set.encode()));
        let count = follow.day_mut(*d);
        count.active = set.len() as u64;
        for (family, n) in batch.actions.get(d).into_iter().flatten() {
            *count.actions.entry(*family).or_default() += n;
        }
    }
    if due_30d {
        let days: Vec<Fids> = (today - 30..today).map(|d| decoded(&day_set(d))).collect();
        let complete = follow.covered((today - 30) * 86_400, today * 86_400);
        follow.active_30d = Some(Window { day: today - 1, active: Fids::union_len(&days) as u64, complete });
        report.counted_30d = true;
    }
    if due_apps && let Some(active) = activity::rolling(&follow, &hours, to) {
        let mut window = Keys::default();
        for h in now_hour - 24..now_hour {
            window.extend(&key_sets.remove(&h).unwrap_or_else(|| decoded_keys(&key_set(h))));
        }
        let chunks = window.chunks(COUNT_CHUNK).into_iter().map(<[u64]>::to_vec).collect();
        d1(inv)?;
        let counts = store.count_apps(keys.clone(), chunks).await?;
        follow.backlog = counts.iter().flat_map(|(_, _, pairs)| pairs.iter().copied()).take(BACKLOG).collect();
        let counts = counts.into_iter().map(|(app, n, _)| (app, n));
        let window = AppsWindow::of(to, now, active, counts);
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
    let body = serde_json::to_string(&payload).map_err(|e| IndexError(e.to_string()))?;
    let state = serde_json::to_string(&follow).map_err(|e| IndexError(e.to_string()))?;
    let keep_from = (now_hour - KEEP_HOURS, today - KEEP_DAYS);
    d1(inv)?;
    let summary = Snapshot { built: inv.now(), body };
    store.save_activity(Saving { sets: writes, keep_from, keys, state, summary }).await?;
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
                Some(body) => HttpResponse { status: 200, retry_after: None, body: Some(body.clone().into_bytes()) },
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
        keys: RefCell<HashMap<u64, u64>>,
        summary: RefCell<Option<Snapshot>>,
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
            let found = names.into_iter().filter_map(|n| Some((n.clone(), sets.get(&n)?.clone()))).collect();
            async move { Ok(found) }.boxed_local()
        }

        fn key_apps(&self, pairs: Vec<u64>) -> LocalBoxFuture<'static, Result<HashMap<u64, u64>, IndexError>> {
            let keys = self.keys.borrow();
            let found = pairs.into_iter().filter_map(|p| Some((p, *keys.get(&p)?))).collect();
            async move { Ok(found) }.boxed_local()
        }

        /// As the SQL does: each run's accounts by app, and its keys not
        /// looked up.
        fn count_apps(
            &self,
            keys: Vec<(u64, u64)>,
            chunks: Vec<Vec<u64>>,
        ) -> LocalBoxFuture<'static, Result<Vec<AppCount>, IndexError>> {
            for (pair, app) in keys {
                self.keys.borrow_mut().entry(pair).or_insert(app);
            }
            let keys = self.keys.borrow();
            let mut out = Vec::new();
            for chunk in chunks {
                let mut by_app: std::collections::BTreeMap<Option<u64>, (BTreeSet<u64>, Vec<u64>)> =
                    std::collections::BTreeMap::new();
                for pair in chunk {
                    let app = keys.get(&pair).copied();
                    let (fids, unknown) = by_app.entry(app).or_default();
                    fids.insert(activity::pair_fid(pair));
                    if app.is_none() {
                        unknown.push(pair);
                    }
                }
                out.extend(by_app.into_iter().map(|(app, (fids, unknown))| (app, fids.len() as u64, unknown)));
            }
            async move { Ok(out) }.boxed_local()
        }

        fn save_activity(&self, saving: Saving) -> LocalBoxFuture<'static, Result<(), IndexError>> {
            let mut stored = self.sets.borrow_mut();
            stored.extend(saving.sets);
            stored.retain(|name, _| {
                let (kind, n) = name.split_at(1);
                let n: i64 = n.parse().unwrap();
                if kind == "d" { n >= saving.keep_from.1 } else { n >= saving.keep_from.0 }
            });
            let mut keys = self.keys.borrow_mut();
            for (pair, app) in saving.keys {
                keys.entry(pair).or_insert(app);
            }
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
            r#"{{"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":{},"blockNumber":{block},"mergeMessageBody":{{"message":{{"data":{{"type":"MESSAGE_TYPE_{kind}","fid":{fid},"timestamp":1}},"signer":"0x0{seq}"}}}}}}"#,
            (block << 14) | seq
        )
    }

    fn page(events: &[String]) -> String {
        format!(r#"{{"events":[{}]}}"#, events.join(","))
    }

    fn run_once(hub: &Rc<Hub>, store: &Memory) -> Result<Report, IndexError> {
        let inv = Invocation::new(hub.clone(), 45);
        run(&inv, store).now_or_never().unwrap()
    }

    fn state(store: &Memory) -> Follow {
        serde_json::from_str(store.state.borrow().as_deref().unwrap()).unwrap()
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
        assert_eq!((report.actions, report.behind, report.failed.len(), report.counted_30d), (5, Some(90), 0, false));
        let follow = state(&store);
        assert_eq!(follow.since, Some(at));
        assert_eq!(follow.reached(), Some(at));
        let today = follow.days.last().unwrap();
        assert_eq!((today.day, today.active), (day(at), 3));
        assert_eq!(today.actions[&allthing_core::activity::Family::Cast], 2);
        assert_eq!(Fids::decode(&store.sets.borrow()[&hour_set(activity::bucket(at))]), Some(Fids::of([7, 9, 42])));
        let payload: ActivityPayload = serde_json::from_str(&store.summary.borrow().as_ref().unwrap().body).unwrap();
        // Counting began 90 seconds ago: no whole window yet.
        assert_eq!((payload.rolling_24h, payload.days.len(), payload.since), (None, 1, Some(at)));

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
        *store.state.borrow_mut() = Some(serde_json::to_string(&follow).unwrap());
        let mut hub = Hub::default();
        let from = ((START << 14) + 1).to_string();
        // Shard 1 goes on from the next block; shard 2 jumps 100 blocks.
        hub.events.insert(("1".into(), from.clone()), page(&[block(START + 1, at + 1)]));
        hub.events
            .insert(("2".into(), from), page(&[block(START + 100, at + 100), merge(START + 100, 1, 3, "CAST_ADD")]));
        run_once(&Rc::new(hub), &store).unwrap();
        let follow = state(&store);
        assert_eq!(follow.gaps, [(at, at + 100)]);
        assert!(!follow.covered(at - 10, at + 200));
    }

    fn pair(fid: u64, key: &str) -> u64 {
        key_pair(fid, key).unwrap()
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
        *store.state.borrow_mut() = Some(serde_json::to_string(&follow).unwrap());
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
            sets.insert(key_set(now_hour - 1), Keys::of([pair(42, "0x01"), pair(7, "0x01")]).encode());
            sets.insert(key_set(now_hour - 20), Keys::of([pair(42, "0x02"), pair(9, "0x03")]).encode());
            sets.insert(key_set(now_hour), Keys::of([pair(5, "0x05")]).encode());
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
        store.sets.borrow_mut().insert(key_set(now_hour - 1), Keys::of((1..=40).map(|fid| pair(fid, "0x01"))).encode());
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
                merge(START + 1, 1, 100, "CAST_ADD"),
                merge(START + 1, 2, 8, "REACTION_ADD"),
            ]),
        );
        hub.signers.insert("8".into(), r#"{"signers":[{"key":"0x02","requestFid":309857}]}"#.into());
        let hub = Rc::new(hub);
        let report = run_once(&hub, &store).unwrap();
        assert_eq!((report.learned, report.looked_up), (1, 1));
        let requests = hub.requests.borrow();
        assert!(requests.iter().any(|url| url.contains("/signersByFid?fid=8&")));
        assert!(!requests.iter().any(|url| url.contains("fid=100")), "{requests:?}");
        let keys = store.keys.borrow();
        assert_eq!((keys[&pair(100, "0x01")], keys[&pair(8, "0x02")]), (9152, 309_857));
        // This run's keys are kept in their hour.
        let set = Keys::decode(&store.sets.borrow()[&key_set(activity::bucket(at))]).unwrap();
        assert_eq!(set, Keys::of([pair(8, "0x02"), pair(100, "0x01")]));
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
        *store.state.borrow_mut() = Some(serde_json::to_string(&follow).unwrap());
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
    }
}
