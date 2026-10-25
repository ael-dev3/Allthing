// SPDX-License-Identifier: AGPL-3.0-or-later
//! Images and videos with a cast: the composer's attach button, which on a
//! phone opens its photos, and the files pasted (a screenshot) or dropped on
//! the composer (`compose::Composer::take_files`), where this server takes
//! them. Each is checked against what it takes (`/api/v1/media`: its types
//! and sizes), taken
//! apart without its metadata if it's a picture
//! (`allthing_core::metadata`), and uploaded at once, as the Worker takes
//! it: the file itself as the body (`allthing_core::media`). Its tile shows
//! it, with how far its upload has gone, and Remove, which stops it; one
//! that didn't go says why, as the Worker said, with Retry. The cast embeds
//! the URLs of those uploaded, in order, before its GIF or sticker; with
//! those and the cast it quotes, it carries two embeds at most.
//!
//! A video in an MP4 or MOV is remuxed first (`allthing_core::fmp4`): the
//! same frames and sound, as a fragmented MP4 with an HLS index, so the cast
//! embeds a playlist, the only kind of video the Farcaster app plays. One
//! that can't be is uploaded as it is.
//!
//! In the composer's part (`split_composer.wasm`), with the composer's
//! pickers and bar.

use allthing_core::fmp4;
use allthing_core::media::{EMBEDS_FULL, MEDIA_PATH, MediaType, accepted, check_file, size_label};
use allthing_core::metadata;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, BlobPropertyBag, File, ProgressEvent, XmlHttpRequest};

use crate::components::compose::{Attachment, ChunkUpload, Composer, INCOMING, LIVE, Live, Sending};

/// A picture with a frame and a corner of sky: attach.
const PICTURE: &str = "M3 3.5h10a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1zM2.5 10.5l3.2-3.2 2.6 2.6 1.7-1.7 3.5 3.5";
const LOST: &str = "The upload didn't go through: check the connection, and try again.";

/// Attach what was pasted or dropped, now and whenever more comes.
pub fn take_incoming(composer: Composer) {
    Effect::new(move |_| {
        composer.incoming.track();
        let files = INCOMING.with(|incoming| std::mem::take(&mut *incoming.borrow_mut()));
        if !files.is_empty() {
            // What the draft holds is read, not followed.
            untrack(|| add(composer, files));
        }
    });
}

/// Attach `files`, as far as there's room and each can go; say why of those
/// that can't.
pub fn add(composer: Composer, files: Vec<File>) {
    let Some(limits) = composer.limits.get_untracked() else { return };
    let mut said = Vec::new();
    for file in files {
        if composer.embeds_left() == 0 {
            said.push(EMBEDS_FULL.to_owned());
            break;
        }
        match check_file(&file.name(), &file.type_(), file.size() as u64, &limits) {
            Ok(media) => start(composer, file, media),
            Err(why) => said.push(why),
        }
    }
    composer.note.set((!said.is_empty()).then(|| said.join(" ")));
}

/// A file's tile, then its upload.
fn start(composer: Composer, file: File, media: MediaType) {
    let id = Attachment::next_id();
    let local = web_sys::Url::create_object_url_with_blob(&file).ok();
    let state = if media.is_video() { Sending::Preparing } else { Sending::Up { sent: 0.0, of: file.size() } };
    composer.files.update(|files| files.push(Attachment { id, name: file.name(), media, local, state }));
    LIVE.with(|live| {
        live.borrow_mut().insert(
            id,
            Live { file, body: None, hls: None, multipart: None, request: None, progress: None, ended: None },
        )
    });
    send(composer, id);
}

/// Try a file that didn't go again.
pub fn retry(composer: Composer, id: u64) {
    if LIVE.with(|live| live.borrow().contains_key(&id)) {
        let state = if media_of(composer, id).is_some_and(MediaType::is_video) {
            Sending::Preparing
        } else {
            Sending::Up { sent: 0.0, of: 0.0 }
        };
        set(composer, id, state);
        send(composer, id);
    }
}

fn set(composer: Composer, id: u64, state: Sending) {
    let _ = composer.files.try_update(|files| {
        if let Some(file) = files.iter_mut().find(|f| f.id == id) {
            file.state = state;
        }
    });
}

fn media_of(composer: Composer, id: u64) -> Option<MediaType> {
    composer.files.with_untracked(|files| files.iter().find(|f| f.id == id).map(|f| f.media))
}

