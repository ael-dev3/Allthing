// SPDX-License-Identifier: AGPL-3.0-or-later
//! Readers' attachments (see `allthing_core::media`): `/api/v1/media`, where
//! a signed-in reader's browser uploads an image or a video for a cast, and
//! `/m/<fid>/<id>.<ext>`, where the Worker serves it, to anyone, from R2
//! (the `MEDIA` binding).
//!
//! An upload's body is never read here. The request's own `ReadableStream`
//! goes to R2's `put` as a JavaScript value, so its bytes flow from the
//! client to R2 inside the runtime, and none passes through WebAssembly: an
//! upload takes the same CPU time for 1 KB as for 50 MB. Serving is the same
//! the other way round: R2's body stream is the response's.
//!
//! A video the browser remuxed for HLS (`allthing_core::fmp4`) comes with
//! its index, in a header, which is kept as the file's custom metadata in
//! R2; its two playlists are written from that at `/m/<fid>/<id>.m3u8` and
//! `.v.m3u8`, from the file's metadata alone: one `head`, no body read.
//!
//! Without the binding (until the owner sets R2 up, docs/DEPLOY.md), an
//! upload answers 404, and so does every `/m/` path; `/api/v1/status` says
//! `media.ready: false`.

use std::cell::RefCell;

use std::collections::HashMap;
use std::future::Future;

use serde::de::DeserializeOwned;

use allthing_core::fmp4::{self, HlsIndex};
use allthing_core::media::{
    self, ByteRange, CompleteVideoUpload, LIMITS, MAX_EMBED_URL_BYTES, MediaKey, MediaType, NewVideoUpload, Playlist,
    RangeRead, Refused, UploadLimiter, Uploaded, VIDEO_PART_BYTES, VideoPart, VideoUploadSession, admit, check_upload,
    complete_video_parts, content_range, none_match, preflight_headers, served_headers, served_type, unsatisfied_range,
    video_part_bytes,
};
use futures::future::FutureExt;
use worker::web_sys::ReadableStream;
use worker::{
    Bucket, Context, Env, Headers, HttpMetadata, Method, Object, Range, Request, Response, ResponseBuilder, Result,
    console_error,
};

use crate::auth::{json, random_hex, signed_in_fid};
use crate::error;
use crate::index::{D1Index, MediaUploadRow};

/// The R2 binding attachments are kept in (bucket `allthing-media`).
const BUCKET: &str = "MEDIA";
const UNSET: &str = "Attachments aren't set up on this server";
const SIGN_IN: &str = "Sign in to attach images and videos";
const NOT_KEPT: Refused =
    Refused { status: 503, message: "That file couldn't be kept just now; try again", retry_after: Some(5) };
const NOT_WHOLE: Refused =
    Refused { status: 400, message: "That file didn't arrive whole; try again", retry_after: None };
const BAD_INDEX: Refused =
    Refused { status: 400, message: "That video's stream index doesn't match it; try again", retry_after: None };

thread_local! {
    /// Uploads each reader started in the past minute, in this isolate.
    static LIMITER: RefCell<UploadLimiter> = RefCell::new(UploadLimiter::default());
}

/// The bucket, if this Worker has the binding.
fn bucket(env: &Env) -> Option<Bucket> {
    env.bucket(BUCKET).ok()
}

/// Whether this server takes attachments: the bucket to keep them in, and
/// D1, which counts what they take.
pub fn ready(env: &Env) -> bool {
    bucket(env).is_some() && D1Index::new(env).is_some()
}

/// `/api/v1/media`: GET (or HEAD) says whether attachments can be uploaded
/// here, and their limits; POST uploads one. `now_ms` is Unix milliseconds.
pub async fn handle(req: Request, env: &Env, ctx: &Context, now_ms: f64) -> Result<Response> {
    match req.method() {
        Method::Get | Method::Head => limits(&req, env, (now_ms / 1000.0) as i64),
        Method::Post if req.url()?.query_pairs().any(|(key, _)| key == "upload") => {
            multipart(req, env, ctx, now_ms).await
        }
        Method::Post => upload(req, env, ctx, now_ms).await,
        _ => error(405, "Attachments take GET, and POST", None),
    }
}

