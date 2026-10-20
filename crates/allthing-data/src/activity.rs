// SPDX-License-Identifier: AGPL-3.0-or-later
//! Daily active accounts (see `allthing_core::activity`): the storage the
//! count needs from the runtime, and the cron's run, which reads each
//! shard's hub events on from where the last run stopped and keeps each
//! UTC hour's and day's active fids.

use std::collections::{BTreeSet, HashMap};

use allthing_core::activity::{
    self, ActivityPayload, Batch, Fids, Follow, KEEP_DAYS, KEEP_HOURS, Reached, Window, day, day_set, hour, hour_set,
};
use allthing_core::api::Meta;
use allthing_core::latest::{Cursor, SHARDS};
use futures::future::{LocalBoxFuture, join_all};

use crate::Invocation;
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

/// The follower's storage (D1 in the Worker). Each call is one round trip.
pub trait ActivityStore {
    /// The saved state, as JSON.
    fn activity_state(&self) -> LocalBoxFuture<'static, Result<Option<String>, IndexError>>;
    /// The stored sets of those `names` there are.
    fn activity_sets(&self, names: Vec<String>)
    -> LocalBoxFuture<'static, Result<HashMap<String, String>, IndexError>>;
    /// In one transaction: write `sets`, drop the hourly sets before hour
    /// `keep_from.0` and the daily ones before day `keep_from.1`, save
    /// `state`, and store `summary` as the payload served.
    fn save_activity(
        &self,
        sets: Vec<(String, String)>,
        keep_from: (i64, i64),
        state: String,
        summary: Snapshot,
    ) -> LocalBoxFuture<'static, Result<(), IndexError>>;
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

/// One run: read each shard's next page, add its active accounts to their
/// hours' and days' sets, count the day so far, and store the payload.
pub async fn run(inv: &Invocation, store: &dyn ActivityStore) -> Result<Report, IndexError> {
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
    }
    report.actions = actions.len();
    let batch = Batch::of(&actions);
    let reached = follow.reached().unwrap_or(now);
    report.behind = Some(now - reached);
    let (last_hour, today) = (hour(reached), day(reached));
    // The 30 days to the end of yesterday, once a day, when keeping up (the
    // day sets of a backlog are still filling) and yesterday was read whole.
    let keeping_up = report.behind.is_some_and(|b| b < CAUGHT_UP);
    let due_30d = keeping_up
        && follow.since.is_some_and(|since| since <= (today - 1) * 86_400)
        && follow.active_30d.is_none_or(|w| w.day < today - 1);
    let mut names: BTreeSet<String> = batch.hours.keys().map(|&h| hour_set(h)).collect();
    names.extend(batch.days.keys().map(|&d| day_set(d)));
    names.extend((last_hour - 47..=last_hour).map(hour_set));
    if due_30d {
        names.extend((today - 30..today).map(day_set));
    }
    let stored = store.activity_sets(names.into_iter().collect()).await?;
    let decoded = |name: &str| stored.get(name).and_then(|text| Fids::decode(text)).unwrap_or_default();
    let mut writes = Vec::new();
    let mut hours: HashMap<i64, Fids> = (last_hour - 47..=last_hour).map(|h| (h, decoded(&hour_set(h)))).collect();
    for (h, fids) in &batch.hours {
        let mut set = hours.remove(h).unwrap_or_else(|| decoded(&hour_set(*h)));
        set.extend(fids);
        writes.push((hour_set(*h), set.encode()));
        hours.insert(*h, set);
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
    follow.settle();
    let meta = Meta { fetched_at: (inv.now()) as i64, ..Meta::default() };
    let payload: ActivityPayload = activity::summary(&follow, &hours, meta);
    let body = serde_json::to_string(&payload).map_err(|e| IndexError(e.to_string()))?;
    let state = serde_json::to_string(&follow).map_err(|e| IndexError(e.to_string()))?;
    let keep_from = (last_hour - KEEP_HOURS, today - KEEP_DAYS);
    store.save_activity(writes, keep_from, state, Snapshot { built: inv.now(), body }).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use futures::FutureExt;

    use super::*;
    use crate::indexer::Index;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// Now: 2027-01-15T08:00:00Z, Unix seconds.
    const NOW: i64 = 1_800_000_000;
    /// The newest block, and where a first run starts: 32 hours earlier,
    /// the start of yesterday.
    const HEAD: u64 = 10_000_000;
    const START: u64 = HEAD - 115_200;

    /// A hub with an event page per (shard, first event id).
    #[derive(Default)]
    struct Hub {
        events: HashMap<(String, String), String>,
        requests: RefCell<Vec<String>>,
    }

    fn param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
        url.split(['?', '&']).find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
    }

    impl Host for Hub {
        fn now(&self) -> f64 {
            NOW as f64 * 1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let url = request.url.clone();
            self.requests.borrow_mut().push(url.clone());
            let shard = param(&url, "shard_index").unwrap_or_default().to_owned();
            let from = param(&url, "from_event_id").unwrap_or("newest").to_owned();
            let response = match self.events.get(&(shard, from)) {
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

        fn save_activity(
            &self,
            sets: Vec<(String, String)>,
            keep_from: (i64, i64),
            state: String,
            summary: Snapshot,
        ) -> LocalBoxFuture<'static, Result<(), IndexError>> {
            let mut stored = self.sets.borrow_mut();
            stored.extend(sets);
            stored.retain(|name, _| {
                let (kind, n) = name.split_at(1);
                let n: i64 = n.parse().unwrap();
                if kind == "h" { n >= keep_from.0 } else { n >= keep_from.1 }
            });
            *self.state.borrow_mut() = Some(state);
            *self.summary.borrow_mut() = Some(summary);
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
        assert_eq!(Fids::decode(&store.sets.borrow()[&hour_set(hour(at))]), Some(Fids::of([7, 9, 42])));
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
