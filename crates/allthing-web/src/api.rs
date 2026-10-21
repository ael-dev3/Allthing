// SPDX-License-Identifier: AGPL-3.0-or-later
//! Same-origin calls to the Worker's `/api/v1`, with a short in-memory cache
//! so going back and forth between pages costs no requests. In the static
//! build (`static-host`), the same payloads are built in the browser.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use allthing_core::api::{ApiError, EngagementPayload, EngagementRequest};
use allthing_core::model::UserSummary;
use allthing_core::profile::{ProfilePreview, UserCard};
use serde::de::DeserializeOwned;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// How long a fetched payload is reused without asking the Worker again.
/// Long enough that going back to a feed or post is instant.
const CLIENT_TTL_MS: f64 = 300_000.0;
const LATEST_TTL_MS: f64 = 30_000.0;
/// The signed-in reader's notifications (`components::notifications`).
const NOTIFICATIONS_TTL_MS: f64 = 60_000.0;
/// How many answers the session cache keeps.
const CLIENT_CAPACITY: usize = 200;
/// At most this many prefetches in flight.
const MAX_PREFETCHES: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    NotFound(String),
    /// The Worker or an upstream is shedding load; retry later.
    Busy {
        message: String,
        retry_after: Option<u32>,
    },
    /// The request itself was turned down (HTTP 4xx other than 404 and
    /// 429), such as a search too long to run: asking again won't help.
    Refused(String),
    Failed(String),
}

/// What a request that never reached the API fails with.
pub(crate) const UNREACHABLE: &str = "Network error: could not reach Allthing's API";

impl Failure {
    /// The server or the network may answer next time: busy, or not
    /// reached, rather than a refusal. (Sign-in asks; the static copy has
    /// none.)
    #[cfg(not(feature = "static-host"))]
    pub fn is_transient(&self) -> bool {
        match self {
            Failure::Busy { .. } => true,
            Failure::Failed(message) => message == UNREACHABLE,
            Failure::NotFound(_) | Failure::Refused(_) => false,
        }
    }

    /// The request never reached the API: the reader is offline, or the
    /// network failed on the way.
    pub fn unreachable(&self) -> bool {
        matches!(self, Failure::Failed(message) if message == UNREACHABLE)
    }

    pub fn message(&self) -> String {
        match self {
            Failure::NotFound(m) | Failure::Refused(m) | Failure::Failed(m) => m.clone(),
            Failure::Busy { message, retry_after } => match retry_after {
                Some(s) => format!("{message} (try again in {s}s)"),
                None => message.clone(),
            },
        }
    }
}

#[derive(Debug)]
pub struct Fetched<T> {
    pub data: Arc<T>,
    /// Seconds since the Worker fetched the data upstream.
    pub age: u32,
    /// Served from cache because the upstream is failing.
    pub stale: bool,
}

impl<T> Clone for Fetched<T> {
    fn clone(&self) -> Self {
        Fetched { data: self.data.clone(), age: self.age, stale: self.stale }
    }
}

impl<T> PartialEq for Fetched<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
    }
}

#[derive(Clone)]
struct Raw {
    at: f64,
    status: u16,
    body: Rc<str>,
    age: u32,
    stale: bool,
    retry_after: Option<u32>,
    /// The Worker said to keep it nowhere (`cache-control: no-store`): an
    /// error, or a preload batch it couldn't read.
    no_store: bool,
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Raw>> = RefCell::new(HashMap::new());
    static IN_FLIGHT: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Reactions previews that an engagement batch on its way will file: a
    /// preview opened meanwhile waits for it instead of asking again.
    static COVERED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Profile-card reads waiting for a free prefetch slot, oldest first.
    static CARD_QUEUE: RefCell<std::collections::VecDeque<String>> = const { RefCell::new(std::collections::VecDeque::new()) };
    /// Accounts whose card facts were queued this visit, and when the count
    /// last started over.
    static CARD_ACCOUNTS: RefCell<(f64, std::collections::HashSet<u64>)> = RefCell::new((0.0, std::collections::HashSet::new()));
    /// The people of likes, recasts and replies previews whose card facts
    /// were queued, apart from feed authors', and when the count started.
    static ROW_ACCOUNTS: RefCell<(f64, std::collections::HashSet<u64>)> = RefCell::new((0.0, std::collections::HashSet::new()));
    /// Casts already shown in a feed, so their post page can render at once.
    static SEEN_CASTS: RefCell<HashMap<String, allthing_core::model::Cast>> = RefCell::new(HashMap::new());
    /// Profile cards of the accounts pages have shown (fid -> card), for
    /// cards of the same accounts where a payload has none: who liked a
    /// cast, or a post opened from a feed before its thread arrives.
    static SEEN_CARDS: RefCell<HashMap<u64, UserCard>> = RefCell::new(HashMap::new());
    /// The authors of the casts pages have shown, by lowercase username, so
    /// their profile can show its header before its payload arrives.
    static SEEN_PEOPLE: RefCell<HashMap<String, UserSummary>> = RefCell::new(HashMap::new());
}

/// Remember the profile cards a page's payload carried. A card keeps what
/// an earlier one of the same account knew and it doesn't (the date of its
/// first username, from its profile).
pub fn remember_cards(cards: &BTreeMap<u64, UserCard>) {
    SEEN_CARDS.with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.len() > 2000 {
            seen.clear();
        }
        for (fid, card) in cards {
            let card = card.clone().filled_from(seen.get(fid));
            seen.insert(*fid, card);
        }
    });
}

