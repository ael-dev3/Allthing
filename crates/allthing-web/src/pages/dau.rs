// SPDX-License-Identifier: AGPL-3.0-or-later
//! Farcaster's daily active accounts, as the cron counts them
//! (`allthing_core::activity`), laid out after SnapMeter's dashboard
//! (github.com/ael-dev3/SnapMeter): the past 24 hours against the 24
//! before, the UTC days of the past month, what the accounts did, which
//! apps they used, and how and since when they were counted.

use allthing_core::activity::{ActivityPayload, AppsWindow, DayCount, Family, day, day_name};
use allthing_core::api::ACTIVITY_PATH;
use allthing_core::clients::{client_name, client_profile};
use allthing_core::format::{full_number, plural};
use allthing_core::model::UserSummary;
use leptos::prelude::*;

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

/// "14:05 UTC".
fn clock(at: i64) -> String {
    format!("{:02}:{:02} UTC", at.div_euclid(3_600).rem_euclid(24), at.div_euclid(60).rem_euclid(60))
}

/// "14:05 UTC", with the date when it isn't today.
fn moment(at: i64) -> String {
    if day(at) == day(now()) { clock(at) } else { format!("{}, {}", date(day(at), true), clock(at)) }
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

#[component]
pub fn DauPage() -> impl IntoView {
    set_title("Daily active accounts");
    let data = LocalResource::new(|| api::get::<ActivityPayload>(ACTIVITY_PATH));
    view! {
        <section class="dau">
            <header class="dau-head">
                <h1 class="page-title">"Daily active accounts"</h1>
                <p class="dau-lede">
                    "Farcaster accounts with a message merged into the network, each counted once, as "
                    <a href="https://github.com/ael-dev3/SnapMeter" target="_blank" rel="noopener noreferrer">"SnapMeter"</a>
                    " counts them."
                </p>
            </header>
            {move || match data.get() {
                None => view! { <p class="muted dau-loading">"Loading the counts…"</p> }.into_any(),
                Some(Err(Failure::Busy { .. })) => view! { <NotCounted/> }.into_any(),
                Some(Err(failure)) => view! { <ErrorPanel failure/> }.into_any(),
                Some(Ok(fetched)) => view! { <Dashboard counted=fetched.data.as_ref().clone()/> }.into_any(),
            }}
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
                view! { <p>"Allthing hasn't counted active accounts yet. The count starts within minutes of a deploy; try again shortly."</p> }.into_any()
            }}
        </section>
    }
}

#[component]
fn Dashboard(counted: ActivityPayload) -> impl IntoView {
    let reached = counted.reached;
    // Today and yesterday by the clock, not by how far reading has got:
    // after a deploy or an outage the cron reads past days, faster than
    // events come, until it has caught up.
    let today = day(now());
    let read = |on: i64| counted.days.iter().find(|d| d.day == on && reached.is_some_and(|at| day(at) >= on)).cloned();
    let (so_far, before) = (read(today), read(today - 1));
    let behind = reached.filter(|at| now() - at > BEHIND);
    let month = counted.active_30d;
    let apps = counted.apps_24h.clone().map(|apps| (apps, counted.app_accounts.clone()));
    let begun_today = counted.since.is_some_and(|since| day(since) >= today);
    view! {
        {behind.map(|at| view! {
            <p class="dau-behind" role="status">
                "Catching up: Allthing has read the network's messages to "{moment(at)}
                " so far, and reads them faster than they come until it is current. The counts below go to then."
            </p>
        })}
        <Meter counted=counted.clone()/>
        <div class="dau-tiles">
            <Tile label="Today so far" count=so_far.as_ref().map(|d| d.active)>
                {match (&so_far, reached) {
                    (Some(_), Some(at)) => format!("from 00:00 to {}", clock(at)),
                    _ => "not read yet".to_owned(),
                }}
            </Tile>
            <Tile label="Yesterday" count=before.as_ref().map(|d| d.active)>
                {match &before {
                    Some(d) => day_note(d, reached),
                    None if begun_today => "before counting began".to_owned(),
                    None => "not read yet".to_owned(),
                }}
            </Tile>
            <Tile label="Past 30 days" count=month.map(|w| w.active)>
                {match month {
                    Some(w) => {
                        let to = format!("to the end of {}", date(w.day, false));
                        if w.complete { to } else { format!("{to}, since counting began") }
                    }
                    None => "once a whole day has been read".to_owned(),
                }}
            </Tile>
        </div>
        <Chart days=counted.days.clone() today reached/>
        <Mix counted=counted.clone()/>
        {apps.map(|(apps, accounts)| view! { <Apps apps accounts/> })}
        <Method counted/>
    }
}

