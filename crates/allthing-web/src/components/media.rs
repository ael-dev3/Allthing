// SPDX-License-Identifier: AGPL-3.0-or-later
//! Images and videos attached to casts: an inline grid and an accessible
//! lightbox (keyboard, alt text, swipe).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};

use allthing_core::embeds::{ImageSize, embed_domain, inline_size_url, is_gif, resized_image_url};
use allthing_core::emoji::{self, STICKER_HEIGHT, Sticker};
use allthing_core::model::{Cast, Embed};
use allthing_core::text::truncate_title;
use allthing_core::video::{self, VideoLink};
use leptos::ev;
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use wasm_bindgen::JsCast;
use web_sys::HtmlElement;

use crate::components::player::{HlsVideo, VideoFacade};
use crate::components::text::ExcerptText;
use crate::util::href;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaItem {
    pub kind: MediaKind,
    pub url: String,
    pub alt: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gallery {
    pub items: Vec<MediaItem>,
    pub index: usize,
    /// Who posted the cast, by username: the caption's first words.
    pub author: String,
    /// The cast's words, cut short: the rest of the caption. Each item's
    /// alt text stays on the item, for screen readers.
    pub caption: String,
}

/// How much of a cast's words the media viewer's caption shows, at most:
/// about two lines on a phone (style/allthing.css cuts it there).
const CAPTION_CHARS: usize = 140;

/// Shared state of the lightbox.
#[derive(Clone, Copy)]
pub struct Viewer {
    pub gallery: RwSignal<Option<Gallery>>,
}

thread_local! {
    static RETURN_FOCUS: RefCell<Option<HtmlElement>> = const { RefCell::new(None) };
    /// The sizes of the images seen so far (loaded, or described by
    /// Cloudflare Images), so each tile is laid out at its size before its
    /// image comes, instead of at none and then pushing the page down.
    static IMAGE_SIZES: RefCell<HashMap<String, (u32, u32)>> = RefCell::new(HashMap::new());
    /// The images whose size was asked for: since when, until it came, or
    /// it was learned that it won't.
    static ASKED: RefCell<HashMap<String, Asked>> = RefCell::new(HashMap::new());
    /// Size requests waiting for one of [`SIZE_REQUESTS`] places, in order.
    static QUEUED: RefCell<VecDeque<(String, Probe)>> = const { RefCell::new(VecDeque::new()) };
    static ON_THEIR_WAY: Cell<usize> = const { Cell::new(0) };
    /// Images a card about to show has started loading, the latest ones
    /// held until they have (see [`warm`]).
    static WARMING: RefCell<(HashSet<String>, VecDeque<web_sys::HtmlImageElement>)> =
        RefCell::new((HashSet::new(), VecDeque::new()));
    /// Notified when a size comes, or when waiting for one is over.
    static SIZES: ArcTrigger = ArcTrigger::new();
}

/// How long a list's next card, or the top of a post, stays out of sight
/// waiting for the size of an image it shows alone: drawn without it, the
/// image would grow its box as it came and push everything below it down.
/// A card that waits longer shows with a square box for its image.
const SIZE_WAIT_MS: f64 = 1000.0;
/// Size requests on their way at once.
const SIZE_REQUESTS: usize = 6;

#[derive(Clone, Copy)]
enum Asked {
    Since(f64),
    Over,
}

enum Probe {
    /// Cloudflare Images' description of the image, at this URL.
    Described(String),
    /// The image itself, until its size is known.
    Loaded,
}

/// The size `src` was seen at: here, or by the front page's first cards
/// drawn before the app came (web/paint.js, `__allthingSizes`).
fn known_size(src: &str) -> Option<(u32, u32)> {
    IMAGE_SIZES.with(|sizes| sizes.borrow().get(src).copied()).or_else(|| painted_size(src))
}

fn painted_size(src: &str) -> Option<(u32, u32)> {
    use wasm_bindgen::JsValue;
    let sizes = js_sys::Reflect::get(&window(), &JsValue::from_str("__allthingSizes")).ok()?;
    let size = js_sys::Reflect::get(&sizes, &JsValue::from_str(src)).ok()?.dyn_into::<js_sys::Array>().ok()?;
    let (width, height) = (size.get(0).as_f64()? as u32, size.get(1).as_f64()? as u32);
    (width > 0 && height > 0).then_some((width, height))
}

/// Tiles take sizes learned elsewhere: by the boot screen's cards, whose
/// images have come since the app drew its own (`crate::boot::hand_over`).
pub fn sizes_changed() {
    SIZES.with(ArcTrigger::notify);
}

fn keep_size(src: &str, size: (u32, u32)) {
    IMAGE_SIZES.with(|sizes| {
        let mut sizes = sizes.borrow_mut();
        if sizes.len() > 4000 {
            sizes.clear();
        }
        sizes.insert(src.to_owned(), size);
    });
    sizes_changed();
}

fn remember_size(src: &str, event: &ev::Event) {
    let Some(img) = event.target().and_then(|t| t.dyn_into::<web_sys::HtmlImageElement>().ok()) else { return };
    let size = (img.natural_width(), img.natural_height());
    if size.0 > 0 && size.1 > 0 && known_size(src) != Some(size) {
        // Its tile, and any later one of the same image, take this box.
        keep_size(src, size);
    }
}

/// Find out the size of the image at `url` (an embed's) as a tile shows it,
/// unless it is known or on its way: from Cloudflare Images' description of
/// it, about 110 bytes, or, `eager`ly, for an image hosted elsewhere, from
/// the image itself, whose size is known from its first bytes (what's in
/// view loads it anyway). Each image is asked for once a visit, and
/// [`SIZE_REQUESTS`] at a time; none of this reaches Allthing's API.
pub fn size_up(url: &str, eager: bool) {
    let src = resized_image_url(url, ImageSize::Inline);
    if ASKED.with(|asked| asked.borrow().contains_key(&src)) || known_size(&src).is_some() {
        return;
    }
    let probe = match inline_size_url(url) {
        Some(description) => Probe::Described(description),
        None if eager => Probe::Loaded,
        None => return,
    };
    ASKED.with(|asked| {
        let mut asked = asked.borrow_mut();
        if asked.len() > 4000 {
            asked.retain(|_, a| matches!(a, Asked::Since(_)));
        }
        asked.insert(src.clone(), Asked::Since(js_sys::Date::now()));
    });
    QUEUED.with(|queued| queued.borrow_mut().push_back((src, probe)));
    // Whatever comes of it, waiting for it ends then.
    set_timeout(sizes_changed, std::time::Duration::from_millis(SIZE_WAIT_MS as u64 + 1));
    next_size();
}

fn next_size() {
    while ON_THEIR_WAY.with(Cell::get) < SIZE_REQUESTS {
        let Some((src, probe)) = QUEUED.with(|queued| queued.borrow_mut().pop_front()) else { return };
        ON_THEIR_WAY.with(|n| n.set(n.get() + 1));
        wasm_bindgen_futures::spawn_local(async move {
            let size = match probe {
                Probe::Described(url) => described(&url).await,
                Probe::Loaded => loaded(&src).await,
            };
            ON_THEIR_WAY.with(|n| n.set(n.get().saturating_sub(1)));
            ASKED.with(|asked| asked.borrow_mut().insert(src.clone(), Asked::Over));
            match size {
                Some(size) if known_size(&src).is_none() => keep_size(&src, size),
                _ => sizes_changed(),
            }
            next_size();
        });
    }
}

/// The width and height Cloudflare Images gives in its description at `url`.
async fn described(url: &str) -> Option<(u32, u32)> {
    use wasm_bindgen_futures::JsFuture;
    let response: web_sys::Response = JsFuture::from(window().fetch_with_str(url)).await.ok()?.dyn_into().ok()?;
    if !response.ok() {
        return None;
    }
    let found = JsFuture::from(response.json().ok()?).await.ok()?;
    let dimension = |name: &str| {
        js_sys::Reflect::get(&found, &wasm_bindgen::JsValue::from_str(name)).ok()?.as_f64().filter(|n| *n >= 1.0)
    };
    Some((dimension("width")? as u32, dimension("height")? as u32))
}

/// The size of the image at `src`, as soon as its first bytes tell it:
/// looked at every 50 ms while it loads, for as long as a card waits.
async fn loaded(src: &str) -> Option<(u32, u32)> {
    let image = web_sys::HtmlImageElement::new().ok()?;
    // Asked for as the tile asks for it, so the tile's is the same request.
    image.set_referrer_policy("no-referrer");
    image.set_src(src);
    let started = js_sys::Date::now();
    loop {
        let size = (image.natural_width(), image.natural_height());
        if size.0 > 0 && size.1 > 0 {
            return Some(size);
        }
        if image.complete() || js_sys::Date::now() - started > SIZE_WAIT_MS {
            return None;
        }
        let pause = js_sys::Promise::new(&mut |resolve, _| {
            let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 50);
        });
        let _ = wasm_bindgen_futures::JsFuture::from(pause).await;
    }
}

