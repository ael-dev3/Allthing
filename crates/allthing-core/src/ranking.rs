// SPDX-License-Identifier: AGPL-3.0-or-later
//! Engagement-based ranking. Farcaster has likes, recasts and replies but no
//! downvotes, so a cast's points only ever count positive engagement.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::model::{Cast, ThreadNode};

/// Allthing's ingest-only buffer for small source clock skew, preserving the
/// previously tested five-second case. This is not a protocol threshold and
/// never changes the strict present upper bound used when serving or drawing.
pub const INGEST_FUTURE_ALLOWANCE_SECS: i64 = 5;

pub fn may_ingest_timestamp(ts: i64, now: i64) -> bool {
    ts <= now.saturating_add(INGEST_FUTURE_ALLOWANCE_SECS)
}

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
    SixHours,
    TwelveHours,
    #[default]
    Day,
    Week,
    Month,
    /// Not in the menu any more: a `t=year` link still shows the past year.
    Year,
    All,
}

impl TimeRange {
    /// In menu order, the narrowest first. `Year` is left out of the menu
    /// unless it is the range shown.
    pub const ALL: [TimeRange; 8] = [
        TimeRange::Hour,
        TimeRange::SixHours,
        TimeRange::TwelveHours,
        TimeRange::Day,
        TimeRange::Week,
        TimeRange::Month,
        TimeRange::All,
        TimeRange::Year,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TimeRange::Hour => "hour",
            TimeRange::SixHours => "6h",
            TimeRange::TwelveHours => "12h",
            TimeRange::Day => "day",
            TimeRange::Week => "week",
            TimeRange::Month => "month",
            TimeRange::Year => "year",
            TimeRange::All => "all",
        }
    }

    /// Unknown or missing values fall back to the default, 24 hours.
    pub fn parse(value: Option<&str>) -> TimeRange {
        TimeRange::ALL.into_iter().find(|r| Some(r.as_str()) == value).unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            TimeRange::Hour => "Past hour",
            TimeRange::SixHours => "6 hours",
            TimeRange::TwelveHours => "12 hours",
            TimeRange::Day => "24 hours",
            TimeRange::Week => "7 days",
            TimeRange::Month => "30 days",
            TimeRange::Year => "Past year",
            TimeRange::All => "All time",
        }
    }

    /// The window this range is ranked over: Hypersnap's trending windows up
    /// to a week, Allthing's own index beyond.
    pub fn window(self) -> TrendingWindow {
        match self {
            TimeRange::Hour => TrendingWindow::Hour,
            TimeRange::SixHours => TrendingWindow::SixHours,
            TimeRange::TwelveHours => TrendingWindow::TwelveHours,
            TimeRange::Day => TrendingWindow::Day,
            TimeRange::Week => TrendingWindow::Week,
            TimeRange::Month => TrendingWindow::Month,
            TimeRange::Year => TrendingWindow::Year,
            TimeRange::All => TrendingWindow::All,
        }
    }
}

/// The windows the front page ranks casts over: Hypersnap's trending
/// windows (up to a week), and the longer ones ranked from Allthing's own
/// index of past casts (see [`crate::index`]). Every window but All time
/// holds only the casts posted within its span ([`TrendingWindow::holds`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TrendingWindow {
    Hour,
    SixHours,
    TwelveHours,
    #[default]
    Day,
    Week,
    Month,
    Year,
    All,
}

