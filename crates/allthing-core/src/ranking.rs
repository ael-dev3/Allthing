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
/// shared: their favorites' casts rise far up their hot and top, and the
/// casts of those they see less of drop far down, and lower in new too. It
/// isn't a label on an account, and it hides nothing: every cast stays in
/// the list, only in another place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Leanings {
    pub favorites: BTreeSet<u64>,
    pub less: BTreeSet<u64>,
}

/// A favorite's cast weighs in hot and top as if it had 25 times its points.
///
/// Why 25: most casts on a page of a day's feed have points within about
/// three times one another's (on the front page on 21 October 2026, 28 to
/// 88 around a median of 45), spread over the 24 hours. 25 times lifts a
/// typical cast over the busiest of them in top, and in hot its log,
/// 1.4, is worth 17½ hours of freshness ([`HOT_HALF_LIFE_SECS`]), which
/// takes a cast from the middle of a day's casts to among the newest. In a
/// day of 120 casts, a typical account's median place goes from about 60th
/// to about 4th in hot and in top (`leanings_move_a_typical_account_far`).
pub const FAVORITE_WEIGHT: f64 = 25.0;

/// The cast of an account the reader sees less of weighs a 25th of its
/// points in hot and top: [`FAVORITE_WEIGHT`] the other way, as strong. In
/// top, 270 points, the most on the front page on 21 October 2026, then
/// weigh less than its quietest cast's 22; in hot, a cast is placed as if
/// posted 17½ hours earlier, past the oldest of a day's casts unless it is
/// far busier than they are. In a day of 120 casts, a typical account's
/// median place goes from about 60th to about 117th in hot and in top. Its
/// casts are all still there, at the end.
pub const SHOW_LESS_WEIGHT: f64 = 1.0 / FAVORITE_WEIGHT;

/// In new, the casts of an account the reader sees less of are placed as if
/// posted 6 hours before they were (the time they show stays true). Where a
/// list spans a day, as a busy channel's does, that takes a cast about a
/// quarter of the list further down (a typical account's median place goes
/// from about 60th to about 87th of 120); in the front page's new, the past
/// hour or so of casts, it is the end of the list. A favorite's casts stay
/// in time order.
pub const SHOW_LESS_NEW_SECS: i64 = 6 * 3600;

impl Leanings {
    pub fn is_empty(&self) -> bool {
        self.favorites.is_empty() && self.less.is_empty()
    }

    /// How much more, or less, `cast` weighs for this reader in hot and top.
    /// A favorite wins over seeing less, though the app never keeps an
    /// account as both.
    pub fn weight(&self, cast: &Cast) -> f64 {
        let fid = cast.author.fid;
        if self.favorites.contains(&fid) {
            FAVORITE_WEIGHT
        } else if self.less.contains(&fid) {
            SHOW_LESS_WEIGHT
        } else {
            1.0
        }
    }
}

