// SPDX-License-Identifier: AGPL-3.0-or-later
//! Engagement-based ranking. Farcaster has likes, recasts and replies but no
//! downvotes, so a cast's points only ever count positive engagement.

use std::cmp::Ordering;

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
/// Only the ranges Hypersnap can rank over exist: its widest trending
/// window is 7 days, so there is no past month, past year or all time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimeRange {
    Hour,
    #[default]
    Day,
    Week,
}

impl TimeRange {
    /// In menu order, the widest first.
    pub const ALL: [TimeRange; 3] = [TimeRange::Week, TimeRange::Day, TimeRange::Hour];

    pub fn as_str(self) -> &'static str {
        match self {
            TimeRange::Hour => "hour",
            TimeRange::Day => "day",
            TimeRange::Week => "week",
        }
    }

    /// Unknown or missing values fall back to the default, Today. Links made
    /// when the menu offered longer ranges (`month`, `year`, `all`) open the
    /// widest there is, the past week.
    pub fn parse(value: Option<&str>) -> TimeRange {
        match value {
            Some("month" | "year" | "all") => TimeRange::Week,
            _ => TimeRange::ALL.into_iter().find(|r| Some(r.as_str()) == value).unwrap_or_default(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimeRange::Hour => "Past hour",
            TimeRange::Day => "Today",
            TimeRange::Week => "Past week",
        }
    }

    /// The trending window this range is ranked over.
    pub fn window(self) -> TrendingWindow {
        match self {
            TimeRange::Hour => TrendingWindow::Hour,
            TimeRange::Day => TrendingWindow::Day,
            TimeRange::Week => TrendingWindow::Week,
        }
    }
}

/// The windows Hypersnap ranks trending casts over (`time_window`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TrendingWindow {
    Hour,
    #[default]
    Day,
    Week,
}

impl TrendingWindow {
    pub fn as_str(self) -> &'static str {
        match self {
            TrendingWindow::Hour => "1h",
            TrendingWindow::Day => "24h",
            TrendingWindow::Week => "7d",
        }
    }

    pub fn parse(value: &str) -> Option<TrendingWindow> {
        [TrendingWindow::Hour, TrendingWindow::Day, TrendingWindow::Week].into_iter().find(|w| w.as_str() == value)
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
        assert_eq!(TimeRange::Week.window().as_str(), "7d");
        // Old links to longer ranges open the widest there is.
        for old in ["month", "year", "all"] {
            assert_eq!(TimeRange::parse(Some(old)), TimeRange::Week);
        }
        assert_eq!(TrendingWindow::parse("7d"), Some(TrendingWindow::Week));
        assert_eq!(TrendingWindow::parse("30d"), None);
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
