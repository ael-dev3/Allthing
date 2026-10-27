// SPDX-License-Identifier: AGPL-3.0-or-later
//! Farcaster's daily active accounts, as the cron counts them
//! (`allthing_core::activity`), laid out after SnapMeter's dashboard
//! (github.com/ael-dev3/SnapMeter): its meter (the past 24 hours against
//! the 24 before, today so far, readouts of yesterday, the past 30 days and
//! the 7-day average, and today against a typical day), the history over
//! 48 hours, 7 or 30 days, what the accounts did, which apps they used, and
//! its definitions: how and since when they were counted.

use allthing_core::activity::{
    ACTIVITY_SOURCE, ActivityPayload, AppsWindow, DayCount, Family, day, day_name, median_day, weeks,
};
use allthing_core::api::ACTIVITY_PATH;
use allthing_core::clients::{client_name, client_profile};
use allthing_core::format::{axis, full_number, plural, tick};
use allthing_core::model::UserSummary;
use leptos::prelude::*;
use leptos::wasm_split::{SplitLoaderError, wasm_split};

use crate::api::{self, Failure};
use crate::components::layout::ErrorPanel;
use crate::util::{href, now, set_title, user_href};

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "20 Oct", or "20 Oct 2026" with `year`.
fn date(day: i64, year: bool) -> String {
    let name = day_name(day);
    let (y, rest) = name.split_at(4);
    let month: usize = rest[1..3].parse().unwrap_or(1);
    let d: u32 = rest[4..6].parse().unwrap_or(1);
    let short = format!("{d} {}", MONTHS[month.clamp(1, 12) - 1]);
    if year { format!("{short} {y}") } else { short }
}

/// "14:05".
fn hm(at: i64) -> String {
    format!("{:02}:{:02}", at.div_euclid(3_600).rem_euclid(24), at.div_euclid(60).rem_euclid(60))
}

/// "14:05 UTC".
fn clock(at: i64) -> String {
    format!("{} UTC", hm(at))
}

/// "14:05 UTC", with the date when it isn't today.
fn moment(at: i64) -> String {
    if day(at) == day(now()) { clock(at) } else { format!("{}, {}", date(day(at), true), clock(at)) }
}

/// An evidence timestamp always includes its UTC date.
fn checkpoint_time(at: i64) -> String {
    format!("{}, {}", date(day(at), true), clock(at))
}

/// How far behind the clock reading may be before the page says it is
/// catching up: a run every 2 minutes is normally a few minutes behind.
const BEHIND: i64 = 15 * 60;

/// How much of a day has been read: "19 Oct", "19 Oct, partly counted"
/// (counting began in it, or a gap), or "19 Oct, read to 08:24 UTC so far"
/// while reading is in it.
fn day_note(d: &DayCount, reached: Option<i64>) -> String {
    let name = date(d.day, false);
    match reached {
        Some(at) if day(at) == d.day => format!("{name}, read to {} so far", clock(at)),
        _ if d.complete => name,
        _ => format!("{name}, partly counted"),
    }
}

/// What a family of messages is called on the page.
fn family_name(family: Family) -> &'static str {
    match family {
        Family::Cast => "Casts",
        Family::Reaction => "Reactions",
        Family::Link => "Follows",
        Family::Verification => "Verifications",
        Family::UserData => "Profile changes",
        Family::UsernameProof => "Username proofs",
        Family::Other => "Frame actions",
        Family::StorageLending => "Storage lending",
        Family::Key => "Keys",
        Family::Channel => "Channels",
    }
}

fn percent(part: u64, whole: u64) -> String {
    match (part * 1000).checked_div(whole) {
        None => "0%".into(),
        Some(0) if part > 0 => "<0.1%".into(),
        Some(tenths) if tenths < 100 => format!("{}.{}%", tenths / 10, tenths % 10),
        Some(tenths) => format!("{}%", (tenths + 5) / 10),
    }
}

/// " account" or " accounts", after a count.
fn accounts(n: u64) -> &'static str {
    if n == 1 { " account" } else { " accounts" }
}

/// How a count compares with an earlier one: its direction (a class), an
/// arrow, and the change as a count and as a percent, signed.
struct Change {
    class: &'static str,
    arrow: &'static str,
    count: String,
    pct: String,
}

fn change(now: u64, before: u64) -> Option<Change> {
    (before > 0).then(|| {
        let delta = now.abs_diff(before);
        let pct = format!("{:.1}%", delta as f64 / before as f64 * 100.0);
        match now.cmp(&before) {
            std::cmp::Ordering::Greater => {
                Change { class: "up", arrow: "↑", count: format!("+{}", full_number(delta)), pct: format!("+{pct}") }
            }
            std::cmp::Ordering::Less => {
                Change {
                    class: "down", arrow: "↓", count: format!("−{}", full_number(delta)), pct: format!("−{pct}")
                }
            }
            std::cmp::Ordering::Equal => Change { class: "flat", arrow: "→", count: "0".into(), pct: "0.0%".into() },
        }
    })
}

