// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hypersnap, a Snapchain read node that also serves a Neynar-shaped
//! `/v2/farcaster` API. Allthing reads every Farcaster message through it.
//! The Farcaster app's public API fills in what only the app knows: votes to
//! leave it, and channels' hosts, pictures and member counts.

use std::collections::{HashMap, HashSet};

use allthing_core::api::{FollowsDir, LIBRARY_PAGE_SIZE, LibraryEntry, ReactionKind, library_cursor};
use allthing_core::channels::{
    canonical_parent_url, channel_id_for_url, channel_url_for_id, legacy_url, parent_url_variants,
    short_parent_url_label,
};
use allthing_core::follows;
use allthing_core::index::Source;
use allthing_core::model::Channel;
use allthing_core::normalize::{
    self, RawAppChannel, RawAppChannelResponse, RawCast, RawChannel, RawFeed, RawHubReactions, RawNotifications,
    RawReaction, RawReactionList, RawTransfers, RawUser, RawUserData, RawUserList, RawUserSearch,
};
use allthing_core::provenance::{
    base64_decode, base64_encode, normalize_key, request_fid_from_metadata, verified_request_fid_from_metadata,
};
use allthing_core::ranking::TrendingWindow;
use allthing_core::website::Website;
use serde::Deserialize;

use crate::upstream::{self, Upstream, UpstreamError, get_json, url};
use crate::{HttpMethod, Invocation};

/// The node the browser also submits readers' messages to.
pub const BASE: &str = allthing_core::message::NODE;

fn v2(path: &str, query: &[(&str, &str)]) -> String {
    url(BASE, &format!("v2/farcaster/{path}"), query)
}

fn v1(path: &str, query: &[(&str, &str)]) -> String {
    url(BASE, &format!("v1/{path}"), query)
}

/// The `time_window` Hypersnap is asked for. It ranks trending casts over 1
/// hour, 6 hours, 24 hours and 7 days, and answers any other value with its
/// 24-hour list (12 hours came back as casts 11 to 18 hours old), so 12 hours
/// is asked for as 24 and cut to 12 by its casts' own times (`pages::home`).
pub fn hypersnap_window(window: TrendingWindow) -> TrendingWindow {
    match window {
        TrendingWindow::TwelveHours => TrendingWindow::Day,
        TrendingWindow::Month | TrendingWindow::Year | TrendingWindow::All => TrendingWindow::Week,
        window => window,
    }
}

/// Trending casts over `window`, ranked by Hypersnap; `offset` pages through
/// the list (its cursor is a plain offset, and the list ends at 1,000 casts).
/// Its casts may be older than `window` (see [`hypersnap_window`]): a caller
/// keeps the ones inside it.
pub async fn trending_casts(
    inv: &Invocation,
    limit: u32,
    offset: u32,
    window: TrendingWindow,
) -> Result<Vec<RawCast>, UpstreamError> {
    let (limit, offset) = (limit.to_string(), offset.to_string());
    let window = ("time_window", hypersnap_window(window).as_str());
    let query: &[(&str, &str)] =
        if offset == "0" { &[("limit", &limit), window] } else { &[("limit", &limit), ("cursor", &offset), window] };
    let feed: Option<RawFeed> = get_json(inv, Upstream::Hypersnap, &v2("feed/trending", query), false).await?;
    Ok(feed.map(|f| f.casts).unwrap_or_default())
}

/// A page of the casts of the accounts `fid` follows, newest first (replies
/// among them), and the cursor of the next, older page. Hypersnap joins the
/// follows itself, so a follow list of any size is one read; it sends 100 at
/// most, whatever `limit` asks.
pub async fn following_feed(
    inv: &Invocation,
    fid: u64,
    cursor: Option<&str>,
) -> Result<(Vec<RawCast>, Option<String>), UpstreamError> {
    let fid = fid.to_string();
    let mut query = vec![("fid", fid.as_str()), ("limit", "100")];
    if let Some(cursor) = cursor {
        query.push(("cursor", cursor));
    }
    let feed: Option<BackfillPage> = get_json(inv, Upstream::Hypersnap, &v2("feed/following", &query), false).await?;
    Ok(feed.map(|f| (f.casts, f.next.and_then(|n| n.cursor).filter(|c| !c.is_empty()))).unwrap_or_default())
}

/// A page of one shard's hub events: the next `page_size` after the event
/// `after`, or without it the newest `page_size`, newest first.
pub async fn events(
    inv: &Invocation,
    shard: u32,
    after: Option<u64>,
    page_size: u32,
) -> Result<Vec<u8>, UpstreamError> {
    let (shard, page_size) = (shard.to_string(), page_size.to_string());
    let from = after.map(|id| (id + 1).to_string());
    let mut query = vec![("shard_index", shard.as_str()), ("pageSize", page_size.as_str())];
    match &from {
        Some(from) => query.push(("from_event_id", from)),
        None => query.push(("reverse", "true")),
    }
    let body = upstream::send(inv, Upstream::Hypersnap, HttpMethod::Get, &v1("events", &query), None, true).await?;
    Ok(body.unwrap_or_default())
}

#[derive(Deserialize)]
struct CastList {
    #[serde(default)]
    casts: Vec<RawCast>,
}

/// Casts that one `casts` request reads, at most.
pub const MAX_BULK_CASTS: usize = 100;

/// Fully hydrated casts (embeds, counts) by hash: the first
/// [`MAX_BULK_CASTS`] of `hashes`, in one request.
pub async fn casts_bulk(inv: &Invocation, hashes: &[String]) -> Result<Vec<RawCast>, UpstreamError> {
    let hashes = &hashes[..hashes.len().min(MAX_BULK_CASTS)];
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let list = hashes.join(",");
    let found: Option<CastList> = get_json(inv, Upstream::Hypersnap, &v2("casts", &[("casts", &list)]), true).await?;
    Ok(found.map(|l| l.casts).unwrap_or_default())
}

/// Library hydration must not mistake an unavailable endpoint, an error JSON
/// object or a failed bulk read for a successful lookup with missing casts.
pub async fn library_casts(inv: &Invocation, hashes: &[String]) -> Result<Vec<RawCast>, UpstreamError> {
    #[derive(Deserialize)]
    struct Found {
        casts: Vec<RawCast>,
    }
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    if hashes.len() > LIBRARY_PAGE_SIZE {
        return Err(UpstreamError::Parse("too many Library casts".into()));
    }
    let list = hashes.join(",");
    let found: Found = get_json(inv, Upstream::Hypersnap, &v2("casts", &[("casts", &list)]), false)
        .await?
        .ok_or(UpstreamError::Status(404))?;
    if found.casts.len() > LIBRARY_PAGE_SIZE
        || found.casts.iter().any(|cast| {
            allthing_core::params::hash_from_param(&cast.hash).is_none()
                || cast.author.as_ref().is_none_or(|author| author.fid == 0)
        })
    {
        return Err(UpstreamError::Parse("invalid Library cast lookup".into()));
    }
    Ok(found.casts)
}

