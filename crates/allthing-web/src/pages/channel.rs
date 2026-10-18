// SPDX-License-Identifier: AGPL-3.0-or-later
//! A channel (or any parent-URL feed, like a token's): its banner, picture,
//! description, counts, hosts and pinned cast where the Farcaster app gives
//! them, then its casts.

use std::sync::Arc;

use allthing_core::api::{ChannelPayload, channel_path};
use allthing_core::embeds::{ImageSize, embed_domain, resized_image_url};
use allthing_core::format::full_number;
use allthing_core::listing::top_level;
use allthing_core::model::{Cast, Channel, ExternalLink, UserSummary};
use allthing_core::params::channel_key_from_decoded;
use allthing_core::ranking::{Sort, sorted};
use leptos::prelude::*;
use leptos_router::hooks::{use_params_map, use_query_map};

use crate::Notes;
use crate::api::{self, Fetched};
use crate::components::cast::{CastCard, CastList, CastSkeletons};
use crate::components::chips::Avatar;
use crate::components::layout::{Columns, ErrorPanel, NotFound, SideBox, SortTabs, StaleNotice};
use crate::components::text::CastText;
use crate::util::{channel_href, set_title, use_query_param, user_href};

/// Moderators listed before a "+N" button that shows the rest.
const MODERATORS_SHOWN: usize = 6;

const PIN: &str = "M5.5 2.5h5M6.5 2.5v3.8L4 9.5h8L9.5 6.3V2.5M8 9.5v4";
const LINK: &str = "M6.8 9.2l2.4-2.4M7.4 4.6l1-1a2.6 2.6 0 0 1 3.7 3.7l-1 1M8.6 11.4l-1 1a2.6 2.6 0 0 1-3.7-3.7l1-1";

