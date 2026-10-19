// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writing a cast or a reply, with a GIF, for the signed-in reader.
//! Allthing doesn't post: "Post in Farcaster" opens the draft in the
//! Farcaster app, through farcaster.xyz's compose link, and the reader posts
//! it there (`allthing_core::compose`). The header's Cast button starts a
//! cast, in the channel being read on a channel page; Reply under a post or
//! a reply starts a reply to it.
//!
//! GIFs come from KLIPY. Its terms ask that the browser reach its API and
//! its media itself, never through Allthing's server, and that results show
//! in the order KLIPY gives them, in a grid of their own, under a search box
//! that says "Search KLIPY". The key comes from the Worker
//! (`/api/v1/gifs/key`, for a signed-in session); without one (no
//! `KLIPY_API_KEY` secret) the composer has no GIF button. Not in the
//! static copy, which has no sign-in.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use allthing_core::channels::is_channel_id;
use allthing_core::compose::{Draft, Gif, GifKey, GifPage, SEARCH_PLACEHOLDER, gif_key_path, gifs_url};
use allthing_core::model::Cast;
use allthing_core::text::truncate_title;
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlElement;

use crate::api;
use crate::components::signin::use_session;
use crate::util::{BASE, document};

/// How long typing pauses before a search is sent.
const DEBOUNCE: Duration = Duration::from_millis(350);
/// How long KLIPY's answers are reused this visit.
const KEEP_MS: f64 = 600_000.0;
/// The shortest search sent (an empty one shows what's trending).
const MIN_QUERY: usize = 2;

const PEN: &str = "M10.5 3.2l2.3 2.3M3 13l.6-2.9 7.5-7.5a1.2 1.2 0 0 1 1.7 0l.6.6a1.2 1.2 0 0 1 0 1.7l-7.5 7.5z";
const REPLY: &str = "M6.5 4L3 7.5 6.5 11M3.5 7.5h6a3.5 3.5 0 0 1 3.5 3.5v1";
const OUT: &str = "M9 3h4v4M13 3L7.5 8.5M11.5 9.5v3h-8v-8h3";

/// A page of KLIPY's GIFs, and whether there are more.
type Found = Rc<(Vec<Gif>, bool)>;

thread_local! {
    /// KLIPY's pages this visit, by request, and when they came: reopening
    /// the picker, or going back to an earlier search, asks for nothing again.
    static PAGES: RefCell<HashMap<String, (f64, Found)>> = RefCell::new(HashMap::new());
    static RETURN_FOCUS: RefCell<Option<HtmlElement>> = const { RefCell::new(None) };
}

/// What a reply answers.
#[derive(Clone, Debug, PartialEq)]
struct ReplyTo {
    hash: String,
    username: String,
    excerpt: String,
}

/// What a draft is for.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// A cast, in a channel or not.
    Cast(Option<String>),
    Reply(ReplyTo),
}

/// The composer, held by the app: whether it is open, what the draft is
/// for, and the draft, kept while it is closed unposted.
#[derive(Clone, Copy)]
pub struct Composer {
    open: RwSignal<bool>,
    target: RwSignal<Target>,
    text: RwSignal<String>,
    gif: RwSignal<Option<Gif>>,
    picking: RwSignal<bool>,
}

impl Composer {
    pub fn new() -> Composer {
        Composer {
            open: RwSignal::new(false),
            target: RwSignal::new(Target::Cast(None)),
            text: RwSignal::new(String::new()),
            gif: RwSignal::new(None),
            picking: RwSignal::new(false),
        }
    }

    /// Open the composer for `target`. A draft for something else is set
    /// aside; one for the same is taken up again.
    fn start(self, target: Target) {
        RETURN_FOCUS.with(|r| *r.borrow_mut() = document().active_element().and_then(|el| el.dyn_into().ok()));
        if self.target.get_untracked() != target {
            self.target.set(target);
            self.clear();
        }
        self.picking.set(false);
        self.open.set(true);
    }

    fn clear(self) {
        self.text.set(String::new());
        self.gif.set(None);
    }

    /// Close, keeping the draft, and return focus to where it was.
    fn close(self) {
        self.open.set(false);
        self.picking.set(false);
        if let Some(element) = RETURN_FOCUS.with(|r| r.borrow_mut().take()).filter(|el| el.is_connected()) {
            let _ = element.focus();
        }
    }