#[derive(Deserialize)]
struct LibraryReactionPage {
    messages: Vec<LibraryReactionMessage>,
    #[serde(rename = "nextPageToken")]
    next: String,
}
#[derive(Deserialize)]
struct LibraryReactionMessage {
    data: LibraryReactionData,
}
#[derive(Deserialize)]
struct LibraryReactionData {
    fid: u64,
    timestamp: u64,
    #[serde(rename = "type")]
    kind: serde_json::Value,
    #[serde(rename = "reactionBody")]
    reaction: LibraryReactionBody,
}
#[derive(Deserialize)]
struct LibraryReactionBody {
    #[serde(rename = "type")]
    kind: serde_json::Value,
    #[serde(rename = "targetCastId")]
    target: Option<LibraryTarget>,
    #[serde(rename = "targetUrl")]
    target_url: Option<String>,
}
#[derive(Deserialize)]
struct LibraryTarget {
    fid: u64,
    hash: String,
}

/// A page of the account's retained active reactions. Follow the opaque
/// `next` key to the end; unlike a target's reactors this is a single shard.
pub async fn library_reactions(
    inv: &Invocation,
    fid: u64,
    kind: ReactionKind,
    cursor: Option<&str>,
) -> Result<(Vec<LibraryEntry>, Option<String>), UpstreamError> {
    let fid_text = fid.to_string();
    let size = LIBRARY_PAGE_SIZE.to_string();
    let (number, name, query_kind) = match kind {
        ReactionKind::Likes => (1, "REACTION_TYPE_LIKE", "Like"),
        ReactionKind::Recasts => (2, "REACTION_TYPE_RECAST", "Recast"),
    };
    let mut query = vec![
        ("fid", fid_text.as_str()),
        ("reaction_type", query_kind),
        ("pageSize", size.as_str()),
        ("reverse", "true"),
    ];
    if let Some(cursor) = cursor {
        query.push(("pageToken", cursor));
    }
    let page: LibraryReactionPage = get_json(inv, Upstream::Hypersnap, &v1("reactionsByFid", &query), false)
        .await?
        .ok_or(UpstreamError::Status(404))?;
    let invalid = || UpstreamError::Parse("invalid Library reaction page".into());
    if page.messages.len() > LIBRARY_PAGE_SIZE {
        return Err(invalid());
    }
    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for message in page.messages {
        let data = message.data;
        let matches =
            |value: &serde_json::Value, number, name| value.as_u64() == Some(number) || value.as_str() == Some(name);
        if data.fid != fid
            || !matches(&data.kind, 3, "MESSAGE_TYPE_REACTION_ADD")
            || !matches(&data.reaction.kind, number, name)
        {
            return Err(invalid());
        }
        // A reaction to a URL is valid network history, but is not a cast.
        let Some(target) = data.reaction.target else {
            if data.reaction.target_url.as_deref().is_some_and(|url| !url.trim().is_empty()) {
                continue;
            }
            return Err(invalid());
        };
        let hash = allthing_core::params::hash_from_param(&target.hash).ok_or_else(invalid)?;
        let at = i64::try_from(data.timestamp)
            .ok()
            .and_then(|at| at.checked_add(allthing_core::message::EPOCH))
            .ok_or_else(invalid)?;
        if target.fid == 0 {
            return Err(invalid());
        }
        if seen.insert(hash.clone()) {
            entries.push(LibraryEntry { hash, author_fid: target.fid, at });
        }
    }
    let next = (!page.next.is_empty()).then_some(page.next);
    if let Some(next) = &next
        && (library_cursor(next).is_none() || Some(next.as_str()) == cursor)
    {
        return Err(invalid());
    }
    Ok((entries, next))
}

/// Who liked (`likes`) or recasted (`recasts`) a cast, at most 100
/// (Hypersnap does not page further), as Hypersnap lists them: each shard's
/// reactions oldest first, one shard after another ([`normalize::reactors`]
/// puts them in time order). One kind per call: asked for `likes,recasts`,
/// Hypersnap answers with the likes alone. With them, the bytes they came
/// as: a batch of previews weighs what it parses.
pub async fn reactions(
    inv: &Invocation,
    hash: &str,
    kind: &str,
    limit: u32,
) -> Result<(Vec<RawReaction>, usize), UpstreamError> {
    let url = v2("reactions/cast", &[("hash", hash), ("types", kind), ("limit", &limit.to_string())]);
    let Some(body) = upstream::send(inv, Upstream::Hypersnap, HttpMethod::Get, &url, None, true).await? else {
        return Ok((Vec::new(), 0));
    };
    let found: RawReactionList = upstream::parse(&body)?;
    Ok((found.reactions, body.len()))
}

/// What happened to `fid`'s account, newest first: a page of `limit`
/// notifications from `cursor` (the newest without one), with a cursor for
/// the next. Hypersnap lists likes (recasts among them), follows and
/// replies (mentions and quotes among them); see `normalize::notifications`.
/// An answer of 400 is a failure here, not an account with none: those
/// answer an empty list.
pub async fn notifications(
    inv: &Invocation,
    fid: u64,
    limit: u32,
    cursor: Option<&str>,
) -> Result<RawNotifications, UpstreamError> {
    let (fid, limit) = (fid.to_string(), limit.to_string());
    let mut query = vec![("fid", fid.as_str()), ("limit", limit.as_str())];
    if let Some(cursor) = cursor {
        query.push(("cursor", cursor));
    }
    let found: Option<RawNotifications> =
        get_json(inv, Upstream::Hypersnap, &v2("notifications", &query), false).await?;
    Ok(found.unwrap_or_default())
}

/// The newest recasts of the cast `hash`, by `author`, from the hub: who
/// recasted it and when (Unix seconds), `per_shard` of each shard's newest,
/// one shard after another. Not `reactions/cast`, which lists each shard's
/// oldest first and stops at its limit: for a cast with more recasts than
/// that, it names the first recasters, not the latest.
pub async fn newest_recasts(
    inv: &Invocation,
    author: u64,
    hash: &str,
    per_shard: u32,
) -> Result<Vec<(u64, i64)>, UpstreamError> {
    let (author, per_shard) = (author.to_string(), per_shard.to_string());
    let query = [
        ("target_fid", author.as_str()),
        ("target_hash", hash),
        ("reaction_type", "Recast"),
        ("pageSize", per_shard.as_str()),
        ("reverse", "true"),
    ];
    let found: Option<RawHubReactions> =
        get_json(inv, Upstream::Hypersnap, &v1("reactionsByCast", &query), true).await?;
    Ok(found.as_ref().map(normalize::hub_reactors).unwrap_or_default())
}

