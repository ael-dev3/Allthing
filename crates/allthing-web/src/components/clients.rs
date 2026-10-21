// SPDX-License-Identifier: AGPL-3.0-or-later
//! The share of casts each app posted, from the casts' provenance, and,
//! once the cron has counted them by app, each app's active accounts on
//! hovering it, with how they're counted.

use allthing_core::activity::{AppsWindow, day, day_name};
use allthing_core::clients::{client_name, client_shares, other_clients};
use allthing_core::format::{full_number, plural};
use allthing_core::model::Cast;
use allthing_core::ranking::TrendingWindow;
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::Notes;
use crate::components::popover::Popover;
use crate::util::{dau_href, href, now, with_query};

/// The feed's URL with the reader's sort and time range kept, and the client
/// filter set to `client` (or cleared).
pub fn client_href(sort: Option<String>, range: Option<String>, client: Option<u64>) -> String {
    with_query(&href("/"), &[("sort", sort), ("t", range), ("client", client.map(|fid| fid.to_string()))])
}

/// The client filter in the URL, as an app's fid.
pub fn client_param(query: &leptos_router::params::ParamsMap) -> Option<u64> {
    query.get("client").and_then(|v| v.parse().ok())
}

/// An active client filter as a removable chip. Clearing it keeps the sort
/// and time range.
#[component]
pub fn ClientChip(fid: u64, notes: Notes) -> impl IntoView {
    let query = use_query_map();
    let name = notes.apps.get(&fid).map_or_else(|| format!("app fid {fid}"), |a| client_name(a).to_owned());
    let clear = move || {
        let q = query.read();
        client_href(q.get("sort"), q.get("t"), None)
    };
    let label = format!("Stop showing only casts from {name}");
    view! {
        <p class="client-chip-row">
            <a class="client-chip" href=clear aria-label=label>
                "From "<strong>{name}</strong>" "<span aria-hidden="true">"×"</span>
            </a>
        </p>
    }
}

/// Apps shown by name; the rest are "Other apps".
const TOP_APPS: usize = 4;
/// Casts with a known app needed before shares mean anything.
const MIN_KNOWN: u64 = 20;

/// When a window of accounts by app ends: "14:00 UTC", with its date if
/// that isn't today.
fn window_end(to: i64) -> String {
    let time = format!("{:02}:00 UTC", to.div_euclid(3_600).rem_euclid(24));
    if day(to) == day(now()) { time } else { format!("{} {time}", day_name(day(to))) }
}

/// The card hovering an app in the bar or its legend opens: the accounts
/// active through it in the past 24 hours, and its share of the casts.
#[component]
fn ClientCard(
    name: String,
    fid: u64,
    swatch: usize,
    /// "60%" of `period` trending casts.
    share: String,
    period: &'static str,
    counted: AppsWindow,
) -> impl IntoView {
    let query = use_query_map();
    let target = move || {
        let q = query.read();
        client_href(q.get("sort"), q.get("t"), Some(fid))
    };
    let active = counted.through(fid);
    let known = counted.known_percent();
    // While apps aren't known for every account, some of them may be this
    // app's: its count is a floor.
    let floor = (known < 99).then_some("At least ");
    let accounts = if active == 1 { "active account" } else { "active accounts" };
    let said = match active {
        0 if known < 100 => "No active accounts known yet".to_owned(),
        0 => "No active accounts".to_owned(),
        n => format!("{}{} {accounts}", floor.unwrap_or_default(), full_number(n)),
    };
    let count = match active {
        0 => view! { {said.clone()} }.into_any(),
        n => view! { {floor}<strong>{full_number(n)}</strong>" "{accounts} }.into_any(),
    };
    view! {
        <div class="client-card-body">
            <p class="client-card-name">
                <span class=format!("swatch client-{swatch}") aria-hidden="true"></span>
                {name.clone()}
            </p>
            // The count opens the page of every app's.
            <p class="client-card-active">
                <a href=dau_href() aria-label=format!("{said}, on the page of every app's active accounts")>
                    {count}
                    <span class="client-card-go" aria-hidden="true">"\u{a0}→"</span>
                </a>
            </p>
            <p class="client-card-window">"in the 24 hours to "{window_end(counted.to)}</p>
            // How they're counted, as the cron counts them
            // (`allthing_core::activity`), in a line; /dau says more.
            <p class="client-card-method">
                "Any message (a cast, reaction, follow…) signed with a key this app requested counts its account once, from Hypersnap's events. "
                <a href=dau_href()>"How it's counted"</a>
            </p>
            <p class="client-card-share">{share}" of "{period}" trending casts"</p>
            {(known < 100).then(|| view! {
                <p class="client-card-note">{format!("Apps are known for {known}% of active accounts so far.")}</p>
            })}
            <p class="client-card-links">
                <a class="client-card-filter" href=target>"Show only casts from "{name}</a>
            </p>
        </div>
    }
}