    fn draft(self) -> Draft {
        let (parent_hash, channel) = match self.target.get() {
            Target::Reply(to) => (Some(to.hash), None),
            Target::Cast(channel) => (None, channel),
        };
        let embeds = self.gif.with(|gif| gif.iter().map(|g| g.embed.url.clone()).collect());
        Draft { text: self.text.get(), embeds, parent_hash, channel }
    }
}

impl Default for Composer {
    fn default() -> Self {
        Composer::new()
    }
}

fn use_composer() -> Composer {
    expect_context::<Composer>()
}

/// Ask for the GIF key ahead of a click that opens the composer, so its
/// GIF button is there when it opens.
fn warm() {
    api::prefetch(&gif_key_path());
}

/// Focus `selector`'s element once the page has drawn it.
fn focus_soon(selector: &'static str) {
    request_animation_frame(move || {
        if let Some(element) = document().query_selector(selector).ok().flatten() {
            let _ = element.unchecked_into::<HtmlElement>().focus();
        }
    });
}

/// The channel whose page is open, if any.
fn channel_in_view(path: &str) -> Option<String> {
    let path = path.strip_prefix(BASE).unwrap_or(path);
    let id = path.strip_prefix("/c/")?.trim_end_matches('/').to_lowercase();
    is_channel_id(&id).then_some(id)
}

/// The header's Cast button, for the signed-in reader: a new cast, in the
/// channel being read on a channel page. Only its icon on phones.
#[component]
pub fn ComposeButton() -> impl IntoView {
    let session = use_session();
    let composer = use_composer();
    let location = use_location();
    move || {
        session.user.get().is_some().then(|| {
            view! {
                <button
                    type="button"
                    class="compose-button"
                    aria-label="Write a cast"
                    on:pointerenter=move |_| warm()
                    on:focus=move |_| warm()
                    on:click=move |_| composer.start(Target::Cast(channel_in_view(&location.pathname.get_untracked())))
                >
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=PEN/></svg>
                    <span class="compose-label">"Cast"</span>
                </button>
            }
        })
    }
}

/// Reply, under a post or a reply, for the signed-in reader.
#[component]
pub fn ReplyButton(cast: Cast) -> impl IntoView {
    let session = use_session();
    let composer = use_composer();
    let to = ReplyTo {
        hash: cast.hash.clone(),
        username: cast.author.username.clone(),
        excerpt: truncate_title(cast.text.trim(), 90),
    };
    move || {
        session.user.get().is_some().then(|| {
            let label = format!("Reply to @{}", to.username);
            let to = to.clone();
            view! {
                <button
                    type="button"
                    class="reply-button"
                    aria-label=label
                    on:pointerenter=move |_| warm()
                    on:focus=move |_| warm()
                    on:click=move |_| composer.start(Target::Reply(to.clone()))
                >
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=REPLY/></svg>
                    "Reply"
                </button>
            }
        })
    }
}