/// The casts that quote the cast `hash`, at most `limit`.
pub async fn quotes(inv: &Invocation, hash: &str, limit: u32) -> Result<Vec<RawCast>, UpstreamError> {
    let limit = limit.to_string();
    let query = [("identifier", hash), ("type", "hash"), ("limit", limit.as_str())];
    let found: Option<CastList> = get_json(inv, Upstream::Hypersnap, &v2("cast/quotes", &query), true).await?;
    Ok(found.map(|l| l.casts).unwrap_or_default())
}

pub async fn trending_channels(inv: &Invocation, limit: u32) -> Result<Vec<RawChannel>, UpstreamError> {
    let list: Option<normalize::RawChannelList> =
        get_json(inv, Upstream::Hypersnap, &v2("channel/trending", &[("limit", &limit.to_string())]), false).await?;
    Ok(list.map(|l| l.channels.into_iter().map(|e| e.into_channel()).collect()).unwrap_or_default())
}

pub async fn search_channels(inv: &Invocation, query: &str, limit: u32) -> Result<Vec<RawChannel>, UpstreamError> {
    let list: Option<normalize::RawChannelList> =
        get_json(inv, Upstream::Hypersnap, &v2("channel/search", &[("q", query), ("limit", &limit.to_string())]), true)
            .await?;
    Ok(list.map(|l| l.channels.into_iter().map(|e| e.into_channel()).collect()).unwrap_or_default())
}

#[derive(Deserialize)]
struct ChannelResponse {
    channel: Option<RawChannel>,
}

/// A channel by id, legacy id or parent URL. URL-keyed feeds always exist:
/// their casts live under the URL even when the provider has no record of it.
pub async fn channel(inv: &Invocation, key: &str) -> Result<Option<Channel>, UpstreamError> {
    let legacy = legacy_url(key);
    let is_url = legacy.is_none() && (key.contains(':') || key.contains('/'));
    if legacy.is_none() && !is_url {
        let found: Option<ChannelResponse> =
            get_json(inv, Upstream::Hypersnap, &v2("channel", &[("id", key)]), true).await?;
        return Ok(found.and_then(|r| r.channel).map(|c| normalize::channel(&c)));
    }
    let parent_url = legacy.map_or_else(|| key.to_owned(), str::to_owned);
    let found: Option<ChannelResponse> =
        get_json(inv, Upstream::Hypersnap, &v2("channel", &[("id", &parent_url), ("type", "parent_url")]), true)
            .await?;
    let known = found.and_then(|r| r.channel).map(|c| normalize::channel(&c));
    let id = known
        .as_ref()
        .map(|c| c.id.clone())
        .or_else(|| channel_id_for_url(&parent_url))
        .unwrap_or_else(|| canonical_parent_url(&parent_url));
    let name = known
        .as_ref()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| if legacy.is_some() { key.to_owned() } else { short_parent_url_label(&parent_url) });
    Ok(Some(Channel { id, name, parent_url, ..known.unwrap_or_default() }))
}

/// A page of the casts of the channel at `parent_url`, newest first, from
/// `cursor` (the newest without one). It may have none but a cursor for more
/// (`RawFeed::is_empty_with_more`), which the channel page follows.
pub async fn channel_page(
    inv: &Invocation,
    parent_url: &str,
    limit: u32,
    cursor: Option<&str>,
) -> Result<RawFeed, UpstreamError> {
    let variants = parent_url_variants(parent_url).join(",");
    let limit = limit.to_string();
    let mut query = vec![("parent_urls", variants.as_str()), ("limit", limit.as_str()), ("with_recasts", "false")];
    if let Some(cursor) = cursor {
        query.push(("cursor", cursor));
    }
    let feed: Option<RawFeed> = get_json(inv, Upstream::Hypersnap, &v2("feed/parent_urls", &query), true).await?;
    Ok(feed.unwrap_or_default())
}

/// A backfill may finish only after a readable casts array, not an error
/// object that omits the field.
#[derive(Deserialize)]
struct BackfillPage {
    casts: Vec<RawCast>,
    next: Option<normalize::RawNext>,
}

/// A page of about `limit` of a feed's casts, newest first, starting at
/// `cursor` (the newest without one), and the cursor of the next, older
/// page. A channel is read by its main parent URL only; channel feeds come
/// back at twice the requested limit, so it asks for half. An author's casts
/// include replies (Hypersnap ignores `include_replies`).
pub async fn feed_page(
    inv: &Invocation,
    source: &Source,
    cursor: Option<&str>,
    limit: u32,
) -> Result<(Vec<RawCast>, Option<String>), UpstreamError> {
    let limit = match source {
        Source::Channel(_) => (limit / 2).max(1),
        Source::Author(_) => limit,
    }
    .to_string();
    let (path, key, value) = match source {
        Source::Channel(id) => ("feed/parent_urls", "parent_urls", channel_url_for_id(id)),
        Source::Author(fid) => ("feed/user/casts", "fid", fid.to_string()),
    };
    let mut query = vec![(key, value.as_str()), ("limit", limit.as_str())];
    query.push(match source {
        Source::Channel(_) => ("with_recasts", "false"),
        Source::Author(_) => ("include_replies", "false"),
    });
    if let Some(cursor) = cursor {
        query.push(("cursor", cursor));
    }
    let feed: Option<BackfillPage> = get_json(inv, Upstream::Hypersnap, &v2(path, &query), true).await?;
    Ok(feed.map(|f| (f.casts, f.next.and_then(|n| n.cursor).filter(|c| !c.is_empty()))).unwrap_or_default())
}

#[derive(Deserialize)]
struct UserResponse {
    user: Option<RawUser>,
}

#[derive(Deserialize)]
struct UsernameProof {
    fid: Option<u64>,
}

