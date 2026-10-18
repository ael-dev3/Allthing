// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's index of notable casts (see `allthing_core::index`): the
//! storage it needs from the runtime, and the cron job's run.

use allthing_core::index::{
    Entry, Feed, IndexerState, Ranking, RankingMeta, Span, Step, candidates, due, notable_authors, recompute_reads,
    worth_indexing,
};
use allthing_core::normalize::RawCast;
use allthing_core::ranking::TrendingWindow;
use futures::future::LocalBoxFuture;

use crate::Invocation;
use crate::hypersnap;
use crate::upstream::UpstreamError;

/// Casts a run reads: one page.
pub const PAGE: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexError(pub String);

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "index: {}", self.0)
    }
}

/// A payload the cron keeps fresh for every data center, as served, with
/// when it was built (Unix milliseconds).
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub built: f64,
    pub body: String,
}

/// What a run starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    /// The indexer's state, as JSON.
    pub state: Option<String>,
    pub rankings: Vec<RankingMeta>,
}

/// The index's storage (D1 in the Worker). Each call is one round trip.
pub trait Index {
    /// A span's stored ranking, if it has been computed.
    fn ranking(&self, span: Span) -> LocalBoxFuture<'static, Result<Option<Ranking>, IndexError>>;
    /// The indexer's saved state, and what it needs of each ranking.
    fn load(&self) -> LocalBoxFuture<'static, Result<Saved, IndexError>>;
    /// In one transaction: store `casts` (updating the points of those
    /// already indexed), recompute `rerank`'s ranking as of `now_ms`, and
    /// save `state`.
    fn save(
        &self,
        casts: Vec<Entry>,
        rerank: Option<Span>,
        state: String,
        now_ms: f64,
    ) -> LocalBoxFuture<'static, Result<(), IndexError>>;
    /// The stored copy of payload `name`, if the cron has stored one.
    fn snapshot(&self, _name: &'static str) -> LocalBoxFuture<'static, Result<Option<Snapshot>, IndexError>> {
        Box::pin(async { Ok(None) })
    }
    /// Store a copy of payload `name`, replacing the previous one.
    fn save_snapshot(
        &self,
        _name: &'static str,
        _snapshot: Snapshot,
    ) -> LocalBoxFuture<'static, Result<(), IndexError>> {
        Box::pin(async { Err(IndexError("this index stores no copies".into())) })
    }
}

/// What a run did, for the Worker's log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// "trending 200", "a:3", "c:farcaster" or "rest".
    pub read_from: String,
    pub casts: usize,
    pub kept: usize,
    pub reranked: Option<&'static str>,
    pub feeds: usize,
    pub upstream_error: Option<String>,
}

/// One cron run: read one page of casts, keep the ones worth indexing,
/// recompute a ranking if one is due, and save where to read next.
pub async fn run(inv: &Invocation, index: &dyn Index) -> Result<Report, IndexError> {
    let now_ms = inv.now();
    let now = (now_ms / 1000.0) as i64;
    let Saved { state: saved, rankings: metas } = index.load().await?;
    let mut state = IndexerState::load(saved.as_deref());
    state.count(now_ms, 1 + metas.len() as u64, 0);
    let step = state.plan();
    let read_from = match &step {
        Step::Rest => "rest".to_owned(),
        Step::Trending(offset) => format!("trending {offset}"),
        Step::Backfill(i) => state.feeds[*i].source.key(),
    };
    let page = match &step {
        Step::Rest => Ok((Vec::new(), None)),
        Step::Trending(offset) => {
            hypersnap::trending_casts(inv, PAGE, *offset, TrendingWindow::Week).await.map(|casts| (casts, None))
        }
        Step::Backfill(i) => feed_page(inv, &state.feeds[*i]).await,
    };
    let mut report = Report { read_from, ..Report::default() };
    let (kept, next) = match page {
        Ok((raw, next)) => {
            report.casts = raw.len();
            let found = candidates(&raw, &metas, now);
            state.discover(&notable_authors(&found));
            // An empty page ends a feed, whatever its cursor says.
            let next = next.filter(|_| !raw.is_empty());
            (worth_indexing(&found, &metas, now), Ok(next))
        }
        Err(error) => {
            report.upstream_error = Some(error.to_string());
            (Vec::new(), Err(()))
        }
    };
    state.advance(&step, next);
    let rerank = due(&metas, now_ms).filter(|span| state.can_read(recompute_reads(&metas, *span)));
    // Each new cast writes a row and its time index entry; updates write one.
    let written = 2 * kept.len() as u64 + 1 + 2 * u64::from(rerank.is_some());
    let read = kept.len() as u64 + rerank.map_or(0, |span| recompute_reads(&metas, span));
    state.count(now_ms, read, written);
    report.kept = kept.len();
    report.reranked = rerank.map(Span::name);
    report.feeds = state.feeds.len();
    let json = serde_json::to_string(&state).map_err(|e| IndexError(e.to_string()))?;
    index.save(kept, rerank, json, now_ms).await?;
    Ok(report)
}

