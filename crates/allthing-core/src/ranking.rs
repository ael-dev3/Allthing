// SPDX-License-Identifier: AGPL-3.0-or-later
//! Engagement-based ranking. Farcaster has likes, recasts and replies but no
//! downvotes, so a cast's points only ever count positive engagement.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::model::{Cast, ThreadNode};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sort {
    #[default]
    Hot,
    New,
    Top,
}

impl Sort {
    pub const ALL: [Sort; 3] = [Sort::Hot, Sort::New, Sort::Top];

    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Hot => "hot",
            Sort::New => "new",
            Sort::Top => "top",
        }
    }

    pub fn parse(value: Option<&str>, fallback: Sort) -> Sort {
        match value {
            Some("hot") => Sort::Hot,
            Some("new") => Sort::New,
            Some("top") => Sort::Top,
            _ => fallback,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Sort::Hot => "engagement, decaying with age",
            Sort::New => "newest first",
            Sort::Top => "most engagement",
        }
    }
}

/// How far back the front page reaches, named as Reddit's `t=` names it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimeRange {
    Hour,
    #[default]
    Day,
    Week,
    Month,
    Year,
    All,
}

impl TimeRange {
    /// In menu order, the widest first.
    pub const ALL: [TimeRange; 6] =
        [TimeRange::All, TimeRange::Year, TimeRange::Month, TimeRange::Week, TimeRange::Day, TimeRange::Hour];

    pub fn as_str(self) -> &'static str {
        match self {
            TimeRange::Hour => "hour",
            TimeRange::Day => "day",
            TimeRange::Week => "week",
            TimeRange::Month => "month",
            TimeRange::Year => "year",
            TimeRange::All => "all",
        }
    }

    /// Unknown or missing values fall back to the default, Today.
    pub fn parse(value: Option<&str>) -> TimeRange {
        TimeRange::ALL.into_iter().find(|r| Some(r.as_str()) == value).unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            TimeRange::Hour => "Past hour",
            TimeRange::Day => "Today",
            TimeRange::Week => "Past week",
            TimeRange::Month => "Past month",
            TimeRange::Year => "Past year",
            TimeRange::All => "All time",
        }
    }

    /// The window this range is ranked over: Hypersnap's trending windows up
    /// to a week, Allthing's own index beyond.
    pub fn window(self) -> TrendingWindow {
        match self {
            TimeRange::Hour => TrendingWindow::Hour,
            TimeRange::Day => TrendingWindow::Day,
            TimeRange::Week => TrendingWindow::Week,
            TimeRange::Month => TrendingWindow::Month,
            TimeRange::Year => TrendingWindow::Year,
            TimeRange::All => TrendingWindow::All,
        }
    }
}

/// The windows the front page ranks casts over: Hypersnap's trending
/// windows (`time_window`, at most 7 days), and the longer ones ranked from
/// Allthing's own index of past casts (see [`crate::index`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TrendingWindow {
    Hour,
    #[default]
    Day,
    Week,
    Month,
    Year,
    All,
}

impl TrendingWindow {
    pub const ALL: [TrendingWindow; 6] = [
        TrendingWindow::Hour,
        TrendingWindow::Day,
        TrendingWindow::Week,
        TrendingWindow::Month,
        TrendingWindow::Year,
        TrendingWindow::All,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TrendingWindow::Hour => "1h",
            TrendingWindow::Day => "24h",
            TrendingWindow::Week => "7d",
            TrendingWindow::Month => "30d",
            TrendingWindow::Year => "365d",
            TrendingWindow::All => "all",
        }
    }

    pub fn parse(value: &str) -> Option<TrendingWindow> {
        TrendingWindow::ALL.into_iter().find(|w| w.as_str() == value)
    }

    /// Ranked from Allthing's index rather than Hypersnap's trending feed.
    pub fn is_indexed(self) -> bool {
        matches!(self, TrendingWindow::Month | TrendingWindow::Year | TrendingWindow::All)
    }
}

/// Recasts spread a cast to new audiences, so they count double; each reply
/// counts once, like a like.
pub const RECAST_WEIGHT: u64 = 2;

