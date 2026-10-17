// SPDX-License-Identifier: AGPL-3.0-or-later
//! Casts in listings.

use allthing_core::model::{Cast, Embed};
use allthing_core::text::cast_title;
use leptos::prelude::*;

use crate::Notes;
use crate::components::chips::{BackendBadge, Engagement, OriginChip, TimeAgo, UserChip};
use crate::components::media::{LinkCards, MediaGrid, VideoLinks};
use crate::util::{cast_href, channel_href};

/// The channel line of a cast's meta row.
#[component]
pub fn ChannelLink(cast: Cast) -> impl IntoView {
    cast.channel.map(|channel| {
        view! {
            <span class="in-channel">
                "in "<a class="channel-name" href=channel_href(&channel.id)>"/"{channel.label}</a>
            </span>
        }
    })
}

/// One cast in a listing: who and where, the text as a title, media, and its
/// engagement.
#[component]
pub fn CastCard(
    cast: Cast,
    notes: Notes,
    #[prop(optional)] index: usize,
    #[prop(optional)] show_parent: bool,
) -> impl IntoView {
    let href = cast_href(&cast.hash);
    let title = cast_title(&cast.text, &cast.embeds);
    let score = notes.scores.get(&cast.author.fid).copied();
    let quote = cast.embeds.iter().find_map(|e| match e {
        Embed::Cast { hash, .. } => Some(hash.clone()),
        _ => None,
    });
    let parent = show_parent.then(|| cast.parent_hash.clone()).flatten();
    view! {
        <article
            class="cast-card"
            data-nav=""
            data-hash=cast.hash.clone()
            data-ts=cast.ts
            data-points=allthing_core::ranking::points(&cast)
            style=format!("--i: {}", index.min(16))
        >
            <header class="cast-meta">
                <UserChip user=cast.author.clone() score=score.unwrap_or(0) show_name=true/>
                <span class="sep" aria-hidden="true">"·"</span>
                <a class="when" href=href.clone()><TimeAgo ts=cast.ts/></a>
                <ChannelLink cast=cast.clone()/>
                <OriginChip hash=cast.hash.clone() notes=notes.clone()/>
            </header>
            {parent.map(|p| view! { <p class="replying-to">"replying to "<a href=cast_href(&p)>"a cast"</a></p> })}
            <h2 class="cast-title"><a href=href.clone() data-primary="">{title}</a></h2>
            <MediaGrid cast=cast.clone()/>
            <VideoLinks cast=cast.clone()/>
            <LinkCards cast=cast.clone()/>
            {quote.map(|q| view! { <a class="quote-chip" href=cast_href(&q)>"↳ quotes a cast"</a> })}
            <footer class="cast-foot">
                <Engagement cast=cast.clone() reply_href=href/>
                <BackendBadge/>
            </footer>
        </article>
    }
}

#[component]
pub fn CastList(casts: Vec<Cast>, notes: Notes, #[prop(optional)] show_parent: bool) -> impl IntoView {
    if casts.is_empty() {
        return view! { <p class="empty">"Nothing here yet."</p> }.into_any();
    }
    view! {
        <div class="cast-list">
            {casts
                .into_iter()
                .enumerate()
                .map(|(index, cast)| view! { <CastCard cast notes=notes.clone() index show_parent/> })
                .collect_view()}
        </div>
    }
    .into_any()
}

/// Placeholder cards while a listing loads.
#[component]
pub fn CastSkeletons(#[prop(default = 6)] count: usize) -> impl IntoView {
    view! {
        <div class="cast-list" aria-busy="true" aria-label="Loading casts">
            {(0..count)
                .map(|i| view! {
                    <div class="cast-card skeleton" style=format!("--i: {i}")>
                        <span class="sk sk-meta"></span>
                        <span class="sk sk-title"></span>
                        <span class="sk sk-title short"></span>
                        <span class="sk sk-foot"></span>
                    </div>
                })
                .collect_view()}
        </div>
    }
}
