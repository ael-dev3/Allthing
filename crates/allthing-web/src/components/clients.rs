// SPDX-License-Identifier: AGPL-3.0-or-later
//! The share of casts each app posted, from the casts' provenance.

use allthing_core::clients::{client_name, client_shares, other_clients};
use allthing_core::model::Cast;
use allthing_core::ranking::TrendingWindow;
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::Notes;
use crate::components::popover::Popover;
use crate::util::{href, with_query};

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

/// A thin bar of the apps `casts` (trending over `window`) were posted from,
/// by share, with a legend. Nothing while too few of their apps are known yet.
#[component]
pub fn ClientShareBar(casts: Vec<Cast>, notes: Notes, window: TrendingWindow) -> impl IntoView {
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
    view! {
        <section class="client-share" aria-label=format!("Apps posting {period} trending casts") title=note>
            <div class="share-bar" role="img" aria-label=summary>
                {shares.iter().enumerate().filter(|(_, s)| s.percent > 0).map(|(i, s)| view! {
                    <span class=format!("seg client-{i}") style=format!("flex-grow: {}", s.percent) title=format!("{}: {}%", s.name, s.percent)></span>
                }).collect_view()}
            </div>
            <ul class="client-legend">
                {shares.iter().enumerate().map(|(i, s)| {
                    let label = s.name.clone();
                    let pct = if s.percent == 0 { "<1%".to_owned() } else { format!("{}%", s.percent) };
                    match s.fid {
                        Some(fid) => {
                            let target = move || {
                                let q = query.read();
                                client_href(q.get("sort"), q.get("t"), Some(fid))
                            };
                            view! {
                                <li>
                                    <a href=target aria-current=move || (current() == Some(fid)).then_some("true")>
                                        <span class=format!("swatch client-{i}") aria-hidden="true"></span>
                                        {label}" "<strong class="pct">{pct}</strong>
                                    </a>
                                </li>
                            }
                            .into_any()
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
                                                view! { <li><a href=target>{name}" "<strong class="pct">{pct}</strong></a></li> }
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
