// SPDX-License-Identifier: AGPL-3.0-or-later
//! The front page: trending casts, ranked locally.

use std::sync::Arc;

use allthing_core::api::{HomePayload, home_path};
use allthing_core::listing::{PAGE_SIZE, paginate, parse_page, top_level};
use allthing_core::ranking::{Sort, sorted};
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons};
use crate::components::layout::{Columns, ErrorPanel, Freshness, Pager, SortTabs};
use crate::pages::side::{AboutBox, ChannelsBox, KeyboardBox, RankingBox, SourcesBox};
use crate::util::set_title;

#[component]
pub fn Home() -> impl IntoView {
    let query = use_query_map();
    let sort = Memo::new(move |_| Sort::parse(query.read().get("sort").as_deref(), Sort::Hot));
    let page = Memo::new(move |_| parse_page(query.read().get("page").as_deref()));
    let data = LocalResource::new(|| async { api::get::<HomePayload>(&home_path()).await });
    Effect::new(move |_| set_title(if sort.get() == Sort::Hot { "" } else { sort.get().as_str() }));

    move || {
        let side = |channels: Vec<allthing_core::model::Channel>| {
            view! {
                <AboutBox/>
                <ChannelsBox channels/>
                <RankingBox/>
                <SourcesBox/>
                <KeyboardBox/>
            }
            .into_any()
        };
        match data.get() {
            None => view! {
                <Columns side=side(Vec::new())>
                    <Heading sort=sort.get()/>
                    <CastSkeletons/>
                </Columns>
            }
            .into_any(),
            Some(Err(failure)) => view! { <ErrorPanel failure/> }.into_any(),
            Some(Ok(Fetched { data, age, stale })) => {
                let payload: Arc<HomePayload> = data;
                let notes = Arc::new(payload.notes.clone());
                let ranked = sorted(&top_level(&payload.pool), sort.get());
                let slice = paginate(ranked.len(), page.get(), PAGE_SIZE);
                let casts = ranked[slice.offset..slice.end].to_vec();
                view! {
                    <Columns side=side(payload.channels.clone())>
                        <Heading sort=sort.get()/>
                        <Freshness age stale/>
                        <CastList casts notes/>
                        <Pager path="/".to_owned() slice sort=sort.get()/>
                    </Columns>
                }
                .into_any()
            }
        }
    }
}

#[component]
fn Heading(sort: Sort) -> impl IntoView {
    view! {
        <div class="listing-head">
            <h1 class="page-title">"Front page" <span class="subtitle">"trending across Farcaster"</span></h1>
            <SortTabs path="/".to_owned() current=sort/>
        </div>
    }
}