/// The profile card of an account a page showed during this visit.
pub fn seen_card(fid: u64) -> Option<UserCard> {
    SEEN_CARDS.with(|seen| seen.borrow().get(&fid).cloned())
}

/// Remember the casts a listing shows and the casts they quote, and their
/// authors.
pub fn remember_casts(casts: &[allthing_core::model::Cast], notes: &allthing_core::api::Annotations) {
    SEEN_CASTS.with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.len() > 2000 {
            seen.clear();
        }
        for cast in casts.iter().chain(notes.quotes.values()) {
            seen.insert(cast.hash.clone(), cast.clone());
        }
    });
    SEEN_PEOPLE.with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.len() > 2000 {
            seen.clear();
        }
        for author in casts.iter().chain(notes.quotes.values()).map(|c| &c.author) {
            let key = author.username.to_lowercase();
            if seen.get(&key).is_none_or(|known| known != author) {
                seen.insert(key, author.clone());
            }
        }
    });
}

/// A cast seen in a listing, for rendering its post before the thread loads.
pub fn seen_cast(hash: &str) -> Option<allthing_core::model::Cast> {
    SEEN_CASTS.with(|seen| seen.borrow().get(hash).cloned())
}

/// What this visit has seen of the account named `username` (as a profile
/// URL names it, lowercase): the author of a cast a page showed, and its
/// profile card, if a payload carried one.
pub fn seen_person(username: &str) -> Option<ProfilePreview> {
    let author = SEEN_PEOPLE.with(|seen| seen.borrow().get(username).cloned())?;
    Some(ProfilePreview::new(&author, seen_card(author.fid).as_ref()))
}

/// Whether the reader asked to save data or is on a slow connection.
pub fn constrained_network() -> bool {
    use wasm_bindgen::JsValue;
    let Some(window) = web_sys::window() else { return true };
    let Ok(connection) = js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("connection")) else {
        return false;
    };
    if connection.is_undefined() {
        return false;
    }
    let get = |key: &str| js_sys::Reflect::get(&connection, &JsValue::from_str(key)).ok();
    let save_data = get("saveData").and_then(|v| v.as_bool()).unwrap_or(false);
    let slow = get("effectiveType").and_then(|v| v.as_string()).is_some_and(|t| t.contains("2g"));
    save_data || slow
}

/// What became of a prefetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefetch {
    /// On its way.
    Started,
    /// Not needed: already kept or on its way, or the network is constrained.
    Skipped,
    /// Too many requests on their way; try again later.
    Full,
}

/// Fetch `path` into the client cache in the background, so opening it later
/// is instant. Skipped when already cached or loading, when too many
/// prefetches are running, and on Save-Data or slow connections.
pub fn prefetch(path: &str) {
    start_prefetch(path, |_, _| {});
}

/// [`prefetch`], running `landed` with the answer (if any) and the time it
/// came, before it is kept.
fn start_prefetch(path: &str, landed: impl FnOnce(Option<&Raw>, f64) + 'static) -> Prefetch {
    let now = js_sys::Date::now();
    let cached = CACHE.with(|c| c.borrow().get(path).is_some_and(|r| now - r.at < ttl(path) && r.status == 200));
    if cached || constrained_network() {
        return Prefetch::Skipped;
    }
    let started = IN_FLIGHT.with(|f| {
        let mut f = f.borrow_mut();
        if f.contains(path) {
            Prefetch::Skipped
        } else if f.len() >= MAX_PREFETCHES {
            Prefetch::Full
        } else {
            f.insert(path.to_owned());
            Prefetch::Started
        }
    });
    if started != Prefetch::Started {
        return started;
    }
    let path = path.to_owned();
    wasm_bindgen_futures::spawn_local(async move {
        let raw = fetch_raw(&path).await.ok();
        let now = js_sys::Date::now();
        landed(raw.as_ref(), now);
        if let Some(raw) = raw {
            remember(&path, raw, now);
        }
        IN_FLIGHT.with(|f| f.borrow_mut().remove(&path));
        pump_cards();
    });
    Prefetch::Started
}

/// Preload who liked and recasted a batch of feed cards, and their replies
/// (`/api/v1/engagement`), through [`prefetch`]'s cap and checks, and file
/// each list under its own `/reactions?…&limit=10` path and each replies
/// preview under its own `/replies?hash=…` path, as if each had been asked
/// for alone, so hovering or tapping a count shows its people or replies at
/// once. A preview opened while the batch is on its way waits for it
/// instead of asking again; one the batch lacks then asks for itself. When
/// this returns [`Prefetch::Started`], `landed` runs once the batch is in,
/// whatever came of it.
pub fn preload_engagement(request: &EngagementRequest, landed: impl FnOnce() + 'static) -> Prefetch {
    let covered: Vec<String> = request.preview_paths().collect();
    let filed = covered.clone();
    let started = start_prefetch(&request.path(), move |raw, now| {
        if let Some(raw) = raw {
            seed_engagement(raw, &filed, now);
        }
        COVERED.with(|c| {
            let mut c = c.borrow_mut();
            for path in &filed {
                c.remove(path);
            }
        });
        landed();
    });
    // The fetch starts from a later task, so this is in place before it lands.
    if started == Prefetch::Started {
        COVERED.with(|c| c.borrow_mut().extend(covered));
    }
    started
}