/// The composer's dialog: the draft's text, its GIF, the GIF picker, and
/// the link that opens it in the Farcaster app.
#[component]
pub fn ComposerDialog() -> impl IntoView {
    let composer = use_composer();
    let session = use_session();
    let location = use_location();
    let dialog = NodeRef::<leptos::html::Div>::new();
    // A history navigation, or signing out, closes it.
    Effect::new(move |_| {
        let _ = (location.pathname.get(), location.search.get(), session.user.with(Option::is_some));
        if composer.open.get_untracked() {
            composer.close();
        }
    });
    Effect::new(move |_| {
        let open = composer.open.get();
        if let Some(root) = document().document_element() {
            let _ = root.class_list().toggle_with_force("compose-open", open);
        }
    });
    let keys = window_event_listener(ev::keydown, move |ev| {
        if !composer.open.get_untracked() {
            return;
        }
        match ev.key().as_str() {
            // The picker first, then the dialog.
            "Escape" => {
                ev.prevent_default();
                if composer.picking.get_untracked() {
                    composer.picking.set(false);
                    focus_soon(".gif-button");
                } else {
                    composer.close();
                }
            }
            // Focus stays in the dialog.
            "Tab" => {
                let Some(root) = dialog.get_untracked() else { return };
                let Ok(nodes) = root.query_selector_all("button:not([disabled]), a[href], textarea, input") else {
                    return;
                };
                let first = nodes.get(0).and_then(|n| n.dyn_into::<HtmlElement>().ok());
                let last = nodes.get(nodes.length().saturating_sub(1)).and_then(|n| n.dyn_into::<HtmlElement>().ok());
                let active = document().active_element();
                let at = |el: &Option<HtmlElement>| {
                    el.as_ref().map(|e| e.unchecked_ref::<web_sys::Element>().clone()) == active
                };
                if ev.shift_key() && at(&first) {
                    ev.prevent_default();
                    if let Some(last) = last {
                        let _ = last.focus();
                    }
                } else if !ev.shift_key() && at(&last) {
                    ev.prevent_default();
                    if let Some(first) = first {
                        let _ = first.focus();
                    }
                }
            }
            _ => {}
        }
    });
    on_cleanup(move || keys.remove());
    move || {
        composer.open.get().then(|| {
            // Kept in the client cache for 5 minutes, and by the browser for an hour.
            let key = LocalResource::new(|| async { api::get::<GifKey>(&gif_key_path()).await });
            let gif_key = move || key.get().and_then(Result::ok).map(|k| k.data.key.clone()).filter(|k| !k.is_empty());
            let text = NodeRef::<leptos::html::Textarea>::new();
            Effect::new(move |_| {
                if let Some(text) = text.get() {
                    let _ = text.focus();
                }
            });
            let (title, placeholder, context) = match composer.target.get_untracked() {
                Target::Reply(to) => (
                    "Reply",
                    "Write your reply",
                    Some(view! {
                        <p class="compose-context">
                            "Replying to "<strong>"@"{to.username}</strong>
                            {(!to.excerpt.is_empty()).then(|| view! { <span class="compose-excerpt">{to.excerpt}</span> })}
                        </p>
                    }.into_any()),
                ),
                Target::Cast(channel) => (
                    "New cast",
                    "What's happening?",
                    channel.map(|id| view! { <p class="compose-context">"In "<strong>"/"{id}</strong></p> }.into_any()),
                ),
            };
            let picked = Callback::new(move |gif: Gif| {
                composer.gif.set(Some(gif));
                composer.picking.set(false);
                focus_soon(".compose-text");
            });
            view! {
                <div class="modal-backdrop" on:click=move |ev: ev::MouseEvent| {
                    if ev.target() == ev.current_target() {
                        composer.close();
                    }
                }>
                    <div class="modal compose-modal" role="dialog" aria-modal="true" aria-labelledby="compose-title" node_ref=dialog>
                        <header class="modal-head">
                            <h2 id="compose-title">{title}</h2>
                            <button type="button" class="modal-close" aria-label="Close" on:click=move |_| composer.close()>"×"</button>
                        </header>
                        <div class="modal-body compose-body">
                            {context}
                            <textarea
                                class="compose-text"
                                node_ref=text
                                rows="4"
                                placeholder=placeholder
                                aria-label=placeholder
                                prop:value=move || composer.text.get()
                                on:input=move |ev| composer.text.set(event_target_value(&ev))
                            ></textarea>
                            {move || composer.gif.get().map(|gif| view! {
                                <figure class="compose-gif">
                                    <img src=gif.tile.url.clone() width=gif.tile.width height=gif.tile.height alt=gif.title.clone()/>
                                    <button type="button" class="compose-gif-remove" aria-label="Remove the GIF" on:click=move |_| {
                                        composer.gif.set(None);
                                        focus_soon(".compose-text");
                                    }>"×"</button>
                                </figure>
                            })}
                            {move || composer.picking.get().then(gif_key).flatten().map(|key| view! { <GifPicker key picked/> })}
                        </div>
                        // Below what scrolls, so the picker never hides it.
                        <footer class="compose-foot">
                            <div class="compose-bar">
                                {move || gif_key().map(|_| view! {
                                    <button
                                        type="button"
                                        class="gif-button"
                                        aria-label="Add a GIF"
                                        aria-expanded=move || composer.picking.get().to_string()
                                        aria-controls="gif-picker"
                                        on:click=move |_| composer.picking.update(|p| *p = !*p)
                                    >"GIF"</button>
                                })}
                                <PostLink composer/>
                            </div>
                            <p class="compose-note muted small">"Allthing doesn't post: this opens the Farcaster app with your cast ready to post."</p>
                        </footer>
                    </div>
                </div>
            }
        })
    }
}