#[component]
pub fn DauPage() -> impl IntoView {
    set_title("Daily active accounts");
    let data = LocalResource::new(|| api::get::<ActivityPayload>(ACTIVITY_PATH));
    // The dashboard's code comes while the counts do (the static copy has none).
    #[cfg(not(feature = "static-host"))]
    leptos::task::spawn_local(async {
        let _ = preload_dashboard().await;
    });
    let loading = || view! { <p class="muted dau-loading">"Loading the counts…"</p> };
    view! {
        <section class="dau">
            <header class="dau-head">
                <h1 class="page-title">"Daily active accounts"</h1>
                <p class="dau-lede">
                    "Unique Farcaster accounts with a message merged into the network, each counted once, as "
                    <a href="https://github.com/ael-dev3/SnapMeter" target="_blank" rel="noopener noreferrer">"SnapMeter"</a>
                    " counts them. These are accounts, not people or Allthing readers."
                </p>
            </header>
            {move || match data.get() {
                None => loading().into_any(),
                Some(Err(Failure::Busy { .. })) => view! { <NotCounted/> }.into_any(),
                Some(Err(failure)) => view! { <ErrorPanel failure/> }.into_any(),
                Some(Ok(fetched)) if !fetched.data.is_verified(now()) => view! { <AwaitingVerification/> }.into_any(),
                Some(Ok(fetched)) => crate::split::lazy_page(loading, dashboard(fetched.data.as_ref().clone())).into_any(),
            }}
        </section>
    }
}

/// Legacy or inconsistent responses cannot supply dashboard figures.
#[component]
fn AwaitingVerification() -> impl IntoView {
    view! {
        <section class="dau-card dau-empty dau-unverified" role="status">
            <h2>"Counts await verification"</h2>
            <p>"This response has no coherent verification record for both network shards. Counts and charts appear once Allthing has checked their Hypersnap events. Try again after the next count."</p>
            <p class="dau-links">
                <a href=ACTIVITY_SOURCE target="_blank" rel="noopener noreferrer">"Hypersnap event API"</a>
                " · "<a href=href(ACTIVITY_PATH)>"The data as JSON"</a>
            </p>
        </section>
    }
}

/// Before the cron's first count, and in the static copy, which has no cron.
#[component]
fn NotCounted() -> impl IntoView {
    let static_copy = cfg!(feature = "static-host");
    view! {
        <section class="dau-card dau-empty" role="status">
            <h2>"Not counted yet"</h2>
            {if static_copy {
                view! {
                    <p>
                        "Active accounts are counted by Allthing's Worker, which this copy of the site doesn't have. They're at "
                        <a href="https://allthing.social/dau">"allthing.social/dau"</a>"."
                    </p>
                }
                .into_any()
            } else {
                view! { <p>"Allthing hasn't counted active accounts yet. Counts appear after canonical events from both shards have been read; try again later."</p> }.into_any()
            }}
        </section>
    }
}

/// The dashboard, whose code is fetched the first time it's shown
/// (`crate::split`).
#[wasm_split(dau, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_dashboard))]
fn dashboard(counted: ActivityPayload) -> Result<AnyView, SplitLoaderError> {
    Ok(view! { <Dashboard counted/> }.into_any())
}

#[component]
fn Dashboard(counted: ActivityPayload) -> impl IntoView {
    // Today and yesterday by the clock, not by how far reading has got:
    // after a deploy or an outage the cron reads past days, faster than
    // events come, until it has caught up.
    let today = day(now());
    let behind = counted.reached.filter(|at| now() - at > BEHIND);
    let apps = counted.apps_24h.clone().map(|apps| (apps, counted.app_accounts.clone()));
    view! {
        <Coverage counted=counted.clone()/>
        {behind.map(|at| view! {
            <p class="dau-behind" role="status">
                "Catching up: complete event coverage reaches "{moment(at)}
                ". Later time is still unread; partial totals below are lower bounds."
            </p>
        })}
        <Meter counted=counted.clone() today/>
        <History counted=counted.clone() today/>
        <Mix counted=counted.clone()/>
        {apps.map(|(apps, accounts)| view! { <Apps apps accounts/> })}
        <Method counted/>
    }
}

