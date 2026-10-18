// SPDX-License-Identifier: AGPL-3.0-or-later
//! The share of casts each app posted, from the casts' provenance.

use allthing_core::clients::client_shares;
use allthing_core::model::Cast;
use allthing_core::ranking::TrendingWindow;
use leptos::prelude::*;

use crate::Notes;

/// Apps shown by name; the rest are "Other apps".
const TOP_APPS: usize = 4;
/// Casts with a known app needed before shares mean anything.
const MIN_KNOWN: u64 = 20;

/// A thin bar of the apps `casts` (trending over `window`) were posted from,
/// by share, with a legend. Nothing while too few of their apps are known yet.
#[component]
pub fn ClientShareBar(casts: Vec<Cast>, notes: Notes, window: TrendingWindow) -> impl IntoView {
    let Some(shares) = client_shares(&casts, &notes, TOP_APPS, MIN_KNOWN) else { return ().into_any() };
    let known: u64 = shares.iter().map(|s| s.casts).sum();
    let summary = shares.iter().map(|s| format!("{} {}%", s.name, s.percent)).collect::<Vec<_>>().join(", ");
    let period = match window {
        TrendingWindow::Hour => "the past hour's",
        TrendingWindow::Day => "today's",
        TrendingWindow::Week => "the past week's",
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
                {shares.iter().enumerate().map(|(i, s)| view! {
                    <li>
                        <span class=format!("swatch client-{i}") aria-hidden="true"></span>
                        {s.name.clone()}" "<strong class="pct">{if s.percent == 0 { "<1%".to_owned() } else { format!("{}%", s.percent) }}</strong>
                    </li>
                }).collect_view()}
            </ul>
        </section>
    }
    .into_any()
}