/// Upload a file: a picture without its metadata, anything else as it is,
/// as the body of a POST to the Worker, with its progress as it goes.
fn send(composer: Composer, id: u64) {
    spawn_local(async move {
        let Some(media) = media_of(composer, id) else { return };
        let maximum = composer.limits.get_untracked().map_or(media.max_bytes(), |limits| {
            if media.is_video() { limits.max_video_bytes } else { limits.max_image_bytes }
        });
        let Some((file, kept, hls)) =
            LIVE.with(|live| live.borrow().get(&id).map(|l| (l.file.clone(), l.body.clone(), l.hls.clone())))
        else {
            return;
        };
        let (body, hls) = match kept {
            Some(body) => (body, hls),
            None if metadata::stripped(media) => match without_metadata(&file, media).await {
                Ok(body) => (body, None),
                Err(why) => return set(composer, id, Sending::Failed { why }),
            },
            None if matches!(media, MediaType::Mp4 | MediaType::Mov) => match remuxed(&file, id).await {
                Some((body, index)) => (body, Some(index)),
                None => (file.clone().into(), None),
            },
            None => (file.clone().into(), None),
        };
        if body.size() > maximum as f64 {
            return set(
                composer,
                id,
                Sending::Failed {
                    why: format!("The prepared file is too large: the limit is {}.", size_label(maximum)),
                },
            );
        }
        // A remuxed video goes as an MP4, whatever it was.
        let mime = if hls.is_some() { MediaType::Mp4.mime() } else { media.mime() };
        if media.is_video() && body.size() > allthing_core::media::VIDEO_PART_BYTES as f64 {
            LIVE.with(|live| {
                if let Some(live) = live.borrow_mut().get_mut(&id) {
                    live.body = Some(body.clone());
                    live.hls = hls.clone();
                }
            });
            if let Err(why) = send_chunks(composer, id, &body, mime, hls.as_deref()).await {
                set(composer, id, Sending::Failed { why });
            }
            return;
        }
        set(composer, id, Sending::Up { sent: 0.0, of: body.size() });
        // Removed meanwhile, or the draft cleared.
        let Some(request) = LIVE.with(|live| {
            let mut live = live.borrow_mut();
            let live = live.get_mut(&id)?;
            live.body = Some(body.clone());
            live.hls = hls.clone();
            let request = XmlHttpRequest::new().ok()?;
            live.request = Some(request.clone());
            Some(request)
        }) else {
            return;
        };
        let progress = Closure::<dyn FnMut(ProgressEvent)>::new(move |ev: ProgressEvent| {
            if ev.length_computable() {
                let state = if media.is_video() && ev.loaded() >= ev.total() {
                    Sending::Finishing
                } else {
                    Sending::Up { sent: ev.loaded(), of: ev.total() }
                };
                set(composer, id, state);
            }
        });
        let answer = request.clone();
        let ended = Closure::<dyn FnMut()>::new(move || {
            let state = match answer.status().unwrap_or(0) {
                0 => Sending::Failed { why: LOST.to_owned() },
                // `Uploaded`'s URL, or `ApiError`'s why, read by the browser's
                // own JSON parser: no deserializer of their own in the part.
                status => match (status, field(&answer, "url")) {
                    (200, Some(url)) => Sending::Done { url },
                    _ => Sending::Failed {
                        why: field(&answer, "error")
                            .unwrap_or_else(|| format!("The upload didn't go through (HTTP {status}).")),
                    },
                },
            };
            set(composer, id, state);
        });
        let sent = request.open_with_async("POST", MEDIA_PATH, true).is_ok()
            && request.set_request_header("content-type", mime).is_ok()
            && hls.as_deref().is_none_or(|index| request.set_request_header(fmp4::INDEX_HEADER, index).is_ok())
            && request.upload().is_ok_and(|upload| {
                upload.set_onprogress(Some(progress.as_ref().unchecked_ref()));
                true
            });
        request.set_onload(Some(ended.as_ref().unchecked_ref()));
        request.set_onerror(Some(ended.as_ref().unchecked_ref()));
        request.set_ontimeout(Some(ended.as_ref().unchecked_ref()));
        request.set_timeout(180_000);
        // Kept until the file is let go: a handler isn't dropped while it runs.
        LIVE.with(|live| {
            if let Some(live) = live.borrow_mut().get_mut(&id) {
                live.progress = Some(progress);
                live.ended = Some(ended);
            }
        });
        if !sent || request.send_with_opt_blob(Some(&body)).is_err() {
            set(composer, id, Sending::Failed { why: LOST.to_owned() });
        }
    });
}

