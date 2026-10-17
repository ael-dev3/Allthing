// SPDX-License-Identifier: AGPL-3.0-or-later
//! Images and videos attached to casts: an inline grid, a hover preview that
//! needs no click, and an accessible lightbox (keyboard, alt text, swipe).

use std::cell::RefCell;
use std::time::Duration;

use allthing_core::embeds::{ImageSize, embed_domain, resized_image_url};
use allthing_core::model::{Cast, Embed};
use allthing_core::text::truncate_title;
use allthing_core::video::{self, VideoLink};
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlElement;

use crate::components::player::{HlsVideo, VideoFacade};
use crate::util::{can_hover, cast_href};

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
    /// The cast's text, shown under the media.
    pub caption: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub src: String,
    pub placeholder: String,
    pub alt: String,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// Shared state of the lightbox and the hover preview.
#[derive(Clone, Copy)]
pub struct Viewer {
    pub gallery: RwSignal<Option<Gallery>>,
    pub preview: RwSignal<Option<Preview>>,
}

thread_local! {
    static RETURN_FOCUS: RefCell<Option<HtmlElement>> = const { RefCell::new(None) };
    static HOVER_TIMER: RefCell<Option<TimeoutHandle>> = const { RefCell::new(None) };
}

impl Viewer {
    pub fn new() -> Viewer {
        Viewer { gallery: RwSignal::new(None), preview: RwSignal::new(None) }
    }

    pub fn open(&self, gallery: Gallery) {
        let active = document().active_element().and_then(|e| e.dyn_into::<HtmlElement>().ok());
        RETURN_FOCUS.with(|r| *r.borrow_mut() = active);
        self.preview.set(None);
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

fn clear_hover() {
    HOVER_TIMER.with(|t| {
        if let Some(handle) = t.borrow_mut().take() {
            handle.clear();
        }
    });
}

/// Show an enlarged preview next to `anchor` after a short dwell.
fn schedule_preview(viewer: Viewer, anchor: HtmlElement, item: MediaItem) {
    clear_hover();
    if !can_hover() {
        return;
    }
    let handle = set_timeout_with_handle(
        move || {
            let rect = anchor.get_bounding_client_rect();
            let window = window();
            let vw = window.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1280.0);
            let vh = window.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(800.0);
            // Use the loaded thumbnail's shape, or fall back to the tile's.
            let (nw, nh) = anchor
                .query_selector("img")
                .ok()
                .flatten()
                .and_then(|img| img.dyn_into::<web_sys::HtmlImageElement>().ok())
                .map(|img| (f64::from(img.natural_width()), f64::from(img.natural_height())))
                .filter(|(w, h)| *w > 0.0 && *h > 0.0)
                .unwrap_or((rect.width(), rect.height()));
            let (max_w, max_h) = ((vw * 0.42).min(620.0), (vh * 0.72).min(660.0));
            let scale = (max_w / nw).min(max_h / nh);
            let (width, height) = (nw * scale, nh * scale);
            let right_room = vw - rect.right() - 18.0;
            let left = if right_room >= width { rect.right() + 14.0 } else { (rect.left() - 14.0 - width).max(8.0) };
            let top = (rect.top() + rect.height() / 2.0 - height / 2.0).clamp(8.0, (vh - height - 8.0).max(8.0));
            viewer.preview.set(Some(Preview {
                src: resized_image_url(&item.url, ImageSize::Preview),
                placeholder: resized_image_url(&item.url, ImageSize::Inline),
                alt: item.alt.clone(),
                left,
                top,
                width,
                height,
            }));
        },
        Duration::from_millis(140),
    );
    if let Ok(handle) = handle {
        HOVER_TIMER.with(|t| *t.borrow_mut() = Some(handle));
    }
}

fn hide_preview(viewer: Viewer) {
    clear_hover();
    viewer.preview.set(None);
}

/// A cast's images and videos. In listings, images link to the post; on the
/// post page (`open_on_click`), they open the lightbox.
#[component]
pub fn MediaGrid(cast: Cast, #[prop(optional)] open_on_click: bool) -> impl IntoView {
    let viewer = use_viewer();
    let items = media_items(&cast);
    if items.is_empty() {
        return ().into_any();
    }
    let gallery = Gallery { items: items.clone(), index: 0, caption: truncate_title(&cast.text, 280) };
    let href = cast_href(&cast.hash);
    let count = items.len();
    let tiles = items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let open = {
                let gallery = gallery.clone();
                move || viewer.open(Gallery { index: i, ..gallery.clone() })
            };
            match item.kind {
                MediaKind::Video => {
                    let expand = open_on_click.then(|| Callback::new(move |()| open()));
                    view! {
                        <div class="tile tile-video">
                            <HlsVideo url=item.url.clone() alt=item.alt.clone() on_expand=expand/>
                        </div>
                    }
                    .into_any()
                }
                MediaKind::Image => {
                    let img = view! {
                        <img
                            src=resized_image_url(&item.url, ImageSize::Inline)
                            alt=item.alt.clone()
                            loading="lazy"
                            decoding="async"
                            referrerpolicy="no-referrer"
                        />
                    };
                    let hover_item = item.clone();
                    let enter = move |ev: ev::MouseEvent| {
                        if let Some(anchor) = ev.current_target().and_then(|t| t.dyn_into::<HtmlElement>().ok()) {
                            schedule_preview(viewer, anchor, hover_item.clone());
                        }
                    };
                    let leave = move |_: ev::MouseEvent| hide_preview(viewer);
                    if open_on_click {
                        let label = format!("Open {} in the media viewer", item.alt);
                        view! {
                            <button class="tile tile-image" type="button" aria-label=label
                                on:mouseenter=enter on:mouseleave=leave
                                on:click=move |_| { hide_preview(viewer); open() }>
                                {img}
                            </button>
                        }
                        .into_any()
                    } else {
                        view! {
                            <a class="tile tile-image" href=href.clone() on:mouseenter=enter on:mouseleave=leave
                                on:click=move |_| hide_preview(viewer)>
                                {img}
                            </a>
                        }
                        .into_any()
                    }
                }
            }
        })
        .collect_view();
    view! { <div class=format!("media-grid n{}", count.min(4))>{tiles}</div> }.into_any()
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