/// A thin bar of the apps `casts` (trending over `window`) were posted from,
/// by share, with a legend. Nothing while too few of their apps are known
/// yet. With `counted` (the cron's accounts by app), hovering, focusing or
/// tapping an app opens its card.
#[component]
pub fn ClientShareBar(
    casts: Vec<Cast>,
    notes: Notes,
    window: TrendingWindow,
    #[prop(optional_no_strip)] counted: Option<AppsWindow>,
) -> impl IntoView {
    let Some(shares) = client_shares(&casts, &notes, TOP_APPS, MIN_KNOWN) else { return ().into_any() };
    let query = use_query_map();
    let current = move || client_param(&query.read());
    let rest = StoredValue::new(other_clients(&casts, &notes, TOP_APPS));
    let known: u64 = shares.iter().map(|s| s.casts).sum();
    let summary = shares.iter().map(|s| format!("{} {}%", s.name, s.percent)).collect::<Vec<_>>().join(", ");
    let period = match window {
        TrendingWindow::Hour => "the past hour's",
        TrendingWindow::Day => "today's",
        TrendingWindow::Week => "the past week's",
        TrendingWindow::Month => "the past month's",
        TrendingWindow::Year => "the past year's",
        TrendingWindow::All => "all-time",
    };
    let note = format!(
        "Apps that posted {known} of {period} trending casts, from the app that registered each cast's signing key."
    );
    let pct_of = |percent: u32| if percent == 0 { "<1%".to_owned() } else { format!("{percent}%") };
    // An app's card, for its segment and its legend entry.
    let card = {
        let counted = counted.clone();
        move |i: usize, fid: u64, name: String, share: String| {
            let counted = counted.clone()?;
            Some(ViewFn::from(move || {
                view! {
                    <ClientCard name=name.clone() fid swatch=i share=share.clone() period counted=counted.clone()/>
                }
            }))
        }
    };
    let card_label = |name: &str| format!("{name}: active accounts");
    let counted_rest = StoredValue::new(counted.clone());
    // What the bar is, read out rather than a native tooltip, which would
    // be drawn over the apps' cards, inside it (and the legend says it all).
    view! {
        <section class="client-share" aria-label=format!("Apps posting {period} trending casts") aria-description=note>
            <div class="share-bar" class:counted=counted.is_some() role="img" aria-label=summary>
                {shares.iter().enumerate().filter(|(_, s)| s.percent > 0).map(|(i, s)| {
                    let grow = format!("flex-grow: {}", s.percent);
                    match s.fid.and_then(|fid| Some((fid, card(i, fid, s.name.clone(), pct_of(s.percent))?))) {
                        // The legend's links are the ones to tab to and to read out.
                        Some((fid, content)) => {
                            let target = move || {
                                let q = query.read();
                                client_href(q.get("sort"), q.get("t"), Some(fid))
                            };
                            view! {
                                <span class="seg-slot" style=grow>
                                    <Popover
                                        label=card_label(&s.name)
                                        hover=true
                                        links=true
                                        class="seg-pop"
                                        panel_class="client-card"
                                        trigger=move || view! {
                                            <a class=format!("seg client-{i}") href=target tabindex="-1" aria-hidden="true"></a>
                                        }
                                        content=content
                                    />
                                </span>
                            }
                            .into_any()
                        }
                        None => view! { <span class=format!("seg client-{i}") style=grow></span> }.into_any(),
                    }
                }).collect_view()}
            </div>
            <ul class="client-legend">
                {shares.iter().enumerate().map(|(i, s)| {
                    let label = s.name.clone();
                    let pct = pct_of(s.percent);
                    match s.fid {
                        Some(fid) => {
                            let target = move || {
                                let q = query.read();
                                client_href(q.get("sort"), q.get("t"), Some(fid))
                            };
                            let link = move || view! {
                                <a href=target aria-current=move || (current() == Some(fid)).then_some("true")>
                                    <span class=format!("swatch client-{i}") aria-hidden="true"></span>
                                    {label.clone()}" "<strong class="pct">{pct.clone()}</strong>
                                </a>
                            };
                            match card(i, fid, s.name.clone(), pct_of(s.percent)) {
                                Some(content) => view! {
                                    <li>
                                        <Popover
                                            label=card_label(&s.name)
                                            hover=true
                                            links=true
                                            panel_class="client-card"
                                            trigger=link
                                            content=content
                                        />
                                    </li>
                                }
                                .into_any(),
                                None => view! { <li>{link()}</li> }.into_any(),
                            }
                        }
                        None => view! {
                            <li>
                                <Popover
                                    label="Other apps"
                                    trigger_label="Other apps"
                                    trigger_class="client-other"
                                    hover=true
                                    trigger=move || view! {
                                        <span class=format!("swatch client-{i}") aria-hidden="true"></span>
                                        {label.clone()}" "<strong class="pct">{pct.clone()}</strong>
                                    }
                                    content=move || view! {
                                        <ul class="client-rest">
                                            {rest.get_value().into_iter().map(|r| {
                                                let fid = r.fid.unwrap_or_default();
                                                let name = r.name.clone();
                                                let pct = if r.percent == 0 { "<1%".to_owned() } else { format!("{}%", r.percent) };
                                                let target = move || {
                                                    let q = query.read();
                                                    client_href(q.get("sort"), q.get("t"), Some(fid))
                                                };
                                                let active = counted_rest.get_value().map(|c| c.through(fid)).filter(|n| *n > 0).map(|n| {
                                                    view! { <span class="client-rest-active">{plural(n, "active account", "active accounts")}</span> }
                                                });
                                                view! {
                                                    <li>
                                                        <a href=target>
                                                            <span>{name}" "<strong class="pct">{pct}</strong></span>
                                                            {active}
                                                        </a>
                                                    </li>
                                                }
                                            }).collect_view()}
                                        </ul>
                                    }
                                />
                            </li>
                        }
                        .into_any(),
                    }
                }).collect_view()}
            </ul>
        </section>
    }
    .into_any()
}