/// The source and durable checkpoints behind the figures, before the meter.
#[component]
fn Coverage(counted: ActivityPayload) -> impl IntoView {
    let started = counted.verified_since;
    let reached = counted.reached;
    let has_coverage = started.zip(reached).is_some_and(|(since, to)| since <= to);
    let status = if has_coverage { "Coverage checked" } else { "Coverage building" };
    let gap_note = match counted.gaps.len() {
        0 => "No recorded gaps in the time read. Time before verification began and time still unread are excluded.",
        _ => "There are recorded gaps. Days touching a gap are partial; complete rolling counts exclude those windows.",
    };
    view! {
        <section class="dau-card dau-coverage" aria-labelledby="dau-coverage-title">
            <header class="dau-coverage-head">
                <h2 id="dau-coverage-title">"Hypersnap event coverage"</h2>
                <span class="dau-proof-status">{status}</span>
            </header>
            <p class="dau-source">
                <a href=counted.source.clone() target="_blank" rel="noopener noreferrer">"Hypersnap event API"</a>
                " · Snapshot updated "{checkpoint_time(counted.meta.fetched_at.div_euclid(1_000))}
            </p>
            <dl class="dau-proof-times">
                <div><dt>"Verified since"</dt><dd>{started.map(checkpoint_time).unwrap_or_else(|| "Waiting for both shards".into())}</dd></div>
                <div><dt>"Complete through"</dt><dd>{reached.map(checkpoint_time).unwrap_or_else(|| "No common completed coverage yet".into())}</dd></div>
            </dl>
            <ul class="dau-shards" aria-label="Verification by shard">
                {counted.shards.into_iter().map(|shard| {
                    let through = shard.reached.map(|at| format!("Complete through {}", checkpoint_time(at)))
                        .unwrap_or_else(|| "Waiting for a completed block".into());
                    let progress = shard.read_events.zip(shard.total_events).map(|(read, total)| {
                        format!("{} / {} events read · {}", full_number(u64::from(read)), full_number(u64::from(total)), if read == total { "block complete" } else { "partial block" })
                    }).unwrap_or_else(|| "Waiting for Hypersnap events".into());
                    view! {
                        <li>
                            <h3>{format!("Shard {}", shard.shard)}</h3>
                            <p class="dau-shard-through">{through}</p>
                            <p>{progress}</p>
                            {shard.block.zip(shard.block_at).map(|(block, at)| view! {
                                <details>
                                    <summary>"Checkpoint"</summary>
                                    <p>{format!("Block {} · {}", full_number(block), checkpoint_time(at))}</p>
                                    <p>"Event cursor "<code>{shard.cursor.unwrap_or_default().to_string()}</code></p>
                                </details>
                            })}
                        </li>
                    }
                }).collect_view()}
            </ul>
            <p class="dau-coverage-note">{gap_note}" Partial day totals and app totals with unresolved keys are lower bounds for their stated windows. This checks the events read from this provider; it does not independently audit the whole network."</p>
        </section>
    }
}

/// SnapMeter's meter: the past 24 hours against the 24 before, with today
/// so far beside them; then yesterday, the past 30 days and the 7-day
/// average; and today so far against a typical day.
#[component]
fn Meter(counted: ActivityPayload, today: i64) -> impl IntoView {
    let reached = counted.reached;
    let read = |on: i64| counted.days.iter().find(|d| d.day == on && reached.is_some_and(|at| day(at) >= on)).cloned();
    let (so_far, before) = (read(today), read(today - 1));
    let today_lower_bound = reached.is_none_or(|at| !counted.covers(today * 86_400, at))
        || counted.shards.iter().any(|s| s.read_events != s.total_events);
    let begun_today = counted.since.is_some_and(|since| day(since) >= today);
    // How far the counts go: the dot is a status, its words say which.
    let status = reached.map(|at| {
        if now() - at <= BEHIND {
            ("dau-status", format!("Up to date · {}", clock(at)))
        } else {
            ("dau-status behind", format!("Catching up · {}", moment(at)))
        }
    });
    let window = match counted.rolling_to {
        Some(to) => format!("Unique accounts active in the 24 hours to {}", moment(to)),
        None => "Unique accounts active in a rolling 24 hours".to_owned(),
    };
    // The count and how it compares, or when it comes: the first window
    // ends 24 hours after counting began, on the hour.
    let active = counted.rolling_to.and(counted.rolling_24h);
    let pending = match (counted.rolling_to, active) {
        (_, Some(_)) => None,
        (Some(_), None) => Some(match counted.since.map(|since| (since + 86_400 + 3_599).div_euclid(3_600) * 3_600) {
            Some(at) => format!(
                "The first 24-hour count comes once the 24 hours to {}, {} have been read.",
                date(day(at), false),
                clock(at)
            ),
            None => "The first 24-hour count comes once a whole day has been read.".to_owned(),
        }),
        (None, None) => Some("Counting has just begun.".to_owned()),
    };
    let versus = active.zip(counted.previous_24h).and_then(|(now, previous)| Some((change(now, previous)?, previous)));
    let month = counted.active_30d;
    let week = weeks(&counted.days);
    let trend = week.and_then(|w| change(w.average, w.before));
    let week_note = match week {
        Some(_) if trend.is_some() => " on the 7 days before".to_owned(),
        Some(w) => format!("to the end of {}", date(w.to, false)),
        None => "once 14 days have been read whole".to_owned(),
    };
    // Today so far against the median of the month's days read whole, as
    // SnapMeter measures today against its 30-day median.
    let baseline = so_far.as_ref().zip(median_day(&counted.days).filter(|m| *m > 0)).map(|(d, median)| {
        let pct = (d.active * 100 + median / 2) / median;
        view! {
            <div class="dau-baseline">
                <p class="dau-baseline-label">
                    "Today so far, against the month's median day ("{full_number(median)}")"
                </p>
                <span class="dau-baseline-track" aria-hidden="true">
                    <span style=format!("width: {}%", pct.min(100))></span>
                </span>
                <strong class="dau-baseline-pct">{format!("{pct}%")}</strong>
            </div>
        }
    });
    view! {
        <section class="dau-card dau-meter" aria-labelledby="dau-24h">
            <header class="dau-meter-head">
                <div>
                    <h2 id="dau-24h">"Past 24 hours"</h2>
                    <p class="dau-window">{window}</p>
                </div>
                {status.map(|(class, text)| view! { <p class=class><span class="dau-dot" aria-hidden="true"></span>{text}</p> })}
            </header>
            <div class="dau-primary">
                <div class="dau-headline">
                    <p class="dau-big" class:pending=active.is_none() aria-hidden=active.is_none().then_some("true")>
                        {active.map_or_else(|| "—".to_owned(), full_number)}
                    </p>
                    {pending.map(|text| view! { <p class="dau-what">{text}</p> })}
                    {versus.map(|(c, previous)| view! {
                        <p class=format!("dau-change {}", c.class)>
                            <span aria-hidden="true">{c.arrow}</span>" "<strong>{c.count}</strong>" · "{c.pct}
                            " on the 24 hours before ("{full_number(previous)}")"
                        </p>
                    })}
                </div>
                <Tile label="Today so far" count=so_far.as_ref().map(|d| d.active) kind="dau-today" lower_bound=today_lower_bound>
                    {match (&so_far, reached) {
                        (Some(_), Some(at)) => format!("{} to {}", hm(counted.since.unwrap_or(today * 86_400).max(today * 86_400)), clock(at)),
                        _ => "not read yet".to_owned(),
                    }}
                </Tile>
            </div>
            <div class="dau-readouts">
                <Tile label="Yesterday" count=before.as_ref().map(|d| d.active) lower_bound=before.as_ref().is_some_and(|d| !d.complete)>
                    {match &before {
                        Some(d) => day_note(d, reached),
                        None if begun_today => "before counting began".to_owned(),
                        None => "not read yet".to_owned(),
                    }}
                </Tile>
                <Tile label="Past 30 days" count=month.map(|w| w.active) lower_bound=month.is_some_and(|w| !w.complete)>
                    {match month {
                        Some(w) => {
                            let to = format!("to the end of {}", date(w.day, false));
                            if w.complete { to } else { format!("{to}, since counting began") }
                        }
                        None => "once a whole day has been read".to_owned(),
                    }}
                </Tile>
                <Tile label="7-day average" count=week.map(|w| w.average)>
                    {trend.map(|c| view! {
                        <span class=format!("dau-trend {}", c.class)><span aria-hidden="true">{c.arrow}</span>" "{c.pct}</span>
                    })}
                    {week_note}
                </Tile>
            </div>
            {baseline}
        </section>
    }
}

