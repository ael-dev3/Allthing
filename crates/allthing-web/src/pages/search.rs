// SPDX-License-Identifier: AGPL-3.0-or-later
//! Search results, grouped: people, channels, then casts.

use std::sync::Arc;

use allthing_core::api::{SearchPayload, search_path};
use allthing_core::embeds::{ImageSize, resized_image_url};
use allthing_core::format::{compact_number, plural};
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::Notes;
use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons};
use crate::components::chips::{Avatar, ScoreBadge};
use crate::components::layout::ErrorPanel;
use crate::util::{channel_href, set_title, user_href};

#[component]
pub fn SearchPage() -> impl IntoView {
    let query = use_query_map();
    let q = Memo::new(move |_| query.read().get("q").map(|q| q.trim().to_owned()).filter(|q| !q.is_empty()));
    let data = LocalResource::new(move || {
        let q = q.get();
        async move {
            match q {
                Some(q) => Some(api::get::<SearchPayload>(&search_path(&q)).await),
                None => None,
            }
        }
    });
    Effect::new(move |_| set_title(&q.get().map(|q| format!("“{q}”")).unwrap_or_else(|| "Search".into())));

    move || {
        let Some(term) = q.get() else {
            return view! {
                <section class="search-page">
                    <h1 class="page-title">"Search"</h1>
                    <p class="muted">"Search casts, people and channels from the box at the top."</p>
                </section>
            }
            .into_any();
        };
        match data.get() {
            None | Some(None) => view! {
                <section class="search-page" aria-busy="true">
                    <h1 class="page-title">"Results for “"{term}"”"</h1>
                    <CastSkeletons count=3/>
                </section>
            }
            .into_any(),
            Some(Some(Err(failure))) => view! { <ErrorPanel failure/> }.into_any(),
            Some(Some(Ok(Fetched { data, .. }))) => {
                let payload: Arc<SearchPayload> = data;
                let notes: Notes = Arc::new(payload.notes.clone());
                let nothing = payload.users.is_empty() && payload.channels.is_empty() && payload.casts.is_empty();
                view! {
                    <section class="search-page">
                        <h1 class="page-title">"Results for “"{term}"”"</h1>
                        {nothing.then(|| view! { <p class="empty">"Nothing found. Try another word or a username."</p> })}
                        {(!payload.users.is_empty()).then(|| view! {
                            <section class="results-group" aria-label="People">
                                <h2>"People"</h2>
                                <ul class="people">
                                    {payload.users.iter().map(|u| {
                                        let score = notes.scores.get(&u.fid).copied().unwrap_or(0);
                                        view! {
                                            <li class="person">
                                                <a href=user_href(&u.username)>
                                                    <Avatar url=u.pfp_url.clone() size=40 large=true/>
                                                    <span class="person-text">
                                                        <strong>{u.name().to_owned()}</strong>
                                                        <span class="muted">"@"{u.username.clone()}</span>
                                                    </span>
                                                </a>
                                                <ScoreBadge micros=score/>
                                                <span class="muted small">{format!("{} followers", compact_number(u.follower_count))}</span>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            </section>
                        })}
                        {(!payload.channels.is_empty()).then(|| view! {
                            <section class="results-group" aria-label="Channels">
                                <h2>"Channels"</h2>
                                <ul class="channel-results">
                                    {payload.channels.iter().map(|c| view! {
                                        <li>
                                            <a href=channel_href(&c.id)>
                                                {c.image_url.clone().map(|src| view! { <img src=resized_image_url(&src, ImageSize::Avatar) alt="" width="28" height="28" loading="lazy" referrerpolicy="no-referrer"/> })}
                                                <strong>"/"{c.id.clone()}</strong>
                                            </a>
                                            {c.member_count.map(|m| view! { <span class="muted small">{plural(m, "member", "members")}</span> })}
                                        </li>
                                    }).collect_view()}
                                </ul>
                            </section>
                        })}
                        {(!payload.casts.is_empty()).then(|| view! {
                            <section class="results-group" aria-label="Casts">
                                <h2>"Casts"</h2>
                                <CastList casts=payload.casts.clone() notes=notes.clone()/>
                            </section>
                        })}
                    </section>
                }
                .into_any()
            }
        }
    }
}