/// File the previews of an engagement batch that were `asked` for, each
/// under its own reactions or replies path, dated as the batch: with its
/// arrival time and its age, so the client cache expires each one, and
/// reports its age, as if it had been asked for alone when the batch was.
/// An answer already kept for a path stays when it was read upstream no
/// earlier.
fn seed_engagement(batch: &Raw, asked: &[String], now: f64) {
    if batch.status != 200 {
        return;
    }
    let Ok(payload) = serde_json::from_str::<EngagementPayload>(&batch.body) else { return };
    for (path, preview) in payload.by_path() {
        file_preview(batch, asked, &path, preview, now);
    }
    for (path, preview) in payload.replies_by_path() {
        file_preview(batch, asked, &path, preview, now);
    }
}

/// File one of `batch`'s previews under `path`, for [`seed_engagement`].
fn file_preview<T: serde::Serialize>(batch: &Raw, asked: &[String], path: &str, preview: &T, now: f64) {
    if !asked.iter().any(|p| p == path) {
        return;
    }
    // When an answer was read upstream, on this browser's clock.
    let read_at = |raw: &Raw| raw.at - f64::from(raw.age) * 1000.0;
    let newer = CACHE.with(|c| {
        c.borrow()
            .get(path)
            .is_some_and(|kept| kept.status == 200 && now - kept.at < ttl(path) && read_at(kept) >= read_at(batch))
    });
    if newer {
        return;
    }
    let Ok(body) = serde_json::to_string(preview) else { return };
    keep(path, Raw { status: 200, body: body.into(), retry_after: None, ..batch.clone() }, now);
}

/// A cached answer for `path`, if there is a fresh one: read synchronously,
/// so what was prefetched shows in the same frame, without a loading state.
pub fn ready<T: DeserializeOwned>(path: &str) -> Option<Fetched<T>> {
    cached::<T>(path, js_sys::Date::now()).and_then(Result::ok)
}

/// Whether a fresh answer for `path` is kept, so a page that reads it shows
/// it in the frame it's drawn in.
pub fn kept(path: &str) -> bool {
    let now = js_sys::Date::now();
    CACHE.with(|c| c.borrow().get(path).is_some_and(|r| now - r.at < ttl(path) && r.status == 200))
}

/// Whether a request that brings `path` is on its way.
pub fn loading(path: &str) -> bool {
    IN_FLIGHT.with(|f| f.borrow().contains(path)) || COVERED.with(|c| c.borrow().contains(path))
}

/// Start reading `path` now, for a page about to open: once, as a prefetch
/// is, but past the cap on prefetches in flight, since the page would ask
/// for it a moment later anyway (and waits for this answer instead).
pub fn fetch_ahead(path: &str) {
    if kept_or_loading(path, js_sys::Date::now()) {
        return;
    }
    IN_FLIGHT.with(|f| f.borrow_mut().insert(path.to_owned()));
    let path = path.to_owned();
    wasm_bindgen_futures::spawn_local(async move {
        let raw = fetch_raw(&path).await.ok();
        if let Some(raw) = raw {
            remember(&path, raw, js_sys::Date::now());
        }
        IN_FLIGHT.with(|f| f.borrow_mut().remove(&path));
        pump_cards();
    });
}

/// Profile cards' facts (vote, wallets) prefetched per 5 minutes, at most:
/// with a mouse, and on a touch screen, where each card a reader scrolls
/// past would otherwise cost two requests.
const CARD_ACCOUNTS_PER_WINDOW: usize = 12;
const TOUCH_CARD_ACCOUNTS_PER_WINDOW: usize = 6;
const CARD_WINDOW_MS: f64 = 300_000.0;

/// Load an account's vote and wallet activity ahead of its profile card,
/// when it shows in view: queued behind other prefetches, two in flight,
/// at most [`CARD_ACCOUNTS_PER_WINDOW`] accounts per five minutes
/// ([`TOUCH_CARD_ACCOUNTS_PER_WINDOW`] on a touch screen), and never on
/// Save-Data or 2g. A card a mouse hovers and one a finger taps open both
/// complete. Both endpoints are edge-cached for an hour or more.
pub fn prefetch_card_facts(fid: u64) {
    if fid == 0 || constrained_network() {
        return;
    }
    let cap = if crate::util::can_hover() { CARD_ACCOUNTS_PER_WINDOW } else { TOUCH_CARD_ACCOUNTS_PER_WINDOW };
    let now = js_sys::Date::now();
    let fresh = CARD_ACCOUNTS.with(|a| {
        let mut a = a.borrow_mut();
        if now - a.0 > CARD_WINDOW_MS {
            *a = (now, std::collections::HashSet::new());
        }
        a.1.len() < cap && a.1.insert(fid)
    });
    if fresh {
        CARD_QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            q.push_back(allthing_core::api::vote_path(fid));
            q.push_back(allthing_core::api::wallets_path(fid));
        });
        pump_cards();
    }
}

/// The people of likes, recasts and replies previews whose card facts are
/// preloaded per 5 minutes, at most, on top of feed authors': three lists'.
const ROW_ACCOUNTS_PER_WINDOW: usize = 30;

