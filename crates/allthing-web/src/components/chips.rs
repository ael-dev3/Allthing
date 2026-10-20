// SPDX-License-Identifier: AGPL-3.0-or-later
//! Small inline pieces: times, avatars, user chips, engagement and provenance.

use crate::Notes;
use allthing_core::clients::{client_name, client_profile};
use allthing_core::embeds::{ImageSize, resized_image_url};
use allthing_core::format::{compact_number, full_number, short_address};
use allthing_core::model::{Cast, UserSummary};
use allthing_core::profile::UserCard;
use allthing_core::ranking::points;
use allthing_core::time::{format_date_time, iso_utc, time_ago};
use leptos::prelude::*;

use crate::components::popover::Popover;
use crate::components::profile_card::ProfileHover;
use crate::components::who::{RepliesPreview, WhoList, WhoPreview, use_who_list};
use crate::util::{cast_href, now, user_href};
use allthing_core::api::ReactionKind;
use leptos_router::hooks::use_navigate;

#[component]
pub fn TimeAgo(ts: i64) -> impl IntoView {
    view! { <time datetime=iso_utc(ts) aria-description=format_date_time(ts)>{time_ago(ts, now())}</time> }
}

#[component]
pub fn Avatar(url: Option<String>, #[prop(default = 24)] size: u32, #[prop(optional)] large: bool) -> impl IntoView {
    match url {
        // A large picture (a profile card's, a post's or a profile's) shows
        // the small one under it until it has loaded: the cards around it
        // have most often loaded the small one already. It is in view as it
        // appears, so it isn't left to lazy loading.
        Some(url) if large => view! {
            <img
                class="avatar avatar-large"
                src=resized_image_url(&url, ImageSize::Thumb)
                style=format!("--small: url(\"{}\")", css_url(&resized_image_url(&url, ImageSize::Avatar)))
                alt=""
                width=size
                height=size
                decoding="async"
                referrerpolicy="no-referrer"
            />
        }
        .into_any(),
        Some(url) => view! {
            <img
                class="avatar"
                src=resized_image_url(&url, ImageSize::Avatar)
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

/// `url` as it can sit in a CSS `url("…")`.
fn css_url(url: &str) -> String {
    url.replace('\\', "%5C").replace('"', "%22").replace('\n', "%0A")
}

/// Profile picture, name and handle, linking to the profile. Hovering them,
/// resting keyboard focus on them or tapping them shows the account's
/// profile card.
#[component]
pub fn UserChip(
    user: UserSummary,
    #[prop(optional)] show_name: bool,
    /// What the page's payload says about the account.
    #[prop(optional_no_strip)]
    card: Option<UserCard>,
) -> impl IntoView {
    let href = user_href(&user.username);
    let display = (show_name && !user.display_name.is_empty() && user.display_name != user.username)
        .then(|| user.display_name.clone());
    let (pfp, username) = (user.pfp_url.clone(), user.username.clone());
    view! {
        <ProfileHover user card class="user-chip">
            <a class="user-link" href=href.clone() tabindex="-1" aria-hidden="true">
                <Avatar url=pfp.clone() size=22/>
            </a>
            {display.clone().map(|name| view! { <a class="display-name" href=href.clone()>{name}</a> })}
            <a class="author" href=href.clone()>"@"{username.clone()}</a>
        </ProfileHover>
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
    let likes = reaction_stat(&cast, ReactionKind::Likes, cast.likes, who);
    let recasts = reaction_stat(&cast, ReactionKind::Recasts, cast.recasts, who);
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

/// A like or recast: its icon, then its count. Pointing at either previews
/// who liked or recasted, with "see all" for the full list, which a click
/// on the count opens too (on a touch screen a tap on the count shows the
/// preview). The icon never opens the list: in front of the Worker it is the
/// button that likes or recasts (`acting_stat`); in the static copy, a
/// picture.
fn reaction_stat(cast: &Cast, kind: ReactionKind, n: u64, who: WhoList) -> AnyView {
    #[cfg(not(feature = "static-host"))]
    if let Some(writer) = crate::components::writer::use_writer()
        && let Some(target) = allthing_core::message::CastId::new(cast.author.fid, &cast.hash)
    {
        let url = allthing_core::compose::cast_url(&cast.author.username, &cast.hash);
        return acting_stat(writer, target, cast.hash.clone(), url, kind, n, who);
    }
    let (icon, _, _, class) = reaction_words(kind);
    let lead =
        ViewFn::from(move || view! { <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=icon/></svg> });
    view! { <span class=format!("stat {class}")>{count_stat(kind, Signal::stored(n), cast.hash.clone(), who, lead)}</span> }
        .into_any()
}

/// The icon and the words for each kind, and its class.
fn reaction_words(kind: ReactionKind) -> (&'static str, &'static str, &'static str, &'static str) {
    match kind {
        ReactionKind::Likes => (HEART, " like", " likes", "stat-likes"),
        ReactionKind::Recasts => (RECAST, " recast", " recasts", "stat-recasts"),
    }
}

/// The count, after `lead` (its icon, or the button that likes or
/// recasts): the preview's trigger, or plain text while it is 0.
fn count_stat(kind: ReactionKind, n: Signal<u64>, hash: String, who: WhoList, lead: ViewFn) -> AnyView {
    let (_, one, many, _) = reaction_words(kind);
    let words = move || if n.get() == 1 { one } else { many };
    let body = move || {
        view! {
            <span class="n">{move || compact_number(n.get())}</span>
            <span class="label">{words}</span>
        }
    };
    let preview_hash = hash.clone();
    view! {
        <Popover
            label=match kind { ReactionKind::Likes => "Liked by", ReactionKind::Recasts => "Recasted by" }
            trigger_label=Signal::derive(move || format!("{}{}: show who", compact_number(n.get()), words()))
            trigger_class="stat-count"
            hover=true
            lead
            idle=Signal::derive(move || n.get() == 0)
            on_activate=Callback::new(move |()| who.open(hash.clone(), kind))
            trigger=body
            content=move || view! { <WhoPreview hash=preview_hash.clone() kind/> }
        />
    }
    .into_any()
}

/// A like or recast button before the count, pressed when the reader has,
/// and the count, which follows what they do. For a reader without a key
/// yet, the button approves one first (`Writer::toggle`); signed out, or
/// where the Worker endorses no keys, it opens the cast in the Farcaster app
/// (`url`), to like or recast there.
#[cfg(not(feature = "static-host"))]
fn acting_stat(
    writer: crate::components::writer::Writer,
    target: allthing_core::message::CastId,
    hash: String,
    url: String,
    kind: ReactionKind,
    n: u64,
    who: WhoList,
) -> AnyView {
    use crate::components::handoff::Ask;
    use allthing_core::message::Reaction;
    let (icon, _, _, class) = reaction_words(kind);
    let (reaction, action) = match kind {
        ReactionKind::Likes => (Reaction::Like, "Like"),
        ReactionKind::Recasts => (Reaction::Recast, "Recast"),
    };
    let ask = move |on: bool| match kind {
        ReactionKind::Likes => Ask::like(url.clone(), on),
        ReactionKind::Recasts => Ask::recast(url.clone(), on),
    };
    let key = hash.clone();
    let mine = Memo::new(move |_| writer.reacted(&key, reaction));
    let key = hash.clone();
    let busy = Memo::new(move |_| writer.busy(&key, reaction));
    let key = hash.clone();
    let shown = Memo::new(move |_| (n as i64 + writer.delta(&key, reaction)).max(0) as u64);
    let hands_off = Memo::new(move |_| writer.hands_off());
    let shows_mine = Memo::new(move |_| writer.shows_mine());
    let lead = ViewFn::from(move || {
        let ask = ask.clone();
        view! {
            <button
                type="button"
                class="act"
                aria-label=action
                // A toggle where Allthing writes, and signed in where it
                // hands off (what the reader did in the app shows); signed
                // out, only a way to the app.
                aria-pressed=move || (!hands_off.get() || shows_mine.get()).then(|| mine.get().to_string())
                aria-haspopup=move || hands_off.get().then_some("dialog")
                aria-busy=move || busy.get().then_some("true")
                on:click=move |_| writer.toggle(target, reaction, ask(mine.get_untracked()))
            >
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=icon/></svg>
            </button>
        }
    });
    view! {
        <span class=move || format!("stat {class} acting{}", if mine.get() { " mine" } else { "" })>
            {count_stat(kind, shown.into(), hash, who, lead)}
        </span>
    }
    .into_any()
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
            view! { <span class="origin" aria-description=title>"via "<a href=user_href(&client_profile(&app))>{name}</a></span> }
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
            view! { <span class="origin origin-unknown" aria-description=title>{label}</span> }.into_any()
        }
        _ => ().into_any(),
    }
}

/// A stat with a number: "1,234 followers".
#[component]
pub fn Count(n: u64, #[prop(into)] label: String) -> impl IntoView {
    view! { <span class="count"><strong class="number">{full_number(n)}</strong>" "{label}</span> }
}