/// A chunked upload keeps the acknowledged parts in this draft. Retry
/// continues with the first missing part, rather than sending a video again.
async fn send_chunks(composer: Composer, id: u64, body: &Blob, mime: &str, hls: Option<&str>) -> Result<(), String> {
    let kept = LIVE.with(|live| live.borrow().get(&id).and_then(|l| l.multipart.clone()));
    let mut upload = match kept {
        Some(kept) => kept,
        None => {
            let key = web_sys::window().and_then(|w| w.crypto().ok()).ok_or(LOST)?.random_uuid().replace('-', "");
            let upload = ChunkUpload {
                id: key,
                part_bytes: allthing_core::media::VIDEO_PART_BYTES,
                created: false,
                parts: Vec::new(),
            };
            LIVE.with(|live| {
                if let Some(live) = live.borrow_mut().get_mut(&id) {
                    live.multipart = Some(upload.clone());
                }
            });
            upload
        }
    };
    if !upload.created {
        let data = serde_json::json!({"id":upload.id,"type": mime, "bytes": body.size() as u64, "hls": hls});
        let created = chunk_request(
            composer,
            id,
            &format!("{MEDIA_PATH}?upload=create"),
            json_blob(&data)?,
            "application/json",
            None,
        )
        .await?;
        if created.get("id").and_then(serde_json::Value::as_str) != Some(upload.id.as_str())
            || created.get("part_bytes").and_then(serde_json::Value::as_u64) != Some(upload.part_bytes)
        {
            return Err(LOST.to_owned());
        }
        upload.created = true;
        let kept = LIVE.with(|live| {
            let mut live = live.borrow_mut();
            let Some(live) = live.get_mut(&id) else { return false };
            live.multipart = Some(upload.clone());
            true
        });
        if !kept {
            abort_upload(&upload.id);
            return Ok(());
        }
    }
    let total = body.size() as u64;
    let count = total.div_ceil(upload.part_bytes);
    while (upload.parts.len() as u64) < count {
        if !LIVE.with(|live| live.borrow().contains_key(&id)) {
            return Ok(());
        }
        let number = upload.parts.len() as u32 + 1;
        let first = u64::from(number - 1) * upload.part_bytes;
        let end = (first + upload.part_bytes).min(total);
        let part = body.slice_with_f64_and_f64(first as f64, end as f64).map_err(|_| LOST)?;
        set(composer, id, Sending::Up { sent: first as f64, of: total as f64 });
        let url = format!("{MEDIA_PATH}?upload=part&id={}&part={number}", upload.id);
        let answer = chunk_request(composer, id, &url, part, "application/octet-stream", Some((first, total))).await?;
        if answer.get("part_number").and_then(serde_json::Value::as_u64) != Some(u64::from(number)) {
            return Err(LOST.to_owned());
        }
        let etag = answer
            .get("etag")
            .and_then(serde_json::Value::as_str)
            .filter(|tag| !tag.is_empty() && tag.len() <= 512)
            .ok_or(LOST)?;
        upload.parts.push((number, etag.to_owned()));
        LIVE.with(|live| {
            if let Some(live) = live.borrow_mut().get_mut(&id) {
                live.multipart = Some(upload.clone());
            }
        });
    }
    if !LIVE.with(|live| live.borrow().contains_key(&id)) {
        return Ok(());
    }
    set(composer, id, Sending::Finishing);
    let parts: Vec<_> =
        upload.parts.iter().map(|(number, etag)| serde_json::json!({"part_number":number,"etag":etag})).collect();
    let url = format!("{MEDIA_PATH}?upload=complete&id={}", upload.id);
    let answer =
        chunk_request(composer, id, &url, json_blob(&serde_json::json!({"parts":parts}))?, "application/json", None)
            .await?;
    let url = answer.get("url").and_then(serde_json::Value::as_str).ok_or(LOST)?;
    LIVE.with(|live| {
        if let Some(live) = live.borrow_mut().get_mut(&id) {
            live.multipart = None;
        }
    });
    set(composer, id, Sending::Done { url: url.to_owned() });
    Ok(())
}