/// Load the vote and wallet activity of the people a likes, recasts or
/// replies preview lists, so a card opened from one of its rows is complete
/// at once. They go ahead of feed authors' in the card queue, in the list's
/// order, two in flight; anyone whose facts are kept or on their way is
/// skipped, at most [`ROW_ACCOUNTS_PER_WINDOW`] people are queued per five
/// minutes, and none on Save-Data or 2g. Unlike feed authors', they load on
/// touch screens too: a list opened with a tap is a reader about to tap a
/// name. Both endpoints are edge-cached for an hour or more.
pub fn prefetch_row_facts(fids: &[u64]) {
    if constrained_network() {
        return;
    }
    let now = js_sys::Date::now();
    let mut wanted = Vec::new();
    for &fid in fids {
        let paths = [allthing_core::api::vote_path(fid), allthing_core::api::wallets_path(fid)];
        // Queued already (as a feed author's, say): moved to the front.
        let missing: Vec<String> = paths.into_iter().filter(|path| !kept_or_loading(path, now)).collect();
        if fid == 0 || missing.is_empty() {
            continue;
        }
        let allowed = ROW_ACCOUNTS.with(|a| {
            let mut a = a.borrow_mut();
            if now - a.0 > CARD_WINDOW_MS {
                *a = (now, std::collections::HashSet::new());
            }
            a.1.contains(&fid) || (a.1.len() < ROW_ACCOUNTS_PER_WINDOW && a.1.insert(fid))
        });
        if !allowed {
            break;
        }
        wanted.extend(missing);
    }
    CARD_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        q.retain(|path| !wanted.contains(path));
        for path in wanted.into_iter().rev() {
            q.push_front(path);
        }
    });
    pump_cards();
}

/// Load `fid`'s vote and wallet activity ahead of anything queued: the
/// person on a preview's row the reader points at or touches, whose card is
/// about to open and would read them then anyway, so they don't count
/// towards the lists' five minutes. Not on Save-Data or 2g.
pub fn prefetch_facts_first(fid: u64) {
    if fid == 0 || constrained_network() {
        return;
    }
    let now = js_sys::Date::now();
    let paths = [allthing_core::api::vote_path(fid), allthing_core::api::wallets_path(fid)];
    let wanted: Vec<String> = paths.into_iter().filter(|path| !kept_or_loading(path, now)).collect();
    if wanted.is_empty() {
        return;
    }
    CARD_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        q.retain(|path| !wanted.contains(path));
        for path in wanted.into_iter().rev() {
            q.push_front(path);
        }
    });
    pump_cards();
}

/// Whether `path` is kept fresh or being read.
fn kept_or_loading(path: &str, now: f64) -> bool {
    CACHE.with(|c| c.borrow().get(path).is_some_and(|r| now - r.at < ttl(path) && r.status == 200))
        || IN_FLIGHT.with(|f| f.borrow().contains(path))
}

/// Start queued card reads while prefetch slots are free.
fn pump_cards() {
    while IN_FLIGHT.with(|f| f.borrow().len()) < MAX_PREFETCHES {
        let Some(path) = CARD_QUEUE.with(|q| q.borrow_mut().pop_front()) else { return };
        prefetch(&path);
    }
}

/// The response the inline script in index.html already requested for `path`, if any. It is
/// handed out once.
fn early_response(path: &str) -> Option<js_sys::Promise> {
    use wasm_bindgen::JsValue;
    let window = web_sys::window()?;
    let key = JsValue::from_str("__allthingEarly");
    let early = js_sys::Reflect::get(&window, &key).ok().filter(|e| e.is_object())?;
    let early_path = js_sys::Reflect::get(&early, &JsValue::from_str("path")).ok()?.as_string()?;
    if early_path != path {
        return None;
    }
    let _ = js_sys::Reflect::delete_property(&window, &key);
    js_sys::Reflect::get(&early, &JsValue::from_str("response")).ok()?.dyn_into().ok()
}

async fn fetch_raw(path: &str) -> Result<Raw, Failure> {
    #[cfg(feature = "static-host")]
    if path.starts_with(allthing_core::api::API_PREFIX) {
        let (status, body, retry_after) = crate::host::build(path).await;
        return Ok(Raw {
            at: js_sys::Date::now(),
            status,
            body: body.into(),
            age: 0,
            stale: false,
            retry_after,
            no_store: false,
        });
    }
    let window = web_sys::window().ok_or_else(|| Failure::Failed("no window".into()))?;
    let request = early_response(path).unwrap_or_else(|| window.fetch_with_str(path));
    read_raw(request).await
}

/// The answer a request for an API path brings.
async fn read_raw(request: js_sys::Promise) -> Result<Raw, Failure> {
    let response = JsFuture::from(request).await.map_err(|_| Failure::Failed(UNREACHABLE.into()))?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| Failure::Failed("bad response".into()))?;
    let header = |name: &str| response.headers().get(name).ok().flatten();
    let age = header("x-allthing-age").and_then(|a| a.parse().ok()).unwrap_or(0);
    let stale = header("x-allthing-stale").is_some();
    let retry_after = header("retry-after").and_then(|a| a.parse().ok());
    let no_store = header("cache-control").is_some_and(|c| c.contains("no-store"));
    let text = JsFuture::from(response.text().map_err(|_| Failure::Failed("unreadable response".into()))?)
        .await
        .map_err(|_| Failure::Failed("unreadable response".into()))?
        .as_string()
        .unwrap_or_default();
    Ok(Raw { at: js_sys::Date::now(), status: response.status(), body: text.into(), age, stale, retry_after, no_store })
}

/// Read `path` again from the Worker, past the browser's HTTP cache, and
/// keep what comes: after the reader's write, this visit and the next page
/// load have the new copy (the Worker's own was expired first). Its body,
/// when it came.
#[cfg(not(feature = "static-host"))]
pub async fn reload(path: &str) -> Option<Rc<str>> {
    let window = web_sys::window()?;
    let init = web_sys::RequestInit::new();
    init.set_cache(web_sys::RequestCache::Reload);
    let raw = read_raw(window.fetch_with_str_and_init(path, &init)).await.ok()?;
    let body = (raw.status == 200).then(|| raw.body.clone());
    remember(path, raw, js_sys::Date::now());
    body
}

