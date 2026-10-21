// SPDX-License-Identifier: AGPL-3.0-or-later
//! A cast with its full reply tree.

use std::sync::Arc;

use allthing_core::api::{CastPayload, QuotedCast, cast_path};
use allthing_core::clients::{client_name, client_profile};
use allthing_core::embeds::single_image;
use allthing_core::emoji::reactions;
use allthing_core::format::{full_number, short_address};
use allthing_core::model::Cast;
use allthing_core::params::hash_from_param;
use allthing_core::quotes::single_images;
use allthing_core::ranking::{CommentSort, sort_thread};
use allthing_core::text::truncate_title;
use allthing_core::thread::count_nodes;
use allthing_core::time::format_date_time;
use leptos::prelude::*;
use leptos::wasm_split::{SplitLoaderError, wasm_split};
use leptos_router::hooks::{use_params_map, use_query_map};

use crate::Notes;
use crate::api::{self, Fetched};
use crate::components::cast::{CastSkeletons, ChannelLink};
use crate::components::chips::{Avatar, Engagement, TimeAgo};
use crate::components::layout::{Columns, ErrorPanel, NotFound, StaleNotice};
use crate::components::media::{self, LinkCards, MediaGrid, VideoLinks};
use crate::components::profile_card::ProfileHover;
use crate::components::quote::{POST_QUOTE_CHARS, QuoteCard};
use crate::components::reactions::ReactionChips;
use crate::components::text::{CastText, ExcerptText};
use crate::components::thread::CommentTree;
use crate::util::{cast_href, set_title, user_href, with_query};

/// The page, whose code is fetched the first time it's needed
/// (`crate::split`): as it opens, or ahead of it.
#[wasm_split(post, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_post))]
pub fn post_page() -> Result<AnyView, SplitLoaderError> {
    Ok(view! { <PostPage/> }.into_any())
}