/// Whether the size of an image at `url` is on its way, and worth waiting
/// for still. Reactive: it changes as sizes come, and as waits end.
fn awaited(url: &str) -> bool {
    SIZES.with(|sizes| sizes.track());
    let src = resized_image_url(url, ImageSize::Inline);
    let since = ASKED.with(|asked| match asked.borrow().get(&src) {
        Some(Asked::Since(since)) => Some(*since),
        _ => None,
    });
    since.is_some_and(|since| js_sys::Date::now() - since < SIZE_WAIT_MS) && known_size(&src).is_none()
}

/// Start loading the image at `src`, which a card about to show waits to
/// size: so it comes as soon as it would have without the wait, and the
/// tile's own request is this one.
fn warm(src: &str) {
    let fresh = WARMING.with(|warming| {
        let (seen, _) = &mut *warming.borrow_mut();
        if seen.len() > 4000 {
            seen.clear();
        }
        seen.insert(src.to_owned())
    });
    if !fresh {
        return;
    }
    let Ok(image) = web_sys::HtmlImageElement::new() else { return };
    // Asked for as the tile asks for it, so the tile's is the same request.
    image.set_referrer_policy("no-referrer");
    image.set_src(src);
    WARMING.with(|warming| {
        let (_, held) = &mut *warming.borrow_mut();
        if held.len() >= 24 {
            held.pop_front();
        }
        held.push_back(image);
    });
}

