// SPDX-License-Identifier: AGPL-3.0-or-later
//! Video components: Farcaster HLS videos and plain video files played
//! inline, and click-to-load facades for YouTube, Vimeo, Loom and Streamable
//! links.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use allthing_core::embeds::{host_of, is_hls};
use allthing_core::video::VideoLink;
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    FocusOptions, HtmlElement, HtmlVideoElement, IntersectionObserver, IntersectionObserverEntry,
    IntersectionObserverInit,
};

use crate::components::media::size_style;
use crate::util::prefers_reduced_motion;
use crate::video::{MsePlayer, Stream, attach, known_size, load_stream, mse_supported, native_hls, remember_file_size};

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

/// Where the reader's choice of sound is kept between visits.
const SOUND_KEY: &str = "allthing.sound";

type EventCallback = Closure<dyn FnMut(web_sys::Event)>;

thread_local! {
    /// The sound choice for this page, for when localStorage is blocked.
    static SOUND: Cell<bool> = const { Cell::new(false) };
    /// A video whose autoplay with sound was refused: it plays muted until
    /// the reader's next tap or key press turns its sound on.
    static AWAITING_SOUND: RefCell<Option<HtmlVideoElement>> = const { RefCell::new(None) };
    /// Listens for that tap or key press, from the first time it's needed.
    static INTERACTION: Option<EventCallback> = interaction_listener();
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Whether videos play with sound: on once the reader unmutes a video, off
/// once they mute one. Kept in localStorage, so it holds across visits and
/// tabs; where storage is blocked, it holds for this page.
fn sound_on() -> bool {
    match storage().map(|s| s.get_item(SOUND_KEY)) {
        Some(Ok(value)) => value.as_deref() == Some("on"),
        _ => SOUND.with(Cell::get),
    }
}

fn set_sound(on: bool) {
    SOUND.with(|s| s.set(on));
    if let Some(storage) = storage() {
        let _ = if on { storage.set_item(SOUND_KEY, "on") } else { storage.remove_item(SOUND_KEY) };
    }
}

/// Note on `video` the muted state the player gave it, so its volumechange
/// events can tell the player's changes from the reader's.
fn note_muted(video: &HtmlVideoElement, muted: bool) {
    let _ = video.set_attribute("data-player-muted", if muted { "true" } else { "false" });
}

/// Mute or unmute `video` as the player's own doing.
fn set_muted(video: &HtmlVideoElement, muted: bool) {
    note_muted(video, muted);
    video.set_muted(muted);
}

/// Whether `video` was muted or unmuted by the reader (its controls, a key)
/// rather than by the player.
fn muted_by_reader(video: &HtmlVideoElement) -> bool {
    video.get_attribute("data-player-muted").is_some_and(|noted| (noted == "true") != video.muted())
}

/// Whether the page has the reader's activation right now (a tap, click or
/// key press), where the browser says. Unmuting without it is refused, and
/// in Chrome pauses the video.
fn user_activation() -> bool {
    let Some(window) = web_sys::window() else { return false };
    let activation =
        js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("userActivation")).unwrap_or(JsValue::UNDEFINED);
    if activation.is_undefined() {
        return true;
    }
    js_sys::Reflect::get(&activation, &JsValue::from_str("isActive")).ok().and_then(|a| a.as_bool()).unwrap_or(true)
}

/// Turns on the sound of the video awaiting it at the reader's first tap or
/// key press, which lets a page play sound. Listens in the capture phase, so
/// no other handler can hide the gesture.
fn interaction_listener() -> Option<EventCallback> {
    let window = web_sys::window()?;
    let callback = EventCallback::new(|event: web_sys::Event| {
        if AWAITING_SOUND.with(|a| a.borrow().is_none()) || !event.is_trusted() {
            return;
        }
        // A player's own buttons set its sound themselves, and a key that
        // grants no activation (Escape) waits for the next gesture.
        let on_player = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|t| t.closest(".video").ok().flatten().is_some());
        if on_player || !user_activation() {
            return;
        }
        let Some(video) = AWAITING_SOUND.with(|a| a.borrow_mut().take()) else { return };
        // Still playing (a removed or scrolled-away video is paused), and the
        // reader hasn't muted a video since.
        if sound_on() && video.muted() && !video.paused() {
            set_muted(&video, false);
        }
    });
    for name in ["pointerdown", "pointerup", "touchend", "keydown", "click"] {
        let _ = window.add_event_listener_with_callback_and_bool(name, callback.as_ref().unchecked_ref(), true);
    }
    Some(callback)
}

/// Turn `video`'s sound on at the reader's next tap or key press.
fn await_sound(video: HtmlVideoElement) {
    // Starts listening, the first time.
    INTERACTION.with(|_| ());
    AWAITING_SOUND.with(|a| *a.borrow_mut() = Some(video));
}

