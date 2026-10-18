// SPDX-License-Identifier: AGPL-3.0-or-later
//! A channel (or any parent-URL feed, like a token's).

use std::sync::Arc;

use allthing_core::api::{ChannelPayload, channel_path};
use allthing_core::embeds::{ImageSize, resized_image_url};
use allthing_core::format::full_number;
use allthing_core::listing::top_level;
use allthing_core::params::channel_key_from_param;
use allthing_core::ranking::{Sort, sorted};
use leptos::prelude::*;
use leptos_router::hooks::{use_params_map, use_query_map};

use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons};
use crate::components::layout::{Columns, ErrorPanel, NotFound, SideBox, SortTabs, StaleNotice};
use crate::components::text::CastText;
use crate::util::{channel_href, set_title};

#[component]
pub fn ChannelPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    // /c/<id>, or /c?id=<parent URL> (also the old static site's form).
    let key = Memo::new(move |_| {
        params.read().get("key").or_else(|| query.read().get("id")).and_then(|raw| channel_key_from_param(&raw))
    });
    let sort = Memo::new(move |_| Sort::parse(query.read().get("sort").as_deref(), Sort::Hot));
    let data = LocalResource::new(move || {
        let key = key.get();
        async move {
            match key {
                Some(key) => Some(api::get::<ChannelPayload>(&channel_path(&key)).await),
                None => None,
            }
        }
    });
    Effect::new(move |_| {
        if let Some(Some(Ok(fetched))) = data.get() {
            set_title(&format!("/{}", fetched.data.channel.name));
        }
    });

    move || {
        let Some(key) = key.get() else {
            return view! { <NotFound/> }.into_any();
        };
        let path = channel_href(&key);
        match data.get() {
            None | Some(None) => view! {
                <Columns side=().into_any()>
                    <div class="listing-head"><h1 class="page-title">"/"{allthing_core::channels::channel_label(&key)}</h1></div>
                    <CastSkeletons/>
                </Columns>
            }
            .into_any(),
            Some(Some(Err(failure))) => view! { <ErrorPanel failure/> }.into_any(),
            Some(Some(Ok(Fetched { data, stale, .. }))) => {
                let payload: Arc<ChannelPayload> = data;
                let notes = Arc::new(payload.notes.clone());
                let channel = payload.channel.clone();
                let pool = top_level(&payload.pool);
                let casts = Signal::derive(move || sorted(&pool, sort.get()));
                let side = view! {
                    <SideBox title="About this channel">
                        {channel.description.clone().map(|d| view! { <div class="bio"><CastText text=d mentions=Vec::new()/></div> })}
                        {channel.member_count.map(|m| view! { <p><strong>{full_number(m)}</strong>{if m == 1 { " member" } else { " members" }}</p> })}
                        <p class="muted small">"Feed: "<code>{channel.parent_url.clone()}</code></p>
                    </SideBox>
                }
                .into_any();
                view! {
                    <Columns side>
                        <header class="channel-head">
                            {channel.image_url.clone().map(|src| view! {
                                <img class="channel-image" src=resized_image_url(&src, ImageSize::Thumb) alt="" width="56" height="56" referrerpolicy="no-referrer"/>
                            })}
                            <div>
                                <h1 class="page-title">"/"{channel.name.clone()}</h1>
                                {(channel.name != channel.id).then(|| view! { <p class="muted">{channel.id.clone()}</p> })}
                            </div>
                        </header>
                        <div class="listing-head">{move || view! { <SortTabs path=path.clone() current=sort.get()/> }}</div>
                        <StaleNotice stale/>
                        <CastList casts notes/>
                    </Columns>
                }
                .into_any()
            }
        }
    }
}