/// Whether a card or a page showing the images at `urls` alone should
/// wait for their sizes before it shows, having asked for them (see
/// [`size_up`]) and started loading those that are on their way.
/// Reactive: it turns false once each has come, or the wait for it is over.
pub fn sizing<'a>(urls: impl IntoIterator<Item = &'a str>, eager: bool) -> bool {
    let urls: Vec<&str> = urls.into_iter().collect();
    let mut waiting = false;
    for url in &urls {
        size_up(url, eager);
        if awaited(url) {
            waiting = true;
            // One hosted elsewhere is loading already (`Probe::Loaded`).
            if inline_size_url(url).is_some() {
                warm(&resized_image_url(url, ImageSize::Inline));
            }
        }
    }
    waiting
}

/// The CSS variables a sized media box is laid out with.
pub fn size_style(width: u32, height: u32) -> String {
    format!("--w: {width}; --h: {height}")
}

impl Viewer {
    pub fn new() -> Viewer {
        Viewer { gallery: RwSignal::new(None) }
    }

    pub fn open(&self, gallery: Gallery) {
        let active = document().active_element().and_then(|e| e.dyn_into::<HtmlElement>().ok());
        RETURN_FOCUS.with(|r| *r.borrow_mut() = active);
        self.gallery.set(Some(gallery));
    }

    pub fn close(&self) {
        self.gallery.set(None);
        if let Some(element) = RETURN_FOCUS.with(|r| r.borrow_mut().take()) {
            let _ = element.focus();
        }
    }

    pub fn step(&self, delta: i32) {
        self.gallery.update(|g| {
            if let Some(g) = g {
                let n = g.items.len() as i32;
                g.index = ((g.index as i32 + delta).rem_euclid(n.max(1))) as usize;
            }
        });
    }
}

pub fn use_viewer() -> Viewer {
    expect_context::<Viewer>()
}