#[component]
fn Tile(
    label: &'static str,
    count: Option<u64>,
    #[prop(optional)] kind: &'static str,
    #[prop(optional)] lower_bound: bool,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class=format!("dau-tile {kind}")>
            <h3 class="dau-tile-label">{label}</h3>
            <p class="dau-tile-count">
                {(lower_bound && count.is_some()).then(|| view! { <span class="dau-lower-bound">"≥ "</span> })}
                {count.map_or_else(|| "—".to_owned(), full_number)}
            </p>
            <p class="dau-tile-note">{children.map(|c| c())}</p>
        </div>
    }
}

/// What the history chart can show, SnapMeter's ranges, by index: a `u32`,
/// whose signal's code the app already has.
const RANGES: [&str; 3] = ["48 hours", "7 days", "30 days"];
const MONTH: u32 = 2;

/// One bar: its slot, its count, how it's drawn ("", or lighter: "partial",
/// "today", "before"), and what its readout says after the count, and when.
struct Bar {
    slot: usize,
    active: u64,
    class: &'static str,
    said: String,
    when: String,
}

/// A label under the bars, `at` a fraction of the width.
struct Tick {
    at: f64,
    text: String,
    strong: bool,
    /// Left out on a narrow screen.
    minor: bool,
}

/// A range's bars, `slots` wide, and what they show, read out.
struct Chart {
    slots: usize,
    bars: Vec<Bar>,
    ticks: Vec<Tick>,
    label: String,
    keys: &'static str,
}

