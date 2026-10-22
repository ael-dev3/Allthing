// SPDX-License-Identifier: AGPL-3.0-or-later
//! Inline playback for Farcaster videos, which are HLS streams on Cloudflare
//! Stream. Browsers with native HLS (Safari, iOS, Chrome on Android, and
//! Chrome on computers since about version 142) get the playlist URL directly.
//! Everywhere else (Firefox) a small Media Source
//! Extensions player written here in Rust feeds the stream's fMP4 segments to
//! the video element, so no third-party player script is needed. Segments are
//! fetched straight from the video host, never through Allthing's Worker.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use allthing_core::hls::{self, MasterPlaylist, MediaPlaylist};
use js_sys::{ArrayBuffer, Function, Promise};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, AbortSignal, EventTarget, HtmlVideoElement, MediaSource, MediaSourceReadyState, Response,
    SourceBuffer,
};

/// Keep this many seconds buffered ahead of the playhead.
const BUFFER_AHEAD_SECS: f64 = 24.0;
/// Drop buffered media this far behind the playhead when the buffer is full.
const KEEP_BEHIND_SECS: f64 = 12.0;

pub async fn sleep(ms: i32) {
    let promise = Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = JsFuture::from(promise).await;
}

struct Listener {
    target: EventTarget,
    name: &'static str,
    callback: Function,
}

impl Listener {
    fn new(target: &EventTarget, name: &'static str, callback: &Function) -> Result<Self, JsValue> {
        target.add_event_listener_with_callback(name, callback)?;
        Ok(Self { target: target.clone(), name, callback: callback.clone() })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.target.remove_event_listener_with_callback(self.name, &self.callback);
    }
}

async fn next_event(target: &EventTarget, name: &'static str, signal: &AbortSignal) -> Result<(), String> {
    if signal.aborted() {
        return Err("playback stopped".into());
    }
    let mut listeners = Vec::new();
    let promise = Promise::new(&mut |resolve, reject| {
        for (target, name, callback) in [
            (target, name, &resolve),
            (target, "error", &reject),
            (target, "abort", &reject),
            (signal.as_ref(), "abort", &reject),
        ] {
            match Listener::new(target, name, callback) {
                Ok(listener) => listeners.push(listener),
                Err(error) => {
                    let _ = reject.call1(&JsValue::UNDEFINED, &error);
                }
            }
        }
    });
    let result = JsFuture::from(promise).await;
    drop(listeners);
    result.map(|_| ()).map_err(|_| "video loading interrupted".into())
}

/// Wake the idle buffering loop on cancellation as well as its next tick.
async fn pause(ms: i32, signal: &AbortSignal) {
    if signal.aborted() {
        return;
    }
    let Some(window) = web_sys::window() else { return };
    let mut listener = None;
    let mut timer = None;
    let promise = Promise::new(&mut |resolve, _| {
        listener = Listener::new(signal.as_ref(), "abort", &resolve).ok();
        timer = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms).ok();
        if timer.is_none() {
            let _ = resolve.call0(&JsValue::UNDEFINED);
        }
    });
    let _ = JsFuture::from(promise).await;
    if let Some(timer) = timer {
        window.clear_timeout_with_handle(timer);
    }
    drop(listener);
}

async fn fetch(url: &str, signal: Option<&AbortSignal>) -> Result<Response, String> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_signal(signal);
    let response: Response = JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await
        .map_err(|_| "network error".to_owned())?
        .dyn_into()
        .map_err(|_| "bad response".to_owned())?;
    if response.ok() { Ok(response) } else { Err(format!("HTTP {}", response.status())) }
}

async fn fetch_text(url: &str, signal: Option<&AbortSignal>) -> Result<(String, String), String> {
    let response = fetch(url, signal).await?;
    let final_url = response.url();
    let text = JsFuture::from(response.text().map_err(|_| "unreadable")?)
        .await
        .map_err(|_| "unreadable".to_owned())?
        .as_string()
        .unwrap_or_default();
    Ok((final_url, text))
}

async fn fetch_buffer(url: &str, signal: &AbortSignal) -> Result<ArrayBuffer, String> {
    let response = fetch(url, Some(signal)).await?;
    JsFuture::from(response.array_buffer().map_err(|_| "unreadable")?)
        .await
        .map_err(|_| "unreadable".to_owned())?
        .dyn_into()
        .map_err(|_| "unreadable".to_owned())
}

fn resolve_url(relative: &str, base: &str) -> String {
    web_sys::Url::new_with_base(relative, base).map(|u| u.href()).unwrap_or_else(|_| relative.to_owned())
}

/// A stream's master playlist, fetched once per page session. Following the
/// playlist's redirect also reveals the Cloudflare Stream poster image.
pub struct Stream {
    pub manifest_url: String,
    pub master: MasterPlaylist,
}

impl Stream {
    pub fn poster(&self, height: u32) -> Option<String> {
        hls::stream_thumbnail(&self.manifest_url, height)
    }

    /// The width and height of its largest rendition.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.master
            .variants
            .iter()
            .filter(|v| v.width > 0 && v.height > 0)
            .max_by_key(|v| v.height)
            .map(|v| (v.width, v.height))
    }
}

