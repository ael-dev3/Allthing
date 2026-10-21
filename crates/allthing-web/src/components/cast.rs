// SPDX-License-Identifier: AGPL-3.0-or-later
//! Casts in listings.

use allthing_core::model::Cast;
use leptos::prelude::*;

use crate::Notes;
use crate::api;
use crate::components::chips::{Engagement, OriginChip, TimeAgo, UserChip};
use crate::components::favorite::FavoriteMark;
use crate::components::media::{self, LinkCards, MediaGrid, VideoLinks};
use crate::components::quote::nested_quotes;
use crate::components::text::CastText;
use crate::nav::card_click;
use crate::util::{cast_href, channel_href};
use allthing_core::api::cast_path;
use allthing_core::quotes::single_images;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{IntersectionObserver, IntersectionObserverEntry, IntersectionObserverInit};

/// The channel line of a cast's meta row.
#[component]
pub fn ChannelLink(cast: Cast) -> impl IntoView {
    cast.channel.map(|channel| {
        view! {
            <span class="in-channel">
                "in "<a class="channel-name" href=channel_href(&channel.id)>"/"{channel.label}</a>
            </span>
        }
    })
}

/// One cast in a listing: who and where, its whole text, media, the casts
/// it quotes, and its engagement. The whole card opens the post (text stays
/// selectable), except a quoted cast's card, which opens the quoted cast.
/// Hovering or touching the card prefetches the post's data, and in view it
/// has who liked and recasted it, and its replies, preloaded with its
/// neighbours (`crate::preload`), from the counts it carries in
/// `data-likes`, `data-recasts` and `data-replies`.
#[component]
pub fn CastCard(
    cast: Cast,
    notes: Notes,
    #[prop(optional)] index: usize,
    #[prop(optional)] show_parent: bool,
) -> impl IntoView {
    let href = cast_href(&cast.hash);
    // Quoted casts the payload carries show as cards; others as links.
    let quotes: Vec<AnyView> = nested_quotes(&cast, Some(&notes), 0);
    let parent = show_parent.then(|| cast.parent_hash.clone()).flatten();
    let data_path = cast_path(&cast.hash);
    // The post's data, and its page's code (`crate::split`).
    let warm = move || {
        api::prefetch(&data_path);
        crate::split::preload_page("/p/");
    };
    let warm_touch = warm.clone();
    let click = card_click(href.clone());
    // Its emoji reactions, as its listing carries them, and the reader's
    // own this visit. The static copy has neither.
    #[cfg(not(feature = "static-host"))]
    let reactions = crate::components::reactions::card_reactions(&cast, &notes);
    #[cfg(feature = "static-host")]
    let reactions = ();
    let gone = crate::components::deleted(&cast.hash);
    // Drawn as Back or Forward puts the reader's place back, a card is where
    // it was: it doesn't rise into place again.
    let entrance = if crate::nav::restoring() { "; animation: none" } else { "" };
    let node = NodeRef::<leptos::html::Article>::new();
    Effect::new(move |_| {
        if let Some(element) = node.get() {
            watch_for_prefetch(&element);
        }
    });
    on_cleanup(move || {
        if let Some(element) = node.get_untracked() {
            unwatch(&element);
        }
    });
    view! {
        <article
            class="cast-card"
            class:deleted=gone
            node_ref=node
            data-hash=cast.hash.clone()
            data-fid=cast.author.fid
            data-ts=cast.ts
            data-points=allthing_core::ranking::points(&cast)
            data-likes=cast.likes
            data-recasts=cast.recasts
            data-replies=cast.replies
            style=format!("--i: {}{entrance}", index.min(16))
            on:click=click
            on:pointerenter=move |_| warm()
            on:touchstart=move |_| warm_touch()
        >
            <p class="deleted-note">"You deleted this cast."</p>
            <header class="cast-meta">
                // Who and when, on one line, where a long name gives way to
                // the time; the channel and the app follow it.
                <span class="byline">
                    <UserChip user=cast.author.clone() show_name=true card=notes.users.get(&cast.author.fid).cloned()/>
                    <FavoriteMark fid=cast.author.fid/>
                    // The post's own link: for keyboards and screen readers
                    // (the card's text is plain so it can be selected; a
                    // click on it opens the post through `card_click`).
                    <a class="when" href=href.clone()><span class="visually-hidden">"Open post, "</span><TimeAgo ts=cast.ts/></a>
                </span>
                <ChannelLink cast=cast.clone()/>
                <OriginChip hash=cast.hash.clone() notes=notes.clone()/>
            </header>
            {parent.map(|p| view! { <p class="replying-to">"replying to "<a href=cast_href(&p)>"a cast"</a></p> })}
            {(!cast.text.trim().is_empty()).then(|| view! {
                <div class="cast-text"><CastText text=cast.text.clone() mentions=cast.mentions.clone()/></div>
            })}
            <MediaGrid cast=cast.clone()/>
            <VideoLinks cast=cast.clone()/>
            <LinkCards cast=cast.clone()/>
            {quotes}
            <footer class="cast-foot">
                <Engagement cast=cast.clone() reply_href=href.clone()/>
            </footer>
            {reactions}
        </article>
    }
}