/// Whether `video` has at least [`AUTOPLAY_RATIO`] of itself on screen.
fn mostly_in_view(video: &HtmlVideoElement) -> bool {
    let rect = video.get_bounding_client_rect();
    let height = window().inner_height().ok().and_then(|h| h.as_f64()).unwrap_or(0.0);
    let shown = rect.bottom().min(height) - rect.top().max(0.0);
    rect.height() > 0.0 && shown / rect.height() >= AUTOPLAY_RATIO
}

/// Whether a video higher on the page than `video` is autoplaying and still
/// in view. Autoplay goes to the topmost video in view, so `video` waits:
/// otherwise whichever loaded last would take over, a reply's video from
/// the post's.
fn autoplaying_above(video: &HtmlVideoElement) -> bool {
    ACTIVE.with(|active| {
        active.borrow().as_ref().is_some_and(|(other, by_reader)| {
            !*by_reader
                && other != video
                && !other.paused()
                && video.compare_document_position(other) & web_sys::Node::DOCUMENT_POSITION_PRECEDING != 0
                && mostly_in_view(other)
        })
    })
}

thread_local! {
    /// Fires when an autoplaying video leaves the screen, so the videos still
    /// in view look again for which of them should play.
    static AUTOPLAY_TURN: ArcTrigger = ArcTrigger::new();
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
    let stream = state.stream.clone().ok_or("video is still loading")?;
    // A stream may top out below the requested height. Compare against the
    // rendition it can actually supply, so playing doesn't rebuild the same
    // low-resolution source and discard frames it already buffered.
    let available = allthing_core::hls::choose_variant(&stream.master, height).ok_or("no renditions")?.height;
    if state.player.as_ref().is_some_and(|p| p.height >= available) {
        return Ok(());
    }
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
            // Playing with sound is refused until the reader has interacted
            // with the page: play muted, and turn the sound on at their first
            // tap or key press. Other rejections (an interrupted play, a new
            // source) are not refusals.
            if let Err(error) = wasm_bindgen_futures::JsFuture::from(promise).await {
                let refused = js_sys::Reflect::get(&error, &JsValue::from_str("name"))
                    .ok()
                    .and_then(|n| n.as_string())
                    .is_some_and(|n| n == "NotAllowedError");
                if refused && !video.muted() {
                    set_muted(&video, true);
                    let _ = video.play();
                    await_sound(video);
                }
            }
        });
    }
}

/// Play `video` with sound and controls, as the reader asked: sound then
/// stays on for later videos.
fn play_with_sound(video: &HtmlVideoElement) {
    set_sound(true);
    set_muted(video, false);
    video.set_controls(true);
    claim(video, true);
    play(video);
}

/// Pointer focus on an already visible control must not scroll its video
/// out of view, which would immediately pause autoplay.
fn focus_without_scroll(event: ev::MouseEvent) {
    let button = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest("button").ok().flatten())
        .and_then(|target| target.dyn_into::<HtmlElement>().ok());
    if let Some(button) = button {
        event.prevent_default();
        let options = FocusOptions::new();
        options.set_prevent_scroll(true);
        let _ = button.focus_with_options(&options);
    }
}