/// "Post in Farcaster": the draft's compose link, opened in a new tab (or
/// the Farcaster app, on a phone). The draft is the app's from there on.
#[component]
fn PostLink(composer: Composer) -> impl IntoView {
    let empty = Memo::new(move |_| composer.text.with(|t| t.trim().is_empty()) && composer.gif.with(Option::is_none));
    move || {
        if empty.get() {
            view! { <button type="button" class="button compose-post" disabled=true>"Post in Farcaster"</button> }
                .into_any()
        } else {
            view! {
                <a
                    class="button compose-post"
                    href=move || composer.draft().compose_url()
                    target="_blank"
                    rel="noopener noreferrer"
                    on:click=move |_| {
                        // After the link has opened.
                        set_timeout(
                            move || {
                                composer.clear();
                                composer.close();
                            },
                            Duration::ZERO,
                        );
                    }
                >
                    "Post in Farcaster"
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=OUT/></svg>
                </a>
            }
            .into_any()
        }
    }
}

/// What the picker shows.
#[derive(Clone, PartialEq)]
enum Shown {
    Loading,
    /// Its GIFs are in `gifs`; whether KLIPY has more.
    Ready {
        more: bool,
    },
    Failed(Unread),
}

/// Why KLIPY's answer didn't come.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Unread {
    /// The key's requests for the hour are spent (KLIPY limits a key in
    /// testing to 100 an hour, for every reader together).
    Busy,
    Failed,
}

/// The GIF picker: what's trending on KLIPY, or its results for what is
/// typed, in KLIPY's order. Picking one attaches it to the draft.
#[component]
fn GifPicker(key: String, picked: Callback<Gif>) -> impl IntoView {
    let gifs = RwSignal::new(Vec::<Gif>::new());
    let shown = RwSignal::new(Shown::Loading);
    // The query `gifs` answers, and the page they reach.
    let query = StoredValue::new(String::new());
    let page = StoredValue::new(1u32);
    // A new query makes answers still on their way for an older one moot,
    // and gives its tiles new keys.
    let revision = RwSignal::new(0u64);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if let Some(input) = input.get() {
            let _ = input.focus();
        }
    });
    let load = Callback::new(move |next: u32| {
        let requested = revision.get_untracked();
        let url = gifs_url(&key, &query.get_value(), next);
        spawn_local(async move {
            let found = klipy(&url).await;
            if revision.try_get_untracked() != Some(requested) {
                return;
            }
            match found {
                Ok(found) => {
                    let (more, more_after) = (found.0.clone(), found.1);
                    gifs.update(|g| {
                        if next == 1 {
                            *g = more;
                        } else {
                            g.extend(more);
                        }
                    });
                    page.set_value(next);
                    shown.set(Shown::Ready { more: more_after });
                }
                // A later page that fails leaves what is shown, and its button.
                Err(_) if next > 1 => shown.set(Shown::Ready { more: true }),
                Err(why) => shown.set(Shown::Failed(why)),
            }
        });
    });
    let search = move |q: String| {
        query.set_value(q);
        revision.update(|r| *r = r.wrapping_add(1));
        shown.set(Shown::Loading);
        load.run(1);
    };
    // What's trending, at once.
    load.run(1);
    let on_input = move |ev: ev::Event| {
        if let Some(handle) = timer.try_update_value(Option::take).flatten() {
            handle.clear();
        }
        let q = event_target_value(&ev).trim().to_owned();
        let n = q.chars().count();
        if q == query.get_value() || (n > 0 && n < MIN_QUERY) {
            return;
        }
        timer.set_value(set_timeout_with_handle(move || search(q), DEBOUNCE).ok());
    };
    on_cleanup(move || {
        if let Some(Some(handle)) = timer.try_get_value() {
            handle.clear();
        }
    });
    let heading = move || {
        let q = query.get_value();
        let _ = revision.get();
        if q.is_empty() { "Trending".to_owned() } else { format!("GIFs for “{q}”") }
    };
    view! {
        <section class="gif-picker" id="gif-picker" aria-label="GIFs from KLIPY">
            <input
                class="gif-search"
                type="search"
                node_ref=input
                placeholder=SEARCH_PLACEHOLDER
                aria-label="Search GIFs"
                autocomplete="off"
                enterkeyhint="search"
                on:input=on_input
            />
            <p class="gif-heading muted small" aria-live="polite">{heading}</p>
            <div class="gif-results">
                {move || match shown.get() {
                    Shown::Loading => view! {
                        <ul class="gif-grid" aria-busy="true">
                            {(0..9).map(|_| view! { <li class="gif-tile placeholder"></li> }).collect_view()}
                        </ul>
                    }
                    .into_any(),
                    Shown::Failed(why) => view! {
                        <p class="gif-note">
                            {match why {
                                Unread::Busy => "Too many GIF searches on Allthing just now. ",
                                Unread::Failed => "GIFs couldn't load just now. ",
                            }}
                            <button type="button" class="link-button" on:click=move |_| search(query.get_value())>"Try again"</button>
                        </p>
                    }
                    .into_any(),
                    Shown::Ready { .. } if gifs.with(Vec::is_empty) => view! { <p class="gif-note">"No GIFs found."</p> }.into_any(),
                    Shown::Ready { more } => view! {
                        <ul class="gif-grid">
                            <For
                                each=move || {
                                    let r = revision.get();
                                    gifs.get().into_iter().enumerate().map(move |(i, gif)| (r, i, gif)).collect::<Vec<_>>()
                                }
                                key=|(r, i, _)| (*r, *i)
                                children=move |(_, _, gif)| view! { <GifTile gif picked/> }
                            />
                        </ul>
                        {more.then(|| view! {
                            <button type="button" class="button gif-more" on:click=move |_| load.run(page.get_value() + 1)>"More GIFs"</button>
                        })}
                    }
                    .into_any(),
                }}
            </div>
            <p class="gif-credit">"Powered by KLIPY"</p>
        </section>
    }
}