#[component]
fn PostPage() -> impl IntoView {
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
    // The page, once the images that show alone at its top have their
    // sizes: drawn before, the post would grow as they came, and push its
    // replies down. A post shows once; it doesn't wait again (for another
    // sort of its replies, say).
    let top = Memo::new(move |_| match data.get() {
        Some(Some(Ok(fetched))) => top_images(&fetched.data, sort.get_untracked()),
        _ => Vec::new(),
    });
    let shown_post = StoredValue::new(None::<String>);
    let ready = Memo::new(move |_| {
        let loaded = data.get();
        let post = match &loaded {
            Some(Some(Ok(fetched))) => Some(fetched.data.thread.cast.hash.clone()),
            _ => None,
        };
        let new = post.is_some() && post != shown_post.get_value();
        if new && top.with(|images| media::sizing(images.iter().map(String::as_str), true)) {
            return None;
        }
        shown_post.set_value(post.or(shown_post.get_value()));
        loaded
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
        let loaded = ready.get();
        if let (None | Some(None), Some(cast)) = (&loaded, api::seen_cast(&hash)) {
            // Seen in a feed: show the cast now, with the quoted casts the
            // feed carried; replies arrive with the thread.
            let notes = allthing_core::api::Annotations::default();
            let quotes: Vec<QuotedCast> = cast
                .quoted()
                .take(3)
                .filter_map(|quoted| Some(QuotedCast { hash: quoted.to_owned(), cast: Some(api::seen_cast(quoted)?) }))
                .collect();
            let payload = Arc::new(CastPayload {
                thread: allthing_core::model::ThreadNode { cast: cast.clone(), children: Vec::new() },
                parent: None,
                quotes,
                channel: None,
                notes: notes.clone(),
                meta: allthing_core::api::Meta { backend: Default::default(), fetched_at: 0, stale: false },
            });
            let notes: Notes = Arc::new(notes);
            return view! {
                <Columns>
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
            Some(Some(Ok(Fetched { data, stale, .. }))) => {
                let payload: Arc<CastPayload> = data;
                let notes: Notes = Arc::new(payload.notes.clone());
                api::remember_cards(&notes.users);
                let cast = payload.thread.cast.clone();
                let mut replies = payload.thread.children.clone();
                sort_thread(&mut replies, sort.get());
                let shown = count_nodes(&replies);
                let path = cast_href(&hash);
                let mut in_thread = std::collections::HashSet::from([cast.hash.to_ascii_lowercase()]);
                thread_hashes(&replies, &mut in_thread);
                let (count, _) = crate::components::chips::replies_shown(&cast.hash, cast.replies.max(shown as u64));
                view! {
                    <Columns>
                        {payload.parent.clone().map(|parent| view! { <ParentContext parent/> })}
                        <MainCast cast=cast.clone() notes=notes.clone() payload=payload.clone()/>
                        <StaleNotice stale/>
                        <section class="replies" aria-label="Replies">
                            <div class="replies-head">
                                <h2>{move || if count.get() == 1 { "Reply" } else { "Replies" }}<span class="num">{move || full_number(count.get())}</span></h2>
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
                            {crate::components::outgoing_replies(in_thread)}
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
            <span class="parent-excerpt"><ExcerptText text=truncate_title(&parent.text, 140)/></span>
        </a>
    }
}

#[component]
fn MainCast(cast: Cast, notes: Notes, payload: Arc<CastPayload>) -> impl IntoView {
    // Whether the reader liked and recasted this cast, which may be older
    // than their latest likes and recasts.
    #[cfg(not(feature = "static-host"))]
    if let (Some(writer), Some(target)) =
        (crate::components::writer::use_writer(), allthing_core::message::CastId::new(cast.author.fid, &cast.hash))
    {
        Effect::new(move |_| {
            if writer.shows_mine() {
                writer.look_up(target);
            }
        });
    }
    let author = cast.author.clone();
    let href = user_href(&author.username);
    let gone = crate::components::deleted(&cast.hash);
    let (pfp, name, username) = (author.pfp_url.clone(), author.name().to_owned(), author.username.clone());
    view! {
        <article class="post" class:deleted=gone>
            <p class="deleted-note">"You deleted this cast."</p>
            <h1 class="visually-hidden">{format!("Cast by {}", author.name())}</h1>
            <header class="post-author">
                <ProfileHover user=author.clone() card=notes.users.get(&author.fid).cloned() class="post-author-links">
                    <a href=href.clone() tabindex="-1" aria-hidden="true"><Avatar url=pfp.clone() size=44 large=true/></a>
                    <div class="post-author-text">
                        <a class="display-name" href=href.clone()>{name.clone()}</a>
                        <span class="handle">
                            <a href=href.clone()>"@"{username.clone()}</a>
                        </span>
                    </div>
                </ProfileHover>
            </header>
            {if cast.hidden() {
                let note = if cast.spam {
                    "Post hidden: Allthing leaves out posts with links to vercel.app apps, gig-bot, DEGEN reward or airdrop bait, and DEGEN staking promotions."
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
                Some(q) => view! {
                    <QuoteCard cast=q.clone() open_on_click=true max_chars=POST_QUOTE_CHARS author_card=notes.users.get(&q.author.fid).cloned() notes=Some(notes.clone())/>
                }.into_any(),
                None => view! { <a class="quote-chip" href=cast_href(&quote.hash)>"↳ quoted cast unavailable"</a> }.into_any(),
            }).collect_view()}
            <p class="post-meta">
                <time datetime=allthing_core::time::iso_utc(cast.ts)>{format_date_time(cast.ts)}</time>
                " · "<TimeAgo ts=cast.ts/>
                <ChannelLink cast=cast.clone()/>
                <PostedVia cast=cast.clone() notes=notes.clone()/>
            </p>
            <footer class="cast-foot">
                <Engagement cast=cast.clone()/>
                {crate::components::thread::reply_button(&cast, "cast")}
            </footer>
            <ReactionChips cast=cast.clone() reacted=reactions(payload.thread.children.iter().map(|n| &n.cast))/>
        </article>
    }
}

/// Which app signed the cast, as a quiet line in the post's meta, and the
/// key it signed with: in the open, not in a native tooltip.
#[component]
fn PostedVia(cast: Cast, notes: Notes) -> impl IntoView {
    let origin = notes.origins.get(&cast.hash).cloned();
    let app = notes.app_for(&cast.hash).cloned();
    match app {
        Some(app) => view! {
            <span class="post-via muted">
                "via "<a href=user_href(&client_profile(&app))>{client_name(&app).to_owned()}</a>
                {origin.map(|o| view! { <span class="post-key">" · key "<code>{short_address(&o.signer)}</code></span> })}
            </span>
        }
        .into_any(),
        None => ().into_any(),
    }
}

/// Replies whose images the page waits for, with the cast's: about what the
/// first screen shows below it.
const TOP_REPLIES: usize = 4;

/// The images that show alone at the top of a post's page: the cast's, its
/// quotes' (and theirs'), and those of its first [`TOP_REPLIES`] replies in
/// `sort`'s order.
fn top_images(payload: &CastPayload, sort: CommentSort) -> Vec<String> {
    let cast = &payload.thread.cast;
    let mut images: Vec<&str> = if cast.hidden() { Vec::new() } else { single_image(cast).into_iter().collect() };
    for quote in payload.quotes.iter().filter_map(|quote| quote.cast.as_ref()).filter(|q| !q.hidden()) {
        images.extend(single_images(quote, &payload.notes.quotes));
    }
    let mut replies = payload.thread.children.clone();
    sort_thread(&mut replies, sort);
    let mut first = Vec::new();
    first_nodes(&replies, &mut first);
    images.extend(first.into_iter().filter(|c| !c.hidden()).filter_map(single_image));
    images.into_iter().map(str::to_owned).collect()
}

/// The first [`TOP_REPLIES`] casts of `nodes`, as the page lists them: each
/// reply, then its replies.
fn first_nodes<'a>(nodes: &'a [allthing_core::model::ThreadNode], into: &mut Vec<&'a Cast>) {
    for node in nodes {
        if into.len() == TOP_REPLIES {
            return;
        }
        into.push(&node.cast);
        first_nodes(&node.children, into);
    }
}

/// The hashes of `nodes` and their replies, lowercase.
fn thread_hashes(nodes: &[allthing_core::model::ThreadNode], into: &mut std::collections::HashSet<String>) {
    for node in nodes {
        into.insert(node.cast.hash.to_ascii_lowercase());
        thread_hashes(&node.children, into);
    }
}