fn json_blob(value: &serde_json::Value) -> Result<Blob, String> {
    let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(&value.to_string()));
    Blob::new_with_str_sequence(&parts).map_err(|_| LOST.to_owned())
}

/// One network request, with its callbacks scoped to this future. Aborting
/// a removed attachment fires loadend too, so nothing waits indefinitely.
async fn chunk_request(
    composer: Composer,
    id: u64,
    url: &str,
    body: Blob,
    mime: &str,
    progress: Option<(u64, u64)>,
) -> Result<serde_json::Value, String> {
    let request = XmlHttpRequest::new().map_err(|_| LOST)?;
    request.open_with_async("POST", url, true).map_err(|_| LOST)?;
    request.set_timeout(180_000);
    request.set_request_header("content-type", mime).map_err(|_| LOST)?;
    let live = LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let Some(live) = live.get_mut(&id) else { return false };
        live.request = Some(request.clone());
        true
    });
    if !live {
        return Err(LOST.to_owned());
    }
    let progress_handler = Closure::<dyn FnMut(ProgressEvent)>::new(move |ev: ProgressEvent| {
        if let Some((offset, total)) = progress {
            set(composer, id, Sending::Up { sent: (offset as f64 + ev.loaded()).min(total as f64), of: total as f64 });
        }
    });
    let upload = request.upload().map_err(|_| LOST)?;
    upload.set_onprogress(Some(progress_handler.as_ref().unchecked_ref()));
    let (promise, ended) = {
        let mut ended = None;
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let callback = Closure::<dyn FnMut()>::new(move || {
                let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
            });
            request.set_onloadend(Some(callback.as_ref().unchecked_ref()));
            ended = Some(callback);
        });
        (promise, ended)
    };
    let sent = request.send_with_opt_blob(Some(&body));
    if sent.is_ok() {
        let _ = JsFuture::from(promise).await;
    }
    request.set_onloadend(None);
    upload.set_onprogress(None);
    drop(ended);
    drop(progress_handler);
    LIVE.with(|live| {
        if let Some(live) = live.borrow_mut().get_mut(&id) {
            live.request = None;
        }
    });
    if sent.is_err() {
        return Err(LOST.to_owned());
    }
    let status = request.status().unwrap_or(0);
    let text = request.response_text().ok().flatten().unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if matches!(status, 404 | 410) {
        LIVE.with(|live| {
            if let Some(live) = live.borrow_mut().get_mut(&id) {
                live.multipart = None;
            }
        });
    }
    if status != 200 {
        return Err(value.get("error").and_then(serde_json::Value::as_str).unwrap_or(LOST).to_owned());
    }
    Ok(value)
}

/// Cancel on Remove. The server also reclaims abandoned uploads, including
/// when a browser disappears before it can send this request.
pub fn abort_upload(id: &str) {
    let url = format!("{MEDIA_PATH}?upload=abort&id={id}");
    spawn_local(async move {
        let options = web_sys::RequestInit::new();
        options.set_method("POST");
        let _ = js_sys::Reflect::set(options.as_ref(), &"keepalive".into(), &true.into());
        if let Some(window) = web_sys::window() {
            let _ = JsFuture::from(window.fetch_with_str_and_init(&url, &options)).await;
        }
    });
}

/// A text field of the JSON `request` was answered with.
fn field(request: &XmlHttpRequest, name: &str) -> Option<String> {
    let text = request.response_text().ok().flatten()?;
    let answer = js_sys::JSON::parse(&text).ok()?;
    js_sys::Reflect::get(&answer, &name.into()).ok()?.as_string()
}

/// A picture's bytes, read and put back together without its metadata, as
/// a body of its type.
async fn without_metadata(file: &File, media: MediaType) -> Result<Blob, String> {
    let unreadable = || format!("{} couldn't be read as {}.", file.name(), kind(media));
    let buffer = JsFuture::from(file.array_buffer()).await.map_err(|_| unreadable())?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    let clean = metadata::without_metadata(media, &bytes).ok_or_else(unreadable)?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(clean.as_slice()));
    let options = BlobPropertyBag::new();
    options.set_type(media.mime());
    Blob::new_with_u8_array_sequence_and_options(&parts, &options).map_err(|_| unreadable())
}

