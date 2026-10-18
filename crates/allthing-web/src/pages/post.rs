// SPDX-License-Identifier: AGPL-3.0-or-later
//! A cast with its full reply tree.

use std::sync::Arc;

use allthing_core::api::{CastPayload, cast_path};
use allthing_core::format::{full_number, plural, short_address};
use allthing_core::model::Cast;
use allthing_core::params::hash_from_param;
use allthing_core::ranking::{CommentSort, sort_thread};
use allthing_core::text::truncate_title;
use allthing_core::thread::count_nodes;
use allthing_core::time::format_date_time;
use leptos::prelude::*;
use leptos_router::hooks::{use_params_map, use_query_map};

use crate::Notes;
use crate::api::{self, Fetched};
use crate::components::cast::{CastSkeletons, ChannelLink};
use crate::components::chips::{Avatar, Engagement, OriginChip, ScoreBadge, TimeAgo, UserChip};
use crate::components::layout::{Columns, ErrorPanel, Freshness, NotFound, SideBox};
use crate::components::media::{LinkCards, MediaGrid, VideoLinks};
use crate::components::text::CastText;
use crate::components::thread::CommentTree;
use crate::util::{cast_href, set_title, user_href, with_query};

#[component]
pub fn PostPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let hash = Memo::new(move |_| {
        params.read().get("hash").or_else(|| query.read().get("hash")).and_then(|raw| hash_from_param(&raw))
    });
    let sort = Memo::new(move |_| CommentSort::parse(query.read().get("sort").as_deref()));
    let data = LocalResource::new(move || {
        let hash = hash.get();
        async move {
            match hash {
                Some(hash) => Some(api::get::<CastPayload>(&cast_path(&hash)).await),
                None => None,
            }
        }
    });
    Effect::new(move |_| {
        if let Some(Some(Ok(fetched))) = data.get() {
            let cast = &fetched.data.thread.cast;
            let title = truncate_title(&cast.text, 60);
            set_title(&format!(
                "{} — @{}",
                if title.is_empty() { "cast".to_owned() } else { title },
                cast.author.username
            ));
        }
    });

    move || {
        let Some(hash) = hash.get() else {
            return view! { <NotFound/> }.into_any();
        };
        let loaded = data.get();
        if let (None | Some(None), Some((cast, score))) = (&loaded, api::seen_cast(&hash)) {
            // Seen in a feed: show the cast now; replies arrive with the thread.
            let mut notes = allthing_core::api::Annotations::default();
            if let Some(score) = score {
                notes.scores.insert(cast.author.fid, score);
            }
            let payload = Arc::new(CastPayload {
                thread: allthing_core::model::ThreadNode { cast: cast.clone(), children: Vec::new() },
                parent: None,
                quotes: Vec::new(),
                channel: None,
                notes: notes.clone(),
                meta: allthing_core::api::Meta { backend: Default::default(), fetched_at: 0, stale: false },
            });
            let notes: Notes = Arc::new(notes);
            return view! {
                <Columns side=().into_any()>
                    <MainCast cast notes payload/>
                    <section class="replies" aria-label="Replies" aria-busy="true">
                        <div class="replies-head"><h2>"Replies"</h2></div>
                        <CastSkeletons count=2/>
                    </section>
                </Columns>
            }
            .into_any();
        }
        match loaded {
            None | Some(None) => view! {
                <div class="post-loading" aria-busy="true">
                    <div class="post skeleton"><span class="sk sk-meta"></span><span class="sk sk-title"></span><span class="sk sk-title"></span><span class="sk sk-title short"></span></div>
                </div>
            }
            .into_any(),
            Some(Some(Err(failure))) => view! { <ErrorPanel failure/> }.into_any(),
            Some(Some(Ok(Fetched { data, age, stale }))) => {
                let payload: Arc<CastPayload> = data;
                let notes: Notes = Arc::new(payload.notes.clone());
                let cast = payload.thread.cast.clone();
                let mut replies = payload.thread.children.clone();
                sort_thread(&mut replies, sort.get());
                let shown = count_nodes(&replies);
                let side = view! {
                    <ProvenanceBox cast=cast.clone() notes=notes.clone()/>
                    {payload.channel.clone().map(|channel| view! {
                        <SideBox title=format!("/{}", channel.name)>
                            {channel.description.clone().map(|d| view! { <p>{d}</p> })}
                            {channel.member_count.map(|m| view! { <p class="muted">{plural(m, "member", "members")}</p> })}
                        </SideBox>
                    })}
                }
                .into_any();
                let path = cast_href(&hash);
                view! {
                    <Columns side>
                        {payload.parent.clone().map(|parent| view! { <ParentContext parent/> })}
                        <MainCast cast=cast.clone() notes=notes.clone() payload=payload.clone()/>
                        <Freshness age stale/>
                        <section class="replies" aria-label="Replies">
                            <div class="replies-head">
                                <h2>{if cast.replies.max(shown as u64) == 1 { "Reply" } else { "Replies" }}<span class="num">{full_number(cast.replies.max(shown as u64))}</span></h2>
                                <nav class="sort-tabs" aria-label="Sort replies">
                                    {CommentSort::ALL
                                        .into_iter()
                                        .map(|option| {
                                            let href = with_query(&path, &[("sort", (option != CommentSort::Top).then(|| option.as_str().to_owned()))]);
                                            view! { <a href=href data-noscroll="" class:selected=option == sort.get()>{option.as_str()}</a> }
                                        })
                                        .collect_view()}
                                </nav>
                            </div>
                            {if replies.is_empty() {
                                view! { <p class="empty">"No replies yet."</p> }.into_any()
                            } else {
                                view! { <div class="comments"><CommentTree nodes=replies notes=notes.clone() op_fid=cast.author.fid/></div> }.into_any()
                            }}
                        </section>
                    </Columns>
                }
                .into_any()
            }
        }
    }
}