/// By username. Hypersnap's `by_username` knows each account's current
/// fname only, so any other name, an ENS name like `vitalik.eth` or one the
/// account has since moved away from (`dwr.eth`, now `dwr`), is looked up by
/// the hub's username proof, which names the FID that holds it.
pub async fn user_by_username(inv: &Invocation, username: &str) -> Result<Option<RawUser>, UpstreamError> {
    let found: Option<UserResponse> =
        get_json(inv, Upstream::Hypersnap, &v2("user/by_username", &[("username", username)]), true).await?;
    if let Some(user) = found.and_then(|r| r.user) {
        return Ok(Some(user));
    }
    // Proofs are kept under the lowercase name; the hub answers 400 for none.
    let name = username.to_ascii_lowercase();
    let proof: Option<UsernameProof> =
        get_json(inv, Upstream::Hypersnap, &v1("userNameProofByName", &[("name", &name)]), true).await?;
    match proof.and_then(|p| p.fid) {
        Some(fid) => Ok(users_bulk(inv, &[fid]).await?.into_iter().next()),
        None => Ok(None),
    }
}

pub async fn search_users(inv: &Invocation, query: &str, limit: u32) -> Result<Vec<RawUser>, UpstreamError> {
    let search: Option<RawUserSearch> =
        get_json(inv, Upstream::Hypersnap, &v2("user/search", &[("q", query), ("limit", &limit.to_string())]), true)
            .await?;
    Ok(search.map(RawUserSearch::into_users).unwrap_or_default())
}

#[derive(Deserialize)]
struct CastSearch {
    result: Option<RawFeed>,
}

#[derive(Deserialize)]
struct AppUser {
    result: Option<AppUserResult>,
}

#[derive(Deserialize)]
struct AppUserResult {
    user: Option<AppUserInner>,
}

#[derive(Deserialize)]
struct AppUserInner {
    #[serde(rename = "viewerContext")]
    viewer_context: Option<AppViewerContext>,
}

#[derive(Deserialize)]
struct AppViewerContext {
    #[serde(rename = "optInStatus")]
    opt_in_status: Option<String>,
}

/// The farcaster.xyz app's record of an FID's vote (its only record of
/// "leave" votes), from its public, unauthenticated user endpoint.
pub async fn app_vote(inv: &Invocation, fid: u64) -> Result<Option<String>, UpstreamError> {
    let url = url("https://client.farcaster.xyz", "v2/user-by-fid", &[("fid", &fid.to_string())]);
    let found: Option<AppUser> = get_json(inv, Upstream::FarcasterApp, &url, true).await?;
    Ok(found.and_then(|a| a.result).and_then(|r| r.user).and_then(|u| u.viewer_context).and_then(|v| v.opt_in_status))
}

/// The farcaster.xyz app's record of a channel, by id: display name,
/// description, pictures, true member and follower counts, lead, moderators
/// and pinned cast. Public and unauthenticated, but it sends no CORS headers,
/// so a browser (the static copy) can't read it. `Ok(None)` for an unknown
/// channel, which it answers with HTTP 400.
pub async fn app_channel(inv: &Invocation, id: &str) -> Result<Option<RawAppChannel>, UpstreamError> {
    let url = url("https://api.farcaster.xyz", "v1/channel", &[("channelId", id)]);
    let found: Option<RawAppChannelResponse> = get_json(inv, Upstream::FarcasterApp, &url, true).await?;
    Ok(found.and_then(|r| r.result).and_then(|r| r.channel))
}

/// When `fid` first got a username, from the fname server's public transfer
/// log (which browsers can read too). `Ok(None)` when it never had one.
pub async fn first_username_at(inv: &Invocation, fid: u64) -> Result<Option<i64>, UpstreamError> {
    let url = url("https://fnames.farcaster.xyz", "transfers", &[("fid", &fid.to_string())]);
    let found: Option<RawTransfers> = get_json(inv, Upstream::Fnames, &url, true).await?;
    Ok(found.and_then(|t| normalize::first_username_at(&t, fid)))
}

/// The website `fid`'s profile sets, if it is one a profile can link to.
/// Hypersnap's user objects (`user/by_username`, `user/bulk`) don't carry
/// it: their `profile` holds the bio alone. The hub's user data does, and
/// the node ignores `user_data_type`, so this reads all of it, about 5 KB,
/// and picks the URL ([`normalize::website`]). `Ok(None)` when the account
/// has none, or one that isn't a web address.
pub async fn website(inv: &Invocation, fid: u64) -> Result<Option<Website>, UpstreamError> {
    let found: Option<RawUserData> =
        get_json(inv, Upstream::Hypersnap, &v1("userDataByFid", &[("fid", &fid.to_string())]), true).await?;
    Ok(found.as_ref().and_then(normalize::website))
}

/// Search runs a Tantivy query; keep user input to plain words.
pub fn search_terms(query: &str) -> String {
    query
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '_' | '@' | '#' | '$') { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn search_casts(inv: &Invocation, query: &str, limit: u32) -> Result<Vec<RawCast>, UpstreamError> {
    let search: Option<CastSearch> =
        get_json(inv, Upstream::Hypersnap, &v2("cast/search", &[("q", query), ("limit", &limit.to_string())]), true)
            .await?;
    Ok(search.and_then(|s| s.result).map(|f| f.casts).unwrap_or_default())
}

pub async fn users_bulk(inv: &Invocation, fids: &[u64]) -> Result<Vec<RawUser>, UpstreamError> {
    if fids.is_empty() {
        return Ok(Vec::new());
    }
    let list = fids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    let found: Option<RawUserList> =
        get_json(inv, Upstream::Hypersnap, &v2("user/bulk", &[("fids", &list)]), true).await?;
    Ok(found.map(|l| l.users).unwrap_or_default())
}

pub async fn user_casts(inv: &Invocation, fid: u64, limit: u32) -> Result<Vec<RawCast>, UpstreamError> {
    let feed: Option<RawFeed> = get_json(
        inv,
        Upstream::Hypersnap,
        &v2(
            "feed/user/casts",
            &[("fid", &fid.to_string()), ("limit", &limit.to_string()), ("include_replies", "true")],
        ),
        true,
    )
    .await?;
    Ok(feed.map(|f| f.casts).unwrap_or_default())
}

#[derive(Deserialize)]
struct CastResponse {
    cast: Option<RawCast>,
}

pub async fn cast(inv: &Invocation, hash: &str) -> Result<Option<RawCast>, UpstreamError> {
    let found: Option<CastResponse> =
        get_json(inv, Upstream::Hypersnap, &v2("cast", &[("identifier", hash), ("type", "hash")]), true).await?;
    Ok(found.and_then(|r| r.cast))
}

#[derive(Deserialize)]
struct Conversation {
    cast: Option<RawCast>,
}

#[derive(Deserialize)]
struct ConversationResponse {
    conversation: Option<Conversation>,
}

/// A cast with its replies `depth` levels deep.
pub async fn conversation(inv: &Invocation, hash: &str, depth: u32) -> Result<Option<RawCast>, UpstreamError> {
    match conversation_body(inv, hash, depth).await? {
        Some(body) => parse_conversation(&body),
        None => Ok(None),
    }
}

/// [`conversation`] as Hypersnap sends it, unparsed (`None`: no such cast),
/// so a build can weigh it before parsing it. Hypersnap ignores the
/// `limit`: it sends every direct reply, so a cast with 479 of them came as
/// 650 KB (20 October 2026).
pub async fn conversation_body(inv: &Invocation, hash: &str, depth: u32) -> Result<Option<Vec<u8>>, UpstreamError> {
    let url = v2(
        "cast/conversation",
        &[("identifier", hash), ("type", "hash"), ("reply_depth", &depth.to_string()), ("limit", "50")],
    );
    upstream::send(inv, Upstream::Hypersnap, HttpMethod::Get, &url, None, true).await
}

/// The cast a [`conversation_body`] holds, with its replies.
pub fn parse_conversation(body: &[u8]) -> Result<Option<RawCast>, UpstreamError> {
    let found: ConversationResponse = upstream::parse(body)?;
    Ok(found.conversation.and_then(|c| c.cast))
}

// --- Hub (v1) endpoints, for signers ---------------------------------------

#[derive(Deserialize)]
struct HubMessageData {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct HubMessage {
    data: Option<HubMessageData>,
    hash: Option<String>,
    signer: Option<String>,
}

#[derive(Deserialize)]
struct HubMessages {
    #[serde(default)]
    messages: Vec<HubMessage>,
}

#[derive(Deserialize)]
struct LinkBody {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "targetFid")]
    target_fid: Option<u64>,
}

