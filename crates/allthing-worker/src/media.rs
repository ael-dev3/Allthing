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
//! Without the binding (until the owner sets R2 up, docs/DEPLOY.md), an
//! upload answers 404, and so does every `/m/` path; `/api/v1/status` says
//! `media.ready: false`.

use std::cell::RefCell;

use allthing_core::media::{
    self, ByteRange, LIMITS, MAX_EMBED_URL_BYTES, MediaKey, RangeRead, Refused, UploadLimiter, Uploaded, admit,
    check_upload, content_range, none_match, served_headers, served_type, unsatisfied_range,
};
use futures::future::FutureExt;
use worker::web_sys::ReadableStream;
use worker::{
    Bucket, Context, Env, Headers, HttpMetadata, Method, Object, Range, Request, Response, ResponseBuilder, Result,
    console_error,
};

use crate::auth::{json, random_hex, signed_in_fid};
use crate::error;
use crate::index::D1Index;

/// The R2 binding attachments are kept in (bucket `allthing-media`).
const BUCKET: &str = "MEDIA";
const UNSET: &str = "Attachments aren't set up on this server";
const SIGN_IN: &str = "Sign in to attach images and videos";
const NOT_KEPT: Refused =
    Refused { status: 503, message: "That file couldn't be kept just now; try again", retry_after: Some(5) };
const NOT_WHOLE: Refused =
    Refused { status: 400, message: "That file didn't arrive whole; try again", retry_after: None };

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

/// `POST /api/v1/media`: the file is the body, its type the `Content-Type`
/// (one of `allthing_core::media::MediaType`) and its size the
/// `Content-Length`. Everything is checked before any of it is stored: the
/// origin, the session, the type and size, the reader's pace, their day's
/// quota and the space left.
async fn upload(req: Request, env: &Env, ctx: &Context, now_ms: f64) -> Result<Response> {
    let now = (now_ms / 1000.0) as i64;
    let origin = req.url()?.origin().ascii_serialization();
    // Browsers send Origin with every POST: another site can't upload as a
    // reader.
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
    if let Err(wait) = LIMITER.with(|limiter| limiter.borrow_mut().allow(fid, now_ms)) {
        return error(429, "That's a lot of uploads at once; try again in a moment", Some(wait));
    }
    let Some(key) = random_hex(16).and_then(|id| MediaKey::new(fid, &id, media)) else {
        return error(500, "No randomness for a file's name", None);
    };
    let url = key.url(&origin);
    if url.len() > MAX_EMBED_URL_BYTES {
        return error(500, "This server's address is too long for a cast to embed its files", None);
    }
    let usage = match index.media_usage(fid, now).await {
        Ok(usage) => usage,
        Err(failure) => {
            console_error!("media: usage of {fid}: {}", failure.0);
            return error(503, "Uploads can't be counted just now; try again shortly", Some(30));
        }
    };
    if let Err(refused) = admit(&usage, length, now) {
        return refuse(refused);
    }
    let Some(body) = req.inner().body() else { return refuse(media::EMPTY) };
    // Kept and recorded under waitUntil, so a reader who leaves as their
    // last byte arrives leaves no file that the totals don't count.
    let kept = keep(bucket, index, key, body, length, now).boxed_local().shared();
    let background = kept.clone();
    ctx.wait_until(async move {
        let _ = background.await;
    });
    match kept.await {
        Ok(bytes) => json(&Uploaded { url, media_type: media.mime().to_owned(), bytes }, &[]),
        Err(refused) => refuse(refused),
    }
}

/// Hand the request's body to R2 as it comes, as the declared type, then
/// record it. A file R2 kept that isn't whole, or that can't be recorded, is
/// deleted again, so what is stored is always what is counted.
async fn keep(
    bucket: Bucket,
    index: D1Index,
    key: MediaKey,
    body: ReadableStream,
    length: u64,
    now: i64,
) -> std::result::Result<u64, Refused> {
    let object = key.object();
    let metadata = HttpMetadata { content_type: Some(key.media.mime().to_owned()), ..HttpMetadata::default() };
    let size = match bucket.put(object.as_str(), body).http_metadata(metadata).execute().await {
        Ok(Some(stored)) => stored.size(),
        Ok(None) => return Err(NOT_KEPT),
        Err(failure) => {
            console_error!("media: put {object}: {failure}");
            return Err(NOT_KEPT);
        }
    };
    if size != length {
        let _ = bucket.delete(object.as_str()).await;
        return Err(NOT_WHOLE);
    }
    if let Err(failure) = index.record_media(&object, key.fid, size, key.media.mime(), now).await {
        console_error!("media: record {object}: {}", failure.0);
        let _ = bucket.delete(object.as_str()).await;
        return Err(NOT_KEPT);
    }
    Ok(size)
}

/// `GET` or `HEAD /m/<fid>/<id>.<ext>`: an attachment, to anyone, as the
/// type it was kept as, with `nosniff` and the sandbox policy. A `Range` gets
/// those bytes (206), and an `If-None-Match` that names it, 304. Anything but
/// an attachment's exact path is 404, as is every path without the binding.
pub async fn serve(req: &Request, env: &Env, path: &str) -> Result<Response> {
    let head = match req.method() {
        Method::Get => false,
        Method::Head => true,
        _ => return error(405, "Attachments are read with GET", None),
    };
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