/// The past 24 hours against the 24 before: SnapMeter's meter.
#[component]
fn Meter(counted: ActivityPayload) -> impl IntoView {
    let Some(to) = counted.rolling_to else {
        return view! { <section class="dau-card dau-meter"><p class="dau-what">"Counting has just begun."</p></section> }
            .into_any();
    };
    let Some(active) = counted.rolling_24h else {
        // The first window ends 24 hours after counting began, on the hour.
        let first = counted.since.map(|since| (since + 86_400 + 3_599).div_euclid(3_600) * 3_600);
        let when = match first {
            Some(at) => format!(
                "The first 24-hour count comes once the 24 hours to {}, {} have been read.",
                date(day(at), false),
                clock(at)
            ),
            None => "The first 24-hour count comes once a whole day has been read.".to_owned(),
        };
        return view! {
            <section class="dau-card dau-meter">
                <p class="dau-what">{when}</p>
            </section>
        }
        .into_any();
    };
    let change = counted.previous_24h.filter(|p| *p > 0).map(|previous| {
        let delta = active as f64 / previous as f64 - 1.0;
        let (class, arrow) = if delta >= 0.0 { ("dau-change up", "↑") } else { ("dau-change down", "↓") };
        let text = format!("{arrow} {:.1}% on the 24 hours before ({})", delta.abs() * 100.0, full_number(previous));
        view! { <p class=class>{text}</p> }
    });
    view! {
        <section class="dau-card dau-meter" aria-labelledby="dau-24h">
            <p class="dau-big" id="dau-24h">
                <strong>{full_number(active)}</strong>
                <span class="dau-what">" accounts active in the 24 hours to "{moment(to)}</span>
            </p>
            {change}
        </section>
    }
    .into_any()
}

#[component]
fn Tile(label: &'static str, count: Option<u64>, #[prop(optional)] children: Option<Children>) -> impl IntoView {
    view! {
        <section class="dau-card dau-tile">
            <h2 class="dau-tile-label">{label}</h2>
            <p class="dau-tile-count">{count.map_or_else(|| "—".to_owned(), full_number)}</p>
            <p class="dau-tile-note">{children.map(|c| c())}</p>
        </section>
    }
}