/// POST `body` as JSON to a Worker endpoint and read its JSON answer. For
/// signing in: never cached, and never the static copy's.
#[cfg(not(feature = "static-host"))]
pub async fn post<B: serde::Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, Failure> {
    let failed = |_| Failure::Failed(UNREACHABLE.into());
    let window = web_sys::window().ok_or_else(|| Failure::Failed("no window".into()))?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    let headers = web_sys::Headers::new().map_err(failed)?;
    headers.set("content-type", "application/json").map_err(failed)?;
    init.set_headers(&headers);
    let json = serde_json::to_string(body).map_err(|e| Failure::Failed(e.to_string()))?;
    init.set_body(&wasm_bindgen::JsValue::from_str(&json));
    let response = JsFuture::from(window.fetch_with_str_and_init(path, &init)).await.map_err(failed)?;
    let response: web_sys::Response = response.dyn_into().map_err(failed)?;
    let retry_after = response.headers().get("retry-after").ok().flatten().and_then(|a| a.parse().ok());
    let text = JsFuture::from(response.text().map_err(failed)?).await.map_err(failed)?.as_string().unwrap_or_default();
    let raw = Raw {
        at: js_sys::Date::now(),
        status: response.status(),
        body: text.into(),
        age: 0,
        stale: false,
        retry_after,
        no_store: false,
    };
    Arc::into_inner(decode::<T>(&raw)?.data).ok_or_else(|| Failure::Failed("unreadable response".into()))
}

fn decode<T: DeserializeOwned>(raw: &Raw) -> Result<Fetched<T>, Failure> {
    let error_message = || {
        serde_json::from_str::<ApiError>(&raw.body).map(|e| e.error).unwrap_or_else(|_| format!("HTTP {}", raw.status))
    };
    match raw.status {
        200 => serde_json::from_str::<T>(&raw.body)
            .map(|data| Fetched { data: Arc::new(data), age: raw.age, stale: raw.stale })
            .map_err(|e| Failure::Failed(format!("Unexpected data from the API: {e}"))),
        404 => Err(Failure::NotFound(error_message())),
        429 | 503 => Err(Failure::Busy { message: error_message(), retry_after: raw.retry_after }),
        400..=499 => Err(Failure::Refused(error_message())),
        _ => Err(Failure::Failed(error_message())),
    }
}

/// How long an answer for `path` is reused: the newest casts change by the
/// minute, everything else is kept for a few.
fn ttl(path: &str) -> f64 {
    if path == allthing_core::api::latest_path() {
        LATEST_TTL_MS
    } else if path == allthing_core::scheduled::SCHEDULED_PATH {
        // The reader's own list, read afresh each time.
        0.0
    } else if path.starts_with(allthing_core::notifications::NOTIFICATIONS_PATH) {
        // As long as the Worker keeps them fresh: the bell, asking every few
        // minutes, and the page reuse a read within it.
        NOTIFICATIONS_TTL_MS
    } else {
        CLIENT_TTL_MS
    }
}

/// A recent answer for `path`, if one is kept.
fn cached<T: DeserializeOwned>(path: &str, now: f64) -> Option<Result<Fetched<T>, Failure>> {
    let result = CACHE.with(|c| {
        c.borrow().get(path).filter(|raw| now - raw.at < ttl(path)).map(|raw| {
            let age = raw.age.saturating_add(((now - raw.at).max(0.0) / 1000.0) as u32);
            decode::<T>(&Raw { age, ..raw.clone() })
        })
    });
    // A prefetch can validate JSON syntax without knowing its payload type.
    // Discard structurally invalid data so the next visit can recover.
    if matches!(result, Some(Err(Failure::Failed(_) | Failure::Refused(_) | Failure::Busy { .. }))) {
        CACHE.with(|c| c.borrow_mut().remove(path));
        None
    } else {
        result
    }
}

/// Keep an answer for reuse: only real data and "no such thing". Errors,
/// busy answers, unreadable bodies and what the Worker says to keep nowhere
/// (a preload batch it couldn't read, answered as empty) are asked for
/// again next time.
fn remember(path: &str, raw: Raw, now: f64) {
    let worth = match raw.status {
        _ if raw.no_store => false,
        200 => serde_json::from_str::<serde::de::IgnoredAny>(&raw.body).is_ok(),
        404 => true,
        _ => false,
    };
    if worth && !forgotten(path, now) {
        keep(path, raw, now);
    }
}

fn keep(path: &str, raw: Raw, now: f64) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if !c.contains_key(path) && c.len() >= CLIENT_CAPACITY {
            c.retain(|key, r| now - r.at < ttl(key));
            if c.len() >= CLIENT_CAPACITY
                && let Some(oldest) = c.iter().min_by(|a, b| a.1.at.total_cmp(&b.1.at)).map(|(key, _)| key.clone())
            {
                c.remove(&oldest);
            }
        }
        c.insert(path.to_owned(), raw);
    });
}

/// Keep `value` as the answer for `path`, as if it had just come: what the
/// page already knows, so it isn't asked for (the wallet activity of an
/// account whose profile says it has no verified wallets, say).
pub fn know<T: serde::Serialize>(path: &str, value: &T) {
    let Ok(body) = serde_json::to_string(value) else { return };
    let now = js_sys::Date::now();
    keep(
        path,
        Raw { at: now, status: 200, body: body.into(), age: 0, stale: false, retry_after: None, no_store: false },
        now,
    );
}