impl TrendingWindow {
    pub const ALL: [TrendingWindow; 8] = [
        TrendingWindow::Hour,
        TrendingWindow::SixHours,
        TrendingWindow::TwelveHours,
        TrendingWindow::Day,
        TrendingWindow::Week,
        TrendingWindow::Month,
        TrendingWindow::Year,
        TrendingWindow::All,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TrendingWindow::Hour => "1h",
            TrendingWindow::SixHours => "6h",
            TrendingWindow::TwelveHours => "12h",
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

    /// How far back the window reaches, in seconds; `None` for all time.
    pub fn span_secs(self) -> Option<i64> {
        const HOUR: i64 = 3_600;
        match self {
            TrendingWindow::Hour => Some(HOUR),
            TrendingWindow::SixHours => Some(6 * HOUR),
            TrendingWindow::TwelveHours => Some(12 * HOUR),
            TrendingWindow::Day => Some(24 * HOUR),
            TrendingWindow::Week => Some(7 * 24 * HOUR),
            TrendingWindow::Month => Some(30 * 24 * HOUR),
            TrendingWindow::Year => Some(365 * 24 * HOUR),
            TrendingWindow::All => None,
        }
    }

    /// Whether a cast posted at `ts` is inside the window at `now` (both in
    /// seconds since the Unix epoch, which has no time zone). The window
    /// starts just after `now − span`: a cast exactly that old is out, and
    /// one a second newer is in. Future-dated casts are out in every range,
    /// including all time. A retained cast can become visible when `now` catches up.
    pub fn holds(self, ts: i64, now: i64) -> bool {
        ts <= now && self.span_secs().is_none_or(|span| ts > now - span)
    }

    /// The window in words, for a note under the list.
    pub fn past(self) -> &'static str {
        match self {
            TrendingWindow::Hour => "the past hour",
            TrendingWindow::SixHours => "the past 6 hours",
            TrendingWindow::TwelveHours => "the past 12 hours",
            TrendingWindow::Day => "the past 24 hours",
            TrendingWindow::Week => "the past 7 days",
            TrendingWindow::Month => "the past 30 days",
            TrendingWindow::Year => "the past year",
            TrendingWindow::All => "all time",
        }
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

/// A reader's private preferences. Favorites rise in hot and top;
/// Show less keeps roughly one in ten authored casts in feeds and strongly
/// reduces the remaining casts' rank. Direct profiles and search use the
/// ordinary ranking helpers and remain available.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Leanings {
    pub favorites: BTreeSet<u64>,
    pub less: BTreeSet<u64>,
    /// Accounts the reader follows, for the front page's For you: their
    /// casts rise in hot and top ([`FOLLOWED_HOT_WEIGHT`],
    /// [`FOLLOWED_TOP_WEIGHT`]) and keep their time in new. Empty elsewhere.
    pub followed: BTreeSet<u64>,
}

/// A favorite's cast weighs in hot and top as if it had 25 times its points.
///
/// Why 25: most casts on a page of a day's feed have points within about
/// three times one another's (on the front page on 21 October 2026, 28 to
/// 88 around a median of 45), spread over the 24 hours. 25 times lifts a
/// typical cast over the busiest of them in top, and in hot its log,
/// 1.4, is worth 17½ hours of freshness ([`HOT_HALF_LIFE_SECS`]), which
/// takes a cast from the middle of a day's casts to among the newest. In a
/// list that spans more days, a cast moves that much for each of them
/// ([`span_days`]), so as far up a week's list as a day's. In 120 casts
/// over a day or a week, a typical account's median place goes from about
/// 60th to about 4th in hot and in top (`leanings_move_a_typical_account_far`).
pub const FAVORITE_WEIGHT: f64 = 25.0;

/// Retained Show less casts carry one hundredth of their usual engagement
/// weight in hot and top. This also overrides favorite and followed lifts.
pub const SHOW_LESS_WEIGHT: f64 = 0.01;

/// Stable approximate retention rate for explicit Show less preferences.
/// It is a reduction, not a mute: a large set retains roughly one in ten.
pub const SHOW_LESS_KEEP_DENOMINATOR: u64 = 10;

/// Whether a feed keeps this authored cast for the reader. Sampling depends
/// only on author FID and cast hash, so pagination, sorting, refreshed counts
/// and timestamps cannot make a suppressed post reappear. FNV-1a is fixed
/// here rather than using a process-seeded standard-library hasher.
pub fn keep_for_feed(cast: &Cast, leanings: &Leanings) -> bool {
    if !leanings.less.contains(&cast.author.fid) {
        return true;
    }
    let mut bucket = 0xcbf2_9ce4_8422_2325_u64;
    for byte in cast.author.fid.to_le_bytes().into_iter().chain(cast.hash.bytes().map(|byte| byte.to_ascii_lowercase()))
    {
        bucket = (bucket ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    bucket.is_multiple_of(SHOW_LESS_KEEP_DENOMINATOR)
}

/// In For you's hot, a cast by an account the reader follows weighs as if
/// it had 20 times its points.
///
/// Why 20: For you is the trending pool with the reader's own circle lifted
/// well into it, as X's is. Its log, 1.3, is worth about 16 hours of
/// freshness for each day the list spans ([`HOT_HALF_LIFE_SECS`]), nearly
/// a favorite's 17½: a followed account's cast from this morning ranks with
/// the newest of everyone's, yet one from yesterday still yields to a fresh
/// cast that is far busier. In 120 casts over a day, a typical account's
/// median place goes from about 60th to about 6th, in hot and in top
/// (`following_lifts_a_typical_account_in_hot`). A favorite one follows
/// weighs both ([`FAVORITE_WEIGHT`] times this); one the reader sees less
/// of gets no lift.
pub const FOLLOWED_HOT_WEIGHT: f64 = 20.0;

/// In For you's top, a followed account's cast weighs 6 times its points:
/// less than in hot, because top is the reader asking for the busiest casts
/// of the range, and 6 is enough to put a typical followed cast (about 45
/// points on 21 October 2026) above everyone's busiest of the day (270),
/// without lifting a cast nobody reacted to over busy ones.
pub const FOLLOWED_TOP_WEIGHT: f64 = 6.0;

/// How many days `casts` span, from the oldest to the newest, and one at
/// least: in hot a leaning moves a cast by its move for a day as
/// many times, so as far down, or up, a list of a week as of a day.
pub fn span_days(casts: &[Cast]) -> f64 {
    let oldest = casts.iter().map(|c| c.ts).min().unwrap_or_default();
    let newest = casts.iter().map(|c| c.ts).max().unwrap_or_default();
    ((newest - oldest) as f64 / 86_400.0).max(1.0)
}

impl Leanings {
    pub fn is_empty(&self) -> bool {
        self.favorites.is_empty() && self.less.is_empty() && self.followed.is_empty()
    }

    /// [`Leanings::weight`] in `sort`, with the lift of a followed account:
    /// [`FOLLOWED_HOT_WEIGHT`] in hot and [`FOLLOWED_TOP_WEIGHT`] in top,
    /// times a favorite's, and none for an account the reader sees less of.
    /// In new nothing rises: the list stays in time order.
    pub fn weight_in(&self, cast: &Cast, sort: Sort) -> f64 {
        let base = self.weight(cast);
        let lift = match sort {
            _ if base < 1.0 || !self.followed.contains(&cast.author.fid) => 1.0,
            Sort::Hot => FOLLOWED_HOT_WEIGHT,
            Sort::Top => FOLLOWED_TOP_WEIGHT,
            Sort::New => 1.0,
        };
        match sort {
            Sort::New => 1.0,
            Sort::Hot | Sort::Top => base * lift,
        }
    }

    /// How much more, or less, `cast` weighs for this reader in hot and top.
    /// Show less wins over a stale overlapping favorite preference.
    pub fn weight(&self, cast: &Cast) -> f64 {
        let fid = cast.author.fid;
        if self.less.contains(&fid) {
            SHOW_LESS_WEIGHT
        } else if self.favorites.contains(&fid) {
            FAVORITE_WEIGHT
        } else {
            1.0
        }
    }
}

/// Feed ranking with the reader's leanings and stable Show less reduction.
/// Hot and top retain the existing favorite/followed boosts, while remaining
/// Show less posts receive the smaller weight. New keeps actual timestamps
/// and strict chronological order after thinning.
pub fn sorted_for(casts: &[Cast], sort: Sort, leanings: &Leanings) -> Vec<Cast> {
    if leanings.is_empty() {
        return sorted(casts, sort);
    }
    if sort == Sort::New {
        let mut kept: Vec<Cast> = casts.iter().filter(|cast| keep_for_feed(cast, leanings)).cloned().collect();
        sort_casts(&mut kept, sort);
        return kept;
    }
    let weight = |cast: &Cast| leanings.weight_in(cast, sort);
    let days = span_days(casts);
    let key = |cast: &Cast, weight: f64| match sort {
        Sort::Top => (points(cast) + 1) as f64 * weight,
        Sort::Hot => hot_score(points(cast), cast.ts) + weight.log10() * days,
        Sort::New => cast.ts as f64,
    };
    let mut keyed: Vec<(f64, f64, &Cast)> = casts
        .iter()
        .filter(|cast| keep_for_feed(cast, leanings))
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
        assert_eq!(TimeRange::SixHours.window().as_str(), "6h");
        assert_eq!(TimeRange::TwelveHours.window().as_str(), "12h");
        assert_eq!(TimeRange::Day.window().as_str(), "24h");
        assert_eq!(TimeRange::parse(Some("6h")), TimeRange::SixHours);
        assert_eq!(TimeRange::parse(Some("12h")), TimeRange::TwelveHours);
        // The menu: seven ranges, narrowest first, with Past year left out.
        let menu: Vec<&str> =
            TimeRange::ALL.into_iter().filter(|r| *r != TimeRange::Year).map(TimeRange::label).collect();
        assert_eq!(menu, ["Past hour", "6 hours", "12 hours", "24 hours", "7 days", "30 days", "All time"]);
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

    #[test]
    fn a_window_holds_casts_inside_its_span_at_the_edges() {
        let now = 1_792_000_000;
        // Exactly one span old is out; a second newer is in.
        for (window, span) in [
            (TrendingWindow::Hour, 3_600),
            (TrendingWindow::SixHours, 21_600),
            (TrendingWindow::TwelveHours, 43_200),
            (TrendingWindow::Day, 86_400),
            (TrendingWindow::Week, 604_800),
            (TrendingWindow::Month, 2_592_000),
            (TrendingWindow::Year, 31_536_000),
        ] {
            assert_eq!(window.span_secs(), Some(span), "{window:?}");
            assert!(!window.holds(now - span, now), "{window:?} exact boundary is out");
            assert!(window.holds(now - span + 1, now), "{window:?} one second inside is in");
            assert!(!window.holds(now - span - 1, now), "{window:?} one second outside is out");
            assert!(window.holds(now, now), "{window:?} now is in");
        }
        // No unbounded clock-skew allowance: even a slightly early cast waits
        // until its timestamp is reached, rather than outranking the present.
        for window in TrendingWindow::ALL {
            for future in [now + 1, now + 5, 2_893_718_638, i64::MAX] {
                assert!(!window.holds(future, now), "{window:?} future {future} is out");
            }
            assert!(window.holds(now + 5, now + 5), "{window:?} eligible when the clock catches up");
        }
    }

    #[test]
    fn a_window_for_all_time_has_no_lower_bound_but_excludes_the_future() {
        assert_eq!(TrendingWindow::All.span_secs(), None);
        assert!(TrendingWindow::All.holds(0, 1_792_000_000));
        assert!(TrendingWindow::All.holds(i64::MIN / 2, 1_792_000_000));
    }

    #[test]
    fn ingest_alone_keeps_five_seconds_of_source_skew_with_saturating_arithmetic() {
        let now = 1_792_000_000;
        assert!(may_ingest_timestamp(now + 5, now));
        assert!(!may_ingest_timestamp(now + 6, now));
        assert!(!may_ingest_timestamp(2_893_718_638, now));
        assert!(may_ingest_timestamp(i64::MAX, i64::MAX - 2));
        assert!(!TrendingWindow::All.holds(now + 5, now));
    }

    #[test]
    fn a_window_is_plain_seconds_across_midnight_utc() {
        // 23:50 to 00:20 UTC, with the clock at 00:10: a one-hour window
        // holds 23:50 onward and no time zone shifts the edge.
        let midnight = 1_792_022_400; // 00:00:00 UTC
        let now = midnight + 10 * 60;
        assert!(TrendingWindow::Hour.holds(midnight - 10 * 60, now));
        assert!(TrendingWindow::Hour.holds(midnight - 49 * 60, now));
        assert!(!TrendingWindow::Hour.holds(midnight - 50 * 60, now));
    }

    #[test]
    fn the_windows_say_what_they_cover() {
        assert_eq!(TrendingWindow::SixHours.past(), "the past 6 hours");
        assert_eq!(TrendingWindow::Day.past(), "the past 24 hours");
        assert_eq!(TrendingWindow::All.past(), "all time");
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
            references: Default::default(),
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
        // In hot, as if 17½ hours newer, as two casts span less than a day: a
        // stranger's cast as busy and 17 hours newer is below it, one 18
        // hours newer above.
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
    fn showing_less_removes_most_posts_and_strongly_lowers_the_remaining_hot_and_top() {
        let retained = format!("0x{:040x}", 0);
        let suppressed = format!("0x{:040x}", 1);
        let casts = vec![
            by(1, cast(&retained, 20, 0, 0, "2026-10-17T12:00:00Z")),
            by(1, cast(&suppressed, 1_000_000, 0, 0, "2026-10-17T14:00:00Z")),
            by(2, cast("twenty", 20, 0, 0, "2026-10-17T11:00:00Z")),
            by(3, cast("five", 5, 0, 0, "2026-10-17T13:00:00Z")),
            by(4, cast("quiet", 0, 0, 0, "2026-10-17T07:00:00Z")),
        ];
        let less = Leanings { less: BTreeSet::from([1]), ..Leanings::default() };
        assert!(keep_for_feed(&casts[0], &less));
        assert!(!keep_for_feed(&casts[1], &less));
        for sort in [Sort::Hot, Sort::Top] {
            assert_eq!(
                order(sorted_for(&casts, sort, &less)),
                vec!["twenty".to_owned(), "five".to_owned(), "quiet".to_owned(), retained.clone()]
            );
        }
        assert_eq!(
            order(sorted_for(&casts, Sort::New, &less)),
            vec!["five".to_owned(), retained.clone(), "twenty".to_owned(), "quiet".to_owned()]
        );
        let both = Leanings { favorites: BTreeSet::from([1]), followed: BTreeSet::from([1]), ..less.clone() };
        for sort in Sort::ALL {
            assert_eq!(sorted_for(&casts, sort, &both), sorted_for(&casts, sort, &less));
        }
        assert_eq!(both.weight(&casts[0]), SHOW_LESS_WEIGHT);
        assert_eq!(both.weight_in(&casts[0], Sort::Hot), SHOW_LESS_WEIGHT);
        assert_eq!(both.weight_in(&casts[0], Sort::New), 1.0);
        assert_eq!(sorted(&casts, Sort::New).len(), casts.len(), "ordinary/direct ranking does not thin posts");
    }

    #[test]
    fn show_less_selection_is_stable_across_pages_permutations_and_count_refreshes() {
        let leaning = Leanings { less: BTreeSet::from([42]), ..Leanings::default() };
        let casts: Vec<_> =
            (0..1_000).map(|i| by(42, cast(&format!("0x{i:040x}"), i, 0, 0, "2026-10-17T12:00:00Z"))).collect();
        let selected = |casts: &[Cast]| {
            casts
                .iter()
                .filter(|cast| keep_for_feed(cast, &leaning))
                .map(|cast| cast.hash.clone())
                .collect::<BTreeSet<_>>()
        };
        let kept = selected(&casts);
        assert!((80..=120).contains(&kept.len()), "About 90% should disappear: kept {} of 1,000", kept.len());
        let paged: BTreeSet<_> = casts.chunks(37).flat_map(selected).collect();
        assert_eq!(paged, kept);
        let mut reversed = casts.clone();
        reversed.reverse();
        assert_eq!(selected(&reversed), kept);
        let mut refreshed = casts.clone();
        for cast in &mut refreshed {
            cast.ts += 86_400;
            cast.likes += 999;
            cast.recasts = 500;
            cast.replies = 100;
            cast.hash = cast.hash.to_ascii_uppercase();
        }
        assert_eq!(
            selected(&refreshed).into_iter().map(|hash| hash.to_ascii_lowercase()).collect::<BTreeSet<_>>(),
            kept
        );
        for cast in &mut refreshed {
            cast.author.fid = 7;
        }
        assert!(refreshed.iter().all(|cast| keep_for_feed(cast, &leaning)), "unselected authors keep every post");
        // Fixed vectors lock selection across browser sessions and builds.
        let typical = Leanings { less: BTreeSet::from([100]), ..Leanings::default() };
        let selected: Vec<_> = (200..260)
            .filter(|id| {
                keep_for_feed(&by(100, cast(&format!("0x{id:040x}"), 1, 0, 0, "2026-10-17T12:00:00Z")), &typical)
            })
            .collect();
        assert_eq!(selected, [200, 215, 222, 224, 239, 247], "The 60-post browser fixture keeps 6 posts");
        assert_eq!(
            casts
                .iter()
                .take(40)
                .filter(|cast| keep_for_feed(cast, &leaning))
                .map(|cast| cast.hash.clone())
                .collect::<Vec<_>>(),
            vec![format!("0x{:040x}", 1), format!("0x{:040x}", 10)]
        );
    }

    /// The account the measured test leans on.
    const TYPICAL: u64 = 100;

    /// `days` of casts like a page of the front page's, the same every run:
    /// 114 by 29 accounts at random times over them, with 3 to 446 points
    /// (most near 40), and 6 by a typical account, [`TYPICAL`], spread over
    /// them (over a day, one every 4 hours), the older ones busier, with 20
    /// to 80 points. Over a week they're the same casts, 7 times further
    /// apart. `e2e/leaning-strength.fixture.spec.ts` serves a day's, at the
    /// same ages, on the front page.
    fn casts_over(days: i64) -> Vec<Cast> {
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
        let mut list: Vec<Cast> = (0..114)
            .map(|n| {
                let age = (unit() * 86_400.0 * days as f64) as i64;
                let points = 10f64.powf(1.6 + 0.7 * (unit() + unit() + unit() - 1.5)) as u64;
                at(1 + n % 29, n, points, now - age)
            })
            .collect();
        for (k, points) in (0..).zip([20, 30, 35, 45, 60, 80]) {
            list.push(at(TYPICAL, 200 + k, points, now - (2 + 4 * k as i64) * 3600 * days));
        }
        list
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
        let hashes = |list: &[Cast]| list.iter().map(|c| c.hash.clone()).collect::<BTreeSet<_>>();
        let less = Leanings { less: BTreeSet::from([TYPICAL]), ..Leanings::default() };
        let favorite = Leanings { favorites: BTreeSet::from([TYPICAL]), ..Leanings::default() };
        // A day, as on the front page, and a week, as in a quiet channel.
        for days in [1, 7] {
            let list = casts_over(days);
            let n = list.len() as f64;
            for sort in Sort::ALL {
                let anyone = sorted(&list, sort);
                let (lowered, raised) = (sorted_for(&list, sort, &less), sorted_for(&list, sort, &favorite));
                assert_eq!(raised.len(), list.len());
                assert_eq!(hashes(&raised), hashes(&list));
                assert_eq!(lowered.iter().filter(|c| c.author.fid == TYPICAL).count(), 1);
                assert_eq!(lowered.len(), 115);
                assert_eq!(
                    hashes(&lowered.iter().filter(|c| c.author.fid != TYPICAL).cloned().collect::<Vec<_>>()),
                    hashes(&list.iter().filter(|c| c.author.fid != TYPICAL).cloned().collect::<Vec<_>>())
                );
                let before = median_place(&anyone, TYPICAL);
                let (down, up) = (median_place(&lowered, TYPICAL), median_place(&raised, TYPICAL));
                println!(
                    "{days} day(s), {}: the typical account's median place of {n}: {before} as anyone's, {down} seen less of, {up} as a favorite",
                    sort.as_str()
                );
                // A typical account: in the middle of the list to start with.
                assert!((0.4 * n..=0.6 * n).contains(&before), "{days}d {sort:?} starts at {before}");
                if sort == Sort::New {
                    assert!(
                        lowered.windows(2).all(|pair| pair[0].ts >= pair[1].ts),
                        "new remains chronological after thinning"
                    );
                    // A favorite's casts stay in time order.
                    assert_eq!(raised, anyone);
                } else {
                    // Down at least 40% of the list, into its last quarter.
                    assert!(down - before >= 0.4 * n && down > 0.75 * n, "{days}d {sort:?}: {before} to {down}");
                    // A favorite's: up as far, into its first quarter.
                    assert!(before - up >= 0.4 * n && up <= 0.25 * n, "{days}d {sort:?}: {before} to {up}");
                }
            }
        }
    }

    #[test]
    fn following_lifts_a_typical_account_in_hot() {
        let followed = Leanings { followed: BTreeSet::from([TYPICAL]), ..Leanings::default() };
        let list = casts_over(1);
        let n = list.len() as f64;
        let before = median_place(&sorted(&list, Sort::Hot), TYPICAL);
        let hot = median_place(&sorted_for(&list, Sort::Hot, &followed), TYPICAL);
        let top_before = median_place(&sorted(&list, Sort::Top), TYPICAL);
        let top = median_place(&sorted_for(&list, Sort::Top, &followed), TYPICAL);
        println!("For you, 1 day of {n}: hot {before} to {hot}, top {top_before} to {top}");
        // Hot: from the middle into the first tenth or so.
        assert!(hot <= 0.12 * n, "hot: {before} to {hot}");
        // Top: well up too, into the first quarter.
        assert!(top <= 0.25 * n, "top: {top_before} to {top}");
        // Every cast is still there.
        assert_eq!(sorted_for(&list, Sort::Hot, &followed).len(), list.len());
    }

    #[test]
    fn new_stays_chronological_for_followed_accounts() {
        let casts = vec![
            by(1, cast("old-followed", 90, 0, 0, "2026-10-17T00:00:00Z")),
            by(2, cast("newest", 0, 0, 0, "2026-10-17T02:00:00Z")),
            by(1, cast("middle-followed", 50, 0, 0, "2026-10-17T01:00:00Z")),
        ];
        let followed = Leanings { followed: BTreeSet::from([1]), ..Leanings::default() };
        let favorite_too = Leanings { favorites: BTreeSet::from([1]), ..followed.clone() };
        for leanings in [&followed, &favorite_too] {
            assert_eq!(order(sorted_for(&casts, Sort::New, leanings)), ["newest", "middle-followed", "old-followed"]);
            assert_eq!(leanings.weight_in(&casts[0], Sort::New), 1.0);
        }
    }

    #[test]
    fn a_followed_cast_beats_a_busier_strangers_in_hot_and_top() {
        // A stranger's cast with five times the points, as fresh.
        let casts = vec![
            by(2, cast("stranger", 100, 0, 0, "2026-10-17T00:00:00Z")),
            by(1, cast("followed", 20, 0, 0, "2026-10-17T00:00:00Z")),
        ];
        let followed = Leanings { followed: BTreeSet::from([1]), ..Leanings::default() };
        assert_eq!(order(sorted(&casts, Sort::Hot)), ["stranger", "followed"]);
        for sort in [Sort::Hot, Sort::Top] {
            assert_eq!(order(sorted_for(&casts, sort, &followed)), ["followed", "stranger"], "{sort:?}");
        }
        assert_eq!(followed.weight_in(&casts[1], Sort::Hot), FOLLOWED_HOT_WEIGHT);
        assert_eq!(followed.weight_in(&casts[1], Sort::Top), FOLLOWED_TOP_WEIGHT);
        assert_eq!(followed.weight_in(&casts[0], Sort::Hot), 1.0);
        // A favorite one follows weighs both; one seen less of gets no lift.
        let favorite = Leanings { favorites: BTreeSet::from([1]), ..followed.clone() };
        assert_eq!(favorite.weight_in(&casts[1], Sort::Hot), FAVORITE_WEIGHT * FOLLOWED_HOT_WEIGHT);
        let less = Leanings { less: BTreeSet::from([1]), ..followed };
        assert_eq!(less.weight_in(&casts[1], Sort::Hot), SHOW_LESS_WEIGHT);
    }

    #[test]
    fn hot_moves_grow_with_span_while_new_stays_chronological_after_thinning() {
        // Two days of casts, from the oldest to the newest.
        let newest = by(1, cast("newest", 10, 0, 0, "2026-10-17T00:00:00Z"));
        let oldest = by(3, cast("oldest", 1, 0, 0, "2026-10-15T00:00:00Z"));
        assert_eq!(span_days(&[newest.clone(), oldest.clone()]), 2.0);
        // A day at least.
        assert_eq!(span_days(std::slice::from_ref(&newest)), 1.0);
        assert_eq!(span_days(&[]), 1.0);
        // In hot, a favorite's cast is as if twice 17½ hours newer: a
        // stranger's as busy and 34 hours newer is below it, 35 hours above.
        let favorites = Leanings { favorites: BTreeSet::from([2]), ..Leanings::default() };
        let with_favorite = |at: &str| vec![newest.clone(), by(2, cast("favorite", 10, 0, 0, at)), oldest.clone()];
        let hot = |list: Vec<Cast>| order(sorted_for(&list, Sort::Hot, &favorites));
        assert_eq!(hot(with_favorite("2026-10-15T14:00:00Z")), ["favorite", "newest", "oldest"]);
        assert_eq!(hot(with_favorite("2026-10-15T13:00:00Z")), ["newest", "favorite", "oldest"]);
        let less = Leanings { less: BTreeSet::from([1]), ..Leanings::default() };
        let mut retained = newest.clone();
        retained.hash = format!("0x{:040x}", 0);
        let casts = vec![
            retained.clone(),
            by(2, cast("eleven", 1, 0, 0, "2026-10-16T13:00:00Z")),
            by(4, cast("thirteen", 1, 0, 0, "2026-10-16T11:00:00Z")),
            oldest.clone(),
        ];
        assert_eq!(
            order(sorted_for(&casts, Sort::New, &less)),
            vec![retained.hash, "eleven".into(), "thirteen".into(), "oldest".into()]
        );
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