/// An embedded video: poster first, autoplay in view (muted until the reader
/// turns sound on), and the full player with sound and controls once clicked. `full` is the lightbox mode.
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
    // events), so the play button can step aside during autoplay.
    let playing = RwSignal::new(false);
    // Whether it's muted (the element's volumechange events), for the sound
    // badge and the button's name.
    let muted = RwSignal::new(true);
    let playback = StoredValue::new_local(Rc::new(RefCell::new(Playback {
        player: None,
        stream: None,
        observer: None,
        visibility: None,
    })));
    let in_view = RwSignal::new(false);
    let hls = is_hls(&url);
    // Its width and height, for a box of the right shape: from the
    // playlist, or a plain file's metadata (the lightbox sizes its own).
    let size = RwSignal::new(if full { None } else { known_size(&url) });
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
                play_with_sound(&video);
            }
            return;
        }
        status.set(Status::Loading);
        let src = url.get_value();
        wasm_bindgen_futures::spawn_local(async move {
            let result = load_stream(&src).await;
            // The playlist can finish after navigation removed this video.
            if status.is_disposed() {
                return;
            }
            match result {
                Ok(stream) => {
                    poster.set(stream.poster(if full { 1080 } else { 720 }));
                    if !full && let Some(known) = stream.size() {
                        size.set(Some(known));
                    }
                    playback.with_value(|p| p.borrow_mut().stream = Some(stream));
                    status.set(Status::Ready);
                    // A click while the playlist was loading engaged the
                    // video: play it now, as the click asked.
                    let play_at = play_at.or_else(|| engaged.get_untracked().then_some(INLINE_HEIGHT));
                    if let (Some(height), Some(video)) = (play_at, video_ref.get_untracked()) {
                        match playback.with_value(|p| ensure_source(&video, &src, p, height)) {
                            Ok(()) => play_with_sound(&video),
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
    let pending_timer = StoredValue::new(None::<TimeoutHandle>);
    let cancel_pending = move || {
        pending_timer.update_value(|timer| {
            if let Some(handle) = timer.take() {
                handle.clear();
            }
        });
    };
    on_cleanup(cancel_pending);
    Effect::new(move |_| {
        cancel_pending();
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
        pending_timer.set_value(
            set_timeout_with_handle(
                move || {
                    if !playing.get_untracked() {
                        pending.set(false);
                    }
                },
                AUTOPLAY_PENDING,
            )
            .ok(),
        );
    });

    // Autoplay while mostly on screen, muted unless the reader turned sound
    // on; pause when scrolled away.
    let start_autoplay = move || {
        if engaged.get_untracked() || status.get_untracked() != Status::Ready || !autoplay_allowed() {
            return;
        }
        let Some(video) = video_ref.get_untracked() else { return };
        if reader_watching_other(&video) || autoplaying_above(&video) {
            return;
        }
        if playback.with_value(|p| ensure_source(&video, &url.get_value(), p, PREVIEW_HEIGHT)).is_ok() {
            // Muted and inline before play(), or iOS Safari refuses autoplay
            // or takes the video fullscreen. With sound on, play() falls back
            // to muted when the browser refuses sound.
            video.set_default_muted(true);
            set_muted(&video, !sound_on());
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
        AUTOPLAY_TURN.with(ArcTrigger::track);
        if in_view.get() && status.get() == Status::Ready {
            start_autoplay();
        } else if !in_view.get() {
            previewing.set(false);
            if let Some(video) = video_ref.get_untracked()
                && !video.paused()
            {
                let _ = video.pause();
                // Another video in view may play now: let them look, after
                // this effect has finished.
                set_timeout(|| AUTOPLAY_TURN.with(ArcTrigger::notify), Duration::ZERO);
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
            Ok(()) => play_with_sound(&video),
            Err(e) => status.set(Status::Failed(e)),
        }
    };

    // The badge on an autoplaying video: turns its sound on, or off, and the
    // reader's choice holds for later videos.
    let toggle_sound = move |_: ev::MouseEvent| {
        let Some(video) = video_ref.get_untracked() else { return };
        let sound = video.muted();
        set_sound(sound);
        set_muted(&video, !sound);
        if sound {
            claim(&video, true);
        }
    };

    // Media events are queued. A pause/ended event from an earlier state
    // can arrive after play has resumed; it must not hide active controls.
    let on_stopped = move |_: ev::Event| {
        if video_ref.get_untracked().is_some_and(|video| video.paused() || video.ended()) {
            playing.set(false);
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
            style=move || size.get().map(|(w, h)| size_style(w, h))
            data-sized=move || size.get().map(|_| "")
            data-portrait=move || size.get().and_then(|(w, h)| (h > w).then_some(""))
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
                on:playing=move |_| {
                    if let Some(video) = video_ref.get_untracked() {
                        playing.set(!video.paused() && !video.ended());
                        muted.set(video.muted());
                    }
                }
                on:volumechange=move |_| {
                    let Some(video) = video_ref.get_untracked() else { return };
                    muted.set(video.muted());
                    if muted_by_reader(&video) {
                        note_muted(&video, video.muted());
                        set_sound(!video.muted());
                    }
                }
                on:pause=on_stopped
                on:ended=on_stopped
                on:loadedmetadata=move |_| {
                    let Some(video) = video_ref.get_untracked() else { return };
                    let (w, h) = (video.video_width(), video.video_height());
                    if !hls && !full && w > 0 && h > 0 {
                        remember_file_size(&url.get_value(), w, h);
                        size.set(Some((w, h)));
                    }
                }
                on:error=move |_| {
                    if !hls {
                        status.set(Status::Failed("Could not play this video.".into()));
                    }
                }
            ></video>
            <Show when=move || !engaged.get()>
                // While it autoplays, the button covers the video without a play
                // icon (which would make it look paused), and a badge turns its
                // sound on or off.
                <button
                    class="video-play"
                    type="button"
                    aria-label={
                        let alt = alt.clone();
                        move || {
                            if previewing.get() && playing.get() && muted.get() {
                                format!("Play with sound: {alt}")
                            } else if previewing.get() && playing.get() {
                                format!("Show controls: {alt}")
                            } else {
                                format!("Play video: {alt}")
                            }
                        }
                    }
                    on:click=engage
                >
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l11-6.5z"/></svg>
                </button>
                <button class="video-sound" type="button" on:mousedown=focus_without_scroll on:click=toggle_sound>
                    {move || if muted.get() { "Sound off · tap for sound" } else { "Sound on · tap to mute" }}
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