/// The floating, no-click preview of a hovered image.
#[component]
pub fn HoverPreview() -> impl IntoView {
    let viewer = use_viewer();
    // Scrolling moves the anchor away; drop the preview.
    let handle = window_event_listener(ev::scroll, move |_| {
        if viewer.preview.with_untracked(Option::is_some) {
            hide_preview(viewer);
        }
    });
    on_cleanup(move || handle.remove());
    move || {
        viewer.preview.get().map(|p| {
            view! {
                <div
                    class="hover-preview"
                    aria-hidden="true"
                    style:left=format!("{:.0}px", p.left)
                    style:top=format!("{:.0}px", p.top)
                    style:width=format!("{:.0}px", p.width)
                    style:height=format!("{:.0}px", p.height)
                    style:background-image=format!("url(\"{}\")", p.placeholder)
                >
                    <img src=p.src alt=p.alt referrerpolicy="no-referrer"/>
                </div>
            }
        })
    }
}

/// Modal media viewer: arrow keys and swipes move between items, Escape or
/// a swipe down closes it, and focus stays inside while it is open.
#[component]
pub fn Lightbox() -> impl IntoView {
    let viewer = use_viewer();
    let dialog = NodeRef::<leptos::html::Div>::new();
    let close_button = NodeRef::<leptos::html::Button>::new();
    let start = StoredValue::new(None::<(f64, f64)>);

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
    on_cleanup(move || keys.remove());

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
                    <figcaption class="lb-caption">
                        <span class="lb-alt">{item.alt.clone()}</span>
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
