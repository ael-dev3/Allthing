// SPDX-License-Identifier: AGPL-3.0-or-later
//! Video components: Farcaster HLS videos and plain video files played
//! inline, and click-to-load facades for YouTube, Vimeo, Loom and Streamable
//! links.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use allthing_core::embeds::{host_of, is_hls};
use allthing_core::video::VideoLink;
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{HtmlVideoElement, IntersectionObserver, IntersectionObserverEntry, IntersectionObserverInit};

use crate::util::prefers_reduced_motion;
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
    /// Watches how much of the video is on screen, for autoplay.
    visibility: Option<(IntersectionObserver, ObserverCallback)>,
}

thread_local! {
    /// The one video allowed to play at a time, and whether the reader
    /// started it.
    static ACTIVE: RefCell<Option<(HtmlVideoElement, bool)>> = const { RefCell::new(None) };
}

/// Make `video` the only one playing: pause whichever was. `by_reader` is
/// whether the reader started it (not a muted autoplay).
fn claim(video: &HtmlVideoElement, by_reader: bool) {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        if let Some((previous, _)) = active.as_ref().filter(|(p, _)| p != video) {
            let _ = previous.pause();
        }
        *active = Some((video.clone(), by_reader));
    });
}

/// Whether the reader is watching a video other than `video`: one they
/// started that is still playing. Autoplay doesn't take over from it.
fn reader_watching_other(video: &HtmlVideoElement) -> bool {
    ACTIVE.with(|active| {
        active.borrow().as_ref().is_some_and(|(p, by_reader)| *by_reader && p != video && !p.paused() && !p.ended())
    })
}

/// Muted autoplay in view, unless the reader prefers reduced motion or is
/// saving data.
fn autoplay_allowed() -> bool {
    !prefers_reduced_motion() && !crate::api::constrained_network()
}

/// How long the play icon stays hidden while autoplay is pending.
const AUTOPLAY_PENDING: Duration = Duration::from_millis(2500);