/// How many cards infinite scroll adds at a time.
const STEP: usize = 25;
const LOAD_AHEAD_PX: f64 = 1200.0;
/// Cards drawn per task, about a screenful. A list rendered anew (a sort
/// tab, another page) draws this many in the task that asked for it and the
/// rest a screenful at a time after each paint, so a tap shows at once;
/// scrolling adds cards the same way, instead of 25 in one long task.
const DRAW_STEP: usize = 6;

/// A listing that grows as the reader scrolls. `on_end` asks for more casts
/// (the next server page) a little before the reader reaches the end.
#[component]
pub fn CastList(
    #[prop(into)] casts: Signal<Vec<Cast>>,
    #[prop(into)] notes: Signal<Notes>,
    #[prop(optional)] show_parent: bool,
    #[prop(optional)] on_end: Option<Callback<()>>,
    /// Changes after a server page completes, even if it added no casts.
    #[prop(into, optional)]
    page_cursor: Signal<u32>,
) -> impl IntoView {
    // Back to a list shows as many cards as before, so the reader's place
    // exists before the scroll position is restored.
    let key = crate::nav::location_key();
    // A list rendered anew is a new page view for preloaded previews.
    crate::preload::new_view();
    let shown = RwSignal::new(SHOWN.with(|s| s.borrow().get(&key).copied()).unwrap_or(STEP).max(STEP));
    Effect::new(move |_| {
        let count = shown.get();
        SHOWN.with(|s| s.borrow_mut().insert(key.clone(), count));
    });
    let sentinel = NodeRef::<leptos::html::Div>::new();
    let list = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let notes = notes.get();
        casts.with(|c| api::remember_casts(c, &notes));
        api::remember_cards(&notes.users);
    });
    // How many of the cards shown may be drawn, and the top of the order
    // they were drawn in. Back and forward draw them all at once, so the
    // place being restored is there.
    let drawn = StoredValue::new((DRAW_STEP, Vec::<String>::new()));
    let drew_more = RwSignal::new(0_u32);
    let scheduled = StoredValue::new(false);
    // The cards on the page, which stay there whatever they wait for.
    let on_page = StoredValue::new(std::collections::HashSet::<String>::new());
    // How many are drawn: a screenful at a time, once the images its cards
    // show alone have their sizes (or have been waited for long enough), as
    // web/paint.js draws the first ones. A card drawn before would grow as
    // its image came, and push the cards below it down; drawn one by one, a
    // short list would let what is below it come into view and go again.
    // Only where the list ends in view, or nearly: further down, cards come
    // at once, and their images' sizes long before the reader does.
    let ready = Memo::new(move |_| {
        drew_more.track();
        let shown = shown.get();
        let notes = notes.get();
        casts.with(|c| {
            let top: Vec<String> = c.iter().take(DRAW_STEP).map(|c| c.hash.clone()).collect();
            let restoring = crate::nav::restoring();
            let count = drawn.with_value(|(count, before)| {
                if restoring {
                    usize::MAX
                } else if !before.is_empty() && *before != top {
                    // Another order (a sort tab) is a list drawn anew, from its top.
                    (*count).min(DRAW_STEP)
                } else {
                    *count
                }
            });
            let target = shown.min(c.len());
            let count = target.min(count.max(DRAW_STEP));
            drawn.set_value((count, top));
            let near = !restoring && list.get_untracked().is_none_or(|list| ends_in_view(&list));
            // Every card shown has its images' sizes asked for, the next
            // ones', if they are elsewhere than Cloudflare Images, too.
            for (i, cast) in c.iter().enumerate().take(target) {
                for url in single_images(cast, &notes.quotes) {
                    media::size_up(url, near && i < count);
                }
            }
            let drawn_on = on_page.with_value(|on| c[..count].iter().rposition(|cast| on.contains(&cast.hash)));
            let from = drawn_on.map_or(0, |i| i + 1);
            // Each of them asked and waited for (`filter`, not `any`): their
            // images all start loading meanwhile.
            let waiting =
                near && (from..count).filter(|&i| media::sizing(single_images(&c[i], &notes.quotes), true)).count() > 0;
            let ready = if waiting { from } else { count };
            if ready == count && count < target && !scheduled.get_value() {
                // The rest a screenful at a time, each after the last is painted.
                scheduled.set_value(true);
                after_paint(move || {
                    if scheduled.try_set_value(false).is_some() {
                        return; // The list is gone.
                    }
                    drawn.update_value(|(count, _)| *count = count.saturating_add(DRAW_STEP));
                    drew_more.update(|n| *n = n.wrapping_add(1));
                });
            }
            ready
        })
    });
    let visible = move || {
        let ready = ready.get();
        casts.with(|c| {
            let cards = c.iter().take(ready).cloned().enumerate().collect::<Vec<_>>();
            on_page.set_value(cards.iter().map(|(_, cast)| cast.hash.clone()).collect());
            cards
        })
    };
    // The front page's first cards, drawn before the app came, give way to
    // these once they're drawn (`crate::boot`).
    Effect::new(move |_| {
        if ready.get() > 0 {
            crate::boot::hand_over();
        }
    });
    let grow = move || {
        // Still drawing what is shown: its end isn't the list's end yet.
        if drawn.with_value(|(count, _)| *count) < shown.get_untracked().min(casts.with_untracked(Vec::len)) {
            return;
        }
        let len = casts.with_untracked(Vec::len);
        if shown.get_untracked() < len {
            shown.update(|s| *s += STEP);
        }
        if let Some(on_end) = on_end
            && shown.get_untracked() + STEP / 2 >= casts.with_untracked(Vec::len)
        {
            on_end.run(());
        }
    };
    Effect::new(move |_| {
        let Some(element) = sentinel.get() else { return };
        observe_once_visible(&element, grow);
    });
    // A short or filtered page can leave the sentinel inside the observer's
    // margin, so there is no new intersection event to continue the feed.
    Effect::new(move |_| {
        let _ = (casts.with(Vec::len), shown.get(), drew_more.get(), page_cursor.get());
        request_animation_frame(move || {
            let Some(element) = sentinel.try_get_untracked().flatten() else { return };
            if !element.is_connected() {
                return;
            }
            let height = window().inner_height().ok().and_then(|h| h.as_f64()).unwrap_or(0.0);
            if element.get_bounding_client_rect().top() < height + LOAD_AHEAD_PX {
                grow();
            }
        });
    });
    view! {
        <Show when=move || casts.with(|c| !c.is_empty()) fallback=|| view! { <p class="empty">"Nothing here yet."</p> }>
            <div class="cast-list" node_ref=list>
                <For
                    each=visible
                    key=|(_, cast)| cast.hash.clone()
                    children=move |(index, cast)| view! { <CastCard cast notes=notes.get_untracked() index show_parent/> }
                />
            </div>
            // Its first card waiting for its image's size: the list is still
            // on its way.
            <Show when=move || ready.get() == 0><CastSkeletons/></Show>
        </Show>
        <div class="feed-sentinel" node_ref=sentinel aria-hidden="true"></div>
    }
}

