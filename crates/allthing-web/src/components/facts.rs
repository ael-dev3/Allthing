// SPDX-License-Identifier: AGPL-3.0-or-later
//! Facts about an account that its profile and its profile card share: when
//! it registered, how it voted, and the Farcaster app's label, each saying
//! where it comes from. Labels inform; they never hide anything.

use allthing_core::api::vote_path;
use allthing_core::labels::SpamLabel;
use allthing_core::profile::Registered;
use allthing_core::time::format_date;
use allthing_core::vote::VotePayload;
use leptos::prelude::*;

use crate::api;
use crate::labels::LabelState;
use crate::util::now;

/// When the account registered onchain ("before Nov 2023" for accounts from
/// before the ID registry's migration).
#[component]
pub fn RegisteredFact(ts: i64) -> impl IntoView {
    let Registered { date, note } = Registered::at(ts, now());
    view! {
        <div class="fact" title=note>
            <dt>"Registered"</dt>
            <dd>{date}</dd>
        </div>
    }
}

/// How the account voted when farcaster.xyz asked everyone to stay (yes) or
/// leave (no), loaded when shown. Left out when the sources can't settle it.
#[component]
pub fn VoteFact(fid: u64) -> impl IntoView {
    let data = LocalResource::new(move || async move { api::get::<VotePayload>(&vote_path(fid)).await });
    let row = |answer: AnyView| {
        view! {
            <div class="fact vote-fact">
                <dt>
                    "Farcaster vote "
                    <a class="fact-help" href="https://github.com/ael-dev3/Allthing/blob/main/docs/VOTE.md" title="What was the vote? (docs/VOTE.md)">"?"</a>
                </dt>
                <dd>{answer}</dd>
            </div>
        }
        .into_any()
    };
    move || match data.get() {
        None => {
            Some(row(view! { <span class="vote vote-loading" aria-busy="true" aria-label="Loading the vote"></span> }
                .into_any()))
        }
        Some(Ok(fetched)) => fetched.data.summary().map(|(result, detail)| {
            row(view! { <span class=format!("vote {}", result.class()) title=detail>{result.label()}</span> }
                .into_any())
        }),
        Some(Err(_)) => None,
    }
}

/// The Farcaster app's spam label for the account, read from the static
/// label shards.
#[component]
pub fn LabelFact(label: LocalResource<LabelState>) -> impl IntoView {
    view! {
        <div class="fact">
            <dt>"Farcaster app label"</dt>
            <dd>{move || match label.get() {
                None => view! { <span class="muted">"checking…"</span> }.into_any(),
                Some(state) => view! { <LabelBadge state/> }.into_any(),
            }}</dd>
        </div>
    }
}

#[component]
pub fn LabelBadge(state: LabelState) -> impl IntoView {
    match state {
        LabelState::Labeled(label, meta) => {
            let class = match label {
                SpamLabel::LikelySpam => "spam-label spam-label-spam",
                SpamLabel::Nerfed => "spam-label spam-label-nerfed",
                SpamLabel::NotSpam => "spam-label spam-label-ok",
            };
            let title = format!(
                "{} From the Farcaster app's public spam dataset (github.com/warpcast/labels), newest label {}.",
                label.description(),
                meta.newest_label.map(format_date).unwrap_or_else(|| "of unknown date".into())
            );
            view! { <span class=class title=title>{label.title()}</span> }.into_any()
        }
        LabelState::Unlabeled(meta) => {
            let title = format!(
                "The Farcaster app's public spam dataset (newest label {}) has no label for this account.",
                meta.newest_label.map(format_date).unwrap_or_else(|| "of unknown date".into())
            );
            view! { <span class="spam-label spam-label-none" title=title>"no label"</span> }.into_any()
        }
        LabelState::Unavailable => view! { <span class="muted" title="The label dataset is not available on this deployment.">"unavailable"</span> }.into_any(),
    }
}