thread_local! {
    static STREAMS: RefCell<HashMap<String, Rc<Stream>>> = RefCell::new(HashMap::new());
    /// Plain video files' sizes, from their metadata.
    static FILE_SIZES: RefCell<HashMap<String, (u32, u32)>> = RefCell::new(HashMap::new());
}

/// A video's width and height, if this page has loaded it before: a list
/// rendered again (Back to a feed) then lays it out at its size at once.
pub fn known_size(url: &str) -> Option<(u32, u32)> {
    STREAMS
        .with(|s| s.borrow().get(url).and_then(|stream| stream.size()))
        .or_else(|| FILE_SIZES.with(|s| s.borrow().get(url).copied()))
}

/// A video's poster, asking for nothing: the one its playlist gave, if this
/// page has loaded it (at the height an inline video's poster is, so the
/// browser has it), or Cloudflare Stream's, which sits beside its manifest.
#[cfg(not(feature = "static-host"))]
pub fn known_poster(url: &str) -> Option<String> {
    const HEIGHT: u32 = 720;
    STREAMS
        .with(|s| s.borrow().get(url).and_then(|stream| stream.poster(HEIGHT)))
        .or_else(|| hls::stream_thumbnail(url, HEIGHT))
}

pub fn remember_file_size(url: &str, width: u32, height: u32) {
    FILE_SIZES.with(|s| {
        let mut sizes = s.borrow_mut();
        if sizes.len() > 1000 {
            sizes.clear();
        }
        sizes.insert(url.to_owned(), (width, height));
    });
}

pub async fn load_stream(url: &str) -> Result<Rc<Stream>, String> {
    if let Some(stream) = STREAMS.with(|s| s.borrow().get(url).cloned()) {
        return Ok(stream);
    }
    let (manifest_url, text) = fetch_text(url, None).await?;
    let master = hls::parse_master(&text).ok_or("not an HLS master playlist")?;
    let stream = Rc::new(Stream { manifest_url, master });
    STREAMS.with(|s| s.borrow_mut().insert(url.to_owned(), stream.clone()));
    Ok(stream)
}

pub fn native_hls(video: &HtmlVideoElement) -> bool {
    !video.can_play_type("application/vnd.apple.mpegurl").is_empty()
}

pub fn mse_supported() -> bool {
    js_sys::Reflect::has(&js_sys::global(), &JsValue::from_str("MediaSource")).unwrap_or(false)
}

struct Track {
    playlist: MediaPlaylist,
    base: String,
    buffer: SourceBuffer,
    /// The segment appended last, and where it took the end of the buffered
    /// range the playhead was in (NaN when it didn't extend that range by
    /// about its own length): buffering goes on from there by index.
    last: Option<(usize, f64)>,
}

impl Track {
    /// End of the buffered range containing `time`, if any.
    fn buffered_until(&self, time: f64) -> Option<f64> {
        let ranges = self.buffer.buffered().ok()?;
        (0..ranges.length()).find_map(|i| {
            let (start, end) = (ranges.start(i).ok()?, ranges.end(i).ok()?);
            (start - 0.25 <= time && time <= end).then_some(end)
        })
    }

    async fn append(&self, data: &ArrayBuffer, playhead: f64, signal: &AbortSignal) -> Result<(), String> {
        if signal.aborted() {
            return Err("playback stopped".into());
        }
        let target: &EventTarget = self.buffer.as_ref();
        if self.buffer.append_buffer_with_array_buffer(data).is_err() {
            // Usually QuotaExceededError: free what is behind the playhead, retry once.
            if playhead > KEEP_BEHIND_SECS && self.buffer.remove(0.0, playhead - KEEP_BEHIND_SECS).is_ok() {
                next_event(target, "updateend", signal).await?;
            }
            self.buffer.append_buffer_with_array_buffer(data).map_err(|_| "could not buffer video".to_owned())?;
        }
        next_event(target, "updateend", signal).await?;
        Ok(())
    }
}

/// Plays a stream through Media Source Extensions until dropped.
pub struct MsePlayer {
    controller: AbortController,
    object_url: String,
    pub height: u32,
}

impl Drop for MsePlayer {
    fn drop(&mut self) {
        self.controller.abort();
        let _ = web_sys::Url::revoke_object_url(&self.object_url);
    }
}