/// The past month's UTC days as bars, with a table of them for screen
/// readers and anyone who wants the numbers. The bars stretch to the width;
/// their labels are text, so they stay readable on a phone.
#[component]
fn Chart(days: Vec<DayCount>, today: i64, reached: Option<i64>) -> impl IntoView {
    if days.is_empty() {
        return ().into_any();
    }
    let n = days.len();
    let max = days.iter().map(|d| d.active).max().unwrap_or(0).max(1);
    let reading = reached.map(day);
    // A day's class, and what its bar and its row say: the day being read is
    // today's so far, or, while catching up, a past day read partly so far.
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
    let summary = format!(
        "Active accounts each UTC day from {} to {}: most {} on {}",
        date(days[0].day, false),
        date(days[n - 1].day, false),
        full_number(max),
        days.iter().find(|d| d.active == max).map_or_else(String::new, |d| date(d.day, false)),
    );
    // Ten units a day, the bars 88 high at most under a line at the top.
    // A slot a day up to today's, read or not, and room for a month however
    // many days there are, so a first day isn't one wide bar.
    let first = days[0].day;
    let end = today.max(days[n - 1].day);
    let span = usize::try_from(end - first + 1).unwrap_or(n);
    let slots = span.max(30);
    let slot = |on: i64| slots - span + usize::try_from(on - first).unwrap_or(0);
    // The day pointed at, focused or tapped: its count and date show above
    // its bar (the table has them all). No native tooltip: it shows late,
    // in the system's style, and never on a touch screen.
    let picked = RwSignal::new(None::<usize>);
    let height = |d: &DayCount| (d.active as f64 / max as f64 * 88.0).max(1.0);
    let tips = StoredValue::new(
        days.iter()
            .map(|d| {
                let (_, said, _) = note(d);
                let accounts = if d.active == 1 { " account" } else { " accounts" };
                (slot(d.day), height(d), full_number(d.active), format!("{accounts}{said}"), date(d.day, true))
            })
            .collect::<Vec<_>>(),
    );
    let bars = days
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let h = height(d);
            let (kind, _, _) = note(d);
            let class = if kind.is_empty() { "dau-bar".to_owned() } else { format!("dau-bar {kind}") };
            let x = slot(d.day) * 10;
            // The bar, and over it its whole column, a target easier to hit
            // than a short bar.
            view! {
                <rect class=class class:on=move || picked.get() == Some(i) x=x + 1 y=100.0 - h width="8" height=h/>
                <rect
                    class="dau-hit"
                    x=x
                    y="0"
                    width="10"
                    height="100"
                    on:pointerenter=move |_| picked.set(Some(i))
                    on:click=move |ev| {
                        ev.stop_propagation();
                        picked.set(Some(i));
                    }
                />
            }
        })
        .collect_view();
    // Keyboard: the chart takes focus once, and the arrows go from day to day.
    let keys = move |ev: leptos::ev::KeyboardEvent| {
        let next = match (ev.key().as_str(), picked.get_untracked()) {
            ("ArrowLeft", Some(i)) => Some(i.saturating_sub(1)),
            ("ArrowRight", Some(i)) => Some((i + 1).min(n - 1)),
            ("ArrowLeft" | "ArrowRight" | "End", None) | ("End", _) => Some(n - 1),
            ("Home", _) => Some(0),
            ("Escape", _) => None,
            _ => return,
        };
        ev.prevent_default();
        picked.set(next);
    };
    let tip = move || picked.get().and_then(|i| tips.with_value(|t| t.get(i).cloned()));
    // Above the bar's top: the plot is 170 px tall under 4 px of padding. By
    // the chart's edges it hangs inward, so it stays on the card.
    let tip_style = move || {
        tip().map_or_else(String::new, |(at, h, ..)| {
            let center = (at as f64 + 0.5) / slots as f64 * 100.0;
            format!("left: {center:.2}%; top: {:.1}px", 4.0 + (100.0 - h) * 1.7)
        })
    };
    let tip_side = move || {
        tip().map_or("", |(at, ..)| match (at as f64 + 0.5) / slots as f64 {
            x if x < 0.15 => "dau-tip start",
            x if x > 0.85 => "dau-tip end",
            _ => "dau-tip",
        })
    };
    // A label a week, counted back from the newest day with a bar (today's,
    // once reading has got to it), so every label names a bar above it. A
    // slot left empty while reading catches up has no label of its own.
    let newest = days[n - 1].day;
    let ticks = (0..)
        .map(|week| newest - 7 * week)
        .take_while(|on| *on >= first)
        .map(|on| {
            let left = format!("left: {:.2}%", (slot(on) as f64 + 0.5) / slots as f64 * 100.0);
            view! { <span style=left>{date(on, false)}</span> }
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect_view();
    view! {
        <section class="dau-card dau-chart">
            <h2>"The past month"</h2>
            <div class="dau-plot">
                <span class="dau-max" aria-hidden="true">{full_number(max)}</span>
                <svg
                    viewBox=format!("0 0 {} 100", slots * 10)
                    role="img"
                    aria-label=summary
                    aria-description="Left and right arrows read each day."
                    preserveAspectRatio="none"
                    tabindex="0"
                    on:keydown=keys
                    on:focus=move |_| picked.update(|p| *p = p.or(Some(n - 1)))
                    on:blur=move |_| picked.set(None)
                    on:click=move |_| picked.set(None)
                    on:pointerleave=move |ev| {
                        if ev.pointer_type() == "mouse" {
                            picked.set(None);
                        }
                    }
                >
                    <line class="dau-grid" x1="0" x2=slots * 10 y1="12" y2="12" vector-effect="non-scaling-stroke"/>
                    {bars}
                </svg>
                <div class=tip_side style=tip_style role="status" hidden=move || tip().is_none()>
                    {move || {
                        tip()
                            .map(|(_, _, count, accounts, when)| {
                                view! {
                                    <strong>{count}</strong>
                                    {accounts}
                                    <span class="dau-tip-day">{when}</span>
                                }
                            })
                    }}
                </div>
                <div class="dau-ticks" aria-hidden="true">{ticks}</div>
            </div>
            <p class="dau-legend">
                <span><span class="dau-key"></span>"a day read whole"</span>
                <span><span class="dau-key partial"></span>"partly counted"</span>
                <span><span class="dau-key today"></span>"today so far"</span>
            </p>
            <details class="dau-table">
                <summary>"The days as a table"</summary>
                <table>
                    <thead><tr><th scope="col">"Day (UTC)"</th><th scope="col">"Active accounts"</th><th scope="col">"Messages"</th></tr></thead>
                    <tbody>
                        {days.iter().rev().map(|d| {
                            let messages: u64 = d.actions.values().sum();
                            let (_, _, note) = note(d);
                            view! {
                                <tr>
                                    <th scope="row">{date(d.day, true)}{note}</th>
                                    <td>{full_number(d.active)}</td>
                                    <td>{full_number(messages)}</td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
            </details>
        </section>
    }
    .into_any()
}

/// What the accounts did on the last day read whole; before one, on the
/// day being read.
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
    let mut families: Vec<(Family, u64)> = shown.actions.iter().map(|(f, n)| (*f, *n)).collect();
    families.sort_by(|a, b| b.1.cmp(&a.1).then(family_name(a.0).cmp(family_name(b.0))));
    let when = if reading == Some(shown.day) && shown.day == day(now()) {
        "today so far".to_owned()
    } else {
        format!("on {}", day_note(&shown, counted.reached))
    };
    view! {
        <section class="dau-card dau-mix">
            <h2>"What they did"</h2>
            <p class="muted">{format!("{} merged {when}, by kind.", plural(total, "message", "messages"))}</p>
            <div class="dau-mix-bar" aria-hidden="true">
                {families.iter().enumerate().map(|(i, (_, n))| view! {
                    <span class=format!("dau-mix-seg mix-{}", i.min(5)) style=format!("flex-grow: {n}")></span>
                }).collect_view()}
            </div>
            <ul class="dau-mix-list">
                {families.into_iter().enumerate().map(|(i, (family, n))| view! {
                    <li>
                        <span class=format!("swatch mix-{}", i.min(5)) aria-hidden="true"></span>
                        <span class="dau-mix-name">{family_name(family)}</span>
                        <strong>{full_number(n)}</strong>
                        <span class="muted">{percent(n, total)}</span>
                    </li>
                }).collect_view()}
            </ul>
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
            <p class="muted">
                {format!("Accounts active through each app in the 24 hours to {}. ", moment(apps.to))}
                "An account's app is the one that requested the key it signed with, so an account that used two apps counts for each."
            </p>
            {(known < 100).then(|| view! {
                <p class="dau-note">{format!("Apps are known for {known}% of active accounts so far: the counts below are floors while the rest are looked up.")}</p>
            })}
            <ol class="dau-app-list">
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
                            <span class="dau-app-bar" aria-hidden="true"><span style=width></span></span>
                            <strong class="dau-app-count">{full_number(a.active)}</strong>
                            <span class="dau-app-share muted">{percent(a.active, apps.active)}</span>
                        </li>
                    }
                }).collect_view()}
            </ol>
            <p class="muted dau-apps-foot">
                {(rest > 0).then(|| format!("{rest} more {} with fewer accounts. ", if rest == 1 { "app" } else { "apps" }))}
                {(apps.none > 0).then(|| format!("{} signed with keys no app requested. ", plural(apps.none, "account", "accounts")))}
                {(apps.unknown > 0).then(|| format!("{} with keys whose app isn't known yet.", plural(apps.unknown, "account", "accounts")))}
            </p>
        </section>
    }
}