/// 200 with the limits for a session where attachments are set up; 404
/// where they aren't (as `/api/v1/status` says to anyone), else 401. It
/// makes no subrequest.
fn limits(req: &Request, env: &Env, now: i64) -> Result<Response> {
    if !ready(env) {
        return error(404, UNSET, None);
    }
    if signed_in_fid(req, env, now)?.is_none() {
        return error(401, SIGN_IN, None);
    }
    let headers = Headers::new();
    headers.set("content-type", "application/json; charset=utf-8")?;
    headers.set("cache-control", "private, max-age=300")?;
    Ok(Response::from_json(&LIMITS)?.with_headers(headers))
}

fn refuse(refused: Refused) -> Result<Response> {
    error(refused.status, refused.message, refused.retry_after)
}

const MAX_JSON_BYTES: usize = 24 * 1024;
const SINGLE_VIDEO_BYTES: u64 = 50 * 1024 * 1024;
const BUSY: Refused =
    Refused { status: 409, message: "That upload is busy; try again in a moment", retry_after: Some(2) };
const EXPIRED: Refused =
    Refused { status: 410, message: "That upload expired; remove it and attach the video again", retry_after: None };
const BAD_PART: Refused =
    Refused { status: 400, message: "That video's upload parts don't match; try again", retry_after: None };
const OPERATIONS_FULL: Refused = Refused {
    status: 503,
    message: "Allthing has taken all the upload work it can today; try again tomorrow",
    retry_after: Some(3600),
};

fn unavailable(failure: impl std::fmt::Display) -> Refused {
    console_error!("media: {failure}");
    NOT_KEPT
}

/// Mutations settle under waitUntil even if their response loses its reader.
async fn durable<T: Clone + 'static>(
    ctx: &Context,
    future: impl Future<Output = std::result::Result<T, Refused>> + 'static,
) -> std::result::Result<T, Refused> {
    let kept = future.boxed_local().shared();
    let background = kept.clone();
    ctx.wait_until(async move {
        let _ = background.await;
    });
    kept.await
}

/// Control JSON is bounded before and after reading. File bodies never enter Wasm.
async fn control<T: DeserializeOwned>(req: &mut Request) -> Option<T> {
    let size = req.headers().get("content-length").ok().flatten()?.parse::<usize>().ok()?;
    if size == 0 || size > MAX_JSON_BYTES {
        return None;
    }
    let body = req.text().await.ok()?;
    (body.len() == size && body.len() <= MAX_JSON_BYTES).then(|| serde_json::from_str(&body).ok()).flatten()
}