/// The images and videos of a cast, with descriptive alt text (Farcaster
/// embeds carry none of their own).
pub fn media_items(cast: &Cast) -> Vec<MediaItem> {
    let media: Vec<&Embed> = cast.media().collect();
    let total = media.len();
    media
        .into_iter()
        .enumerate()
        .map(|(i, embed)| {
            let (kind, noun) = match embed {
                Embed::Video { .. } => (MediaKind::Video, "Video"),
                _ => (MediaKind::Image, "Image"),
            };
            let position = if total > 1 { format!(" {} of {total}", i + 1) } else { String::new() };
            let excerpt = truncate_title(&cast.text, 90);
            let alt = if excerpt.is_empty() {
                format!("{noun}{position} posted by @{}", cast.author.username)
            } else {
                format!("{noun}{position} posted by @{}: {excerpt}", cast.author.username)
            };
            MediaItem { kind, url: embed.url().unwrap_or_default().to_owned(), alt }
        })
        .collect()
}

/// Video pages linked from a cast (embeds or URLs in its text), deduplicated.
pub fn video_links(cast: &Cast) -> Vec<(String, VideoLink)> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    let embed_urls = cast.embeds.iter().filter_map(|e| match e {
        Embed::Link { url, .. } => Some(url.clone()),
        _ => None,
    });
    let text_urls = allthing_core::text::tokenize(&cast.text, &[]).into_iter().filter_map(|t| match t {
        allthing_core::text::Token::Url(url) => Some(url),
        _ => None,
    });
    for url in embed_urls.chain(text_urls) {
        if let Some(link) = video::detect(&url)
            && !seen.contains(&link.key())
        {
            seen.push(link.key());
            out.push((url, link));
        }
    }
    out
}

/// One of Allthing's stickers in a cast: large, with no frame, and still
/// for readers who ask for less motion. It is the cast's image embed, which
/// other Farcaster apps show as the GIF it is.
fn sticker_view(sticker: &'static Sticker) -> impl IntoView {
    view! {
        <picture class="sticker">
            <source media="(prefers-reduced-motion: reduce)" srcset=href(&sticker.still())/>
            <img
                src=href(&sticker.file())
                alt=format!("Sticker: {}", sticker.label())
                width=sticker.width
                height=STICKER_HEIGHT
                loading="lazy"
                decoding="async"
            />
        </picture>
    }
}

/// A cast's images and videos. A click or a tap on an image opens it in the
/// lightbox at full size, in a list, where media is smaller (at most 270 px
/// tall, half the window on a phone: style/allthing.css, `--feed-media`), as
/// on the post page, and a video has a button that opens it there. Allthing's
/// stickers show apart, as stickers. A GIF's tile is marked `gif`
/// (`embeds::is_gif`; payloads carry no content type), so a GIF alone shows
/// at about the size chat apps give one, not a photo's. web/paint.js draws
/// the front page's first cards' tiles the same way.
#[component]
pub fn MediaGrid(cast: Cast) -> impl IntoView {
    let viewer = use_viewer();
    let (stickers, items): (Vec<MediaItem>, Vec<MediaItem>) =
        media_items(&cast).into_iter().partition(|item| emoji::sticker_of(&item.url).is_some());
    let stickers = stickers.iter().filter_map(|item| emoji::sticker_of(&item.url)).map(sticker_view).collect_view();
    if items.is_empty() {
        return stickers.into_any();
    }
    let gallery = Gallery {
        items: items.clone(),
        index: 0,
        author: cast.author.username.clone(),
        caption: truncate_title(&cast.text, CAPTION_CHARS),
    };
    let count = items.len();
    let tiles = items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let open = {
                let gallery = gallery.clone();
                move || viewer.open(Gallery { index: i, ..gallery.clone() })
            };
            let gif = is_gif(&item.url, None);
            match item.kind {
                MediaKind::Video => {
                    let expand = Some(Callback::new(move |()| open()));
                    // A GIF can come as a video, as Tenor's MP4s do.
                    let class = if gif { "tile tile-video gif" } else { "tile tile-video" };
                    view! {
                        <div class=class>
                            <HlsVideo url=item.url.clone() alt=item.alt.clone() on_expand=expand/>
                        </div>
                    }
                    .into_any()
                }
                MediaKind::Image => {
                    let src = resized_image_url(&item.url, ImageSize::Inline);
                    // An image alone takes its own shape: its size is asked
                    // for at once, and sizes the tile as soon as it comes.
                    if count == 1 {
                        size_up(&item.url, false);
                    }
                    let size = {
                        let src = src.clone();
                        Memo::new(move |_| {
                            SIZES.with(|sizes| sizes.track());
                            known_size(&src)
                        })
                    };
                    let key = src.clone();
                    let broken = RwSignal::new(false);
                    let style = move || size.get().map(|(w, h)| size_style(w, h));
                    let sized = move || size.get().map(|_| "");
                    let portrait = move || size.get().and_then(|(w, h)| (h > w).then_some(""));
                    // An image alone that can't load, and has no size: its
                    // alt text shows at its own size, not in a square.
                    let unseen = move || (count == 1 && broken.get() && size.get().is_none()).then_some("");
                    let img = view! {
                        <img
                            src=src
                            width=move || size.get().map(|(w, _)| w)
                            height=move || size.get().map(|(_, h)| h)
                            on:load=move |event| remember_size(&key, &event)
                            on:error=move |_| broken.set(true)
                            alt=item.alt.clone()
                            loading="lazy"
                            decoding="async"
                            referrerpolicy="no-referrer"
                        />
                    };
                    let class = if gif { "tile tile-image gif" } else { "tile tile-image" };
                    let label = format!("Open {} in the media viewer", item.alt);
                    view! {
                        <button
                            class=class
                            type="button"
                            aria-label=label
                            style=style
                            data-sized=sized
                            data-portrait=portrait
                            data-broken=unseen
                            on:click=move |_| open()
                        >
                            {img}
                        </button>
                    }
                    .into_any()
                }
            }
        })
        .collect_view();
    view! {
        {stickers}
        <div class=format!("media-grid n{}", count.min(4))>{tiles}</div>
    }
    .into_any()
}