#[component]
pub fn ChannelPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let id = use_query_param("id");
    // /c/<id>, or /c?id=<parent URL> (also the old static site's form).
    let key = Memo::new(move |_| {
        params.read().get("key").or_else(|| id.get()).and_then(|raw| channel_key_from_decoded(&raw))
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
            let channel = &fetched.data.channel;
            let title = channel.heading();
            set_title(&if channel.has_display_name() { format!("{title} (/{})", channel.id) } else { title });
        }
    });

    move || {
        let Some(key) = key.get() else {
            return view! { <NotFound/> }.into_any();
        };
        let path = channel_href(&key);
        match data.get() {
            None | Some(None) => view! {
                <Columns>
                    <header class="channel-hero" aria-busy="true">
                        <div class="channel-hero-body">
                            <h1 class="page-title">"/"{allthing_core::channels::channel_label(&key)}</h1>
                        </div>
                    </header>
                    <CastSkeletons/>
                </Columns>
            }
            .into_any(),
            Some(Some(Err(failure))) => view! { <ErrorPanel failure/> }.into_any(),
            Some(Some(Ok(Fetched { data, stale, .. }))) => {
                let payload: Arc<ChannelPayload> = data;
                let notes: Notes = Arc::new(payload.notes.clone());
                let channel = payload.channel.clone();
                // The pinned cast shows above the feed, not again in it.
                let pinned = channel.pinned.clone();
                let pinned_hash = pinned.as_ref().map(|c| c.hash.clone());
                let pool: Vec<Cast> =
                    top_level(&payload.pool).into_iter().filter(|c| Some(&c.hash) != pinned_hash.as_ref()).collect();
                let casts = Signal::derive(move || sorted(&pool, sort.get()));
                // Hypersnap's count of casters, when the header shows the
                // Farcaster app's member and follower counts instead.
                let casters =
                    channel.casters.filter(|_| channel.follower_count.is_some() || channel.member_count.is_some());
                let feed = channel.parent_url.clone();
                let side = view! {
                    <SideBox title="About this channel">
                        {casters.map(|n| view! {
                            <p title="Counted by Hypersnap from the channel's casts.">
                                <strong>{full_number(n)}</strong>{if n == 1 { " account has cast here" } else { " accounts have cast here" }}
                            </p>
                        })}
                        <p class="muted small">"Feed: "<code>{feed}</code></p>
                    </SideBox>
                }
                .into_any();
                view! {
                    <Columns side>
                        <ChannelHeader channel=channel.clone()/>
                        {pinned.map(|cast| {
                            api::remember_casts(std::slice::from_ref(&cast), &notes);
                            view! {
                                <section class="pinned-cast" aria-labelledby="pinned-label">
                                    <p class="pinned-label" id="pinned-label">
                                        <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=PIN/></svg>
                                        "Pinned"
                                    </p>
                                    <CastCard cast notes=notes.clone()/>
                                </section>
                            }
                        })}
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

/// The banner, picture, names, description, counts, link and hosts. Each part
/// shows only when known: without the Farcaster app's details (in the static
/// copy, say) this is the name and Hypersnap's count of casters.
#[component]
fn ChannelHeader(channel: Channel) -> impl IntoView {
    let title = channel.heading();
    let subtitle = channel.subheading();
    let picture_alt = format!("Picture of {title}");
    let counts = channel.counts();
    let Channel {
        header_image_url, image_url, description, description_mentions, external_link, lead, moderators, ..
    } = channel;
    let has_banner = header_image_url.is_some();
    let has_stats = !counts.is_empty() || external_link.is_some();
    let has_hosts = lead.is_some() || !moderators.is_empty();
    view! {
        <header class="channel-hero" class:has-banner=has_banner>
            {header_image_url.map(|src| view! {
                <div class="channel-banner">
                    <img src=resized_image_url(&src, ImageSize::Preview) alt="" decoding="async" referrerpolicy="no-referrer"/>
                </div>
            })}
            <div class="channel-hero-body">
                <div class="channel-identity">
                    {image_url.map(|src| view! {
                        <img class="channel-image" src=resized_image_url(&src, ImageSize::Thumb) alt=picture_alt width="88" height="88" decoding="async" referrerpolicy="no-referrer"/>
                    })}
                    <div class="channel-names">
                        <h1 class="page-title">{title}</h1>
                        {subtitle.map(|s| view! { <p class="channel-handle">{s}</p> })}
                    </div>
                </div>
                {description.map(|text| view! {
                    <div class="channel-description bio"><CastText text mentions=description_mentions/></div>
                })}
                {has_stats.then(|| view! {
                    <div class="channel-stats">
                        {counts
                            .into_iter()
                            .map(|count| view! {
                                <span class="count" title=count.describe()>
                                    <strong class="number">{full_number(count.value())}</strong>" "{count.noun()}
                                </span>
                            })
                            .collect_view()}
                        {external_link.map(|link| view! { <ChannelLink link/> })}
                    </div>
                })}
                {has_hosts.then(|| view! { <Hosts lead moderators/> })}
            </div>
        </header>
    }
}

/// The link the channel's hosts set, such as "Join" or "Subscribe".
#[component]
fn ChannelLink(link: ExternalLink) -> impl IntoView {
    let ExternalLink { url, title } = link;
    let text = if title.is_empty() { embed_domain(&url) } else { title };
    let href = url.clone();
    view! {
        <a class="channel-link" href=href target="_blank" rel="noopener noreferrer nofollow ugc" title=url>
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=LINK/></svg>
            <span>{text}</span>
        </a>
    }
}

/// One host: picture and name, linking to the profile.
#[component]
fn Host(user: UserSummary) -> impl IntoView {
    view! {
        <a class="host" href=user_href(&user.username) title=format!("@{}", user.username)>
            <Avatar url=user.pfp_url.clone() size=24/>
            <span class="host-name">{user.name().to_owned()}</span>
        </a>
    }
}

/// "Lead" and "Moderators", the moderators capped at [`MODERATORS_SHOWN`]
/// until "+N" is pressed.
#[component]
fn Hosts(lead: Option<UserSummary>, moderators: Vec<UserSummary>) -> impl IntoView {
    let all = RwSignal::new(false);
    let hidden = moderators.len().saturating_sub(MODERATORS_SHOWN);
    let more = move || hidden != 0 && !all.get();
    view! {
        <dl class="channel-hosts">
            {lead.map(|user| view! {
                <div class="host-group">
                    <dt>"Lead"</dt>
                    <dd><Host user/></dd>
                </div>
            })}
            {(!moderators.is_empty()).then(|| view! {
                <div class="host-group">
                    <dt>"Moderators"</dt>
                    <dd>
                        <ul class="host-list">
                            {move || {
                                let shown = if all.get() { moderators.len() } else { MODERATORS_SHOWN };
                                moderators.iter().take(shown).map(|user| view! { <li><Host user=user.clone()/></li> }).collect_view()
                            }}
                            <Show when=more>
                                <li>
                                    <button type="button" class="host-more" on:click=move |_| all.set(true)>
                                        "+"{hidden}<span class="visually-hidden">" more moderators"</span>
                                    </button>
                                </li>
                            </Show>
                        </ul>
                    </dd>
                </div>
            })}
        </dl>
    }
}
