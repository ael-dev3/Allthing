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
//! In the composer's part (`split_composer.wasm`), with the composer's
//! pickers and bar.

use allthing_core::media::{EMBEDS_FULL, MEDIA_PATH, MediaType, accepted, check_file, size_label};
use allthing_core::metadata;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, BlobPropertyBag, File, ProgressEvent, XmlHttpRequest};

use crate::components::compose::{Attachment, Composer, INCOMING, LIVE, Live, Sending};

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
    let state = Sending::Up { sent: 0.0, of: file.size() };
    composer.files.update(|files| files.push(Attachment { id, name: file.name(), media, local, state }));
    LIVE.with(|live| {
        live.borrow_mut().insert(id, Live { file, body: None, request: None, progress: None, ended: None })
    });
    send(composer, id);
}

/// Try a file that didn't go again.
pub fn retry(composer: Composer, id: u64) {
    if LIVE.with(|live| live.borrow().contains_key(&id)) {
        set(composer, id, Sending::Up { sent: 0.0, of: 0.0 });
        send(composer, id);
    }
}

fn set(composer: Composer, id: u64, state: Sending) {
    composer.files.update(|files| {
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
        let Some((file, kept)) = LIVE.with(|live| live.borrow().get(&id).map(|l| (l.file.clone(), l.body.clone())))
        else {
            return;
        };
        let body = match kept {
            Some(body) => body,
            None if metadata::stripped(media) => match without_metadata(&file, media).await {
                Ok(body) => body,
                Err(why) => return set(composer, id, Sending::Failed { why }),
            },
            None => file.clone().into(),
        };
        // Removed meanwhile, or the draft cleared.
        let Some(request) = LIVE.with(|live| {
            let mut live = live.borrow_mut();
            let live = live.get_mut(&id)?;
            live.body = Some(body.clone());
            let request = XmlHttpRequest::new().ok()?;
            live.request = Some(request.clone());
            Some(request)
        }) else {
            return;
        };
        let progress = Closure::<dyn FnMut(ProgressEvent)>::new(move |ev: ProgressEvent| {
            if ev.length_computable() {
                set(composer, id, Sending::Up { sent: ev.loaded(), of: ev.total() });
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
            && request.set_request_header("content-type", media.mime()).is_ok()
            && request.upload().is_ok_and(|upload| {
                upload.set_onprogress(Some(progress.as_ref().unchecked_ref()));
                true
            });
        request.set_onload(Some(ended.as_ref().unchecked_ref()));
        request.set_onerror(Some(ended.as_ref().unchecked_ref()));
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
                    class="attach-button"
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
            class:sending=move || matches!(state.get(), Some(Sending::Up { .. }))
            class:failed=move || matches!(state.get(), Some(Sending::Failed { .. }))
        >
            {shown}
            {progress}
            <button type="button" class="compose-tile-remove" aria-label=label on:click=move |_| composer.remove_file(id)>"×"</button>
        </li>
    }
    .into_any()
}