/// A cast's engagement points: likes + 2 × recasts + replies.
pub fn points(cast: &Cast) -> u64 {
    cast.likes + RECAST_WEIGHT * cast.recasts + cast.replies
}

/// Seconds since the Unix epoch at which reddit's hot ranking starts counting.
const HOT_EPOCH: i64 = 1_134_028_003;
/// Ten times the points buys this many seconds (12.5 hours) of freshness.
pub const HOT_HALF_LIFE_SECS: i64 = 45_000;

/// Reddit's "hot" formula over engagement points: log10 of the points plus a
/// time bonus. Points are never negative here, so there is no sign term.
pub fn hot_score(points: u64, ts: i64) -> f64 {
    let order = (points.max(1) as f64).log10();
    let score = order + (ts - HOT_EPOCH) as f64 / HOT_HALF_LIFE_SECS as f64;
    (score * 1e7).round() / 1e7
}

fn newest_first(a: &Cast, b: &Cast) -> Ordering {
    b.ts.cmp(&a.ts).then_with(|| a.hash.cmp(&b.hash))
}

pub fn sort_casts(casts: &mut [Cast], sort: Sort) {
    match sort {
        Sort::New => casts.sort_by(newest_first),
        Sort::Top => casts.sort_by(|a, b| points(b).cmp(&points(a)).then_with(|| newest_first(a, b))),
        Sort::Hot => {
            casts.sort_by(|a, b| {
                hot_score(points(b), b.ts).total_cmp(&hot_score(points(a), a.ts)).then_with(|| newest_first(a, b))
            });
        }
    }
}

pub fn sorted(casts: &[Cast], sort: Sort) -> Vec<Cast> {
    let mut out = casts.to_vec();
    sort_casts(&mut out, sort);
    out
}

/// A reader's own leanings about accounts, kept in their browser and never
/// shared: their favorites' casts rise in hot and top. It isn't a label on an
/// account, and it hides nothing: new stays in time order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Leanings {
    pub favorites: BTreeSet<u64>,
}

/// A favorite's cast weighs in hot and top as if it had this many times its
/// points.
pub const FAVORITE_WEIGHT: f64 = 4.0;

impl Leanings {
    pub fn is_empty(&self) -> bool {
        self.favorites.is_empty()
    }

    /// How much more, or less, `cast` weighs for this reader.
    pub fn weight(&self, cast: &Cast) -> f64 {
        if self.favorites.contains(&cast.author.fid) { FAVORITE_WEIGHT } else { 1.0 }
    }
}

/// [`sorted`], with the reader's leanings: in hot and top a cast weighs its
/// points times [`Leanings::weight`] (in hot, its log moves by the weight's);
/// new is in time order whoever cast.
pub fn sorted_for(casts: &[Cast], sort: Sort, leanings: &Leanings) -> Vec<Cast> {
    let mut out = casts.to_vec();
    if leanings.is_empty() || sort == Sort::New {
        sort_casts(&mut out, sort);
        return out;
    }
    let key = |cast: &Cast| match sort {
        Sort::Top => points(cast) as f64 * leanings.weight(cast),
        _ => hot_score(points(cast), cast.ts) + leanings.weight(cast).log10(),
    };
    out.sort_by(|a, b| key(b).total_cmp(&key(a)).then_with(|| newest_first(a, b)));
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CommentSort {
    #[default]
    Top,
    New,
    Old,
}

impl CommentSort {
    pub const ALL: [CommentSort; 3] = [CommentSort::Top, CommentSort::New, CommentSort::Old];

    pub fn as_str(self) -> &'static str {
        match self {
            CommentSort::Top => "top",
            CommentSort::New => "new",
            CommentSort::Old => "old",
        }
    }

    pub fn parse(value: Option<&str>) -> CommentSort {
        match value {
            Some("new") => CommentSort::New,
            Some("old") => CommentSort::Old,
            _ => CommentSort::Top,
        }
    }
}