/// Attach `stream` to `video` at the smallest rendition at least
/// `target_height` tall. Errors when the browser cannot decode the codecs.
pub fn attach(video: &HtmlVideoElement, stream: Rc<Stream>, target_height: u32) -> Result<MsePlayer, String> {
    let mut variant = hls::choose_variant(&stream.master, target_height).ok_or("no renditions")?.clone();
    if variant.codecs.is_empty() {
        // Playlists without CODECS: assume Cloudflare Stream's H.264 + AAC.
        variant.codecs = vec!["avc1.4d401f".into(), "mp4a.40.2".into()];
    }
    let audio_uri = hls::audio_uri(&stream.master, &variant).map(str::to_owned);
    let (video_type, audio_type) = hls::mime_types(&variant.codecs);
    let video_type = video_type.ok_or("no video codec")?;
    // Audio muxed into the video segments needs one buffer with both codecs.
    let (video_type, audio_type) = match (audio_uri.is_some(), audio_type) {
        (true, Some(audio)) => (video_type, Some(audio)),
        (false, Some(_)) => (format!("video/mp4; codecs=\"{}\"", variant.codecs.join(",")), None),
        (_, None) => (video_type, None),
    };
    if !MediaSource::is_type_supported(&video_type)
        || audio_type.as_deref().is_some_and(|t| !MediaSource::is_type_supported(t))
    {
        return Err("This browser cannot decode this video's format (H.264).".into());
    }
    let media_source = MediaSource::new().map_err(|_| "Media Source Extensions unavailable")?;
    let controller = AbortController::new().map_err(|_| "could not create a player")?;
    let object_url =
        web_sys::Url::create_object_url_with_source(&media_source).map_err(|_| "could not create a player")?;
    video.set_src(&object_url);

    let signal = controller.signal();
    let player = MsePlayer { controller, object_url, height: variant.height };
    let video = video.clone();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = run(
            &video,
            &media_source,
            &stream,
            &variant.uri,
            audio_uri.as_deref(),
            &video_type,
            audio_type.as_deref(),
            &signal,
        )
        .await
            && !signal.aborted()
        {
            web_sys::console::warn_1(&JsValue::from_str(&format!("video: {error}")));
            let _ = video.set_attribute("data-error", &error);
        }
    });
    Ok(player)
}

#[allow(clippy::too_many_arguments)]
async fn run(
    video: &HtmlVideoElement,
    media_source: &MediaSource,
    stream: &Stream,
    video_uri: &str,
    audio_uri: Option<&str>,
    video_type: &str,
    audio_type: Option<&str>,
    signal: &AbortSignal,
) -> Result<(), String> {
    if media_source.ready_state() != MediaSourceReadyState::Open {
        next_event(media_source.as_ref(), "sourceopen", signal).await?;
    }
    let mut tracks = Vec::new();
    for (uri, mime) in std::iter::once((video_uri, video_type)).chain(audio_uri.zip(audio_type)) {
        let url = resolve_url(uri, &stream.manifest_url);
        let (base, text) = fetch_text(&url, Some(signal)).await?;
        let playlist = hls::parse_media(&text).ok_or("bad media playlist")?;
        let buffer = media_source.add_source_buffer(mime).map_err(|_| "unsupported codec")?;
        tracks.push(Track { playlist, base, buffer, last: None });
    }
    if signal.aborted() {
        return Ok(());
    }
    let duration = tracks.iter().map(|t| t.playlist.duration()).fold(0.0, f64::max);
    if duration > 0.0 {
        media_source.set_duration(duration);
    }
    for track in &tracks {
        if let Some(init) = &track.playlist.init_uri {
            let data = fetch_buffer(&resolve_url(init, &track.base), signal).await?;
            track.append(&data, 0.0, signal).await?;
        }
    }

    // Buffer from wherever the playhead is (seeks included), a little ahead.
    let mut ended = false;
    while !signal.aborted() {
        let playhead = video.current_time();
        let mut worked = false;
        let mut complete = true;
        for track in tracks.iter_mut() {
            if track.buffer.updating() {
                complete = false;
                continue;
            }
            let range_end = track.buffered_until(playhead);
            let edge = range_end.unwrap_or(playhead);
            let count = track.playlist.segments.len();
            if count == 0 || edge >= track.playlist.duration() - 0.1 {
                continue;
            }
            let mut index = track.playlist.segment_at(edge + 0.05);
            if let Some((last, end)) = track.last
                && (index == last || range_end.is_some_and(|range_end| (range_end - end).abs() < 0.001))
            {
                // Going on from the last append, or it did not reach past
                // this point (a gap): the next segment. A playlist can be off
                // from its media by a little per segment, so looking the edge
                // up by time can land on a segment already in, or skip one.
                if last + 1 >= count {
                    // All in, though the media may end a little before the
                    // playlist says: the stream must end now, or the video
                    // stalls on its last frame instead of looping.
                    continue;
                }
                index = last + 1;
            }
            complete = false;
            if edge - playhead > BUFFER_AHEAD_SECS {
                continue;
            }
            let segment = &track.playlist.segments[index];
            let length = segment.duration;
            let data = fetch_buffer(&resolve_url(&segment.uri, &track.base), signal).await?;
            if signal.aborted() {
                return Ok(());
            }
            track.append(&data, playhead, signal).await?;
            // (Audio codecs trim a little from a stream's edges, which
            // matters for a short last segment.)
            let grown =
                track.buffered_until(playhead).filter(|end| (end - edge - length).abs() < (length / 2.0).max(0.25));
            track.last = Some((index, grown.unwrap_or(f64::NAN)));
            worked = true;
            ended = false;
        }
        if complete && !ended {
            if media_source.ready_state() == MediaSourceReadyState::Open {
                let _ = media_source.end_of_stream();
            }
            ended = true;
        }
        if !worked {
            pause(if video.paused() { 600 } else { 250 }, signal).await;
        }
    }
    Ok(())
}
