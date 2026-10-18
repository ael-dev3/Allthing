// SPDX-License-Identifier: AGPL-3.0-or-later
//! Channel directory and search.

use allthing_core::api::{ChannelsPayload, channels_path};
use allthing_core::embeds::{ImageSize, resized_image_url};
use allthing_core::format::full_number;
use leptos::prelude::*;

use crate::api;
use crate::components::layout::ErrorPanel;
use crate::util::{channel_href, href, set_title, use_query_param};

#[component]
pub fn ChannelsPage() -> impl IntoView {
    let query = use_query_param("q");
    let q = Memo::new(move |_| query.get().map(|q| q.trim().to_owned()).filter(|q| !q.is_empty()));
    let data = LocalResource::new(move || {
        let q = q.get();
        async move { api::get::<ChannelsPayload>(&channels_path(q.as_deref())).await }
    });
    Effect::new(move |_| {
        set_title(&q.get().map_or_else(|| "Channels".to_owned(), |q| format!("Channels matching {q}")))
    });
    view! {
        <section class="directory">
            <div class="listing-head">
                <h1 class="page-title">"Channels"</h1>
                <form class="search" method="get" action=href("/channels") role="search">
                    <label class="sr-only" for="channel-search">"Search channels"</label>
                    <input id="channel-search" type="search" name="q" placeholder="search channels" maxlength="64" prop:value=move || q.get().unwrap_or_default()/>
                    <button type="submit" class="button">"Search"</button>
                </form>
            </div>
            {move || match data.get() {
                None => view! { <p class="muted">"Loading channels…"</p> }.into_any(),
                Some(Err(failure)) => view! { <ErrorPanel failure/> }.into_any(),
                Some(Ok(fetched)) if fetched.data.channels.is_empty() => view! { <p class="empty">"No channels found."</p> }.into_any(),
                Some(Ok(fetched)) => view! {
                    <ul class="channel-grid">
                        {fetched.data.channels.iter().enumerate().map(|(i, c)| view! {
                            <li class="channel-card" style=format!("--i: {}", i.min(16))>
                                <a href=channel_href(&c.id)>
                                    {c.image_url.clone().map(|src| view! { <img src=resized_image_url(&src, ImageSize::Avatar) alt="" width="40" height="40" loading="lazy" referrerpolicy="no-referrer"/> })}
                                    <span class="channel-text">
                                        <strong>"/"{c.id.clone()}</strong>
                                        {c.description.clone().map(|d| view! { <span class="muted">{allthing_core::text::truncate_title(&d, 90)}</span> })}
                                    </span>
                                    {c.member_count.map(|m| view! { <span class="members">{full_number(m)}</span> })}
                                </a>
                            </li>
                        }).collect_view()}
                    </ul>
                }
                .into_any(),
            }}
        </section>
    }
}