#[cfg(not(feature = "static-host"))]
thread_local! {
    /// The answers last forgotten, and when (`forget`).
    static FORGOTTEN: RefCell<Option<(String, f64)>> = const { RefCell::new(None) };
}

/// Forget the answers kept for paths that start with `prefix`: a reader's
/// own, when someone else signs in. An answer still on its way to a request
/// made before then was the last reader's, and isn't kept either.
#[cfg(not(feature = "static-host"))]
pub fn forget(prefix: &str) {
    forget_at(prefix, js_sys::Date::now());
}

#[cfg(not(feature = "static-host"))]
fn forget_at(prefix: &str, now: f64) {
    FORGOTTEN.with(|f| *f.borrow_mut() = Some((prefix.to_owned(), now)));
    CACHE.with(|c| c.borrow_mut().retain(|path, _| !path.starts_with(prefix)));
}

/// Whether the answer for `path` to a request made at `asked` came after
/// it was forgotten.
fn forgotten(path: &str, asked: f64) -> bool {
    #[cfg(not(feature = "static-host"))]
    {
        FORGOTTEN
            .with(|f| f.borrow().as_ref().is_some_and(|(prefix, at)| asked <= *at && path.starts_with(prefix.as_str())))
    }
    #[cfg(feature = "static-host")]
    {
        let _ = (path, asked);
        false
    }
}

/// A recent answer for `path`, read at once, without waiting or fetching:
/// a popover whose data is kept renders it in the frame it opens.
pub fn peek<T: DeserializeOwned>(path: &str) -> Option<Result<Fetched<T>, Failure>> {
    cached(path, js_sys::Date::now())
}

/// Wait while a request that brings `path` is on its way: a prefetch, an
/// engagement batch that covers it, or the search box's suggestions (up to
/// 10 s).
async fn arriving(path: &str) {
    let on_its_way = || IN_FLIGHT.with(|f| f.borrow().contains(path)) || COVERED.with(|c| c.borrow().contains(path));
    for _ in 0..400 {
        if !on_its_way() {
            break;
        }
        crate::video::sleep(25).await;
    }
}

/// What a request already made brings for `path`, once it is in, without
/// fetching it otherwise: `None` when nothing is kept or on its way.
pub async fn settled<T: DeserializeOwned>(path: &str) -> Option<Result<Fetched<T>, Failure>> {
    arriving(path).await;
    peek(path)
}

/// GET an API path, reusing a recent answer for the same path.
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<Fetched<T>, Failure> {
    // A request that brings this path is on its way: wait for it, don't
    // repeat it.
    arriving(path).await;
    let now = js_sys::Date::now();
    if let Some(result) = cached(path, now) {
        return result;
    }
    let mine = IN_FLIGHT.with(|f| f.borrow_mut().insert(path.to_owned()));
    let raw = fetch_raw(path).await;
    if mine {
        IN_FLIGHT.with(|f| f.borrow_mut().remove(path));
    }
    let result = raw.and_then(|raw| {
        let result = decode::<T>(&raw);
        if result.is_ok() || raw.status == 404 {
            remember(path, raw, now);
        }
        result
    });
    // Foreground reads share the prefetch slots. Release queued card facts
    // after either success or failure, once any reusable answer is filed.
    pump_cards();
    result
}

#[cfg(test)]
mod tests {
    use allthing_core::api::{ReactionKind, ReactionsPayload, RepliesPayload, reactions_path, replies_path};

    use super::*;

    fn response(status: u16, body: &str) -> Raw {
        Raw { at: 1000.0, status, body: body.into(), age: 10, stale: false, retry_after: Some(5), no_store: false }
    }