#[component]
fn GifTile(gif: Gif, picked: Callback<Gif>) -> impl IntoView {
    let (src, width, height, title) = (gif.tile.url.clone(), gif.tile.width, gif.tile.height, gif.title.clone());
    view! {
        <li>
            <button type="button" class="gif-tile" title=title.clone() aria-label=title on:click=move |_| picked.run(gif.clone())>
                <img src=src width=width height=height alt="" loading="lazy" decoding="async"/>
            </button>
        </li>
    }
}

/// A page of KLIPY's results, asked for from the browser, as KLIPY
/// requires, and kept for this visit.
async fn klipy(url: &str) -> Result<Found, Unread> {
    let now = js_sys::Date::now();
    let kept = PAGES.with(|p| p.borrow().get(url).filter(|(at, _)| now - at < KEEP_MS).map(|(_, page)| page.clone()));
    if let Some(page) = kept {
        return Ok(page);
    }
    let failed = |_| Unread::Failed;
    let window = web_sys::window().ok_or(Unread::Failed)?;
    let response: web_sys::Response =
        JsFuture::from(window.fetch_with_str(url)).await.map_err(failed)?.dyn_into().map_err(failed)?;
    match response.status() {
        200 => {}
        429 => return Err(Unread::Busy),
        _ => return Err(Unread::Failed),
    }
    let text = JsFuture::from(response.text().map_err(failed)?).await.map_err(failed)?.as_string();
    let page: GifPage = text.and_then(|text| serde_json::from_str(&text).ok()).ok_or(Unread::Failed)?;
    if !page.result {
        return Err(Unread::Failed);
    }
    let page = Rc::new(page.gifs());
    PAGES.with(|p| {
        let mut p = p.borrow_mut();
        p.retain(|_, (at, _)| now - *at < KEEP_MS);
        p.insert(url.to_owned(), (now, page.clone()));
    });
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cast_from_a_channel_page_goes_to_that_channel() {
        assert_eq!(channel_in_view("/c/dev"), Some("dev".to_owned()));
        assert_eq!(channel_in_view("/c/Dev/"), Some("dev".to_owned()));
        assert_eq!(channel_in_view("/"), None);
        assert_eq!(channel_in_view("/channels"), None);
        assert_eq!(channel_in_view("/c/"), None);
    }
}
