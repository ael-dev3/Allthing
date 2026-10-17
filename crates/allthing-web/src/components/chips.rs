// SPDX-License-Identifier: AGPL-3.0-or-later
//! Small inline pieces: times, avatars, user chips, scores, engagement and
//! provenance.

use crate::Notes;
use allthing_core::embeds::{ImageSize, resized_image_url};
use allthing_core::format::{compact_number, full_number, score, short_address};
use allthing_core::model::{Cast, UserSummary};
use allthing_core::provenance::Backend;
use allthing_core::ranking::points;
use allthing_core::time::{format_date_time, iso_utc, time_ago};
use leptos::prelude::*;

use crate::util::{PROVENANCE_DOC_URL, now, user_href};

#[component]
pub fn TimeAgo(ts: i64) -> impl IntoView {
    view! { <time datetime=iso_utc(ts) title=format_date_time(ts)>{time_ago(ts, now())}</time> }
}

#[component]
pub fn Avatar(url: Option<String>, #[prop(default = 24)] size: u32, #[prop(optional)] large: bool) -> impl IntoView {
    let variant = if large { ImageSize::Thumb } else { ImageSize::Avatar };
    match url {
        Some(url) => view! {
            <img
                class="avatar"
                src=resized_image_url(&url, variant)
                alt=""
                width=size
                height=size
                loading="lazy"
                decoding="async"
                referrerpolicy="no-referrer"
            />
        }
        .into_any(),
        None => view! { <span class="avatar avatar-empty" style:width=format!("{size}px") style:height=format!("{size}px")></span> }.into_any(),
    }
}

pub fn score_tier(micros: u32) -> &'static str {
    match micros {
        900_000.. => "high",
        500_000..900_000 => "mid",
        _ => "low",
    }
}

/// A Neynar user score, read onchain. Zero means Neynar has no score.
#[component]
pub fn ScoreBadge(micros: u32) -> impl IntoView {
    if micros == 0 {
        return ().into_any();
    }
    let value = score(micros);
    let title = format!(
        "Neynar user score {value} (0–1, higher means more likely a quality account). Read onchain from Neynar's score contract on Base; updated weekly."
    );
    let label = format!("Neynar score {value}");
    view! {
        <span class=format!("score score-{}", score_tier(micros)) title=title aria-label=label>
            {value}
        </span>
    }
    .into_any()
}

/// Profile picture, name and handle, linking to the profile.
#[component]
pub fn UserChip(
    user: UserSummary,
    #[prop(optional)] score: Option<u32>,
    #[prop(optional)] show_name: bool,
) -> impl IntoView {
    let href = user_href(&user.username);
    let display = (show_name && !user.display_name.is_empty() && user.display_name != user.username)
        .then(|| user.display_name.clone());
    view! {
        <span class="user-chip">
            <a class="user-link" href=href.clone() tabindex="-1" aria-hidden="true">
                <Avatar url=user.pfp_url.clone() size=22/>
            </a>
            {display.map(|name| view! { <a class="display-name" href=href.clone()>{name}</a> })}
            <a class="author" href=href>"@"{user.username.clone()}</a>
            {score.map(|micros| view! { <ScoreBadge micros/> })}
        </span>
    }
}

/// Likes, recasts and replies as what they are: Farcaster has no downvotes.
#[component]
pub fn Engagement(cast: Cast, #[prop(optional)] reply_href: Option<String>) -> impl IntoView {
    let pts = points(&cast);
    let title = format!(
        "{} likes, {} recasts, {} replies. Ranked by engagement points: likes + 2 × recasts + replies = {}.",
        full_number(cast.likes),
        full_number(cast.recasts),
        full_number(cast.replies),
        full_number(pts)
    );
    let replies = view! {
        <span class="stat stat-replies">
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z"/></svg>
            <span class="n">{compact_number(cast.replies)}</span>
            <span class="label">{if cast.replies == 1 { " reply" } else { " replies" }}</span>
        </span>
    };
    view! {
        <div class="engagement" title=title>
            <span class="stat stat-likes">
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M8 13.5S2.5 10 2.5 6.2A2.8 2.8 0 0 1 8 5a2.8 2.8 0 0 1 5.5 1.2C13.5 10 8 13.5 8 13.5z"/></svg>
                <span class="n">{compact_number(cast.likes)}</span>
                <span class="label">{if cast.likes == 1 { " like" } else { " likes" }}</span>
            </span>
            <span class="stat stat-recasts">
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M4 5.5h7l-1.8-1.8M12 10.5H5l1.8 1.8M4 5.5v3M12 10.5v-3"/></svg>
                <span class="n">{compact_number(cast.recasts)}</span>
                <span class="label">{if cast.recasts == 1 { " recast" } else { " recasts" }}</span>
            </span>
            {match reply_href {
                Some(href) => view! { <a class="replies-link" href=href>{replies}</a> }.into_any(),
                None => replies.into_any(),
            }}
        </div>
    }
}

/// "via Warpcast", from the app that requested the cast's signer key.
#[component]
pub fn OriginChip(hash: String, notes: Notes) -> impl IntoView {
    let origin = notes.origins.get(&hash).cloned();
    let app = notes.app_for(&hash).cloned();
    match (origin, app) {
        (Some(origin), Some(app)) => {
            let name = if app.display_name.is_empty() { app.username.clone() } else { app.display_name.clone() };
            let title = format!(
                "Signed with key {} requested by @{} (fid {}). Read through Hypersnap.",
                short_address(&origin.signer),
                app.username,
                app.fid
            );
            view! { <span class="origin" title=title>"via "<a href=user_href(&app.username)>{name}</a></span> }
                .into_any()
        }
        (Some(origin), None) => {
            let title = match origin.app_fid {
                Some(fid) => format!("Signed with key {}, requested by fid {fid}.", short_address(&origin.signer)),
                None => {
                    format!("Signed with key {}, registered without an app attribution.", short_address(&origin.signer))
                }
            };
            let label =
                origin.app_fid.map_or_else(|| "via unattributed key".to_owned(), |fid| format!("via app fid {fid}"));
            view! { <span class="origin origin-unknown" title=title>{label}</span> }.into_any()
        }
        _ => ().into_any(),
    }
}

/// Which backend Allthing read the data from.
#[component]
pub fn BackendBadge(#[prop(optional)] backend: Backend) -> impl IntoView {
    let title = format!(
        "Read from {} (haatz.quilibrium.com), a read node that mirrors Snapchain mainnet. Farcaster does not record which node a message was first submitted to; see the provenance design doc.",
        backend.label()
    );
    view! {
        <a class="backend" href=PROVENANCE_DOC_URL title=title target="_blank" rel="noopener">
            <span class="dot" aria-hidden="true"></span>
            {backend.label()}
        </a>
    }
}

/// A stat with a number: "1,234 followers".
#[component]
pub fn Count(n: u64, #[prop(into)] label: String) -> impl IntoView {
    view! { <span class="count"><strong class="number">{full_number(n)}</strong>" "{label}</span> }
}
