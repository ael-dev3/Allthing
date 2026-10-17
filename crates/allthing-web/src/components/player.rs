// SPDX-License-Identifier: AGPL-3.0-or-later
//! Video components: Farcaster HLS videos and plain video files played
//! inline, and click-to-load facades for YouTube, Vimeo, Loom and Streamable
//! links.

use std::cell::RefCell;
use std::rc::Rc;

use allthing_core::embeds::{host_of, is_hls};
use allthing_core::video::VideoLink;
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use web_sys::{HtmlVideoElement, IntersectionObserver, IntersectionObserverEntry, IntersectionObserverInit};

use crate::util::can_hover;
use crate::video::{MsePlayer, Stream, attach, load_stream, mse_supported, native_hls};

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Idle,
    Loading,
    Ready,
    Failed(String),
}

/// Rendition heights: hover previews stay light; playing inline and in the
/// lightbox ask for more.
const PREVIEW_HEIGHT: u32 = 480;
const INLINE_HEIGHT: u32 = 720;
const FULL_HEIGHT: u32 = 1080;

type ObserverCallback = Closure<dyn FnMut(js_sys::Array)>;

struct Playback {
    player: Option<MsePlayer>,
    stream: Option<Rc<Stream>>,
    observer: Option<(IntersectionObserver, ObserverCallback)>,
}

/// Make sure `video` has a source of at least `height`, keeping its position.
fn ensure_source(
    video: &HtmlVideoElement,
    url: &str,
    playback: &Rc<RefCell<Playback>>,
    height: u32,
) -> Result<(), String> {
    // Plain video files, and HLS in Safari: the browser plays the URL itself.
    if !is_hls(url) || native_hls(video) {
        if video.get_attribute("src").is_none() {
            video.set_src(url);
        }
        return Ok(());
    }
    if !mse_supported() {
        return Err("This browser cannot play streaming video.".into());
    }
    let mut state = playback.borrow_mut();
    if state.player.as_ref().is_some_and(|p| p.height >= height) {
        return Ok(());
    }
    let stream = state.stream.clone().ok_or("video is still loading")?;
    let position = video.current_time();
    state.player = None;
    state.player = Some(attach(video, stream, height)?);
    if position > 0.0 {
        video.set_current_time(position);
    }
    Ok(())
}

fn play(video: &HtmlVideoElement) {
    if let Ok(promise) = video.play() {
        let video = video.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // Autoplay with sound can be refused; fall back to muted.
            if wasm_bindgen_futures::JsFuture::from(promise).await.is_err() && !video.muted() {
                video.set_muted(true);
                let _ = video.play();
            }
        });
    }
}