#[derive(Deserialize)]
struct LinkData {
    #[serde(rename = "type")]
    kind: Option<String>,
    fid: Option<u64>,
    timestamp: Option<u32>,
    #[serde(rename = "linkBody")]
    link_body: Option<LinkBody>,
}

#[derive(Deserialize)]
struct LinkMessage {
    data: Option<LinkData>,
}

#[derive(Deserialize)]
struct LinkPage {
    #[serde(default)]
    messages: Vec<LinkMessage>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

/// A page of an account's followers (`linksByTargetFid`) or of whom it
/// follows (`linksByFid`), newest first, `page_size` links a shard, from
/// where `ask` says each shard goes on (its first page without), and the
/// bytes it came as.
///
/// The node's token for followers is base64 of a JSON array with each
/// shard's key, `null` for a shard at its end. Sent back as it came, a
/// `null` starts that shard over from its newest link (on 21 October 2026),
/// so a list whose shards end on different pages never ends: a shard read
/// to its end, or left out, is sent as `[]`, which reads nothing of it. The
/// token for whom an account follows is base64 of its one shard's key.
pub async fn follow_links(
    inv: &Invocation,
    dir: FollowsDir,
    fid: u64,
    page_size: usize,
    ask: Option<&[Option<&[u8]>]>,
) -> Result<(follows::Read, usize), UpstreamError> {
    let (fid, size) = (fid.to_string(), page_size.to_string());
    let token = ask.map(|ask| match dir {
        FollowsDir::Followers => {
            let shards: Vec<String> = ask
                .iter()
                .map(|key| {
                    let bytes: Vec<String> = key.unwrap_or_default().iter().map(u8::to_string).collect();
                    format!("[{}]", bytes.join(","))
                })
                .collect();
            base64_encode(format!("[{}]", shards.join(",")).as_bytes())
        }
        FollowsDir::Following => base64_encode(ask.first().copied().flatten().unwrap_or_default()),
    });
    let (path, account) = match dir {
        FollowsDir::Followers => ("linksByTargetFid", "target_fid"),
        FollowsDir::Following => ("linksByFid", "fid"),
    };
    let mut query =
        vec![(account, fid.as_str()), ("link_type", "follow"), ("pageSize", size.as_str()), ("reverse", "true")];
    if let Some(token) = &token {
        query.push(("pageToken", token.as_str()));
    }
    let Some(body) = upstream::send(inv, Upstream::Hypersnap, HttpMethod::Get, &v1(path, &query), None, true).await?
    else {
        return Ok((follows::Read::default(), 0));
    };
    let page: LinkPage = upstream::parse(&body)?;
    let links = page
        .messages
        .into_iter()
        .filter_map(|message| {
            let data = message.data?;
            let body = data.link_body?;
            let follow =
                data.kind.as_deref() == Some("MESSAGE_TYPE_LINK_ADD") && body.kind.as_deref() == Some("follow");
            let other = match dir {
                FollowsDir::Followers => data.fid?,
                FollowsDir::Following => body.target_fid?,
            };
            follow.then_some(follows::Link { fid: other, ts: data.timestamp? })
        })
        .collect();
    let token = page.next_page_token.filter(|t| !t.is_empty());
    let next = match dir {
        FollowsDir::Followers => token
            .and_then(|t| base64_decode(&t))
            .and_then(|json| serde_json::from_slice::<Vec<Option<Vec<u8>>>>(&json).ok())
            .unwrap_or_default(),
        FollowsDir::Following => vec![token.and_then(|t| base64_decode(&t)).filter(|key| !key.is_empty())],
    };
    Ok((follows::Read { links, next }, body.len()))
}

/// The signer of one cast.
pub async fn cast_signer(inv: &Invocation, fid: u64, hash: &str) -> Result<Option<String>, UpstreamError> {
    let found: Option<HubMessage> =
        get_json(inv, Upstream::Hypersnap, &v1("castById", &[("fid", &fid.to_string()), ("hash", hash)]), true).await?;
    Ok(found.and_then(|m| m.signer).and_then(|s| normalize_key(&s)))
}

/// cast hash -> signer for an account's most recent casts.
pub async fn recent_cast_signers(
    inv: &Invocation,
    fid: u64,
    page_size: u32,
) -> Result<HashMap<String, String>, UpstreamError> {
    let found: Option<HubMessages> = get_json(
        inv,
        Upstream::Hypersnap,
        &v1("castsByFid", &[("fid", &fid.to_string()), ("pageSize", &page_size.to_string()), ("reverse", "true")]),
        true,
    )
    .await?;
    let mut out = HashMap::new();
    for message in found.map(|m| m.messages).unwrap_or_default() {
        // castsByFid also returns CastRemove messages.
        if message.data.as_ref().and_then(|d| d.kind.as_deref()) != Some("MESSAGE_TYPE_CAST_ADD") {
            continue;
        }
        if let (Some(hash), Some(signer)) = (message.hash, message.signer.as_deref().and_then(normalize_key)) {
            out.insert(allthing_core::params::normalize_hash(&hash), signer);
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignerEventBody {
    key: Option<String>,
    event_type: Option<String>,
    metadata: Option<String>,
    metadata_type: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnChainEvent {
    signer_event_body: Option<SignerEventBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HubSigner {
    key: Option<String>,
    request_fid: Option<u64>,
    on_chain_event: Option<OnChainEvent>,
}

#[derive(Deserialize)]
struct HubSigners {
    #[serde(default)]
    signers: Vec<HubSigner>,
}

#[derive(Deserialize)]
struct HubEvents {
    #[serde(default)]
    events: Vec<OnChainEvent>,
}

fn app_from_event(body: &SignerEventBody) -> Option<u64> {
    if body.metadata_type != Some(1) {
        return None;
    }
    request_fid_from_metadata(&base64_decode(body.metadata.as_deref()?)?)
}

/// signer key -> requesting app fid (`None` when the key carries no
/// signed-key-request metadata), for an account's active signers: onchain
/// KeyRegistry keys and gasless `KEY_ADD` keys alike.
pub async fn active_signers(inv: &Invocation, fid: u64) -> Result<HashMap<String, Option<u64>>, UpstreamError> {
    let found: Option<HubSigners> = get_json(
        inv,
        Upstream::Hypersnap,
        &v1("signersByFid", &[("fid", &fid.to_string()), ("pageSize", "1000")]),
        true,
    )
    .await?;
    let mut out = HashMap::new();
    for signer in found.map(|s| s.signers).unwrap_or_default() {
        let Some(key) = signer.key.as_deref().and_then(normalize_key) else { continue };
        let app = signer
            .request_fid
            .filter(|fid| *fid > 0)
            .or_else(|| signer.on_chain_event.as_ref()?.signer_event_body.as_ref().and_then(app_from_event));
        out.insert(key, app);
    }
    Ok(out)
}

/// Every onchain signer ever added for an account, including removed keys,
/// whose casts outlive them since Snapchain V14.
pub async fn all_onchain_signers(inv: &Invocation, fid: u64) -> Result<HashMap<String, Option<u64>>, UpstreamError> {
    let found: Option<HubEvents> = get_json(
        inv,
        Upstream::Hypersnap,
        &v1("onChainEventsByFid", &[("fid", &fid.to_string()), ("event_type", "EVENT_TYPE_SIGNER")]),
        true,
    )
    .await?;
    let mut out = HashMap::new();
    for body in found.map(|e| e.events).unwrap_or_default().iter().filter_map(|e| e.signer_event_body.as_ref()) {
        if body.event_type.as_deref() != Some("SIGNER_EVENT_TYPE_ADD") {
            continue;
        }
        if let Some(key) = body.key.as_deref().and_then(normalize_key) {
            out.insert(key, app_from_event(body));
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedSignerBody {
    key: Option<String>,
    key_type: Option<u32>,
    event_type: Option<String>,
    metadata: Option<String>,
    metadata_type: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedSignerEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    fid: Option<u64>,
    signer_event_body: Option<VerifiedSignerBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedSigner {
    source: Option<String>,
    key: Option<String>,
    key_type: Option<u32>,
    fid: Option<u64>,
    request_fid: Option<u64>,
    on_chain_event: Option<VerifiedSignerEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedSigners {
    // Required: a successful response without the collection is not evidence.
    signers: Vec<VerifiedSigner>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedSignerEvents {
    events: Vec<VerifiedSignerEvent>,
    next_page_token: Option<String>,
}

fn verified_signer_key(value: &str) -> Option<String> {
    normalize_key(value).filter(|key| key.len() == 66)
}

fn verified_onchain_entry(event: &VerifiedSignerEvent, fid: u64) -> Option<(String, u64)> {
    if event.kind.as_deref() != Some("EVENT_TYPE_SIGNER") || event.fid != Some(fid) {
        return None;
    }
    let body = event.signer_event_body.as_ref()?;
    if body.event_type.as_deref() != Some("SIGNER_EVENT_TYPE_ADD") || body.key_type != Some(1) {
        return None;
    }
    let key = verified_signer_key(body.key.as_deref()?)?;
    let app = match (body.metadata_type, body.metadata.as_deref()) {
        (Some(0), Some("")) => 0,
        (Some(1), Some(metadata)) => verified_request_fid_from_metadata(metadata)?,
        _ => return None,
    };
    Some((key, app))
}

fn insert_verified_app(out: &mut HashMap<String, u64>, conflicts: &mut HashSet<String>, key: String, app: u64) {
    if conflicts.contains(&key) {
        return;
    }
    if out.get(&key).is_some_and(|previous| *previous != app) {
        out.remove(&key);
        conflicts.insert(key);
    } else {
        out.insert(key, app);
    }
}

fn verified_active_apps(page: VerifiedSigners, fid: u64) -> HashMap<String, u64> {
    let partial = page.next_page_token.is_some_and(|token| !token.is_empty());
    let (mut out, mut conflicts) = (HashMap::new(), HashSet::new());
    for signer in page.signers {
        if signer.fid != Some(fid) || signer.key_type != Some(1) {
            continue;
        }
        let Some(key) = signer.key.as_deref().and_then(verified_signer_key) else { continue };
        let app = match signer.source.as_deref() {
            Some("SIGNER_SOURCE_OFFCHAIN") => signer.request_fid.filter(|app| *app > 0),
            Some("SIGNER_SOURCE_ONCHAIN") => signer
                .on_chain_event
                .as_ref()
                .and_then(|event| verified_onchain_entry(event, fid))
                .filter(|(event_key, _)| *event_key == key)
                .map(|(_, app)| app),
            _ => None,
        };
        if let Some(app) = app.filter(|app| !partial || *app > 0) {
            insert_verified_app(&mut out, &mut conflicts, key, app);
        }
    }
    out
}

fn verified_history_apps(page: VerifiedSignerEvents, fid: u64) -> HashMap<String, u64> {
    let partial = page.next_page_token.is_some_and(|token| !token.is_empty());
    let (mut out, mut conflicts) = (HashMap::new(), HashSet::new());
    for event in page.events {
        if let Some((key, app)) = verified_onchain_entry(&event, fid).filter(|(_, app)| !partial || *app > 0) {
            insert_verified_app(&mut out, &mut conflicts, key, app);
        }
    }
    out
}

/// Exact 32-byte active signer keys with a positively identified requesting
/// app. Zero requires an explicit onchain add with empty type-0 metadata;
/// absent keys and malformed or incomplete evidence remain absent (unknown).
pub async fn verified_signer_apps(inv: &Invocation, fid: u64) -> Result<HashMap<String, u64>, UpstreamError> {
    let page: VerifiedSigners = get_json(
        inv,
        Upstream::Hypersnap,
        &v1("signersByFid", &[("fid", &fid.to_string()), ("pageSize", "1000")]),
        false,
    )
    .await?
    .ok_or_else(|| UpstreamError::Parse("missing signer response".into()))?;
    Ok(verified_active_apps(page, fid))
}

/// Positive attribution facts for exact keys in one bounded onchain-history
/// page. A missing key is unknown, including when another page exists.
pub async fn verified_onchain_signers(inv: &Invocation, fid: u64) -> Result<HashMap<String, u64>, UpstreamError> {
    let page: VerifiedSignerEvents = get_json(
        inv,
        Upstream::Hypersnap,
        &v1("onChainEventsByFid", &[("fid", &fid.to_string()), ("event_type", "EVENT_TYPE_SIGNER")]),
        false,
    )
    .await?
    .ok_or_else(|| UpstreamError::Parse("missing signer history response".into()))?;
    Ok(verified_history_apps(page, fid))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// A hub that knows fid 3 by its fname `dwr` and its old ENS name
    /// `dwr.eth`, and nobody else; and the user data of fid 3, whose
    /// website is set, and fid 2, who cleared theirs.
    struct Hub {
        requests: RefCell<Vec<String>>,
    }

    impl Host for Hub {
        fn now(&self) -> f64 {
            1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let path = request.url.trim_start_matches(BASE).to_owned();
            self.requests.borrow_mut().push(path.clone());
            let ok = |json: &str| HttpResponse { status: 200, retry_after: None, body: Some(json.into()) };
            let status = |status: u16| HttpResponse { status, retry_after: None, body: None };
            let dwr = r#"{"fid":3,"username":"dwr","display_name":"Dan Romero"}"#;
            let response = match path.as_str() {
                "/v2/farcaster/user/by_username?username=dwr" => ok(&format!(r#"{{"user":{dwr}}}"#)),
                "/v1/userNameProofByName?name=dwr.eth" => {
                    ok(r#"{"name":"dwr.eth","fid":3,"type":"USERNAME_TYPE_ENS_L1"}"#)
                }
                "/v2/farcaster/user/bulk?fids=3" => ok(&format!(r#"{{"users":[{dwr}]}}"#)),
                // The hub answers 400 for a name nobody holds.
                p if p.starts_with("/v1/userNameProofByName") => status(400),
                "/v1/userDataByFid?fid=3" => {
                    ok(&user_data(&[("USER_DATA_TYPE_BIO", "gm"), ("USER_DATA_TYPE_URL", "https://danromero.org")]))
                }
                "/v1/userDataByFid?fid=2" => {
                    ok(&user_data(&[("USER_DATA_TYPE_URL", ""), ("USER_DATA_TYPE_LOCATION", "geo:34.05,-118.24")]))
                }
                _ => status(404),
            };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    /// A `userDataByFid` answer with a message of each (type, value).
    fn user_data(messages: &[(&str, &str)]) -> String {
        let messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|(kind, value)| {
                serde_json::json!({ "data": { "type": "MESSAGE_TYPE_USER_DATA_ADD", "fid": 3, "timestamp": 141496553,
                    "userDataBody": { "type": kind, "value": value } } })
            })
            .collect();
        serde_json::json!({ "messages": messages, "nextPageToken": "" }).to_string()
    }

    fn lookup(name: &str) -> (Option<u64>, Vec<String>) {
        let hub = Rc::new(Hub { requests: RefCell::new(vec![]) });
        let inv = Invocation::new(hub.clone(), 45);
        let user = user_by_username(&inv, name).now_or_never().unwrap().unwrap();
        (user.map(|u| u.fid), hub.requests.take())
    }

    #[test]
    fn a_website_is_one_read_of_the_accounts_user_data() {
        let read = |fid: u64| {
            let hub = Rc::new(Hub { requests: RefCell::new(vec![]) });
            let inv = Invocation::new(hub.clone(), 45);
            let website = website(&inv, fid).now_or_never().unwrap().unwrap();
            (website.map(|w| w.href), hub.requests.take())
        };
        assert_eq!(read(3), (Some("https://danromero.org".into()), vec!["/v1/userDataByFid?fid=3".to_owned()]));
        // Cleared, or no user data at all (the hub answers 404).
        assert_eq!(read(2).0, None);
        assert_eq!(read(99).0, None);
    }

    #[test]
    fn a_current_fname_takes_one_request() {
        assert_eq!(lookup("dwr"), (Some(3), vec!["/v2/farcaster/user/by_username?username=dwr".to_owned()]));
    }

    #[test]
    fn a_name_the_account_moved_away_from_is_found_by_its_proof() {
        let (fid, requests) = lookup("dwr.eth");
        assert_eq!(fid, Some(3));
        assert_eq!(
            requests,
            [
                "/v2/farcaster/user/by_username?username=dwr.eth",
                "/v1/userNameProofByName?name=dwr.eth",
                "/v2/farcaster/user/bulk?fids=3",
            ]
        );
        // Proofs are kept lowercase.
        assert_eq!(lookup("DWR.eth").0, Some(3));
    }

    #[test]
    fn a_name_nobody_holds_is_not_found() {
        let (fid, requests) = lookup("nobody.eth");
        assert_eq!(fid, None);
        assert_eq!(requests.len(), 2);
    }

    #[test]
    fn hypersnap_is_asked_for_the_windows_it_ranks() {
        assert_eq!(hypersnap_window(TrendingWindow::Hour).as_str(), "1h");
        assert_eq!(hypersnap_window(TrendingWindow::SixHours).as_str(), "6h");
        // Not a window of its own: its 24-hour list, cut to 12 hours after.
        assert_eq!(hypersnap_window(TrendingWindow::TwelveHours).as_str(), "24h");
        assert_eq!(hypersnap_window(TrendingWindow::Day).as_str(), "24h");
        assert_eq!(hypersnap_window(TrendingWindow::Week).as_str(), "7d");
    }

    fn verified_metadata(fid: u64) -> String {
        let word = |n: u64| {
            let mut out = vec![0; 24];
            out.extend(n.to_be_bytes());
            out
        };
        let mut bytes = word(32);
        bytes.extend(word(fid));
        bytes.extend([0; 12]);
        bytes.extend([1; 20]);
        bytes.extend(word(128));
        bytes.extend(word(200_000_000));
        bytes.extend(word(65));
        bytes.extend([7; 65]);
        bytes.extend([0; 31]);
        base64_encode(&bytes)
    }

    fn verified_add(key: &str, app: Option<u64>) -> serde_json::Value {
        let (metadata_type, metadata) = match app {
            Some(app) => (1, verified_metadata(app)),
            None => (0, String::new()),
        };
        serde_json::json!({"type":"EVENT_TYPE_SIGNER","fid":42,"signerEventBody":{
            "key":key,"keyType":1,"eventType":"SIGNER_EVENT_TYPE_ADD",
            "metadataType":metadata_type,"metadata":metadata
        }})
    }

    fn verified_signer(key: &str, app: u64) -> serde_json::Value {
        serde_json::json!({"source":"SIGNER_SOURCE_OFFCHAIN","fid":42,"keyType":1,"key":key,"requestFid":app})
    }

    #[test]
    fn verified_signer_identity_keeps_keys_with_the_same_prefix_separate() {
        let key_a = format!("0x{}", "ab".repeat(32));
        let key_b = format!("0x{}cd", "ab".repeat(31));
        let page = serde_json::from_value(serde_json::json!({"signers":[
            verified_signer(&key_a,9152), verified_signer(&key_b,309857),
            verified_signer("0xabababab",9152)
        ]}))
        .unwrap();
        assert_eq!(verified_active_apps(page, 42), HashMap::from([(key_a, 9152), (key_b, 309857)]));
    }

    #[test]
    fn verified_metadata_requires_a_complete_positive_request() {
        assert_eq!(verified_request_fid_from_metadata(&verified_metadata(9152)), Some(9152));
        assert_eq!(verified_request_fid_from_metadata(&verified_metadata(0)), None);
        let mut truncated = base64_decode(&verified_metadata(9152)).unwrap();
        truncated.truncate(160);
        assert_eq!(verified_request_fid_from_metadata(&base64_encode(&truncated)), None);
        assert_eq!(verified_request_fid_from_metadata(&(verified_metadata(9152) + "ignored")), None);
    }

    #[test]
    fn verified_onchain_none_requires_explicit_empty_type_zero_metadata() {
        let key = format!("0x{}", "ab".repeat(32));
        let good = verified_add(&key, None);
        let entry = |value| verified_onchain_entry(&serde_json::from_value(value).unwrap(), 42);
        assert_eq!(entry(good.clone()), Some((key.clone(), 0)));
        let mut absent = good.clone();
        absent["signerEventBody"].as_object_mut().unwrap().remove("metadata");
        assert_eq!(entry(absent), None);
        let mut nonempty = good.clone();
        nonempty["signerEventBody"]["metadata"] = serde_json::json!("AA==");
        assert_eq!(entry(nonempty), None);
        let mut unknown = good.clone();
        unknown["signerEventBody"]["metadataType"] = serde_json::json!(99);
        assert_eq!(entry(unknown), None);
        let mut wrong_fid = good.clone();
        wrong_fid["fid"] = serde_json::json!(7);
        assert_eq!(entry(wrong_fid), None);
        let mut removed = good;
        removed["signerEventBody"]["eventType"] = serde_json::json!("SIGNER_EVENT_TYPE_REMOVE");
        assert_eq!(entry(removed), None);
    }

    #[test]
    fn verified_active_onchain_keys_must_match_their_add_event() {
        let key = format!("0x{}", "ab".repeat(32));
        let other = format!("0x{}", "cd".repeat(32));
        let mut record = serde_json::json!({"source":"SIGNER_SOURCE_ONCHAIN","fid":42,
            "keyType":1,"key":key,"onChainEvent":verified_add(&key,Some(9152))});
        let read = |record| {
            let page = serde_json::from_value(serde_json::json!({"signers":[record]})).unwrap();
            verified_active_apps(page, 42)
        };
        assert_eq!(read(record.clone()), HashMap::from([(key, 9152)]));
        record["key"] = serde_json::json!(other);
        assert!(read(record).is_empty());
    }

    #[test]
    fn verified_pages_leave_missing_and_conflicting_keys_unknown() {
        let key_a = format!("0x{}", "ab".repeat(32));
        let key_b = format!("0x{}", "cd".repeat(32));
        let page = serde_json::from_value(serde_json::json!({"events":[
            verified_add(&key_a,Some(9152)), verified_add(&key_b,None)
        ],"nextPageToken":"more"}))
        .unwrap();
        assert_eq!(verified_history_apps(page, 42), HashMap::from([(key_a.clone(), 9152)]));
        let page = serde_json::from_value(serde_json::json!({"signers":[
            verified_signer(&key_a,9152), verified_signer(&key_a,309857), verified_signer(&key_a,9152)
        ]}))
        .unwrap();
        assert!(verified_active_apps(page, 42).is_empty());
        assert!(serde_json::from_str::<VerifiedSigners>(r#"{"nextPageToken":""}"#).is_err());
        assert!(serde_json::from_str::<VerifiedSignerEvents>(r#"{"events":null}"#).is_err());
    }

    struct VerifiedSignerHub {
        body: String,
        requests: RefCell<Vec<String>>,
    }

    impl Host for VerifiedSignerHub {
        fn now(&self) -> f64 {
            0.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            self.requests.borrow_mut().push(request.url);
            let response = HttpResponse { status: 200, retry_after: None, body: Some(self.body.clone().into()) };
            async move { Ok(response) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    #[test]
    fn verified_signer_reads_are_bounded_hypersnap_calls_and_reject_missing_arrays() {
        let key = format!("0x{}", "ab".repeat(32));
        let hub = Rc::new(VerifiedSignerHub {
            body: serde_json::json!({"signers":[verified_signer(&key,9152)],"nextPageToken":"more"}).to_string(),
            requests: RefCell::new(vec![]),
        });
        let inv = Invocation::new(hub.clone(), 45);
        assert_eq!(verified_signer_apps(&inv, 42).now_or_never().unwrap().unwrap(), HashMap::from([(key, 9152)]));
        assert_eq!(hub.requests.take(), [format!("{BASE}/v1/signersByFid?fid=42&pageSize=1000")]);
        let hub = Rc::new(VerifiedSignerHub { body: "{}".into(), requests: RefCell::new(vec![]) });
        let inv = Invocation::new(hub.clone(), 45);
        assert!(matches!(verified_onchain_signers(&inv, 42).now_or_never().unwrap(), Err(UpstreamError::Parse(_))));
        assert_eq!(hub.requests.take(), [format!("{BASE}/v1/onChainEventsByFid?fid=42&event_type=EVENT_TYPE_SIGNER")]);
    }
}