/// A range's chart, what it covers, its legend, a sentence on it, and its
/// table.
struct Plot {
    sub: String,
    chart: Chart,
    legend: Vec<(&'static str, String)>,
    summary: String,
    /// "days" or "hours".
    unit: &'static str,
    head: Vec<&'static str>,
    rows: Vec<(String, Vec<String>)>,
}

/// The UTC days of the `span` to today (or to the newest day read, should
/// reading be past it), a slot a day, so a short history keeps the room of
/// a whole range and today's slot is empty until it is read.
fn by_day(counted: &ActivityPayload, today: i64, span: i64) -> Option<Plot> {
    let reading = counted.reached.map(day);
    let end = today.max(counted.days.last()?.day);
    let start = end - span + 1;
    let days: Vec<&DayCount> = counted.days.iter().filter(|d| (start..=end).contains(&d.day)).collect();
    let newest = days.last()?.day;
    // A day's class, and what its readout and its row say: the day being
    // read is today's so far, or, while catching up, a past day read partly
    // so far.
    let note = |d: &DayCount| {
        if Some(d.day) == reading && d.day == today {
            ("today", " so far", " (so far)")
        } else if Some(d.day) == reading {
            ("partial", " (read so far)", " (read so far)")
        } else if d.complete {
            ("", "", "")
        } else {
            ("partial", " (partly counted)", " (partly)")
        }
    };
    let bars: Vec<Bar> = days
        .iter()
        .map(|d| {
            let (class, said, _) = note(d);
            let slot = usize::try_from(d.day - start).unwrap_or(0);
            Bar {
                slot,
                active: d.active,
                class,
                said: format!("{}{said}", accounts(d.active)),
                when: date(d.day, true),
            }
        })
        .collect();
    let max = days.iter().map(|d| d.active).max().unwrap_or(0);
    let busiest = days.iter().find(|d| d.active == max).map_or(newest, |d| d.day);
    let label = format!(
        "Active accounts each UTC day from {} to {}: most {} on {}",
        date(days[0].day, false),
        date(newest, false),
        full_number(max),
        date(busiest, false)
    );
    // A label a week (a day, over a week), counted back from the newest day
    // with a bar (today's, once reading has got to it), so every label
    // names a bar above it. A slot left empty while reading catches up has
    // no label of its own.
    let every = if span > 7 { 7 } else { 1 };
    let mut ticks: Vec<Tick> = (0..)
        .map(|k| newest - every * k)
        .take_while(|on| *on >= start)
        .map(|on| Tick {
            at: ((on - start) as f64 + 0.5) / span as f64,
            text: date(on, false),
            strong: on == newest,
            minor: false,
        })
        .collect();
    ticks.reverse();
    let whole: Vec<u64> = days.iter().filter(|d| d.complete).map(|d| d.active).collect();
    let summary = match whole.len() as u64 {
        0 => "No day here has been read whole yet.".to_owned(),
        n => format!(
            "On average {} a day over the {} read whole; the most, {}, on {}.",
            full_number((whole.iter().sum::<u64>() + n / 2) / n),
            plural(n, "day", "days"),
            full_number(max),
            date(busiest, false)
        ),
    };
    let mut legend = vec![("", "a whole UTC day".to_owned())];
    if days.iter().any(|d| !note(d).0.is_empty()) {
        legend.push(("light", "today so far, or a day partly counted".to_owned()));
    }
    let rows = days
        .iter()
        .rev()
        .map(|d| {
            let messages: u64 = d.actions.values().sum();
            (format!("{}{}", date(d.day, true), note(d).2), vec![full_number(d.active), full_number(messages)])
        })
        .collect();
    let slots = usize::try_from(span).unwrap_or(30);
    Some(Plot {
        sub: format!("Unique accounts each UTC day, the {span} days to {}", date(end, false)),
        chart: Chart { slots, bars, ticks, label, keys: "Left and right arrows read each day." },
        legend,
        summary,
        unit: "days",
        head: vec!["Day (UTC)", "Active accounts", "Merge events"],
        rows,
    })
}

/// The 48 hours to the end of the 24-hour windows, an hour a slot: the 24
/// the meter counts, and the 24 before them, lighter.
fn by_hour(counted: &ActivityPayload) -> Option<Plot> {
    let to = counted.rolling_to?;
    let n = counted.hourly.len();
    let first = to - n as i64 * 3_600;
    let name = |i: usize| {
        let from = first + i as i64 * 3_600;
        format!("{}, {}–{}", date(day(from), false), hm(from), clock(from + 3_600))
    };
    let bars: Vec<Bar> = (counted.hourly.iter().enumerate())
        .filter_map(|(i, active)| {
            let active = (*active)?;
            let class = if i + 24 >= n { "" } else { "before" };
            Some(Bar { slot: i, active, class, said: accounts(active).to_owned(), when: name(i) })
        })
        .collect();
    let max = bars.iter().map(|b| b.active).max()?;
    let busiest = bars.iter().find(|b| b.active == max).map_or(0, |b| b.slot);
    let label = format!(
        "Active accounts each UTC hour of the {n} to {}: most {} in {}",
        moment(to),
        full_number(max),
        name(busiest)
    );
    // A label every 6 hours, on the hours' boundaries inside the chart: the
    // date at midnight, and on a narrow screen every 12 hours.
    let ticks = (1..n)
        .filter_map(|i| {
            let at = first + i as i64 * 3_600;
            let h = at.div_euclid(3_600).rem_euclid(24);
            (h % 6 == 0).then(|| Tick {
                at: i as f64 / n as f64,
                text: if h == 0 { date(day(at), false) } else { hm(at) },
                strong: false,
                minor: h % 12 != 0,
            })
        })
        .collect();
    let summary = format!(
        "The busiest hour: {}, {}. An account active in several hours counts in each.",
        plural(max, "account", "accounts"),
        name(busiest)
    );
    let legend = vec![("", format!("the 24 hours to {}", moment(to))), ("light", "the 24 before".to_owned())];
    let rows = bars.iter().rev().map(|b| (b.when.clone(), vec![full_number(b.active)])).collect();
    Some(Plot {
        sub: format!("Unique accounts each UTC hour, the {n} hours to {}", moment(to)),
        chart: Chart { slots: n, bars, ticks, label, keys: "Left and right arrows read each hour." },
        legend,
        summary,
        unit: "hours",
        head: vec!["Hour (UTC)", "Active accounts"],
        rows,
    })
}

/// The history: by hour over the past 48 hours, or by UTC day over 7 or 30
/// days, with a table of what it shows. Each range is drawn once; the
/// buttons show one.
#[component]
fn History(counted: ActivityPayload, today: i64) -> impl IntoView {
    let plots: Vec<(u32, Plot)> = [by_hour(&counted), by_day(&counted, today, 7), by_day(&counted, today, 30)]
        .into_iter()
        .zip(0..)
        .filter_map(|(plot, range)| Some((range, plot?)))
        .collect();
    if !plots.iter().any(|(range, _)| *range == MONTH) {
        return ().into_any();
    }
    let range = RwSignal::new(MONTH);
    let buttons = (plots.iter().map(|(r, _)| *r))
        .map(|r| {
            view! {
                <button type="button" aria-pressed=move || (range.get() == r).to_string() on:click=move |_| range.set(r)>
                    {RANGES[r as usize]}
                </button>
            }
        })
        .collect_view();
    let shown = plots
        .into_iter()
        .map(|(r, Plot { sub, chart, legend, summary, unit, head, rows })| {
            view! {
                <div class="dau-history" hidden=move || range.get() != r>
                    <p class="dau-sub">{sub}</p>
                    <Bars chart/>
                    <p class="dau-legend">
                        {legend.into_iter().map(|(class, text)| view! {
                            <span><span class=format!("dau-key {class}")></span>{text}</span>
                        }).collect_view()}
                    </p>
                    <p class="dau-summary">{summary}</p>
                    <details class="dau-table">
                        <summary>{format!("The {unit} as a table")}</summary>
                        <table>
                            <thead><tr>{head.into_iter().map(|h| view! { <th scope="col">{h}</th> }).collect_view()}</tr></thead>
                            <tbody>
                                {rows.into_iter().map(|(name, cells)| view! {
                                    <tr>
                                        <th scope="row">{name}</th>
                                        {cells.into_iter().map(|c| view! { <td>{c}</td> }).collect_view()}
                                    </tr>
                                }).collect_view()}
                            </tbody>
                        </table>
                    </details>
                </div>
            }
        })
        .collect_view();
    view! {
        <section class="dau-card dau-chart" aria-labelledby="dau-history">
            <header class="dau-chart-head">
                <h2 id="dau-history">"Active accounts"</h2>
                <div class="dau-range" role="group" aria-label="Range">{buttons}</div>
            </header>
            {shown}
        </section>
    }
    .into_any()
}

/// A chart's bars on a value axis of round steps. A bar pointed at, tapped
/// or reached with the arrow keys shows its count and when by it (above a
/// short bar, beside a tall one's top, so it stays on the plot); the others
/// dim. No native tooltip: it shows late, in the system's style, and never
/// on a touch screen.
#[component]
fn Bars(chart: Chart) -> impl IntoView {
    let Chart { slots, bars, ticks, label, keys } = chart;
    let n = bars.len();
    let (step, top) = axis(bars.iter().map(|b| b.active).max().unwrap_or(0));
    let height = move |v: u64| v as f64 / top as f64 * 100.0;
    let width = 100.0 / slots.max(1) as f64;
    let picked = RwSignal::new(None::<usize>);
    let grid = (0..=top / step)
        .map(|k| {
            let style = format!("bottom: {:.2}%", height(k * step));
            view! { <span class="dau-gridline" class:base={k == 0} style=style><span>{tick(k * step)}</span></span> }
        })
        .collect_view();
    let tips = StoredValue::new(
        bars.iter()
            .map(|b| {
                ((b.slot as f64 + 0.5) * width, height(b.active), full_number(b.active), b.said.clone(), b.when.clone())
            })
            .collect::<Vec<_>>(),
    );
    // Each bar's whole column is what the pointer and a tap hit: a target
    // easier to hit than a short bar.
    let columns = bars
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let column = format!("left: {:.3}%; width: {width:.3}%", b.slot as f64 * width);
            let bar = format!("height: {:.2}%", height(b.active).max(0.5));
            view! {
                <span
                    class="dau-col"
                    style=column
                    on:pointerenter=move |_| picked.set(Some(i))
                    on:click=move |ev| {
                        ev.stop_propagation();
                        picked.set(Some(i));
                    }
                >
                    <span class=format!("dau-bar {}", b.class) class:on=move || picked.get() == Some(i) style=bar></span>
                </span>
            }
        })
        .collect_view();
    // Keyboard: the chart takes focus once, and the arrows go from bar to bar.
    let keydown = move |ev: leptos::ev::KeyboardEvent| {
        let last = n.saturating_sub(1);
        let next = match (ev.key().as_str(), picked.get_untracked()) {
            ("ArrowLeft", Some(i)) => Some(i.saturating_sub(1)),
            ("ArrowRight", Some(i)) => Some((i + 1).min(last)),
            ("ArrowLeft" | "ArrowRight" | "End", None) | ("End", _) => Some(last),
            ("Home", _) => Some(0),
            ("Escape", _) => None,
            _ => return,
        };
        ev.prevent_default();
        picked.set(next);
    };
    // The readout: above a short bar, hanging inward by the chart's edges;
    // beside the top of a bar taller than `TALL`, toward the middle, where
    // one above it would reach the range buttons over the chart.
    const TALL: f64 = 55.0;
    let readout = move || {
        let (x, h, count, said, when) = picked.get().and_then(|i| tips.with_value(|t| t.get(i).cloned()))?;
        let (class, style) = match (h > TALL, x) {
            (true, x) if x < 50.0 => ("dau-tip right", format!("left: calc({x:.2}% + 10px); bottom: {h:.2}%")),
            (true, x) => ("dau-tip left", format!("left: calc({x:.2}% - 10px); bottom: {h:.2}%")),
            (false, x) if x < 15.0 => ("dau-tip start", format!("left: {x:.2}%; bottom: calc({h:.2}% + 8px)")),
            (false, x) if x > 85.0 => ("dau-tip end", format!("left: {x:.2}%; bottom: calc({h:.2}% + 8px)")),
            (false, x) => ("dau-tip", format!("left: {x:.2}%; bottom: calc({h:.2}% + 8px)")),
        };
        Some(view! {
            <div class=class style=style>
                <strong>{count}</strong>
                {said}
                <span class="dau-tip-day">{when}</span>
            </div>
        })
    };
    let ticks = ticks
        .into_iter()
        .map(|t| {
            view! { <span class:strong=t.strong class:minor=t.minor style=format!("left: {:.2}%", t.at * 100.0)>{t.text}</span> }
        })
        .collect_view();
    view! {
        <div class="dau-plot">
            <div class="dau-area">
                <div class="dau-grid" aria-hidden="true">{grid}</div>
                <div
                    class="dau-cols"
                    role="img"
                    aria-label=label
                    aria-description=keys
                    tabindex="0"
                    on:keydown=keydown
                    on:focus=move |_| picked.update(|p| *p = p.or(n.checked_sub(1)))
                    on:blur=move |_| picked.set(None)
                    on:click=move |_| picked.set(None)
                    on:pointerleave=move |ev| {
                        if ev.pointer_type() == "mouse" {
                            picked.set(None);
                        }
                    }
                >
                    {columns}
                </div>
                <div class="dau-tip-layer" role="status">{readout}</div>
            </div>
            <div class="dau-ticks" aria-hidden="true">{ticks}</div>
        </div>
    }
}