/// An embedded video: poster first, a muted preview on hover, and the full
/// player with sound and controls once clicked. `full` is the lightbox mode.
/// HLS streams go through the Rust player; plain files play natively.
#[component]
pub fn HlsVideo(
    url: String,
    alt: String,
    #[prop(optional)] full: bool,
    #[prop(default = None)] on_expand: Option<Callback<()>>,
) -> impl IntoView {
    let container = NodeRef::<leptos::html::Div>::new();
    let video_ref = NodeRef::<leptos::html::Video>::new();
    let poster = RwSignal::new(None::<String>);
    let status = RwSignal::new(Status::Idle);
    let engaged = RwSignal::new(full);
    let previewing = RwSignal::new(false);
    // Views and cleanups must be Send; the browser objects live in local storage.
    let playback =
        StoredValue::new_local(Rc::new(RefCell::new(Playback { player: None, stream: None, observer: None })));
    let hls = is_hls(&url);
    let url = StoredValue::new(url);

    let load = move |play_at: Option<u32>| {
        if status.get_untracked() != Status::Idle {
            return;
        }
        if !hls {
            // A plain file has no playlist to read: let the browser fetch
            // its metadata, which also paints the first frame as a poster.
            let Some(video) = video_ref.get_untracked() else { return };
            video.set_preload("metadata");
            video.set_src(&format!("{}#t=0.1", url.get_value()));
            status.set(Status::Ready);
            if play_at.is_some() {
                video.set_muted(false);
                video.set_controls(true);
                play(&video);
            }
            return;
        }
        status.set(Status::Loading);
        wasm_bindgen_futures::spawn_local(async move {
            let src = url.get_value();
            match load_stream(&src).await {
                Ok(stream) => {
                    poster.set(stream.poster(if full { 1080 } else { 720 }));
                    playback.with_value(|p| p.borrow_mut().stream = Some(stream));
                    status.set(Status::Ready);
                    if let (Some(height), Some(video)) = (play_at, video_ref.get_untracked()) {
                        match playback.with_value(|p| ensure_source(&video, &src, p, height)) {
                            Ok(()) => {
                                video.set_muted(false);
                                video.set_controls(true);
                                play(&video);
                            }
                            Err(e) => status.set(Status::Failed(e)),
                        }
                    }
                }
                Err(e) => status.set(Status::Failed(format!("Could not load the video ({e})."))),
            }
        });
    };

    // Fetch the playlist (and so the poster) once the video nears the viewport.
    Effect::new(move |_| {
        let Some(element) = container.get() else { return };
        if full {
            load(Some(FULL_HEIGHT));
            return;
        }
        if playback.with_value(|p| p.borrow().observer.is_some()) {
            return;
        }
        let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            let visible = entries
                .iter()
                .filter_map(|e| e.dyn_into::<IntersectionObserverEntry>().ok())
                .any(|e| e.is_intersecting());
            if visible {
                load(None);
            }
        });
        let options = IntersectionObserverInit::new();
        options.set_root_margin("600px 0px");
        if let Ok(observer) = IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options) {
            observer.observe(&element);
            playback.with_value(|p| p.borrow_mut().observer = Some((observer, callback)));
        }
    });
    on_cleanup(move || {
        playback.with_value(|playback| {
            let mut state = playback.borrow_mut();
            if let Some((observer, _)) = state.observer.take() {
                observer.disconnect();
            }
            state.player = None;
        });
    });

    let start_preview = move |_: ev::MouseEvent| {
        if engaged.get_untracked() || !can_hover() || status.get_untracked() != Status::Ready {
            return;
        }
        let Some(video) = video_ref.get_untracked() else { return };
        if playback.with_value(|p| ensure_source(&video, &url.get_value(), p, PREVIEW_HEIGHT)).is_ok() {
            video.set_muted(true);
            previewing.set(true);
            play(&video);
        }
    };
    let stop_preview = move |_: ev::MouseEvent| {
        if engaged.get_untracked() {
            return;
        }
        previewing.set(false);
        if let Some(video) = video_ref.get_untracked() {
            let _ = video.pause();
        }
    };
    let engage = move |_: ev::MouseEvent| {
        let Some(video) = video_ref.get_untracked() else { return };
        engaged.set(true);
        previewing.set(false);
        if status.get_untracked() != Status::Ready {
            load(Some(INLINE_HEIGHT));
            return;
        }
        match playback.with_value(|p| ensure_source(&video, &url.get_value(), p, INLINE_HEIGHT)) {
            Ok(()) => {
                video.set_muted(false);
                video.set_controls(true);
                play(&video);
            }
            Err(e) => status.set(Status::Failed(e)),
        }
    };

    view! {
        <div
            class="video"
            class:engaged=move || engaged.get()
            class:previewing=move || previewing.get()
            class:loading=move || status.get() == Status::Loading
            node_ref=container
            on:mouseenter=start_preview
            on:mouseleave=stop_preview
        >
            <video
                node_ref=video_ref
                poster=move || poster.get()
                aria-label=alt.clone()
                playsinline=true
                preload="none"
                loop=!full
                prop:controls=move || engaged.get()
                on:error=move |_| {
                    if !hls {
                        status.set(Status::Failed("Could not play this video.".into()));
                    }
                }
            ></video>
            <Show when=move || !engaged.get()>
                <button class="video-play" type="button" aria-label=format!("Play video: {alt}") on:click=engage>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l11-6.5z"/></svg>
                </button>
            </Show>
            {on_expand.map(|expand| view! {
                <button class="video-expand" type="button" aria-label="Open in the media viewer" on:click=move |_| expand.run(())>
                    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M9.5 2.5h4v4M13.5 2.5l-5 5M6.5 13.5h-4v-4M2.5 13.5l5-5"/></svg>
                </button>
            })}
            {move || match status.get() {
                Status::Failed(message) => Some(view! { <p class="video-note" role="status">{message}</p> }),
                _ => None,
            }}
        </div>
    }
}

/// A video page link played in place, loading nothing from the provider
/// until the reader asks for it.
#[component]
pub fn VideoFacade(link: VideoLink, url: String, title: String) -> impl IntoView {
    let loaded = RwSignal::new(false);
    let provider = link.provider();
    let player = link.player_url();
    let host = host_of(&player).unwrap_or_default();
    let thumbnail = link.thumbnail_url();
    let label = if title.is_empty() { format!("{provider} video") } else { format!("{provider} video: {title}") };
    view! {
        <div class="facade" data-provider=provider.to_lowercase()>
            {move || if loaded.get() {
                view! {
                    <iframe
                        src=player.clone()
                        title=label.clone()
                        allow="autoplay; encrypted-media; picture-in-picture; fullscreen"
                        allowfullscreen=true
                        referrerpolicy="strict-origin-when-cross-origin"
                    ></iframe>
                }
                .into_any()
            } else {
                view! {
                    <button class="facade-button" type="button" aria-label=format!("Play {label}") on:click=move |_| loaded.set(true)>
                        {thumbnail.clone().map(|src| view! { <img src=src alt="" loading="lazy" decoding="async" referrerpolicy="no-referrer"/> })}
                        <span class="facade-play" aria-hidden="true">
                            <svg viewBox="0 0 24 24"><path d="M8 5.5v13l11-6.5z"/></svg>
                        </span>
                        <span class="facade-note">{format!("{provider} · loads from {host} when played")}</span>
                    </button>
                }
                .into_any()
            }}
            <a class="facade-source" href=url target="_blank" rel="noopener noreferrer nofollow">{format!("Open on {provider}")}</a>
        </div>
    }
}