/// Call `f` every time `element` comes within 1200px of the viewport.
fn observe_once_visible(element: &web_sys::Element, f: impl Fn() + 'static) {
    let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
        if entries.iter().filter_map(|e| e.dyn_into::<IntersectionObserverEntry>().ok()).any(|e| e.is_intersecting()) {
            f();
        }
    });
    let options = IntersectionObserverInit::new();
    options.set_root_margin(&format!("{LOAD_AHEAD_PX}px 0px"));
    if let Ok(observer) = IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options) {
        observer.observe(element);
        let observer = StoredValue::new_local((observer, callback));
        on_cleanup(move || observer.with_value(|(observer, _)| observer.disconnect()));
    }
}

thread_local! {
    /// How many cards each list (by page key) had shown.
    static SHOWN: std::cell::RefCell<std::collections::HashMap<String, usize>> = Default::default();
    /// One observer for every card: posts that stay in view get prefetched,
    /// and who liked and recasted the cards in view is preloaded.
    static PREFETCH_OBSERVER: Option<IntersectionObserver> = prefetch_observer();
    /// Viewport prefetches allowed: a small token bucket (6 a minute).
    static VIEWPORT_TOKENS: std::cell::Cell<(f64, f64)> = const { std::cell::Cell::new((6.0, 0.0)) };
}