/// What the accounts did on the last day read whole; before one, on the
/// day being read: SnapMeter's mix, a bar a kind.
#[component]
fn Mix(counted: ActivityPayload) -> impl IntoView {
    let reading = counted.reached.map(day);
    let shown = (counted.days.iter().rev().find(|d| d.complete))
        .or_else(|| counted.days.iter().find(|d| Some(d.day) == reading))
        .or_else(|| counted.days.last())
        .cloned();
    let Some(shown) = shown else { return ().into_any() };
    let total: u64 = shown.actions.values().sum();
    if total == 0 {
        return ().into_any();
    }
    // Most first, then by name: ten kinds at most, so each goes in its place
    // (a sort would bring the standard library's, 4 KB of it).
    let mut families: Vec<(&str, u64)> = Vec::new();
    for (family, n) in &shown.actions {
        let name = family_name(*family);
        let at = families.iter().position(|(m, k)| k < n || (k == n && *m > name)).unwrap_or(families.len());
        families.insert(at, (name, *n));
    }
    let most = families.first().map_or(1, |f| f.1).max(1);
    let when = if reading == Some(shown.day) && shown.day == day(now()) {
        "today so far".to_owned()
    } else {
        format!("on {}", day_note(&shown, counted.reached))
    };
    view! {
        <section class="dau-card dau-mix">
            <h2>"What they did"</h2>
            <p class="dau-sub">{format!("{} {when}, by kind.", plural(total, "message merge event", "message merge events"))}</p>
            <ol class="dau-rows dau-mix-list">
                {families.into_iter().map(|(name, n)| view! {
                    <li>
                        <span class="dau-mix-name">{name}</span>
                        <span class="dau-row-bar" aria-hidden="true">
                            <span style=format!("width: {:.1}%", n as f64 / most as f64 * 100.0)></span>
                        </span>
                        <strong class="dau-row-count">{full_number(n)}</strong>
                        <span class="dau-row-share">{percent(n, total)}</span>
                    </li>
                }).collect_view()}
            </ol>
        </section>
    }
    .into_any()
}