/// Share of a video that must be on screen for it to autoplay.
const AUTOPLAY_RATIO: f64 = 0.6;

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
            // Autoplay with sound can be refused; fall back to muted. Other
            // rejections (an interrupted play, a new source) are not refusals.
            if let Err(error) = wasm_bindgen_futures::JsFuture::from(promise).await {
                let refused = js_sys::Reflect::get(&error, &JsValue::from_str("name"))
                    .ok()
                    .and_then(|n| n.as_string())
                    .is_some_and(|n| n == "NotAllowedError");
                if refused && !video.muted() {
                    video.set_muted(true);
                    let _ = video.play();
                }
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
    // Whether frames are actually advancing (the element's playing/pause
    // events), so the play button can step aside during muted autoplay.
    let playing = RwSignal::new(false);
    let playback = StoredValue::new_local(Rc::new(RefCell::new(Playback {
        player: None,
        stream: None,
        observer: None,
        visibility: None,
    })));
    let in_view = RwSignal::new(false);
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
                claim(&video, true);
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
                    // A click while the playlist was loading engaged the
                    // video: play it now, as the click asked.
                    let play_at = play_at.or_else(|| engaged.get_untracked().then_some(INLINE_HEIGHT));
                    if let (Some(height), Some(video)) = (play_at, video_ref.get_untracked()) {
                        match playback.with_value(|p| ensure_source(&video, &src, p, height)) {
                            Ok(()) => {
                                video.set_muted(false);
                                video.set_controls(true);
                                claim(&video, true);
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
            if let Some((observer, _)) = state.visibility.take() {
                observer.disconnect();
            }
            state.player = None;
        });
    });

    // From the moment a video is due to autoplay until its first frame plays,
    // the play icon would flash over the poster: hide it while autoplay is
    // pending, and bring it back if playback hasn't started after a while
    // (autoplay refused, or a stream this browser can't decode).
    let pending = RwSignal::new(false);
    let pending_generation = StoredValue::new(0u32);
    Effect::new(move |_| {
        let due = !full
            && in_view.get()
            && !engaged.get()
            && !matches!(status.get(), Status::Failed(_))
            && autoplay_allowed();
        if !due {
            pending.set(false);
            return;
        }
        if playing.get_untracked() {
            return;
        }
        pending.set(true);
        let generation = pending_generation.get_value().wrapping_add(1);
        pending_generation.set_value(generation);
        set_timeout(
            move || {
                if pending_generation.get_value() == generation && !playing.get_untracked() {
                    pending.set(false);
                }
            },
            AUTOPLAY_PENDING,
        );
    });

    // Autoplay muted while mostly on screen; pause when scrolled away.
    let start_autoplay = move || {
        if engaged.get_untracked() || status.get_untracked() != Status::Ready || !autoplay_allowed() {
            return;
        }
        let Some(video) = video_ref.get_untracked() else { return };
        if reader_watching_other(&video) {
            return;
        }
        if playback.with_value(|p| ensure_source(&video, &url.get_value(), p, PREVIEW_HEIGHT)).is_ok() {
            // Muted and inline before play(), or iOS Safari refuses autoplay
            // or takes the video fullscreen.
            video.set_default_muted(true);
            video.set_muted(true);
            let _ = video.set_attribute("playsinline", "");
            previewing.set(true);
            claim(&video, false);
            play(&video);
        }
    };
    Effect::new(move |_| {
        if full {
            return;
        }
        if in_view.get() && status.get() == Status::Ready {
            start_autoplay();
        } else if !in_view.get() {
            previewing.set(false);
            if let Some(video) = video_ref.get_untracked() {
                let _ = video.pause();
            }
        }
    });
    Effect::new(move |_| {
        let Some(element) = container.get() else { return };
        if full || playback.with_value(|p| p.borrow().visibility.is_some()) {
            return;
        }
        let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            if let Some(entry) =
                entries.iter().filter_map(|e| e.dyn_into::<IntersectionObserverEntry>().ok()).next_back()
            {
                in_view.set(entry.is_intersecting() && entry.intersection_ratio() >= AUTOPLAY_RATIO);
            }
        });
        let options = IntersectionObserverInit::new();
        let thresholds = js_sys::Array::of2(&JsValue::from_f64(0.0), &JsValue::from_f64(AUTOPLAY_RATIO));
        options.set_threshold(&thresholds);
        if let Ok(observer) = IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options) {
            observer.observe(&element);
            playback.with_value(|p| p.borrow_mut().visibility = Some((observer, callback)));
        }
    });

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
                claim(&video, true);
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
            class:autoplaying=move || previewing.get() && playing.get() && !engaged.get()
            class:autoplay-pending=move || pending.get() && !playing.get() && !engaged.get()
            class:loading=move || status.get() == Status::Loading
            node_ref=container
        >
            <video
                node_ref=video_ref
                poster=move || poster.get()
                aria-label=alt.clone()
                playsinline=true
                muted=true
                preload="none"
                loop=!full
                prop:controls=move || engaged.get()
                on:playing=move |_| playing.set(true)
                on:pause=move |_| playing.set(false)
                on:ended=move |_| playing.set(false)
                on:error=move |_| {
                    if !hls {
                        status.set(Status::Failed("Could not play this video.".into()));
                    }
                }
            ></video>
            <Show when=move || !engaged.get()>
                // While it autoplays muted, the button covers the video without
                // a play icon (which would make it look paused) and offers sound.
                <button
                    class="video-play"
                    type="button"
                    aria-label={
                        let alt = alt.clone();
                        move || {
                            if previewing.get() && playing.get() {
                                format!("Play with sound: {alt}")
                            } else {
                                format!("Play video: {alt}")
                            }
                        }
                    }
                    on:click=engage
                >
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l11-6.5z"/></svg>
                    <span class="video-sound" aria-hidden="true">"Sound off · tap for sound"</span>
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