fn prefetch_observer() -> Option<IntersectionObserver> {
    // The static build runs payload builders in the browser; prefetching
    // everything in view would be too heavy there. Its popovers load their
    // lists when they open.
    if cfg!(feature = "static-host") {
        return None;
    }
    let callback = Closure::<dyn FnMut(js_sys::Array)>::new(|entries: js_sys::Array| {
        for entry in entries.iter().filter_map(|e| e.dyn_into::<IntersectionObserverEntry>().ok()) {
            // Any part in view counts here: the observer also reports a card
            // coming into view and leaving it, whatever its threshold.
            crate::preload::noticed(entry.target(), entry.is_intersecting());
            if !entry.is_intersecting() {
                continue;
            }
            let Some(hash) = entry.target().get_attribute("data-hash") else { continue };
            let target = entry.target();
            // The author's card facts, so hovering their name, or tapping it,
            // shows them at once. On a touch screen only for a card the
            // reader stops at: a finger scrolls past many more.
            if let Some(fid) = target.get_attribute("data-fid").and_then(|f| f.parse::<u64>().ok()) {
                if crate::util::can_hover() {
                    idle(move || api::prefetch_card_facts(fid));
                } else {
                    let card = target.clone();
                    set_timeout(
                        move || {
                            if still_visible(&card) {
                                idle(move || api::prefetch_card_facts(fid));
                            }
                        },
                        std::time::Duration::from_millis(600),
                    );
                }
            }
            // Only cards that stay in view for a moment, and only when idle.
            set_timeout(
                move || {
                    if still_visible(&target) && take_viewport_token() {
                        idle(move || api::prefetch(&cast_path(&hash)));
                    }
                },
                std::time::Duration::from_millis(600),
            );
        }
    });
    let options = IntersectionObserverInit::new();
    options.set_threshold(&JsValue::from_f64(0.6));
    let observer = IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options).ok();
    callback.forget();
    observer
}

/// Whether a list ends within half a screen below the viewport: a card
/// added to it now could be in view before its images come.
fn ends_in_view(list: &web_sys::Element) -> bool {
    let height = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    list.get_bounding_client_rect().bottom() < height * 1.5
}

fn still_visible(element: &web_sys::Element) -> bool {
    let rect = element.get_bounding_client_rect();
    let height = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    rect.top() < height && rect.bottom() > 0.0
}

fn take_viewport_token() -> bool {
    VIEWPORT_TOKENS.with(|cell| {
        let (tokens, at) = cell.get();
        let now = js_sys::Date::now();
        let tokens = (tokens + (now - at) / 10_000.0).min(6.0);
        if tokens >= 1.0 {
            cell.set((tokens - 1.0, now));
            true
        } else {
            cell.set((tokens, now));
            false
        }
    })
}

/// Run `f` in a task of its own once the next frame has been painted.
pub fn after_paint(f: impl FnOnce() + 'static) {
    request_animation_frame(move || set_timeout(f, std::time::Duration::ZERO));
}

/// Run `f` when the browser is idle (or soon, where that isn't supported).
pub fn idle(f: impl FnOnce() + 'static) {
    let callback = Closure::once_into_js(f);
    let window = window();
    let request = js_sys::Reflect::get(&window, &JsValue::from_str("requestIdleCallback"))
        .ok()
        .and_then(|r| r.dyn_into::<js_sys::Function>().ok());
    match request {
        Some(request) => {
            let _ = request.call1(&window, &callback);
        }
        None => {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), 200);
        }
    }
}

fn watch_for_prefetch(element: &web_sys::Element) {
    PREFETCH_OBSERVER.with(|o| {
        if let Some(observer) = o {
            observer.observe(element);
        }
    });
}

fn unwatch(element: &web_sys::Element) {
    PREFETCH_OBSERVER.with(|o| {
        if let Some(observer) = o {
            observer.unobserve(element);
        }
    });
    crate::preload::forget(element);
}

/// Placeholder cards while a listing loads.
#[component]
pub fn CastSkeletons(#[prop(default = 6)] count: usize) -> impl IntoView {
    view! {
        <div class="cast-list" aria-busy="true" aria-label="Loading casts">
            {(0..count)
                .map(|i| view! {
                    <div class="cast-card skeleton" style=format!("--i: {i}")>
                        <span class="sk sk-meta"></span>
                        <span class="sk sk-title"></span>
                        <span class="sk sk-title short"></span>
                        <span class="sk sk-foot"></span>
                    </div>
                })
                .collect_view()}
        </div>
    }
}