/// Sort every level of a reply tree.
pub fn sort_thread(nodes: &mut [ThreadNode], sort: CommentSort) {
    match sort {
        CommentSort::Top => {
            nodes.sort_by(|a, b| points(&b.cast).cmp(&points(&a.cast)).then_with(|| a.cast.ts.cmp(&b.cast.ts)))
        }
        CommentSort::New => nodes.sort_by_key(|n| std::cmp::Reverse(n.cast.ts)),
        CommentSort::Old => nodes.sort_by_key(|n| n.cast.ts),
    }
    for node in nodes {
        sort_thread(&mut node.children, sort);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_ranges_map_to_trending_windows() {
        assert_eq!(TimeRange::parse(None), TimeRange::Day);
        assert_eq!(TimeRange::parse(Some("week")), TimeRange::Week);
        assert_eq!(TimeRange::parse(Some("fortnight")), TimeRange::Day);
        for range in TimeRange::ALL {
            assert_eq!(TimeRange::parse(Some(range.as_str())), range);
        }
        assert_eq!(TimeRange::Hour.window().as_str(), "1h");
        assert_eq!(TimeRange::Day.window().as_str(), "24h");
        // Every range has its own window: up to a week from Hypersnap, the
        // longer ones from Allthing's index.
        let windows: Vec<TrendingWindow> = TimeRange::ALL.into_iter().map(TimeRange::window).collect();
        for (i, window) in windows.iter().enumerate() {
            assert!(!windows[..i].contains(window), "{window:?} is shared");
            assert_eq!(TrendingWindow::parse(window.as_str()), Some(*window));
        }
        for range in [TimeRange::Month, TimeRange::Year, TimeRange::All] {
            assert!(range.window().is_indexed());
        }
        assert!(!TimeRange::Week.window().is_indexed());
        assert_eq!(TrendingWindow::parse("30d"), Some(TrendingWindow::Month));
        assert_eq!(TrendingWindow::parse("90d"), None);
    }
    use crate::model::UserSummary;
    use crate::time::parse_rfc3339;

    fn cast(hash: &str, likes: u64, recasts: u64, replies: u64, at: &str) -> Cast {
        Cast {
            hash: hash.into(),
            author: UserSummary { fid: 1, username: "a".into(), display_name: String::new(), pfp_url: None },
            text: hash.into(),
            mentions: vec![],
            ts: parse_rfc3339(at).unwrap(),
            parent_hash: None,
            parent_url: None,
            channel: None,
            embeds: vec![],
            likes,
            recasts,
            replies,
            mini_app: false,
            spam: false,
        }
    }

    #[test]
    fn favorites_rise_in_hot_and_top_and_new_stays_in_time_order() {
        let by = |fid: u64, mut c: Cast| {
            c.author.fid = fid;
            c
        };
        // A stranger's busy cast, a favorite's quieter one, and a stranger's
        // newest, quietest one.
        let casts = vec![
            by(1, cast("busy", 30, 0, 0, "2026-10-17T00:00:00Z")),
            by(2, cast("favorite", 10, 0, 0, "2026-10-17T00:00:00Z")),
            by(3, cast("newest", 1, 0, 0, "2026-10-17T01:00:00Z")),
        ];
        let order = |sorted: Vec<Cast>| sorted.into_iter().map(|c| c.hash).collect::<Vec<_>>();
        let none = Leanings::default();
        for sort in [Sort::Hot, Sort::Top, Sort::New] {
            assert_eq!(sorted_for(&casts, sort, &none), sorted(&casts, sort));
        }
        assert_eq!(order(sorted(&casts, Sort::Top)), ["busy", "favorite", "newest"]);
        let leanings = Leanings { favorites: BTreeSet::from([2]) };
        // 10 points weigh as 40, over 30.
        assert_eq!(order(sorted_for(&casts, Sort::Top, &leanings)), ["favorite", "busy", "newest"]);
        assert_eq!(order(sorted_for(&casts, Sort::Hot, &leanings)), ["favorite", "busy", "newest"]);
        assert_eq!(order(sorted_for(&casts, Sort::New, &leanings)), ["newest", "busy", "favorite"]);
        // Four times, no more: 10 points weigh less than 50.
        let busier = vec![by(1, cast("busier", 50, 0, 0, "2026-10-17T00:00:00Z")), casts[1].clone()];
        assert_eq!(order(sorted_for(&busier, Sort::Top, &leanings)), ["busier", "favorite"]);
        assert_eq!(leanings.weight(&casts[1]), FAVORITE_WEIGHT);
        assert_eq!(leanings.weight(&casts[0]), 1.0);
    }

    #[test]
    fn points_count_recasts_double_and_replies_once() {
        assert_eq!(points(&cast("x", 10, 3, 4, "2026-10-17T00:00:00Z")), 20);
    }

    #[test]
    fn hot_matches_reddits_formula() {
        let ts = parse_rfc3339("2026-10-17T00:00:00Z").unwrap();
        let bonus = (ts - 1_134_028_003) as f64 / 45_000.0;
        assert!((hot_score(100, ts) - (2.0 + bonus)).abs() < 1e-6);
        assert!((hot_score(1, ts) - bonus).abs() < 1e-6);
        assert!((hot_score(0, ts) - bonus).abs() < 1e-6);
        // 10x the points buys 12.5 hours.
        let later = hot_score(10, ts + 45_000);
        assert!((later - hot_score(100, ts)).abs() < 1e-6);
    }

    #[test]
    fn sorts_new_top_and_hot() {
        let old = cast("0xold", 500, 50, 0, "2026-10-14T00:00:00Z");
        let fresh = cast("0xfresh", 20, 2, 0, "2026-10-17T10:00:00Z");
        let mid = cast("0xmid", 100, 0, 0, "2026-10-16T12:00:00Z");
        let hashes = |casts: Vec<Cast>| casts.into_iter().map(|c| c.hash).collect::<Vec<_>>();
        let all = [old.clone(), fresh.clone(), mid.clone()];
        assert_eq!(hashes(sorted(&all, Sort::New)), ["0xfresh", "0xmid", "0xold"]);
        assert_eq!(hashes(sorted(&all, Sort::Top)), ["0xold", "0xmid", "0xfresh"]);
        assert_eq!(hashes(sorted(&all, Sort::Hot)), ["0xfresh", "0xmid", "0xold"]);
        // Replies are engagement too.
        let chatty = cast("0xchatty", 0, 0, 900, "2026-10-14T00:00:00Z");
        assert_eq!(hashes(sorted(&[old, chatty], Sort::Top)), ["0xchatty", "0xold"]);
    }

    #[test]
    fn parses_sorts() {
        assert_eq!(Sort::parse(Some("top"), Sort::Hot), Sort::Top);
        assert_eq!(Sort::parse(Some("controversial"), Sort::Hot), Sort::Hot);
        assert_eq!(Sort::parse(None, Sort::New), Sort::New);
        assert_eq!(CommentSort::parse(Some("old")), CommentSort::Old);
        assert_eq!(CommentSort::parse(Some("x")), CommentSort::Top);
    }

    #[test]
    fn sorts_every_thread_level() {
        let node = |c: Cast, children: Vec<ThreadNode>| ThreadNode { cast: c, children };
        let mut nodes = vec![
            node(
                cast("0xa", 1, 0, 0, "2026-10-17T01:00:00Z"),
                vec![
                    node(cast("0xa1", 0, 0, 0, "2026-10-17T02:00:00Z"), vec![]),
                    node(cast("0xa2", 5, 0, 0, "2026-10-17T03:00:00Z"), vec![]),
                ],
            ),
            node(cast("0xb", 9, 0, 0, "2026-10-17T00:00:00Z"), vec![]),
        ];
        sort_thread(&mut nodes, CommentSort::Top);
        assert_eq!(nodes[0].cast.hash, "0xb");
        assert_eq!(nodes[1].children[0].cast.hash, "0xa2");
        sort_thread(&mut nodes, CommentSort::Old);
        assert_eq!(nodes[0].cast.hash, "0xb");
        assert_eq!(nodes[1].children[0].cast.hash, "0xa1");
    }
}