    #[test]
    fn errors_are_asked_for_again() {
        for status in [400, 403, 429, 500, 502, 503, 504] {
            let path = format!("/test/error/{status}");
            remember(&path, response(status, r#"{"error":"unavailable"}"#), 1000.0);
            assert!(cached::<serde_json::Value>(&path, 2000.0).is_none(), "kept HTTP {status}");
        }
        remember("/test/garbled", response(200, "<html>"), 1000.0);
        assert!(cached::<serde_json::Value>("/test/garbled", 2000.0).is_none());
    }

    #[test]
    fn answers_are_told_apart_by_what_trying_again_can_do() {
        let decoded = |status, body| decode::<serde_json::Value>(&response(status, body)).err();
        assert_eq!(decoded(404, r#"{"error":"No such cast"}"#), Some(Failure::NotFound("No such cast".into())));
        assert_eq!(
            decoded(400, r#"{"error":"Search query too long"}"#),
            Some(Failure::Refused("Search query too long".into()))
        );
        assert_eq!(decoded(403, "{}"), Some(Failure::Refused("HTTP 403".into())));
        assert_eq!(
            decoded(429, r#"{"error":"slow down"}"#),
            Some(Failure::Busy { message: "slow down".into(), retry_after: Some(5) })
        );
        assert_eq!(
            decoded(502, r#"{"error":"upstream timed out"}"#),
            Some(Failure::Failed("upstream timed out".into()))
        );
        assert!(!Failure::Refused("Search query too long".into()).is_transient());
        assert!(Failure::Failed(UNREACHABLE.into()).unreachable());
        assert!(!Failure::Failed("upstream timed out".into()).unreachable());
    }

    #[test]
    fn malformed_prefetched_payloads_do_not_poison_the_typed_cache() {
        for body in ["{}", r#"{"query":"recovery","users":[]}"#] {
            // Prefetching only knows the path, not its eventual payload type.
            remember("/test/wrong-shape", response(200, body), 1000.0);
            assert!(cached::<allthing_core::api::SearchPayload>("/test/wrong-shape", 2000.0).is_none());
            assert!(CACHE.with(|c| !c.borrow().contains_key("/test/wrong-shape")));
        }
    }

    #[test]
    fn what_the_worker_says_to_keep_nowhere_is_asked_for_again() {
        // A preview batch the Worker couldn't read: empty, and `no-store`.
        let skipped =
            Raw { no_store: true, ..response(200, r#"{"previews":[],"meta":{"backend":"hypersnap","fetched_at":1}}"#) };
        remember("/test/skipped", skipped, 1000.0);
        assert!(cached::<serde_json::Value>("/test/skipped", 2000.0).is_none());
        remember("/test/kept", response(200, r#"{"previews":[]}"#), 1000.0);
        assert!(cached::<serde_json::Value>("/test/kept", 2000.0).is_some());
    }

    #[test]
    fn remembered_profile_cards_keep_what_a_profile_told() {
        // From the profile: dwr got his first username on 17 Jun 2021.
        let profile = UserCard {
            followers: 10,
            registered_at: Some(1_699_386_171),
            first_username_at: Some(1_623_888_000),
            ..Default::default()
        };
        remember_cards(&BTreeMap::from([(3, profile)]));
        // A later feed knows newer counts, but not that date.
        let feed = UserCard { followers: 12, registered_at: Some(1_699_386_171), ..Default::default() };
        remember_cards(&BTreeMap::from([(3, feed)]));
        let seen = seen_card(3).unwrap();
        assert_eq!((seen.followers, seen.first_username_at), (12, Some(1_623_888_000)));
        assert_eq!(seen_card(4), None);
    }

    #[cfg(not(feature = "static-host"))]
    #[test]
    fn a_reader_s_forgotten_answers_are_not_kept_even_when_they_come_late() {
        let page = response(200, r#"{"groups":[]}"#);
        remember("/test/own?cursor=a", page.clone(), 1000.0);
        forget_at("/test/own", 2000.0);
        assert!(cached::<serde_json::Value>("/test/own?cursor=a", 2500.0).is_none());
        // Asked for before someone else signed in, and come since: the last
        // reader's.
        remember("/test/own", page.clone(), 1500.0);
        assert!(cached::<serde_json::Value>("/test/own", 2500.0).is_none());
        // Asked for since.
        remember("/test/own", page.clone(), 2100.0);
        assert!(cached::<serde_json::Value>("/test/own", 2500.0).is_some());
        // Nobody's own.
        remember("/test/everyone", page, 1500.0);
        assert!(cached::<serde_json::Value>("/test/everyone", 2500.0).is_some());
    }

    #[test]
    fn data_and_missing_pages_are_reused_until_they_expire() {
        remember("/test/data", response(200, r#"{"casts":[]}"#), 1000.0);
        let hit = cached::<serde_json::Value>("/test/data", 3000.0).unwrap().unwrap();
        assert_eq!(hit.data["casts"], serde_json::json!([]));
        assert_eq!(hit.age, 12);
        assert!(cached::<serde_json::Value>("/test/data", 1000.0 + CLIENT_TTL_MS).is_none());

        remember("/test/missing", response(404, r#"{"error":"unknown user"}"#), 1000.0);
        assert!(matches!(cached::<serde_json::Value>("/test/missing", 2000.0), Some(Err(Failure::NotFound(_)))));
    }

    #[test]
    fn fresh_responses_cannot_exceed_the_documented_cache_capacity() {
        CACHE.with(|c| c.borrow_mut().clear());
        for n in 0..225 {
            let at = 1000.0 + f64::from(n);
            let mut raw = response(200, "{}");
            raw.at = at;
            remember(&format!("/test/capacity/{n}"), raw, at);
        }
        assert_eq!(CACHE.with(|c| c.borrow().len()), 200);
        assert!(cached::<serde_json::Value>("/test/capacity/0", 2000.0).is_none());
        assert!(cached::<serde_json::Value>("/test/capacity/224", 2000.0).unwrap().is_ok());

        // Refreshing an existing entry doesn't evict another fresh answer.
        let mut refreshed = response(200, "{}");
        refreshed.at = 2000.0;
        remember("/test/capacity/224", refreshed, 2000.0);
        assert_eq!(CACHE.with(|c| c.borrow().len()), 200);
        assert!(cached::<serde_json::Value>("/test/capacity/25", 2000.0).unwrap().is_ok());

        // At capacity, expired answers go before the still-fresh refresh.
        let mut next = response(200, "{}");
        next.at = 301_500.0;
        remember("/test/capacity/next", next, 301_500.0);
        assert_eq!(CACHE.with(|c| c.borrow().len()), 2);
        assert!(cached::<serde_json::Value>("/test/capacity/224", 301_500.0).unwrap().is_ok());
    }

    fn batch(at: f64, age: u32, body: &str) -> Raw {
        Raw { at, status: 200, body: body.into(), age, stale: false, retry_after: None, no_store: false }
    }

    /// A batch that read who liked `hash`: one person, and more.
    fn likes_batch(hash: &str, name: &str) -> String {
        format!(
            r#"{{"previews":[{{"hash":"{hash}","kind":"likes","users":[{{"fid":2,"username":"{name}"}}],"more":true,"meta":{{"backend":"hypersnap","fetched_at":7}}}}],"meta":{{"backend":"hypersnap","fetched_at":7}}}}"#
        )
    }

    #[test]
    fn engagement_batches_file_each_preview_under_its_own_path() {
        let hash = format!("0x{}", "c".repeat(40));
        let likes = reactions_path(&hash, ReactionKind::Likes, 10);
        let recasts = reactions_path(&hash, ReactionKind::Recasts, 10);
        seed_engagement(&batch(1000.0, 30, &likes_batch(&hash, "reader")), &[likes.clone(), recasts.clone()], 1000.0);
        let hit = cached::<ReactionsPayload>(&likes, 3000.0).unwrap().unwrap();
        assert_eq!((hit.data.hash.as_str(), hit.data.kind), (hash.as_str(), ReactionKind::Likes));
        assert_eq!(hit.data.users[0].username, "reader");
        assert!(hit.data.more && hit.data.meta.fetched_at == 7);
        // Dated as the batch: its age when it came, plus the time since,
        // and kept as long as the batch.
        assert_eq!(hit.age, 32);
        assert!(cached::<ReactionsPayload>(&likes, 1000.0 + CLIENT_TTL_MS).is_none());
        // A list the batch didn't bring loads when its popover opens.
        assert!(cached::<ReactionsPayload>(&recasts, 3000.0).is_none());
    }

    #[test]
    fn engagement_batches_file_each_replies_preview_under_its_own_path() {
        let (hash, other) = (format!("0x{}", "f".repeat(40)), format!("0x{}", "a".repeat(40)));
        let (replies, likes) = (replies_path(&hash), reactions_path(&hash, ReactionKind::Likes, 10));
        let batch_body = format!(
            r#"{{"previews":[{{"hash":"{hash}","kind":"likes","users":[{{"fid":2,"username":"liker"}}],"meta":{{"backend":"hypersnap","fetched_at":7}}}}],
                "replies":[{{"hash":"{hash}","replies":[{{"hash":"0x01","author":{{"fid":3,"username":"replier"}},"text":"First!","ts":1}}],
                             "notes":{{"users":{{"3":{{"bio":"Replies.","followers":4}}}}}},"meta":{{"backend":"hypersnap","fetched_at":7}}}},
                           {{"hash":"{other}","replies":[],"meta":{{"backend":"hypersnap","fetched_at":7}}}}],
                "meta":{{"backend":"hypersnap","fetched_at":7}}}}"#
        );
        seed_engagement(&batch(1000.0, 30, &batch_body), &[likes.clone(), replies.clone()], 1000.0);
        // Read back as /api/v1/replies would answer, dated as the batch.
        let hit = cached::<RepliesPayload>(&replies, 3000.0).unwrap().unwrap();
        assert_eq!(hit.data.hash, hash);
        assert_eq!(hit.data.replies[0].text, "First!");
        assert_eq!(hit.data.notes.users[&3].bio, "Replies.");
        assert_eq!(hit.age, 32);
        assert!(cached::<RepliesPayload>(&replies, 1000.0 + CLIENT_TTL_MS).is_none());
        // Its lists alongside, as before.
        assert_eq!(cached::<ReactionsPayload>(&likes, 3000.0).unwrap().unwrap().data.users[0].username, "liker");
        // A replies preview that wasn't asked for isn't filed.
        assert!(cached::<RepliesPayload>(&replies_path(&other), 3000.0).is_none());
    }

    #[test]
    fn a_batch_files_only_what_was_asked_and_never_replaces_a_newer_answer() {
        let hash = format!("0x{}", "d".repeat(40));
        let path = reactions_path(&hash, ReactionKind::Likes, 10);
        let single = |name: &str, at: f64, age: u32| {
            let body = format!(
                r#"{{"hash":"{hash}","kind":"likes","users":[{{"fid":3,"username":"{name}"}}],"meta":{{"backend":"hypersnap","fetched_at":0}}}}"#
            );
            Raw { at, status: 200, body: body.into(), age, stale: false, retry_after: None, no_store: false }
        };
        let name = |now: f64| cached::<ReactionsPayload>(&path, now).unwrap().unwrap().data.users[0].username.clone();
        let asked = std::slice::from_ref(&path);
        // Not asked for: not filed.
        seed_engagement(&batch(1000.0, 30, &likes_batch(&hash, "batch")), &[], 1000.0);
        assert!(cached::<ReactionsPayload>(&path, 1000.0).is_none());
        // Read upstream before the batch's data was: replaced.
        remember(&path, single("older", 1000.0, 40), 1000.0);
        seed_engagement(&batch(1000.0, 30, &likes_batch(&hash, "batch")), asked, 1000.0);
        assert_eq!(name(1000.0), "batch");
        // Read upstream after it: kept.
        remember(&path, single("newer", 2000.0, 0), 2000.0);
        seed_engagement(&batch(2000.0, 30, &likes_batch(&hash, "batch")), asked, 2000.0);
        assert_eq!(name(2000.0), "newer");
        // A batch that failed, or isn't a batch, files nothing.
        let other = format!("0x{}", "e".repeat(40));
        let other_path = reactions_path(&other, ReactionKind::Likes, 10);
        let failed = Raw { status: 503, ..batch(1000.0, 0, &likes_batch(&other, "busy")) };
        seed_engagement(&failed, std::slice::from_ref(&other_path), 1000.0);
        seed_engagement(&batch(1000.0, 0, r#"{"error":"nope"}"#), std::slice::from_ref(&other_path), 1000.0);
        assert!(cached::<ReactionsPayload>(&other_path, 1000.0).is_none());
    }
}