/// Accounts by the app that requested the key they signed with.
#[component]
fn Apps(apps: AppsWindow, accounts: Vec<UserSummary>) -> impl IntoView {
    let shown = apps.apps.iter().take(allthing_core::activity::NAMED_APPS).copied().collect::<Vec<_>>();
    let rest = apps.apps.len().saturating_sub(shown.len());
    let known = apps.known_percent();
    let top = shown.first().map_or(1, |a| a.active).max(1);
    view! {
        <section class="dau-card dau-apps">
            <h2>"By app"</h2>
            <p class="dau-sub">
                {format!("Accounts active through each app in the 24 hours to {}. ", moment(apps.to))}
                "An account's app is the one that requested the key it signed with, so an account that used two apps counts for each."
            </p>
            {(known < 100).then(|| view! {
                <p class="dau-note">{format!("Apps are known for {known}% of active accounts so far: the counts below are floors while the rest are looked up.")}</p>
            })}
            <ol class="dau-rows dau-app-list">
                {shown.into_iter().map(|a| {
                    let account = accounts.iter().find(|u| u.fid == a.app);
                    let name = account.map_or_else(|| format!("app fid {}", a.app), |u| client_name(u).to_owned());
                    let link = account.map(|u| user_href(&client_profile(u)));
                    let pfp = account.and_then(|u| u.pfp_url.clone());
                    let width = format!("width: {:.1}%", a.active as f64 / top as f64 * 100.0);
                    view! {
                        <li>
                            <span class="dau-app-name">
                                {match pfp {
                                    Some(src) => view! { <img src=src alt="" width="20" height="20" loading="lazy" referrerpolicy="no-referrer"/> }.into_any(),
                                    None => view! { <span class="dau-app-avatar" aria-hidden="true"></span> }.into_any(),
                                }}
                                {match link {
                                    Some(link) => view! { <a href=link>{name}</a> }.into_any(),
                                    None => view! { <span>{name}</span> }.into_any(),
                                }}
                            </span>
                            <span class="dau-row-bar" aria-hidden="true"><span style=width></span></span>
                            <strong class="dau-row-count dau-app-count">{full_number(a.active)}</strong>
                            <span class="dau-row-share dau-app-share">{percent(a.active, apps.active)}</span>
                        </li>
                    }
                }).collect_view()}
            </ol>
            <p class="dau-sub dau-apps-foot">
                {(rest > 0).then(|| format!("{rest} more {} with fewer accounts. ", if rest == 1 { "app" } else { "apps" }))}
                {(apps.none > 0).then(|| format!("{} signed with keys no app requested. ", plural(apps.none, "account", "accounts")))}
                {(apps.unknown > 0).then(|| format!("{} with keys whose app isn't known yet.", plural(apps.unknown, "account", "accounts")))}
            </p>
        </section>
    }
}

