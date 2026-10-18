// SPDX-License-Identifier: AGPL-3.0-or-later
//! Small inline pieces: times, avatars, user chips, engagement and provenance.

use crate::Notes;
use allthing_core::clients::{client_name, client_profile};
use allthing_core::embeds::{ImageSize, resized_image_url};
use allthing_core::format::{compact_number, full_number, short_address};
use allthing_core::model::{Cast, UserSummary};
use allthing_core::ranking::points;
use allthing_core::time::{format_date_time, iso_utc, time_ago};
use leptos::prelude::*;

use crate::components::popover::Popover;
use crate::components::who::{RepliesPreview, WhoPreview, use_who_list};
use crate::util::{cast_href, now, user_href};
use allthing_core::api::ReactionKind;
use leptos_router::hooks::use_navigate;

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

/// Profile picture, name and handle, linking to the profile.
#[component]
pub fn UserChip(user: UserSummary, #[prop(optional)] show_name: bool) -> impl IntoView {
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
        </span>
    }
}

fn stat_body(icon: &'static str, n: u64, one: &'static str, many: &'static str) -> AnyView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=icon/></svg>
        <span class="n">{compact_number(n)}</span>
        <span class="label">{if n == 1 { one } else { many }}</span>
    }
    .into_any()
}

const HEART: &str = "M8 13.5S2.5 10 2.5 6.2A2.8 2.8 0 0 1 8 5a2.8 2.8 0 0 1 5.5 1.2C13.5 10 8 13.5 8 13.5z";
const RECAST: &str = "M4 5.5h7l-1.8-1.8M12 10.5H5l1.8 1.8M4 5.5v3M12 10.5v-3";
const BUBBLE: &str = "M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z";

/// Likes, recasts and replies as what they are: Farcaster has no downvotes.
/// Hovering a count previews who liked or recasted, or the replies; clicking
/// opens the full list, or the post.
#[component]
pub fn Engagement(cast: Cast, #[prop(optional)] reply_href: Option<String>) -> impl IntoView {
    let who = use_who_list();
    let navigate = use_navigate();
    let pts = points(&cast);
    let title = format!(
        "{} likes, {} recasts, {} replies. Ranked by engagement points: likes + 2 × recasts + replies = {}.",
        full_number(cast.likes),
        full_number(cast.recasts),
        full_number(cast.replies),
        full_number(pts)
    );
    let cast_hash = cast.hash.clone();
    let reaction = |kind: ReactionKind, n: u64, class: &'static str| {
        let (icon, one, many) = match kind {
            ReactionKind::Likes => (HEART, " like", " likes"),
            ReactionKind::Recasts => (RECAST, " recast", " recasts"),
        };
        if n == 0 {
            return view! { <span class=format!("stat {class}")>{stat_body(icon, n, one, many)}</span> }.into_any();
        }
        let hash = cast_hash.clone();
        let preview_hash = hash.clone();
        view! {
            <Popover
                label=match kind { ReactionKind::Likes => "Liked by", ReactionKind::Recasts => "Recasted by" }
                trigger_label=format!("{}{}: show who", compact_number(n), many)
                trigger_class=format!("stat {class}")
                hover=true
                on_activate=Callback::new(move |()| who.open(hash.clone(), kind))
                trigger=move || stat_body(icon, n, one, many)
                content=move || view! { <WhoPreview hash=preview_hash.clone() kind/> }
            />
        }
        .into_any()
    };
    let likes = reaction(ReactionKind::Likes, cast.likes, "stat-likes");
    let recasts = reaction(ReactionKind::Recasts, cast.recasts, "stat-recasts");
    let replies = if cast.replies == 0 {
        view! { <span class="stat stat-replies">{stat_body(BUBBLE, 0, " reply", " replies")}</span> }.into_any()
    } else {
        let n = cast.replies;
        let target = reply_href.unwrap_or_else(|| cast_href(&cast.hash));
        let hash = cast.hash.clone();
        view! {
            <Popover
                label="Replies"
                trigger_label=format!("{} {}: preview", compact_number(n), if n == 1 { "reply" } else { "replies" })
                trigger_class="stat stat-replies"
                hover=true
                on_activate=Callback::new(move |()| navigate(&target, Default::default()))
                trigger=move || stat_body(BUBBLE, n, " reply", " replies")
                content=move || view! { <RepliesPreview hash=hash.clone()/> }
            />
        }
        .into_any()
    };
    // The ranking note is for screen readers: a native tooltip on the row
    // would sit on top of the counts' own popovers.
    view! { <div class="engagement">{likes}{recasts}{replies}<span class="visually-hidden">{title}</span></div> }
}

/// "via Farcaster", from the app that requested the cast's signer key.
#[component]
pub fn OriginChip(hash: String, notes: Notes) -> impl IntoView {
    let origin = notes.origins.get(&hash).cloned();
    let app = notes.app_for(&hash).cloned();
    match (origin, app) {
        (Some(origin), Some(app)) => {
            let name = client_name(&app).to_owned();
            let title = format!(
                "Signed with key {} requested by {} (app fid {}).",
                short_address(&origin.signer),
                name,
                app.fid
            );
            view! { <span class="origin" title=title>"via "<a href=user_href(&client_profile(&app))>{name}</a></span> }
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

/// A stat with a number: "1,234 followers".
#[component]
pub fn Count(n: u64, #[prop(into)] label: String) -> impl IntoView {
    view! { <span class="count"><strong class="number">{full_number(n)}</strong>" "{label}</span> }
}