/// A feed's next page, and the cursor after it. The provider adapter maps
/// missing feeds to empty pages; failures must keep the cursor retryable.
async fn feed_page(inv: &Invocation, feed: &Feed) -> Result<(Vec<RawCast>, Option<String>), UpstreamError> {
    hypersnap::feed_page(inv, &feed.source, feed.cursor.as_deref(), PAGE).await
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::rc::Rc;

    use allthing_core::index::{RANKED, SEED_CHANNELS};
    use futures::FutureExt;
    use serde_json::json;

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    const NOW_MS: f64 = 1_792_000_000_000.0;
    const DAY: i64 = 86_400;

    /// D1 in memory: casts, rankings and state, ranked the way the SQL does.
    #[derive(Default)]
    pub(crate) struct Memory {
        pub casts: RefCell<BTreeMap<String, (i64, u64)>>,
        pub rankings: RefCell<BTreeMap<&'static str, (i64, u64, Option<i64>, u64, String)>>,
        pub state: RefCell<Option<String>>,
        pub fail: bool,
    }

    impl Memory {
        pub(crate) fn rerank(&self, span: Span, now_ms: f64) {
            let cutoff = span.cutoff((now_ms / 1000.0) as i64);
            let casts = self.casts.borrow();
            let mut inside: Vec<Entry> = casts
                .iter()
                .filter(|(_, (ts, _))| *ts >= cutoff)
                .map(|(h, (ts, p))| Entry(h.clone(), *ts, *p))
                .collect();
            let total = inside.len() as u64;
            let oldest = inside.iter().map(|e| e.1).min();
            inside.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
            inside.truncate(RANKED);
            let floor = if inside.len() >= RANKED { inside.last().map_or(0, |e| e.2) } else { 0 };
            let body = serde_json::to_string(&inside).unwrap();
            self.rankings.borrow_mut().insert(span.name(), (now_ms as i64, total, oldest, floor, body));
        }
    }

    impl Index for Memory {
        fn ranking(&self, span: Span) -> LocalBoxFuture<'static, Result<Option<Ranking>, IndexError>> {
            let found = if self.fail {
                Err(IndexError("down".into()))
            } else {
                Ok(self
                    .rankings
                    .borrow()
                    .get(span.name())
                    .and_then(|(built, total, oldest, _, body)| Ranking::from_row(*built, *total, *oldest, body)))
            };
            async move { found }.boxed_local()
        }

        fn load(&self) -> LocalBoxFuture<'static, Result<Saved, IndexError>> {
            let metas = self
                .rankings
                .borrow()
                .iter()
                .map(|(name, (built, total, _, floor, _))| RankingMeta {
                    span: Span::parse(name).unwrap(),
                    built: *built,
                    total: *total,
                    floor: *floor,
                })
                .collect();
            let saved = Saved { state: self.state.borrow().clone(), rankings: metas };
            async move { Ok(saved) }.boxed_local()
        }

        fn save(
            &self,
            casts: Vec<Entry>,
            rerank: Option<Span>,
            state: String,
            now_ms: f64,
        ) -> LocalBoxFuture<'static, Result<(), IndexError>> {
            for Entry(hash, ts, points) in casts {
                self.casts.borrow_mut().insert(hash, (ts, points));
            }
            if let Some(span) = rerank {
                self.rerank(span, now_ms);
            }
            *self.state.borrow_mut() = Some(state);
            async { Ok(()) }.boxed_local()
        }
    }

    /// Hypersnap's trending and feed pages: casts by hash with their time
    /// and likes, each feed in pages of two.
    struct Feeds {
        trending: Vec<(String, u64, i64, u64)>,
        authors: BTreeMap<u64, Vec<(String, i64, u64)>>,
        requests: RefCell<Vec<String>>,
    }

    fn cast_json(hash: &str, fid: u64, ts: i64, likes: u64) -> serde_json::Value {
        json!({
            "hash": hash,
            "author": { "fid": fid, "username": format!("u{fid}") },
            "text": "words",
            "timestamp": allthing_core::time::iso_utc(ts),
            "reactions": { "likes_count": likes, "recasts_count": 0 },
            "replies": { "count": 0 },
        })
    }

    impl Host for Feeds {
        fn now(&self) -> f64 {
            NOW_MS
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let url = request.url.clone();
            self.requests.borrow_mut().push(url.clone());
            let param = |key: &str| {
                url.split(['?', '&']).find_map(|pair| pair.strip_prefix(key)?.strip_prefix('=')).map(str::to_owned)
            };
            let body = if url.contains("/feed/trending?") {
                let casts: Vec<_> =
                    self.trending.iter().map(|(h, fid, ts, likes)| cast_json(h, *fid, *ts, *likes)).collect();
                json!({ "casts": casts })
            } else if url.contains("/feed/user/casts?") {
                let fid: u64 = param("fid").unwrap().parse().unwrap();
                let from: usize = param("cursor").map_or(0, |c| c.parse().unwrap());
                let all = self.authors.get(&fid).cloned().unwrap_or_default();
                let page: Vec<_> = all.iter().skip(from).take(2).map(|(h, ts, l)| cast_json(h, fid, *ts, *l)).collect();
                let next = (from + 2 < all.len()).then(|| json!({ "cursor": (from + 2).to_string() }));
                json!({ "casts": page, "next": next })
            } else {
                json!({ "casts": [] })
            };
            let response =
                HttpResponse { status: 200, retry_after: None, body: Some(serde_json::to_vec(&body).unwrap()) };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn now() -> i64 {
        (NOW_MS / 1000.0) as i64
    }

    fn run_once(host: &Rc<Feeds>, index: &Memory) -> Report {
        let inv = Invocation::new(host.clone(), 45);
        run(&inv, index).now_or_never().unwrap().unwrap()
    }

    #[test]
    fn runs_fill_the_index_and_rank_month_year_and_all_time_apart() {
        let n = now();
        let host = Rc::new(Feeds {
            // This week's trending, by authors 7 and 8.
            trending: vec![
                ("0xa1".into(), 7, n - DAY, 900),
                ("0xa2".into(), 8, n - 2 * DAY, 300),
                ("0xa3".into(), 8, n - 2 * DAY, 5),
            ],
            authors: BTreeMap::from([
                (
                    7,
                    vec![
                        ("0xa1".into(), n - DAY, 900),
                        ("0xb1".into(), n - 20 * DAY, 400),
                        ("0xb2".into(), n - 200 * DAY, 2000),
                        ("0xb3".into(), n - 800 * DAY, 5000),
                    ],
                ),
                (8, vec![("0xa2".into(), n - 2 * DAY, 300)]),
            ]),
            requests: RefCell::new(Vec::new()),
        });
        let index = Memory::default();
        let first = run_once(&host, &index);
        assert_eq!((first.read_from.as_str(), first.casts, first.kept), ("trending 0", 3, 2));
        assert_eq!(first.reranked, Some("month"));
        assert_eq!(first.feeds, SEED_CHANNELS.len() + 2);
        // Then the seed channels (empty here, so finished), then authors 7 and 8.
        let mut reports = Vec::new();
        for _ in 0..13 {
            reports.push(run_once(&host, &index));
        }
        let read: Vec<&str> = reports.iter().map(|r| r.read_from.as_str()).collect();
        assert_eq!(&read[..3], ["c:farcaster", "c:base", "c:ethereum"]);
        assert!(read.contains(&"a:7") && read.contains(&"a:8"));
        assert_eq!(index.casts.borrow().len(), 5);

        // Rankings are due again once five minutes have passed; here, force them.
        for span in Span::ALL {
            index.rerank(span, NOW_MS);
        }
        let page = |span| index.ranking(span).now_or_never().unwrap().unwrap().unwrap().page(0, 100);
        assert_eq!(page(Span::Month), ["0xa1", "0xb1", "0xa2"]);
        assert_eq!(page(Span::Year), ["0xb2", "0xa1", "0xb1", "0xa2"]);
        assert_eq!(page(Span::All), ["0xb3", "0xb2", "0xa1", "0xb1", "0xa2"]);
        let state = IndexerState::load(index.state.borrow().as_deref());
        assert!(state.finished.contains(&"c:farcaster".to_owned()), "{:?}", state.finished);
        assert!(state.written > 0);
    }

    #[test]
    fn a_ranking_waits_its_turn() {
        let host = Rc::new(Feeds { trending: vec![], authors: BTreeMap::new(), requests: RefCell::new(Vec::new()) });
        let index = Memory::default();
        let reranked: Vec<Option<&str>> = (0..4).map(|_| run_once(&host, &index).reranked).collect();
        assert_eq!(reranked, [Some("month"), Some("year"), Some("all"), None]);
    }
    struct RetriedFeed {
        now: Cell<f64>,
        first: RefCell<Option<HttpResponse>>,
        requests: RefCell<Vec<String>>,
    }

    impl Host for RetriedFeed {
        fn now(&self) -> f64 {
            self.now.get()
        }
        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            self.requests.borrow_mut().push(request.url);
            let response = self.first.borrow_mut().take().unwrap_or_else(|| HttpResponse {
                status: 200,
                retry_after: None,
                body: Some(
                    serde_json::to_vec(&json!({
                        "casts": [cast_json("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 7, now() - DAY, 300)],
                        "next": {"cursor":"page-2"}
                    }))
                    .unwrap(),
                ),
            });
            async move { Ok(response) }.boxed_local()
        }
    }

    fn parked_feed() -> Memory {
        let state = IndexerState {
            runs: 1,
            feeds: vec![Feed { source: allthing_core::index::Source::Author(7), cursor: Some("page-1".into()) }],
            ..IndexerState::default()
        };
        Memory { state: RefCell::new(Some(serde_json::to_string(&state).unwrap())), ..Memory::default() }
    }

    fn assert_retry(first: HttpResponse) {
        let host = Rc::new(RetriedFeed {
            now: Cell::new(NOW_MS),
            first: RefCell::new(Some(first)),
            requests: RefCell::new(vec![]),
        });
        let index = parked_feed();
        let inv = Invocation::new(host.clone(), 45);
        let report = run(&inv, &index).now_or_never().unwrap().unwrap();
        assert!(report.upstream_error.is_some(), "an unreadable page must be reported as a failure");
        let saved = IndexerState::load(index.state.borrow().as_deref());
        assert_eq!(saved.feeds.len(), 1, "a failed feed must remain queued");
        assert_eq!(saved.feeds[0].cursor.as_deref(), Some("page-1"));
        assert!(!saved.finished.iter().any(|key| key == "a:7"));

        host.now.set(NOW_MS + 10_000.0);
        let inv = Invocation::new(host.clone(), 45);
        let recovered = run(&inv, &index).now_or_never().unwrap().unwrap();
        assert!(recovered.upstream_error.is_none());
        assert_eq!(recovered.kept, 1);
        let saved = IndexerState::load(index.state.borrow().as_deref());
        assert_eq!(saved.feeds[0].cursor.as_deref(), Some("page-2"));
        assert_eq!(host.requests.borrow().len(), 2);
        assert!(host.requests.borrow().iter().all(|url| url.contains("cursor=page-1")));
    }

    #[test]
    fn a_rate_limited_backfill_retries_its_cursor() {
        assert_retry(HttpResponse { status: 429, retry_after: Some("1".into()), body: None });
    }

    #[test]
    fn a_malformed_backfill_retries_its_cursor() {
        assert_retry(HttpResponse { status: 200, retry_after: None, body: Some(b"not json".to_vec()) });
    }

    #[test]
    fn a_backfill_error_object_does_not_finish_the_feed() {
        assert_retry(HttpResponse {
            status: 200,
            retry_after: None,
            body: Some(br#"{"error":"temporarily unavailable"}"#.to_vec()),
        });
    }

    #[test]
    fn a_missing_backfill_is_still_finished() {
        let host = Rc::new(RetriedFeed {
            now: Cell::new(NOW_MS),
            first: RefCell::new(Some(HttpResponse { status: 404, retry_after: None, body: None })),
            requests: RefCell::new(vec![]),
        });
        let index = parked_feed();
        let inv = Invocation::new(host, 45);
        let report = run(&inv, &index).now_or_never().unwrap().unwrap();
        assert!(report.upstream_error.is_none());
        let saved = IndexerState::load(index.state.borrow().as_deref());
        assert!(saved.feeds.is_empty());
        assert!(saved.finished.iter().any(|key| key == "a:7"));
    }
}
