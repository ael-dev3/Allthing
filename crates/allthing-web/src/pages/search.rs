// SPDX-License-Identifier: AGPL-3.0-or-later
//! Search results, grouped: people, channels, then casts.

use std::sync::Arc;

use allthing_core::api::{SearchPayload, search_path};
use allthing_core::embeds::{ImageSize, resized_image_url};
use allthing_core::format::compact_number;
use allthing_core::profile::UserCard;
use leptos::prelude::*;
use leptos::wasm_split::{SplitLoaderError, wasm_split};

use crate::Notes;
use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons};
use crate::components::chips::Avatar;
use crate::components::layout::ErrorPanel;
use crate::components::profile_card::ProfileHover;
use crate::util::{channel_href, set_title, use_query_param, user_href};

/// People and channels shown before "Show all", so casts stay near the top.
const PEOPLE_SHOWN: usize = 6;
const CHANNELS_SHOWN: usize = 8;

/// The page, whose code is fetched the first time it's needed
/// (`crate::split`): as it opens, or ahead of it.
#[wasm_split(search, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_search))]
pub fn search_page() -> Result<AnyView, SplitLoaderError> {
    Ok(view! { <SearchPage/> }.into_any())
}

#[component]
fn SearchPage() -> impl IntoView {
    let query = use_query_param("q");
    let q = Memo::new(move |_| query.get().map(|q| q.trim().to_owned()).filter(|q| !q.is_empty()));
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
                let all_people = RwSignal::new(false);
                let all_channels = RwSignal::new(false);
                let people_total = payload.users.len();
                let channels_total = payload.channels.len();
                let users = payload.users.clone();
                let channels = payload.channels.clone();
                view! {
                    <section class="search-page">
                        <h1 class="page-title">"Results for “"{term}"”"</h1>
                        {nothing.then(|| view! { <p class="empty">"Nothing found. Try another word or a username."</p> })}
                        {(!payload.users.is_empty()).then(|| view! {
                            <section class="results-group" aria-label="People">
                                <h2>"People" <span class="count">{people_total}</span></h2>
                                <ul class="people">
                                    {move || users.iter().take(if all_people.get() { usize::MAX } else { PEOPLE_SHOWN }).map(|u| {
                                        // A person found is a whole profile: its card needs nothing more.
                                        let card = UserCard::of(u);
                                        let href = user_href(&u.username);
                                        let (pfp, name, username) = (u.pfp_url.clone(), u.name().to_owned(), u.username.clone());
                                        view! {
                                            <li class="person">
                                                <ProfileHover user=u.summary() card=Some(card) class="person-links">
                                                    <a class="person-link" href=href.clone()>
                                                        <Avatar url=pfp.clone() size=40 large=true/>
                                                        <span class="person-text">
                                                            <strong>{name.clone()}</strong>
                                                            <span class="muted">"@"{username.clone()}</span>
                                                        </span>
                                                    </a>
                                                </ProfileHover>
                                                <span class="muted small">{format!("{} followers", compact_number(u.follower_count))}</span>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                                <ShowAll shown=all_people total=people_total limit=PEOPLE_SHOWN noun="people"/>
                            </section>
                        })}
                        {(!payload.channels.is_empty()).then(|| view! {
                            <section class="results-group" aria-label="Channels">
                                <h2>"Channels" <span class="count">{channels_total}</span></h2>
                                <ul class="channel-results">
                                    {move || channels.iter().take(if all_channels.get() { usize::MAX } else { CHANNELS_SHOWN }).map(|c| view! {
                                        <li>
                                            <a href=channel_href(&c.id)>
                                                {c.image_url.clone().map(|src| view! { <img src=resized_image_url(&src, ImageSize::Avatar) alt="" width="28" height="28" loading="lazy" referrerpolicy="no-referrer"/> })}
                                                <strong>"/"{c.id.clone()}</strong>
                                            </a>
                                            // Search results carry no count (see `pages::searched_channel`): the channel page has the real ones.
                                            {c.counts().first().map(|count| view! { <span class="muted small" aria-description=count.describe()>{count.label()}</span> })}
                                        </li>
                                    }).collect_view()}
                                </ul>
                                <ShowAll shown=all_channels total=channels_total limit=CHANNELS_SHOWN noun="channels"/>
                            </section>
                        })}
                        {(!payload.casts.is_empty()).then(|| view! {
                            <section class="results-group" aria-label="Casts">
                                <h2>"Casts" <span class="count">{payload.casts.len()}</span></h2>
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

/// "Show all N people" under a capped group; disappears once pressed.
#[component]
fn ShowAll(shown: RwSignal<bool>, total: usize, limit: usize, noun: &'static str) -> impl IntoView {
    (total > limit).then(|| {
        view! {
            <Show when=move || !shown.get()>
                <button type="button" class="link-button show-all" on:click=move |_| shown.set(true)>
                    {format!("Show all {total} {noun}")}
                </button>
            </Show>
        }
    })
}
