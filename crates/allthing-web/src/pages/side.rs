// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sidebar boxes shared by several pages.

use allthing_core::format::full_number;
use allthing_core::model::Channel;
use leptos::prelude::*;

use crate::components::layout::SideBox;
use crate::util::{channel_href, href};

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
            <p><a href=href("/channels")>"All channels →"</a></p>
        </SideBox>
    }
}