/// Links (other than playable videos) under a cast.
#[component]
pub fn LinkCards(cast: Cast) -> impl IntoView {
    let videos: Vec<String> = video_links(&cast).into_iter().map(|(url, _)| url).collect();
    cast.embeds
        .iter()
        .filter_map(|e| match e {
            Embed::Link { url, image_url, title } if !videos.contains(url) && !cast.text.contains(url.as_str()) => {
                Some((url.clone(), image_url.clone(), title.clone()))
            }
            _ => None,
        })
        .map(|(url, image, title)| {
            let domain = embed_domain(&url);
            view! {
                <a class="link-card" href=url.clone() target="_blank" rel="noopener noreferrer nofollow ugc">
                    {image.map(|src| view! { <img src=resized_image_url(&src, ImageSize::Thumb) alt="" loading="lazy" referrerpolicy="no-referrer"/> })}
                    <span class="link-text">
                        <span class="link-title">{title.unwrap_or_else(|| url.clone())}</span>
                        <span class="link-domain">{domain}</span>
                    </span>
                </a>
            }
        })
        .collect_view()
}

/// Click-to-load players for YouTube, Vimeo, Loom and Streamable links.
#[component]
pub fn VideoLinks(cast: Cast) -> impl IntoView {
    let title = truncate_title(&cast.text, 80);
    video_links(&cast)
        .into_iter()
        .take(2)
        .map(|(url, link)| view! { <VideoFacade link url title=title.clone()/> })
        .collect_view()
}