/// A video as a fragmented MP4 and its HLS index, if it can be remuxed.
async fn remuxed(file: &File, id: u64) -> Option<(Blob, String)> {
    let length = file.size() as u64;
    let mut offset = 0_u64;
    // Top-level boxes can put moov after a very large mdat. Read only
    // headers and moov: never copy the video's payload into WebAssembly.
    for n in 0..1024 {
        if n % 16 == 0 && !yield_upload(id).await {
            return None;
        }
        if offset.checked_add(8)? > length {
            return None;
        }
        let head = read_slice(file, offset, (offset + 16).min(length)).await?;
        let short = u32::from_be_bytes(head.get(..4)?.try_into().ok()?) as u64;
        let (size, header) = match short {
            0 => (length - offset, 8),
            1 => (u64::from_be_bytes(head.get(8..16)?.try_into().ok()?), 16),
            n => (n, 8),
        };
        if size < header || offset.checked_add(size)? > length {
            return None;
        }
        if head.get(4..8)? == b"moov" {
            if size - header > fmp4::MAX_MOOV_BYTES as u64 {
                return None;
            }
            let moov = read_slice(file, offset + header, offset + size).await?;
            if !yield_upload(id).await {
                return None;
            }
            let plan = fmp4::plan(&moov, length).ok()?;
            let parts = js_sys::Array::new();
            for (n, part) in plan.parts.into_iter().enumerate() {
                if n % 256 == 0 && !yield_upload(id).await {
                    return None;
                }
                match part {
                    fmp4::RemuxPart::Bytes(bytes) => {
                        parts.push(&js_sys::Uint8Array::from(bytes.as_slice()));
                    }
                    fmp4::RemuxPart::Source { offset, len } => {
                        parts.push(&file.slice_with_f64_and_f64(offset as f64, (offset + len) as f64).ok()?.into());
                    }
                }
            }
            let options = BlobPropertyBag::new();
            options.set_type(MediaType::Mp4.mime());
            let blob = Blob::new_with_blob_sequence_and_options(&parts, &options).ok()?;
            return Some((blob, plan.index.encode()));
        }
        offset += size;
    }
    None
}

/// Give paint, input and cancellation a task between bounded preparation steps.
async fn yield_upload(id: u64) -> bool {
    if !LIVE.with(|live| live.borrow().contains_key(&id)) {
        return false;
    }
    if let Some(window) = web_sys::window() {
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            if window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0).is_err() {
                let _ = reject.call0(&wasm_bindgen::JsValue::NULL);
            }
        });
        let _ = JsFuture::from(promise).await;
    }
    LIVE.with(|live| live.borrow().contains_key(&id))
}