/// How the counts are made, and what they cover: SnapMeter's labels.
#[component]
fn Method(counted: ActivityPayload) -> impl IntoView {
    let gaps = counted.gaps.clone();
    view! {
        <section class="dau-card dau-method">
            <h2>"How they're counted"</h2>
            <ul>
                <li>
                    <strong>"Active"</strong>
                    " means at least one message merged into Farcaster: a cast, reaction, follow, profile change, verification, username proof, frame action, storage lending, key or channel message, or the removal of one. That is SnapMeter's closed list for Snapchain, message types 1–8 and 11–21. Pruned, revoked and failed messages, and onchain events, don't count."
                </li>
                <li><strong>"Once"</strong>" per window for each account, however many messages it sent, on whichever shard."</li>
                <li>
                    <strong>"When"</strong>
                    " is the time of the block that confirmed the message. The 24-hour windows are SnapMeter's "
                    <code>"(t − 24 h, t]"</code>
                    ", ending on the hour; days are UTC calendar days."
                </li>
                <li>
                    <strong>"From"</strong>
                    " the event stream of Hypersnap's public node, read every 2 minutes by Allthing's Worker"
                    {counted.since.map(|since| format!(", since {}", moment(since)))}
                    {counted.reached.map(|at| format!("; read to {}", moment(at)))}
                    "."
                </li>
                <li>
                    <strong>"Gaps: "</strong>
                    {if gaps.is_empty() {
                        "none. Every block since counting began has been read.".to_owned()
                    } else {
                        let spans: Vec<String> = gaps.iter().map(|(a, b)| format!("{} to {}", moment(*a), moment(*b))).collect();
                        format!("not read: {}. Windows and days that touch a gap are left out or marked partly counted.", spans.join("; "))
                    }}
                </li>
                <li>
                    <strong>"Exact"</strong>
                    " because every account is kept, as a compact set per hour and per day, so a window counts each one once. Nothing is sampled or estimated."
                </li>
            </ul>
            <p class="dau-links">
                <a href="https://github.com/ael-dev3/SnapMeter/blob/main/docs/metrics.md" target="_blank" rel="noopener noreferrer">"SnapMeter's definitions"</a>
                " · "
                <a href=href(ACTIVITY_PATH)>"The data as JSON"</a>
            </p>
        </section>
    }
}