/// Modal media viewer: arrow keys and swipes move between items, Escape or
/// a swipe down closes it, and focus stays inside while it is open.
#[component]
pub fn Lightbox() -> impl IntoView {
    let viewer = use_viewer();
    let location = use_location();
    let dialog = NodeRef::<leptos::html::Div>::new();
    let close_button = NodeRef::<leptos::html::Button>::new();
    let start = StoredValue::new(None::<(f64, f64)>);

    // History navigation leaves the post without clicking the viewer's
    // close button. Dismiss it without focusing an opener on the old page.
    Effect::new(move |_| {
        let _ = (location.pathname.get(), location.search.get());
        viewer.gallery.set(None);
        RETURN_FOCUS.with(|r| r.borrow_mut().take());
    });

    let keys = window_event_listener(ev::keydown, move |ev| {
        if viewer.gallery.with_untracked(Option::is_none) {
            return;
        }
        match ev.key().as_str() {
            "Escape" => {
                ev.prevent_default();
                viewer.close();
            }
            "ArrowLeft" => {
                ev.prevent_default();
                viewer.step(-1);
            }
            "ArrowRight" => {
                ev.prevent_default();
                viewer.step(1);
            }
            "Tab" => {
                // Keep focus inside the dialog.
                let Some(root) = dialog.get_untracked() else { return };
                let Ok(nodes) = root.query_selector_all("button, video, iframe, a[href]") else { return };
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
    on_cleanup(move || {
        keys.remove();
        RETURN_FOCUS.with(|r| r.borrow_mut().take());
        if let Some(root) = document().document_element() {
            let _ = root.class_list().remove_1("lightbox-open");
        }
    });

    // Lock page scroll and move focus in when opening.
    Effect::new(move |_| {
        let open = viewer.gallery.with(Option::is_some);
        if let Some(root) = document().document_element() {
            let _ = root.class_list().toggle_with_force("lightbox-open", open);
        }
        if open && let Some(button) = close_button.get() {
            let _ = button.focus();
        }
    });

    let on_pointer_down =
        move |ev: ev::PointerEvent| start.set_value(Some((f64::from(ev.client_x()), f64::from(ev.client_y()))));
    let on_pointer_up = move |ev: ev::PointerEvent| {
        let Some((x, y)) = start.get_value() else { return };
        start.set_value(None);
        let (dx, dy) = (f64::from(ev.client_x()) - x, f64::from(ev.client_y()) - y);
        if dx.abs() > 50.0 && dx.abs() > dy.abs() * 1.4 {
            viewer.step(if dx < 0.0 { 1 } else { -1 });
        } else if dy > 90.0 && dy > dx.abs() * 1.4 {
            viewer.close();
        }
    };

    move || {
        let gallery = viewer.gallery.get()?;
        let total = gallery.items.len();
        let item = gallery.items.get(gallery.index)?.clone();
        let counter = (total > 1).then(|| format!("{} of {total}", gallery.index + 1));
        let stage = match item.kind {
            MediaKind::Image => view! {
                <img
                    class="lb-media"
                    src=resized_image_url(&item.url, ImageSize::Full)
                    alt=item.alt.clone()
                    referrerpolicy="no-referrer"
                    draggable="false"
                />
            }
            .into_any(),
            MediaKind::Video => view! {
                <div class="lb-media lb-video"><HlsVideo url=item.url.clone() alt=item.alt.clone() full=true/></div>
            }
            .into_any(),
        };
        Some(view! {
            <div
                class="lightbox"
                role="dialog"
                aria-modal="true"
                aria-label="Media viewer"
                node_ref=dialog
                on:pointerdown=on_pointer_down
                on:pointerup=on_pointer_up
            >
                <div class="lb-backdrop" on:click=move |_| viewer.close()></div>
                <figure class="lb-figure">
                    <div class="lb-stage" data-index=gallery.index>{stage}</div>
                    // Who posted it, and the cast's words: the item's alt text
                    // ("Image 1 of 2 posted by…") is on the item itself, and the
                    // counter beside says which of them this is.
                    <figcaption class="lb-caption">
                        <span class="lb-text">
                            <span class="lb-author">"@"{gallery.author.clone()}</span>
                            {(!gallery.caption.is_empty()).then(|| view! {
                                <span class="lb-sep" aria-hidden="true">" · "</span>
                                <ExcerptText text=gallery.caption.clone()/>
                            })}
                        </span>
                        {counter.map(|c| view! { <span class="lb-counter" aria-live="polite">{c}</span> })}
                    </figcaption>
                </figure>
                <button class="lb-close" type="button" aria-label="Close media viewer (Escape)" node_ref=close_button
                    on:click=move |_| viewer.close()>
                    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 3.5l9 9M12.5 3.5l-9 9"/></svg>
                </button>
                {(total > 1).then(|| view! {
                    <button class="lb-prev" type="button" aria-label="Previous (Left arrow)" on:click=move |_| viewer.step(-1)>
                        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M10 3L5 8l5 5"/></svg>
                    </button>
                    <button class="lb-next" type="button" aria-label="Next (Right arrow)" on:click=move |_| viewer.step(1)>
                        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 3l5 5-5 5"/></svg>
                    </button>
                })}
            </div>
        })
    }
}