fn hls_index(media: MediaType, length: u64, line: Option<&str>) -> std::result::Result<Option<HlsIndex>, Refused> {
    match line {
        None => Ok(None),
        Some(line) => match HlsIndex::parse(line) {
            Some(index) if media == MediaType::Mp4 && index.total() == length => Ok(Some(index)),
            _ => Err(BAD_INDEX),
        },
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn new_row(
    fid: u64,
    media: MediaType,
    length: u64,
    hls: Option<String>,
    mode: &str,
    now: i64,
    correlation: Option<String>,
) -> Option<MediaUploadRow> {
    let id = correlation.or_else(|| random_hex(16))?;
    let key = MediaKey::new(fid, &id, media)?;
    Some(MediaUploadRow {
        id,
        key: key.object(),
        fid,
        bytes: length,
        media_type: media.mime().to_owned(),
        hls,
        upload_id: None,
        mode: mode.to_owned(),
        state: "active".to_owned(),
        parts: "[]".to_owned(),
        created_at: now,
        expires_at: now + media::UPLOAD_IDLE_SECONDS,
        lock_id: random_hex(16)?,
        lock_until: now + media::UPLOAD_LEASE_SECONDS,
        cancel_requested: 0,
    })
}

fn uploaded(row: &MediaUploadRow, origin: &str) -> std::result::Result<Uploaded, Refused> {
    let key = MediaKey::from_path(&format!("/m/{}", row.key)).ok_or(NOT_KEPT)?;
    let url = if row.hls.is_some() { format!("{origin}{}", key.playlist_path()) } else { key.url(origin) };
    if url.len() > MAX_EMBED_URL_BYTES {
        return Err(NOT_KEPT);
    }
    Ok(Uploaded { url, media_type: row.media_type.clone(), bytes: row.bytes })
}

async fn reserve(index: &D1Index, row: &MediaUploadRow) -> std::result::Result<(), Refused> {
    if index.reserve_media(row).await.map_err(unavailable)? {
        return Ok(());
    }
    if index.media_cancelled(&row.id, row.fid, row.created_at).await.map_err(unavailable)? {
        return Err(EXPIRED);
    }
    let usage = index.media_usage(row.fid, row.created_at).await.map_err(unavailable)?;
    admit(&usage, row.bytes, row.created_at)?;
    Err(Refused {
        status: 409,
        message: "Finish or remove another attachment before starting this one",
        retry_after: Some(5),
    })
}

async fn operation(index: &D1Index, now: i64) -> std::result::Result<(), Refused> {
    if index.media_operation(now).await.map_err(unavailable)? {
        Ok(())
    } else {
        Err(Refused { retry_after: Some((86_400 - now.rem_euclid(86_400)) as u32), ..OPERATIONS_FULL })
    }
}

/// The legacy single-body endpoint remains for images and small videos.
async fn upload(req: Request, env: &Env, ctx: &Context, now_ms: f64) -> Result<Response> {
    let now = (now_ms / 1000.0) as i64;
    let origin = req.url()?.origin().ascii_serialization();
    if req.headers().get("origin")?.as_deref() != Some(origin.as_str()) {
        return error(403, "Uploads must come from Allthing itself", None);
    }
    let (Some(bucket), Some(index)) = (bucket(env), D1Index::new(env)) else { return error(404, UNSET, None) };
    let Some(fid) = signed_in_fid(&req, env, now)? else { return error(401, SIGN_IN, None) };
    let declared = req.headers().get("content-type")?;
    let (media, length) = match check_upload(declared.as_deref(), req.headers().get("content-length")?.as_deref()) {
        Ok(checked) => checked,
        Err(refused) => return refuse(refused),
    };
    if media.is_video() && length > SINGLE_VIDEO_BYTES {
        return error(413, "Large videos use a chunked upload: attach the video again in Allthing", None);
    }
    let hls = req.headers().get(fmp4::INDEX_HEADER)?;
    if let Err(refused) = hls_index(media, length, hls.as_deref()) {
        return refuse(refused);
    }
    if let Err(wait) = LIMITER.with(|limiter| limiter.borrow_mut().allow(fid, now_ms)) {
        return error(429, "That's a lot of uploads at once; try again in a moment", Some(wait));
    }
    let Some(row) = new_row(fid, media, length, hls, "single", now, None) else { return refuse(NOT_KEPT) };
    let answer = match uploaded(&row, &origin) {
        Ok(answer) => answer,
        Err(why) => return refuse(why),
    };
    let Some(body) = req.inner().body() else { return refuse(media::EMPTY) };
    match durable(ctx, keep_single(bucket, index, row, body, now)).await {
        Ok(()) => json(&answer, &[]),
        Err(why) => refuse(why),
    }
}

async fn keep_single(
    bucket: Bucket,
    index: D1Index,
    mut row: MediaUploadRow,
    body: ReadableStream,
    now: i64,
) -> std::result::Result<(), Refused> {
    operation(&index, now).await?;
    reserve(&index, &row).await?;
    // Even the legacy single-body route uses an abortable R2 multipart.
    // Otherwise a very slow put could finish after its expired reservation
    // was cleaned and leave an uncounted object behind.
    let metadata = HttpMetadata { content_type: Some(row.media_type.clone()), ..HttpMetadata::default() };
    let mut create = bucket.create_multipart_upload(row.key.as_str()).http_metadata(metadata);
    if let Some(hls) = &row.hls {
        create = create.custom_metadata(HashMap::from([(fmp4::INDEX_METADATA.to_owned(), hls.clone())]));
    }
    let upload = match create.execute().await {
        Ok(upload) => upload,
        Err(why) => {
            let _ = index.forget_media(&row).await;
            return Err(unavailable(why));
        }
    };
    row.upload_id = Some(upload.upload_id().await);
    match index.media_open(&row, row.upload_id.as_deref().unwrap(), now).await {
        Ok(true) => {}
        result => {
            if upload.abort().await.is_ok() {
                let _ = index.forget_media(&row).await;
            }
            return Err(match result {
                Err(why) => unavailable(why.0),
                _ => EXPIRED,
            });
        }
    }
    if let Err(why) = operation(&index, now).await {
        let _ = discard(&bucket, &index, &row).await;
        return Err(why);
    }
    let part = match upload.upload_part(1, body).await {
        Ok(part) => part,
        Err(why) => {
            let _ = discard(&bucket, &index, &row).await;
            return Err(unavailable(why));
        }
    };
    if let Err(why) = operation(&index, now).await {
        let _ = discard(&bucket, &index, &row).await;
        return Err(why);
    }
    match upload.complete([part]).await {
        Ok(stored) if stored.size() == row.bytes => match index.finish_media(&row, now).await {
            Ok(true) => return Ok(()),
            Err(why) => {
                let _ = discard(&bucket, &index, &row).await;
                return Err(unavailable(why.0));
            }
            _ => {}
        },
        Ok(_) => {
            let _ = discard(&bucket, &index, &row).await;
            return Err(NOT_WHOLE);
        }
        Err(why) => {
            let _ = discard(&bucket, &index, &row).await;
            return Err(unavailable(why));
        }
    }
    let _ = discard(&bucket, &index, &row).await;
    Err(NOT_KEPT)
}

/// Session control and parts use the same authenticated same-origin route.
async fn multipart(mut req: Request, env: &Env, ctx: &Context, now_ms: f64) -> Result<Response> {
    let now = (now_ms / 1000.0) as i64;
    let url = req.url()?;
    let origin = url.origin().ascii_serialization();
    if req.headers().get("origin")?.as_deref() != Some(origin.as_str()) {
        return error(403, "Uploads must come from Allthing itself", None);
    }
    let (Some(bucket), Some(index)) = (bucket(env), D1Index::new(env)) else { return error(404, UNSET, None) };
    let Some(fid) = signed_in_fid(&req, env, now)? else { return error(401, SIGN_IN, None) };
    let query = |name: &str| url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned());
    let action = query("upload").unwrap_or_default();
    if action == "create" {
        let Some(asked) = control::<NewVideoUpload>(&mut req).await else {
            return error(400, "That upload request couldn't be read", None);
        };
        let (media, length) = match check_upload(Some(&asked.media_type), Some(&asked.bytes.to_string())) {
            Ok(pair) if pair.0.is_video() => pair,
            Ok(_) => return error(400, "Chunked uploads are for videos", None),
            Err(why) => return refuse(why),
        };
        if let Err(why) = hls_index(media, length, asked.hls.as_deref()) {
            return refuse(why);
        }
        if let Err(wait) = LIMITER.with(|limiter| limiter.borrow_mut().allow(fid, now_ms)) {
            return error(429, "That's a lot of uploads at once; try again in a moment", Some(wait));
        }
        if asked.id.as_ref().is_some_and(|id| !valid_id(id)) {
            return error(400, "That upload id couldn't be read", None);
        }
        if let Some(id) = &asked.id {
            match index.media_cancelled(id, fid, now).await {
                Ok(true) => return refuse(EXPIRED),
                Ok(false) => {}
                Err(why) => return refuse(unavailable(why.0)),
            }
            match index.media_upload(id, fid).await {
                Ok(Some(row)) => {
                    if row.mode != "multipart"
                        || row.bytes != length
                        || row.media_type != media.mime()
                        || row.hls != asked.hls
                    {
                        return error(409, "That upload id belongs to a different attachment", None);
                    }
                    if row.state != "active" && row.state != "done"
                        || row.cancel_requested != 0
                        || row.expires_at <= now
                    {
                        return refuse(EXPIRED);
                    }
                    if row.upload_id.is_none() {
                        return refuse(BUSY);
                    }
                    return json(&VideoUploadSession { id: row.id, part_bytes: VIDEO_PART_BYTES }, &[]);
                }
                Ok(None) => {}
                Err(why) => return refuse(unavailable(why.0)),
            }
        }
        let Some(row) = new_row(fid, media, length, asked.hls, "multipart", now, asked.id) else {
            return refuse(NOT_KEPT);
        };
        if uploaded(&row, &origin).is_err() {
            return refuse(NOT_KEPT);
        }
        return match durable(ctx, create_video(bucket, index, row, now)).await {
            Ok(answer) => json(&answer, &[]),
            Err(why) => refuse(why),
        };
    }
    let Some(id) = query("id").filter(|id| valid_id(id)) else {
        return error(400, "That upload id couldn't be read", None);
    };
    let existing = match index.media_upload(&id, fid).await {
        Ok(row) => row,
        Err(why) => return refuse(unavailable(why.0)),
    };
    if action == "abort" {
        // Known files can always be cancelled, even after today's write
        // budget is exhausted. Unknown-id tombstones share that budget.
        if existing.is_none() {
            match index.media_cancelled(&id, fid, now).await {
                Ok(true) => return Ok(Response::empty()?.with_status(204)),
                Ok(false) => {}
                Err(why) => return refuse(unavailable(why.0)),
            }
            if let Err(why) = operation(&index, now).await {
                return refuse(why);
            }
        }
        if let Err(why) = index.cancel_media(&id, fid, now).await {
            return refuse(unavailable(why.0));
        }
    }
    let row = match existing {
        Some(row) if row.mode == "multipart" => row,
        _ if action == "abort" => return Ok(Response::empty()?.with_status(204)),
        _ => return error(404, "That upload wasn't found", None),
    };
    if action == "abort" {
        if row.state == "done" {
            return Ok(Response::empty()?.with_status(204));
        }
        if operation(&index, now).await.is_err() {
            return Ok(Response::empty()?.with_status(204));
        }
        return match durable(ctx, abort_video(bucket, index, row, now)).await {
            Ok(()) => Ok(Response::empty()?.with_status(204)),
            Err(why) => refuse(why),
        };
    }
    if action == "complete" {
        let Some(asked) = control::<CompleteVideoUpload>(&mut req).await else {
            return error(400, "That upload request couldn't be read", None);
        };
        let parts: Vec<VideoPart> = serde_json::from_str(&row.parts).unwrap_or_default();
        if !complete_video_parts(row.bytes, &asked.parts) || asked.parts != parts {
            return refuse(BAD_PART);
        }
        let answer = match uploaded(&row, &origin) {
            Ok(answer) => answer,
            Err(why) => return refuse(why),
        };
        if row.state == "done" {
            return json(&answer, &[]);
        }
        if row.cancel_requested != 0 || row.expires_at <= now {
            return refuse(EXPIRED);
        }
        return match durable(ctx, complete_video(bucket, index, row, asked.parts, now)).await {
            Ok(()) => json(&answer, &[]),
            Err(why) => refuse(why),
        };
    }
    if action != "part" {
        return error(400, "Unknown upload operation", None);
    }
    if row.state != "active" || row.cancel_requested != 0 || row.expires_at <= now {
        return refuse(EXPIRED);
    }
    let Some(part) = query("part").and_then(|part| part.parse::<u16>().ok()) else { return refuse(BAD_PART) };
    let expected = video_part_bytes(row.bytes, part);
    let declared = req.headers().get("content-length")?.and_then(|value| value.parse::<u64>().ok());
    if expected.is_none() || expected != declared {
        return refuse(BAD_PART);
    }
    let parts: Vec<VideoPart> = serde_json::from_str(&row.parts).unwrap_or_default();
    if let Some(kept) = parts.get(usize::from(part - 1)) {
        return json(kept, &[]);
    }
    if usize::from(part) != parts.len() + 1 {
        return refuse(BAD_PART);
    }
    let Some(body) = req.inner().body() else { return refuse(media::EMPTY) };
    match durable(ctx, keep_part(bucket, index, row, part, body, now)).await {
        Ok(answer) => json(&answer, &[]),
        Err(why) => refuse(why),
    }
}

async fn create_video(
    bucket: Bucket,
    index: D1Index,
    row: MediaUploadRow,
    now: i64,
) -> std::result::Result<VideoUploadSession, Refused> {
    operation(&index, now).await?;
    reserve(&index, &row).await?;
    let metadata = HttpMetadata { content_type: Some(row.media_type.clone()), ..HttpMetadata::default() };
    let mut create = bucket.create_multipart_upload(row.key.as_str()).http_metadata(metadata);
    if let Some(hls) = &row.hls {
        create = create.custom_metadata(HashMap::from([(fmp4::INDEX_METADATA.to_owned(), hls.clone())]));
    }
    let upload = match create.execute().await {
        Ok(upload) => upload,
        Err(why) => {
            let _ = index.forget_media(&row).await;
            return Err(unavailable(why));
        }
    };
    let upload_id = upload.upload_id().await;
    match index.media_open(&row, &upload_id, now).await {
        Ok(true) => {}
        result => {
            // Cancellation may have arrived while create was in flight.
            // No parts exist yet; clear only after R2 confirms abort.
            if upload.abort().await.is_ok() {
                let _ = index.forget_media(&row).await;
            }
            return Err(match result {
                Err(why) => unavailable(why.0),
                _ => EXPIRED,
            });
        }
    }
    Ok(VideoUploadSession { id: row.id, part_bytes: VIDEO_PART_BYTES })
}

async fn claim(
    index: &D1Index,
    row: &MediaUploadRow,
    now: i64,
    cleanup: bool,
) -> std::result::Result<MediaUploadRow, Refused> {
    let token = random_hex(16).ok_or(NOT_KEPT)?;
    index.claim_media(&row.id, row.fid, &token, now, cleanup).await.map_err(unavailable)?.ok_or(BUSY)
}

async fn keep_part(
    bucket: Bucket,
    index: D1Index,
    row: MediaUploadRow,
    part: u16,
    body: ReadableStream,
    now: i64,
) -> std::result::Result<VideoPart, Refused> {
    operation(&index, now).await?;
    let row = claim(&index, &row, now, false).await?;
    // The state may have advanced since the request read it, before claiming.
    let mut parts: Vec<VideoPart> = serde_json::from_str(&row.parts).map_err(unavailable)?;
    if let Some(kept) = parts.get(usize::from(part - 1)) {
        let _ = index.release_media_lock(&row).await;
        return Ok(kept.clone());
    }
    if usize::from(part) != parts.len() + 1 {
        let _ = index.release_media_lock(&row).await;
        return Err(BAD_PART);
    }
    let upload_id = row.upload_id.as_deref().ok_or(BUSY)?;
    let upload = bucket.resume_multipart_upload(&row.key, upload_id).map_err(unavailable)?;
    let kept = match upload.upload_part(part, body).await {
        Ok(kept) => VideoPart { part_number: kept.part_number(), etag: kept.etag() },
        Err(why) => {
            let _ = index.release_media_lock(&row).await;
            return Err(unavailable(why));
        }
    };
    parts.push(kept.clone());
    match index.media_parts(&row, &serde_json::to_string(&parts).map_err(unavailable)?, now).await {
        Ok(true) => Ok(kept),
        Ok(false) => {
            let _ = discard(&bucket, &index, &row).await;
            Err(EXPIRED)
        }
        Err(why) => {
            let _ = index.release_media_lock(&row).await;
            Err(unavailable(why.0))
        }
    }
}

async fn complete_video(
    bucket: Bucket,
    index: D1Index,
    row: MediaUploadRow,
    parts: Vec<VideoPart>,
    now: i64,
) -> std::result::Result<(), Refused> {
    operation(&index, now).await?;
    let row = claim(&index, &row, now, false).await?;
    // A previous complete may have kept the object but lost the D1 response.
    let stored = match bucket.head(&row.key).await {
        Err(why) => {
            let _ = index.release_media_lock(&row).await;
            return Err(unavailable(why));
        }
        Ok(Some(stored)) => stored,
        Ok(None) => {
            let upload =
                bucket.resume_multipart_upload(&row.key, row.upload_id.as_deref().ok_or(BUSY)?).map_err(unavailable)?;
            match upload
                .complete(parts.into_iter().map(|part| worker::UploadedPart::new(part.part_number, part.etag)))
                .await
            {
                Ok(stored) => stored,
                Err(why) => {
                    let _ = index.release_media_lock(&row).await;
                    return Err(unavailable(why));
                }
            }
        }
    };
    if stored.size() != row.bytes {
        let _ = discard(&bucket, &index, &row).await;
        return Err(NOT_WHOLE);
    }
    match index.finish_media(&row, now).await {
        Ok(true) => Ok(()),
        Ok(false) => {
            let _ = discard(&bucket, &index, &row).await;
            Err(EXPIRED)
        }
        // Leave its reservation and object intact so Retry can finish safely.
        Err(why) => {
            let _ = index.release_media_lock(&row).await;
            Err(unavailable(why.0))
        }
    }
}

async fn abort_video(
    bucket: Bucket,
    index: D1Index,
    row: MediaUploadRow,
    now: i64,
) -> std::result::Result<(), Refused> {
    let row = claim(&index, &row, now, true).await?;
    discard(&bucket, &index, &row).await
}

/// Keep the reservation unless R2 confirms no parts or completed object remain.
async fn discard(bucket: &Bucket, index: &D1Index, row: &MediaUploadRow) -> std::result::Result<(), Refused> {
    if !index.begin_media_cleanup(row, (worker::Date::now().as_millis() / 1000) as i64).await.map_err(unavailable)? {
        return Err(BUSY);
    }
    if let Some(upload_id) = &row.upload_id {
        let upload = bucket.resume_multipart_upload(&row.key, upload_id).map_err(unavailable)?;
        if let Err(why) = upload.abort().await {
            // A completed multipart no longer has an upload to abort. An
            // existing object proves that transition. R2's documented 10024
            // also confirms the upload no longer exists (including expiry).
            // Any other unknown failure conservatively retains its quota.
            if !why.to_string().contains("(10024)") && bucket.head(&row.key).await.map_err(unavailable)?.is_none() {
                let _ = index.release_media_lock(row).await;
                return Err(unavailable(why));
            }
        }
    }
    bucket.delete(&row.key).await.map_err(unavailable)?;
    index.forget_media(row).await.map_err(unavailable)?;
    Ok(())
}

/// Existing index cron: one expired upload every other minute, no new trigger.
pub async fn cleanup(env: &Env, index: &D1Index, now: i64) {
    if now.div_euclid(60).rem_euclid(2) != 0 {
        return;
    }
    let Some(bucket) = bucket(env) else { return };
    let expired = match index.expired_media(now).await {
        Ok(rows) => rows,
        Err(why) => {
            console_error!("media cleanup: {}", why.0);
            return;
        }
    };
    for old in expired {
        if let Ok(row) = claim(index, &old, now, true).await {
            let _ = discard(&bucket, index, &row).await;
        }
    }
}

/// `GET` or `HEAD /m/<fid>/<id>.<ext>`: an attachment, to anyone, as the
/// type it was kept as, with `nosniff` and the sandbox policy. A `Range` gets
/// those bytes (206), and an `If-None-Match` that names it, 304. Anything but
/// an attachment's exact path is 404, as is every path without the binding.
pub async fn serve(req: &Request, env: &Env, path: &str) -> Result<Response> {
    let head = match req.method() {
        Method::Get => false,
        Method::Head => true,
        Method::Options => return preflight(),
        _ => return error(405, "Attachments are read with GET", None),
    };
    if let Some((key, playlist)) = Playlist::from_path(path) {
        let Some(bucket) = bucket(env) else { return not_found() };
        return serve_playlist(req, &bucket, &key, playlist, head).await;
    }
    let (Some(key), Some(bucket)) = (MediaKey::from_path(path), bucket(env)) else { return not_found() };
    let object = key.object();
    let if_none_match = req.headers().get("if-none-match")?;
    // A HEAD, and a GET that may need no body, read the file's metadata only.
    if head || if_none_match.is_some() {
        let found = match bucket.head(object.as_str()).await {
            Ok(Some(found)) => found,
            Ok(None) => return not_found(),
            Err(failure) => return unreadable(&object, &failure),
        };
        if if_none_match.is_some_and(|tags| none_match(&tags, &found.http_etag())) {
            return Ok(ResponseBuilder::new().with_status(304).with_headers(headers_of(&found)?).empty());
        }
        if head {
            let headers = headers_of(&found)?;
            headers.set("content-length", &found.size().to_string())?;
            return Ok(ResponseBuilder::new().with_headers(headers).empty());
        }
    }
    let Some(range) = req.headers().get("range")?.as_deref().and_then(ByteRange::parse) else {
        return match bucket.get(object.as_str()).execute().await {
            Ok(Some(found)) => {
                let size = found.size();
                body_of(&found, 200, size, None)
            }
            Ok(None) => not_found(),
            Err(failure) => unreadable(&object, &failure),
        };
    };
    let read = match range.read() {
        RangeRead::From { offset, length: Some(length) } => Range::OffsetWithLength { offset, length },
        RangeRead::From { offset, length: None } => Range::OffsetToEnd { offset },
        RangeRead::Suffix(suffix) => Range::Suffix { suffix },
    };
    match bucket.get(object.as_str()).range(read).execute().await {
        Ok(Some(found)) => match range.within(found.size()) {
            Some((first, last)) => {
                body_of(&found, 206, last - first + 1, Some(content_range(first, last, found.size())))
            }
            None => unsatisfiable(found.size()),
        },
        Ok(None) => not_found(),
        // R2 refuses a range that starts past the end: say what there is.
        Err(failure) => match bucket.head(object.as_str()).await {
            Ok(Some(found)) if range.within(found.size()).is_none() => unsatisfiable(found.size()),
            Ok(None) => not_found(),
            _ => unreadable(&object, &failure),
        },
    }
}

/// A CORS preflight: any site may read attachments, with a `Range`.
fn preflight() -> Result<Response> {
    let headers = Headers::new();
    for (name, value) in preflight_headers() {
        headers.set(name, value)?;
    }
    Ok(ResponseBuilder::new().with_status(204).with_headers(headers).empty())
}

/// `GET` or `HEAD` a remuxed video's playlist, written from its index in
/// R2's metadata (one `head`): 404 for a file without one.
async fn serve_playlist(
    req: &Request,
    bucket: &Bucket,
    key: &MediaKey,
    playlist: Playlist,
    head: bool,
) -> Result<Response> {
    let object = key.object();
    let found = match bucket.head(object.as_str()).await {
        Ok(Some(found)) => found,
        Ok(None) => return not_found(),
        Err(failure) => return unreadable(&object, &failure),
    };
    let Some(index) = found.custom_metadata()?.get(fmp4::INDEX_METADATA).and_then(|line| HlsIndex::parse(line)) else {
        return not_found();
    };
    let etag = playlist.etag(&found.http_etag());
    let headers = Headers::new();
    for (name, value) in served_headers(fmp4::PLAYLIST_TYPE, &etag) {
        headers.set(name, &value)?;
    }
    // A playlist is read whole.
    headers.delete("accept-ranges")?;
    if req.headers().get("if-none-match")?.is_some_and(|tags| none_match(&tags, &etag)) {
        return Ok(ResponseBuilder::new().with_status(304).with_headers(headers).empty());
    }
    let text = match playlist {
        Playlist::Master => fmp4::master_playlist(&index, &key.id),
        Playlist::Media => fmp4::media_playlist(&index, &key.id),
    };
    headers.set("content-length", &text.len().to_string())?;
    if head {
        return Ok(ResponseBuilder::new().with_headers(headers).empty());
    }
    Ok(ResponseBuilder::new().with_headers(headers).fixed(text.into_bytes()))
}

/// The headers of a served file: the type it was kept as, if it is one of
/// ours, its ETag, a year's caching, `nosniff`, the sandbox policy and leave
/// for other sites and apps to show it.
fn headers_of(found: &Object) -> Result<Headers> {
    let headers = Headers::new();
    let content_type = served_type(found.http_metadata().content_type.as_deref());
    for (name, value) in served_headers(content_type, &found.http_etag()) {
        headers.set(name, &value)?;
    }
    Ok(headers)
}

/// The file's bytes as R2 streams them (all of it, or a range of `length`
/// bytes), straight into the response.
fn body_of(found: &Object, status: u16, length: u64, range: Option<String>) -> Result<Response> {
    let headers = headers_of(found)?;
    headers.set("content-length", &length.to_string())?;
    if let Some(range) = range {
        headers.set("content-range", &range)?;
    }
    let Some(body) = found.body() else { return unreadable(&found.key(), &"no body".into()) };
    Ok(ResponseBuilder::new().with_status(status).with_headers(headers).body(body.response_body()?))
}

fn not_found() -> Result<Response> {
    error(404, "Not found", None)
}

fn unsatisfiable(size: u64) -> Result<Response> {
    let mut response = error(416, "That range is past the end of the file", None)?;
    response.headers_mut().set("content-range", &unsatisfied_range(size))?;
    Ok(response)
}

fn unreadable(object: &str, failure: &worker::Error) -> Result<Response> {
    console_error!("media: read {object}: {failure}");
    error(503, "That file can't be read just now; try again shortly", Some(5))
}