/// [`sorted`], with the reader's leanings ([`Leanings::weight`]). In top a
/// cast weighs its points plus one, times its weight: the one is so that a
/// cast nobody has reacted to yet moves too, and the others keep their
/// order either way. In hot the log of its points moves by the weight's, as
/// if it were 17½ hours newer or older. In new only seeing less moves a
/// cast, as if it were [`SHOW_LESS_NEW_SECS`] older. On a tie, a favorite's
/// cast goes first and one seen less of last. Every cast is kept.
pub fn sorted_for(casts: &[Cast], sort: Sort, leanings: &Leanings) -> Vec<Cast> {
    if leanings.is_empty() {
        return sorted(casts, sort);
    }
    // In new, a favorite's casts are in time order like anyone's.
    let weight = |cast: &Cast| match sort {
        Sort::New => leanings.weight(cast).min(1.0),
        Sort::Hot | Sort::Top => leanings.weight(cast),
    };
    let key = |cast: &Cast, weight: f64| match sort {
        Sort::Top => (points(cast) + 1) as f64 * weight,
        Sort::Hot => hot_score(points(cast), cast.ts) + weight.log10(),
        Sort::New if weight < 1.0 => (cast.ts - SHOW_LESS_NEW_SECS) as f64,
        Sort::New => cast.ts as f64,
    };
    let mut keyed: Vec<(f64, f64, &Cast)> = casts
        .iter()
        .map(|cast| {
            let weight = weight(cast);
            (key(cast, weight), weight, cast)
        })
        .collect();
    keyed.sort_by(|(key_a, weight_a, a), (key_b, weight_b, b)| {
        key_b.total_cmp(key_a).then_with(|| weight_b.total_cmp(weight_a)).then_with(|| newest_first(a, b))
    });
    keyed.into_iter().map(|(_, _, cast)| cast.clone()).collect()
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

    fn by(fid: u64, mut cast: Cast) -> Cast {
        cast.author.fid = fid;
        cast
    }

    fn order(list: Vec<Cast>) -> Vec<String> {
        list.into_iter().map(|c| c.hash).collect()
    }

    #[test]
    fn favorites_rise_far_in_hot_and_top_and_new_stays_in_time_order() {
        // A stranger's busy cast, a favorite's quieter one, and a stranger's
        // newest, quietest one.
        let casts = vec![
            by(1, cast("busy", 30, 0, 0, "2026-10-17T00:00:00Z")),
            by(2, cast("favorite", 10, 0, 0, "2026-10-17T00:00:00Z")),
            by(3, cast("newest", 1, 0, 0, "2026-10-17T01:00:00Z")),
        ];
        let none = Leanings::default();
        for sort in Sort::ALL {
            assert_eq!(sorted_for(&casts, sort, &none), sorted(&casts, sort));
        }
        assert_eq!(order(sorted(&casts, Sort::Top)), ["busy", "favorite", "newest"]);
        let leanings = Leanings { favorites: BTreeSet::from([2]), ..Leanings::default() };
        // 10 points weigh as (10 + 1) × 25 = 275, over 30.
        assert_eq!(order(sorted_for(&casts, Sort::Top, &leanings)), ["favorite", "busy", "newest"]);
        assert_eq!(order(sorted_for(&casts, Sort::Hot, &leanings)), ["favorite", "busy", "newest"]);
        assert_eq!(sorted_for(&casts, Sort::New, &leanings), sorted(&casts, Sort::New));
        // 25 times, no more: 275 is less than 300 points.
        let busier = vec![by(1, cast("busier", 300, 0, 0, "2026-10-17T00:00:00Z")), casts[1].clone()];
        assert_eq!(order(sorted_for(&busier, Sort::Top, &leanings)), ["busier", "favorite"]);
        // In hot, as if 17½ hours newer: a stranger's cast as busy and 17
        // hours newer is below it, one 18 hours newer above.
        let newer = |hours: &str| by(1, cast("newer", 10, 0, 0, &format!("2026-10-17T{hours}:00:00Z")));
        let pair = |hours| vec![newer(hours), casts[1].clone()];
        assert_eq!(order(sorted_for(&pair("17"), Sort::Hot, &leanings)), ["favorite", "newer"]);
        assert_eq!(order(sorted_for(&pair("18"), Sort::Hot, &leanings)), ["newer", "favorite"]);
        // A cast nobody has liked yet rises too: (0 + 1) × 25 weighs as 24
        // points do, and goes first on that tie.
        let quiet = vec![
            by(1, cast("twenty-five", 25, 0, 0, "2026-10-17T00:00:00Z")),
            by(3, cast("twenty-four", 24, 0, 0, "2026-10-17T01:00:00Z")),
            by(2, cast("none yet", 0, 0, 0, "2026-10-17T00:00:00Z")),
        ];
        assert_eq!(order(sorted_for(&quiet, Sort::Top, &leanings)), ["twenty-five", "none yet", "twenty-four"]);
        assert_eq!(leanings.weight(&casts[1]), FAVORITE_WEIGHT);
        assert_eq!(leanings.weight(&casts[0]), 1.0);
    }

    #[test]
    fn showing_less_drops_far_in_hot_and_top_and_lower_in_new() {
        // The busiest cast is by an account the reader sees less of; the
        // quietest is a stranger's, 5 hours older.
        let casts = vec![
            by(1, cast("less", 100, 0, 0, "2026-10-17T12:00:00Z")),
            by(2, cast("twenty", 20, 0, 0, "2026-10-17T12:00:00Z")),
            by(3, cast("five", 5, 0, 0, "2026-10-17T12:00:00Z")),
            by(4, cast("quiet", 0, 0, 0, "2026-10-17T07:00:00Z")),
        ];
        assert_eq!(order(sorted(&casts, Sort::Top)), ["less", "twenty", "five", "quiet"]);
        assert_eq!(order(sorted(&casts, Sort::Hot)), ["less", "twenty", "five", "quiet"]);
        assert_eq!(order(sorted(&casts, Sort::New)), ["five", "less", "twenty", "quiet"]);
        let leanings = Leanings { less: BTreeSet::from([1]), ..Leanings::default() };
        assert!(!leanings.is_empty());
        // 100 points weigh as (100 + 1) / 25, about 4: under 5, over none. In
        // hot, as if 17½ hours older. Nothing is left out.
        assert_eq!(order(sorted_for(&casts, Sort::Top, &leanings)), ["twenty", "five", "less", "quiet"]);
        assert_eq!(order(sorted_for(&casts, Sort::Hot, &leanings)), ["twenty", "five", "less", "quiet"]);
        // In new, as if 6 hours older: below the cast 5 hours older.
        assert_eq!(order(sorted_for(&casts, Sort::New, &leanings)), ["five", "twenty", "quiet", "less"]);
        // One nobody has liked yet goes below a stranger's with no likes
        // either, however much newer: (0 + 1) / 25 is less than 0 + 1.
        let unliked = vec![by(1, cast("less", 0, 0, 0, "2026-10-17T12:00:00Z")), casts[3].clone()];
        assert_eq!(order(sorted_for(&unliked, Sort::Top, &leanings)), ["quiet", "less"]);
        assert_eq!(leanings.weight(&casts[0]), SHOW_LESS_WEIGHT);
        assert_eq!(leanings.weight(&casts[1]), 1.0);
        // An account kept as both counts as a favorite, in new too.
        let both = Leanings { favorites: BTreeSet::from([1]), less: BTreeSet::from([1]) };
        assert_eq!(both.weight(&casts[0]), FAVORITE_WEIGHT);
        assert_eq!(sorted_for(&casts, Sort::New, &both), sorted(&casts, Sort::New));
    }

    /// The account the measured test leans on.
    const TYPICAL: u64 = 100;

    /// A day of casts like a page of the front page's, the same every run:
    /// 114 by 29 accounts at random times over the 24 hours, with 3 to 446
    /// points (most near 40), and 6 by a typical account, [`TYPICAL`], one
    /// every 4 hours, the older ones busier, with 20 to 80 points.
    /// `e2e/leaning-strength.fixture.spec.ts` serves the same casts, at the
    /// same ages, on the front page.
    fn a_day_of_casts() -> Vec<Cast> {
        // Numerical Recipes' generator, seeded with a date: no dependency,
        // and the same numbers in the e2e spec.
        let mut state: u32 = 20_261_021;
        let mut unit = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            f64::from(state >> 8) / f64::from(1u32 << 24)
        };
        let now = parse_rfc3339("2026-10-21T12:00:00Z").unwrap();
        let at = |fid: u64, n: u64, points: u64, ts: i64| {
            let mut c = by(fid, cast(&format!("0x{n:040x}"), points, 0, 0, "2026-10-21T12:00:00Z"));
            c.ts = ts;
            c
        };
        let mut day: Vec<Cast> = (0..114)
            .map(|n| {
                let age = (unit() * 86_400.0) as i64;
                let points = 10f64.powf(1.6 + 0.7 * (unit() + unit() + unit() - 1.5)) as u64;
                at(1 + n % 29, n, points, now - age)
            })
            .collect();
        for (k, points) in (0..).zip([20, 30, 35, 45, 60, 80]) {
            day.push(at(TYPICAL, 200 + k, points, now - (2 + 4 * k as i64) * 3600));
        }
        day
    }

    /// The median place, counting from 1, of `fid`'s casts in `list`.
    fn median_place(list: &[Cast], fid: u64) -> f64 {
        let places: Vec<usize> =
            list.iter().enumerate().filter(|(_, c)| c.author.fid == fid).map(|(i, _)| i + 1).collect();
        let len = places.len();
        (places[(len - 1) / 2] + places[len / 2]) as f64 / 2.0
    }

    #[test]
    fn leanings_move_a_typical_account_far() {
        let day = a_day_of_casts();
        let n = day.len() as f64;
        let hashes = |list: &[Cast]| list.iter().map(|c| c.hash.clone()).collect::<BTreeSet<_>>();
        let less = Leanings { less: BTreeSet::from([TYPICAL]), ..Leanings::default() };
        let favorite = Leanings { favorites: BTreeSet::from([TYPICAL]), ..Leanings::default() };
        for sort in Sort::ALL {
            let anyone = sorted(&day, sort);
            let (lowered, raised) = (sorted_for(&day, sort, &less), sorted_for(&day, sort, &favorite));
            // Every cast is there, once.
            for list in [&lowered, &raised] {
                assert_eq!(list.len(), day.len());
                assert_eq!(hashes(list), hashes(&day));
            }
            let before = median_place(&anyone, TYPICAL);
            let (down, up) = (median_place(&lowered, TYPICAL), median_place(&raised, TYPICAL));
            println!(
                "{}: the typical account's median place of {n}: {before} as anyone's, {down} seen less of, {up} as a favorite",
                sort.as_str()
            );
            // A typical account: in the middle of the list to start with.
            assert!((0.4 * n..=0.6 * n).contains(&before), "{sort:?} starts at {before}");
            if sort == Sort::New {
                // 6 hours of a day's casts is about a quarter of them, so a
                // cast moves about a quarter of the list down, and those near
                // the end only as far as the end: a fifth at least.
                assert!(down - before >= 0.2 * n, "new: {before} to {down}");
                // A favorite's casts stay in time order.
                assert_eq!(raised, anyone);
            } else {
                // Down at least 40% of the list, into its last quarter.
                assert!(down - before >= 0.4 * n && down > 0.75 * n, "{sort:?}: {before} to {down}");
                // A favorite's: up as far, into its first quarter.
                assert!(before - up >= 0.4 * n && up <= 0.25 * n, "{sort:?}: {before} to {up}");
            }
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
