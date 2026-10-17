// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sidebar boxes shared by several pages.

use allthing_core::format::full_number;
use allthing_core::model::Channel;
use leptos::prelude::*;

use crate::components::layout::SideBox;
use crate::util::{PROVENANCE_DOC_URL, SOURCE_URL, channel_href};

#[component]
pub fn AboutBox() -> impl IntoView {
    view! {
        <SideBox title="The Allthing">
            <p>"A quiet, read-only window onto Farcaster's open conversation. No sign-in, no posting, nobody hidden."</p>
            <p><a href="/about">"How it works →"</a></p>
        </SideBox>
    }
}

#[component]
pub fn ChannelsBox(channels: Vec<Channel>) -> impl IntoView {
    let fallback = allthing_core::channels::DEFAULT_CHANNELS.iter().map(|id| ((*id).to_owned(), None));
    let list: Vec<(String, Option<u64>)> = if channels.is_empty() {
        fallback.collect()
    } else {
        channels.into_iter().map(|c| (c.id, c.member_count)).collect()
    };
    view! {
        <SideBox title="Channels">
            <ul class="channel-list">
                {list
                    .into_iter()
                    .map(|(id, members)| view! {
                        <li>
                            <a href=channel_href(&id)>"/"{id.clone()}</a>
                            {members.map(|m| view! { <span class="muted">{full_number(m)}</span> })}
                        </li>
                    })
                    .collect_view()}
            </ul>
            <p><a href="/channels">"All channels →"</a></p>
        </SideBox>
    }
}

#[component]
pub fn RankingBox() -> impl IntoView {
    view! {
        <SideBox title="How ranking works">
            <p>
                "Farcaster has likes, recasts and replies, and no downvotes. A cast's engagement is "
                <strong>"likes + 2 × recasts + replies"</strong>"."
            </p>
            <p>
                <strong>"hot"</strong>" weighs engagement against age (ten times the engagement buys 12.5 hours), "
                <strong>"top"</strong>" is engagement alone, "<strong>"new"</strong>" is time alone."
            </p>
        </SideBox>
    }
}

#[component]
pub fn SourcesBox() -> impl IntoView {
    view! {
        <SideBox title="Where data comes from">
            <ul class="plain">
                <li><strong>"Casts, profiles, channels"</strong>": Hypersnap, a read node that mirrors Snapchain mainnet."</li>
                <li><strong>"Posted via"</strong>": the app that registered the signing key."</li>
                <li><strong>"Scores"</strong>": Neynar user scores, read onchain on Base."</li>
                <li><strong>"Labels"</strong>": the Farcaster app's public spam dataset."</li>
            </ul>
            <p><a href=PROVENANCE_DOC_URL>"Provenance design →"</a>" · "<a href=SOURCE_URL>"source"</a></p>
        </SideBox>
    }
}

#[component]
pub fn KeyboardBox() -> impl IntoView {
    view! {
        <SideBox title="Keyboard">
            <dl class="keys">
                <dt><kbd>"j"</kbd>" "<kbd>"k"</kbd></dt><dd>"next / previous"</dd>
                <dt><kbd>"o"</kbd></dt><dd>"open"</dd>
                <dt><kbd>"x"</kbd></dt><dd>"collapse a reply"</dd>
                <dt><kbd>"←"</kbd>" "<kbd>"→"</kbd></dt><dd>"browse media"</dd>
            </dl>
        </SideBox>
    }
}