async fn read_slice(file: &File, first: u64, end: u64) -> Option<Vec<u8>> {
    let blob = file.slice_with_f64_and_f64(first as f64, end as f64).ok()?;
    let buffer = JsFuture::from(blob.array_buffer()).await.ok()?;
    Some(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// "a JPEG", for what is said of a file.
fn kind(media: MediaType) -> &'static str {
    match media {
        MediaType::Png => "a PNG",
        MediaType::Jpeg => "a JPEG",
        MediaType::Gif => "a GIF",
        MediaType::Webp => "a WebP image",
        MediaType::Mp4 => "an MP4",
        MediaType::Mov => "a MOV",
        MediaType::Webm => "a WebM",
    }
}

/// The attach button, and its file picker, a phone's photos: there once
/// this server has said it takes files, and only then. With no room left
/// (two embeds), it says so instead.
#[component]
pub fn AttachButton(composer: Composer) -> impl IntoView {
    let input = NodeRef::<leptos::html::Input>::new();
    let full = Memo::new(move |_| composer.embeds_left() == 0);
    let limits = Memo::new(move |_| composer.limits.get());
    move || {
        limits.get().map(|limits| {
            let hint = format!(
                "Images up to {}, videos up to {}",
                size_label(limits.max_image_bytes),
                size_label(limits.max_video_bytes)
            );
            view! {
                <button
                    type="button"
                    class="compose-tool attach-button"
                    aria-label="Attach images or videos"
                    aria-disabled=move || full.get().then_some("true")
                    aria-description=move || if full.get() { EMBEDS_FULL.to_owned() } else { hint.clone() }
                    on:click=move |_| {
                        if full.get_untracked() {
                            composer.note.set(Some(EMBEDS_FULL.to_owned()));
                        } else if let Some(input) = input.get_untracked() {
                            input.click();
                        }
                    }
                >
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=PICTURE/><circle cx="10.6" cy="6.4" r="1"/></svg>
                </button>
                <input
                    type="file"
                    class="attach-input"
                    node_ref=input
                    multiple=true
                    accept=accepted()
                    tabindex="-1"
                    aria-hidden="true"
                    on:change=move |_| {
                        let Some(input) = input.get_untracked() else { return };
                        let picked = input.files().map(|list| (0..list.length()).filter_map(|i| list.get(i)).collect::<Vec<_>>());
                        // The same file picked again is a change too.
                        input.set_value("");
                        add(composer, picked.unwrap_or_default());
                    }
                />
            }
        })
    }
}

/// The draft's files, as tiles, in order: each with its preview, how far
/// its upload has gone, and Remove; under them, why any didn't go, with
/// Retry.
#[component]
pub fn Attached(composer: Composer) -> impl IntoView {
    let ids = Memo::new(move |_| composer.files.with(|files| files.iter().map(|f| f.id).collect::<Vec<_>>()));
    let failed = Memo::new(move |_| {
        composer.files.with(|files| {
            files
                .iter()
                .filter_map(|f| match &f.state {
                    Sending::Failed { why } => Some((f.id, f.name.clone(), why.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    });
    view! {
        <Show when=move || ids.with(|ids| !ids.is_empty())>
            <ul class="compose-media" aria-label="Attached">
                <For each=move || ids.get() key=|id| *id children=move |id| view! { <Tile composer id/> }/>
            </ul>
        </Show>
        {move || {
            failed.get().into_iter().map(|(id, name, why)| view! {
                <p class="compose-media-error" role="alert">
                    <span>{format!("{name}: {why}")}</span>
                    <button type="button" class="link-button" on:click=move |_| retry(composer, id)>"Retry"</button>
                    <button type="button" class="link-button" on:click=move |_| composer.remove_file(id)>"Remove"</button>
                </p>
            }).collect_view()
        }}
    }
}

/// A file's tile: its picture, or its video's first frame, with how far
/// its upload has gone.
#[component]
fn Tile(composer: Composer, id: u64) -> impl IntoView {
    let Some(file) = composer.files.with_untracked(|files| files.iter().find(|f| f.id == id).cloned()) else {
        return ().into_any();
    };
    let state =
        Memo::new(move |_| composer.files.with(|files| files.iter().find(|f| f.id == id).map(|f| f.state.clone())));
    let preview = file.preview();
    let name = file.name.clone();
    let shown = if file.media.is_video() {
        view! {
            <video src=format!("{preview}#t=0.1") muted=true prop:muted=true playsinline=true preload="metadata" aria-label=name.clone()></video>
            <span class="compose-tile-video" aria-hidden="true">"Video"</span>
        }
        .into_any()
    } else {
        view! { <img src=preview alt=name.clone()/> }.into_any()
    };
    let label = format!("Remove {name}");
    let progress = move || match state.get() {
        Some(Sending::Preparing | Sending::Finishing) => {
            let label = if matches!(state.get(), Some(Sending::Preparing)) {
                "Preparing video…"
            } else {
                "Finishing upload…"
            };
            Some(view! { <span class="compose-processing" role="status">{label}</span> }.into_any())
        }
        Some(Sending::Up { sent, of }) => {
            let percent = if of > 0.0 { (sent / of * 100.0).clamp(0.0, 100.0).round() } else { 0.0 };
            let name = name.clone();
            Some(
                view! {
                    <div
                        class="compose-progress"
                        role="progressbar"
                        aria-label=format!("Uploading {name}")
                        aria-valuemin="0"
                        aria-valuemax="100"
                        aria-valuenow=percent.to_string()
                    >
                        <span style=format!("width: {percent}%")></span>
                    </div>
                }
                .into_any(),
            )
        }
        Some(Sending::Failed { .. }) => {
            Some(view! { <span class="compose-tile-failed" aria-hidden="true">"!"</span> }.into_any())
        }
        _ => None,
    };
    view! {
        <li
            class="compose-tile"
            class:sending=move || matches!(state.get(), Some(Sending::Preparing | Sending::Up { .. } | Sending::Finishing))
            class:failed=move || matches!(state.get(), Some(Sending::Failed { .. }))
        >
            {shown}
            {progress}
            <button type="button" class="compose-tile-remove" aria-label=label on:click=move |_| composer.remove_file(id)>"×"</button>
        </li>
    }
    .into_any()
}