#[component]
fn ParentContext(parent: Cast) -> impl IntoView {
    view! {
        <a class="parent-context" href=cast_href(&parent.hash)>
            <span class="muted">"In reply to "</span>
            <strong>"@"{parent.author.username.clone()}</strong>
            <span class="parent-excerpt">{truncate_title(&parent.text, 140)}</span>
        </a>
    }
}

#[component]
fn MainCast(cast: Cast, notes: Notes, payload: Arc<CastPayload>) -> impl IntoView {
    let score = notes.scores.get(&cast.author.fid).copied().unwrap_or(0);
    let author = cast.author.clone();
    view! {
        <article class="post">
            <header class="post-author">
                <a href=user_href(&author.username) tabindex="-1" aria-hidden="true"><Avatar url=author.pfp_url.clone() size=44 large=true/></a>
                <div class="post-author-text">
                    <a class="display-name" href=user_href(&author.username)>{author.name().to_owned()}</a>
                    <span class="handle">
                        <a href=user_href(&author.username)>"@"{author.username.clone()}</a>
                        <ScoreBadge micros=score/>
                    </span>
                </div>
            </header>
            {if cast.hidden() {
                let note = if cast.spam {
                    "Hidden as spam: Allthing leaves out links to vercel.app apps, gig-bot, and DEGEN reward or airdrop bait."
                } else {
                    "Mini app post hidden: Allthing does not run mini apps or frames."
                };
                view! { <p class="mini-app-note">{note}</p> }.into_any()
            } else {
                view! { <div class="post-text"><CastText text=cast.text.clone() mentions=cast.mentions.clone()/></div> }.into_any()
            }}
            <MediaGrid cast=cast.clone() open_on_click=true/>
            <VideoLinks cast=cast.clone()/>
            <LinkCards cast=cast.clone()/>
            {payload.quotes.iter().map(|quote| match &quote.cast {
                Some(q) => view! { <QuoteCard cast=q.clone() notes=notes.clone()/> }.into_any(),
                None => view! { <a class="quote-chip" href=cast_href(&quote.hash)>"↳ quoted cast unavailable"</a> }.into_any(),
            }).collect_view()}
            <p class="post-meta">
                <time datetime=allthing_core::time::iso_utc(cast.ts)>{format_date_time(cast.ts)}</time>
                " · "<TimeAgo ts=cast.ts/>
                <ChannelLink cast=cast.clone()/>
                <OriginChip hash=cast.hash.clone() notes=notes.clone()/>
            </p>
            <footer class="cast-foot">
                <Engagement cast=cast.clone()/>
            </footer>
        </article>
    }
}

#[component]
fn QuoteCard(cast: Cast, notes: Notes) -> impl IntoView {
    let score = notes.scores.get(&cast.author.fid).copied().unwrap_or(0);
    view! {
        <blockquote class="quote">
            <div class="cast-meta"><UserChip user=cast.author.clone() score/>" · "<TimeAgo ts=cast.ts/></div>
            <div class="quote-text"><CastText text=truncate_title(&cast.text, 400) mentions=cast.mentions.clone()/></div>
            <MediaGrid cast=cast.clone() open_on_click=true/>
            <a class="quote-link" href=cast_href(&cast.hash)>"open quoted cast →"</a>
        </blockquote>
    }
}

/// Which app signed the cast.
#[component]
fn ProvenanceBox(cast: Cast, notes: Notes) -> impl IntoView {
    let origin = notes.origins.get(&cast.hash).cloned();
    let app = notes.app_for(&cast.hash).cloned();
    view! {
        <SideBox title="Posted via">
            <dl class="provenance">
                <dt>"App"</dt>
                <dd>{match (&origin, &app) {
                    (Some(_), Some(app)) => view! {
                        <a href=user_href(&app.username)>{app.name().to_owned()}</a>
                        <span class="muted">{format!(" · app fid {}", app.fid)}</span>
                    }.into_any(),
                    (Some(o), None) => match o.app_fid {
                        Some(fid) => view! { <span>{format!("app fid {fid}")}</span> }.into_any(),
                        None => view! { <span class="muted">"a key registered without app attribution"</span> }.into_any(),
                    },
                    (None, _) => view! { <span class="muted">"still being resolved"</span> }.into_any(),
                }}</dd>
                {origin.map(|o| view! {
                    <dt>"Signer key"</dt>
                    <dd><code title=o.signer.clone()>{short_address(&o.signer)}</code></dd>
                })}
            </dl>
        </SideBox>
    }
}