/// How the counts are made, and what they cover: SnapMeter's definitions.
#[component]
fn Method(counted: ActivityPayload) -> impl IntoView {
    let gaps = if counted.gaps.is_empty() {
        "No recorded gaps within the completed coverage shown above.".to_owned()
    } else {
        let spans: Vec<String> =
            counted.gaps.iter().map(|(a, b)| format!("{} to {}", moment(*a), moment(*b))).collect();
        format!(
            "Not fully covered: {}. Missing source history or expired local data leave windows out or mark days partly counted.",
            spans.join("; ")
        )
    };
    let from = format!(
        "The event stream of Hypersnap's public node, read every 2 minutes by Allthing's Worker{}{}. A message's time is that of the block that confirmed it.",
        counted.since.map(|since| format!(", since {}", moment(since))).unwrap_or_default(),
        counted.reached.map(|at| format!("; read to {}", moment(at))).unwrap_or_default(),
    );
    let mut terms: Vec<(&str, String)> = vec![
        (
            "Active",
            "At least one message merged into Farcaster: a cast, reaction, follow, profile change, verification, username proof, frame action, storage lending, key or channel message, or the removal of one. That is SnapMeter's closed list for Snapchain, message types 1–8 and 11–21. Pruned, revoked and failed messages, and onchain events, don't count. Each account counts once in a window, however many messages it sent, on whichever shard. The action chart counts message merge events: a message present on both shards contributes to both there; account totals still count its account once.".to_owned(),
        ),
        (
            "Past 24 hours",
            "SnapMeter's rolling window, (t − 24 h, t], to the start of the hour in progress, against the 24 hours before it.".to_owned(),
        ),
        (
            "Today, a day",
            "A UTC calendar day from 00:00, today's so far. A day is read whole when counting had begun by its start and no gap touches it; the others are partly counted.".to_owned(),
        ),
        ("Past 30 days", "The 30 UTC days to the end of yesterday, each account once: not the days added up.".to_owned()),
        ("7-day average", "The mean of the last 7 days read whole, against the 7 days before them.".to_owned()),
        (
            "By app",
            "The app that requested the key a message was signed with, so an account that used two apps counts for each.".to_owned(),
        ),
        ("From", from),
        ("Gaps", gaps),
        (
            "Exact",
            "Every account in the verified events is kept as a compact set per hour and per day. A fully covered window counts each account once, without sampling. Partial counts are lower bounds; events outside verified coverage have not been counted.".to_owned(),
        ),
    ];
    if counted.hourly.iter().any(Option::is_some) {
        terms.insert(
            5,
            (
                "By hour",
                "Each UTC hour's accounts, an hour ending on its last second. An account active in several hours counts in each.".to_owned(),
            ),
        );
    }
    view! {
        <section class="dau-card dau-method" aria-labelledby="dau-how">
            <h2 id="dau-how">"How they're counted"</h2>
            <dl class="dau-terms">
                {terms.into_iter().map(|(term, text)| view! { <div><dt>{term}</dt><dd>{text}</dd></div> }).collect_view()}
            </dl>
            <p class="dau-links">
                <a href="https://github.com/ael-dev3/SnapMeter/blob/main/docs/metrics.md" target="_blank" rel="noopener noreferrer">"SnapMeter's definitions"</a>
                " · "
                <a href=href(ACTIVITY_PATH)>"The data as JSON"</a>
            </p>
        </section>
    }
}
