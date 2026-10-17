// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hypersnap, a Snapchain read node that also serves a Neynar-shaped
//! `/v2/farcaster` API. Allthing reads every Farcaster message through it.

use std::collections::HashMap;

use allthing_core::channels::{
    canonical_parent_url, channel_id_for_url, legacy_url, parent_url_variants, short_parent_url_label,
};
use allthing_core::model::Channel;
use allthing_core::normalize::{self, RawCast, RawChannel, RawFeed, RawUser, RawUserList, RawUserSearch};
use allthing_core::provenance::{base64_decode, normalize_key, request_fid_from_metadata};
use serde::Deserialize;

use crate::Invocation;
use crate::upstream::{Upstream, UpstreamError, get_json, url};

pub const BASE: &str = "https://haatz.quilibrium.com";

fn v2(path: &str, query: &[(&str, &str)]) -> String {
    url(BASE, &format!("v2/farcaster/{path}"), query)
}

fn v1(path: &str, query: &[(&str, &str)]) -> String {
    url(BASE, &format!("v1/{path}"), query)
}

pub async fn trending_casts(inv: &Invocation, limit: u32) -> Result<Vec<RawCast>, UpstreamError> {
    let feed: Option<RawFeed> =
        get_json(inv, Upstream::Hypersnap, &v2("feed/trending", &[("limit", &limit.to_string())]), false).await?;
    Ok(feed.map(|f| f.casts).unwrap_or_default())
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
    Ok(Some(Channel {
        id: known
            .as_ref()
            .map(|c| c.id.clone())
            .or_else(|| channel_id_for_url(&parent_url))
            .unwrap_or_else(|| canonical_parent_url(&parent_url)),
        name: known
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| if legacy.is_some() { key.to_owned() } else { short_parent_url_label(&parent_url) }),
        description: known.as_ref().and_then(|c| c.description.clone()),
        image_url: known.as_ref().and_then(|c| c.image_url.clone()),
        member_count: known.as_ref().and_then(|c| c.member_count),
        created_at: known.as_ref().and_then(|c| c.created_at),
        parent_url,
    }))
}

pub async fn channel_casts(inv: &Invocation, parent_url: &str, limit: u32) -> Result<Vec<RawCast>, UpstreamError> {
    let variants = parent_url_variants(parent_url).join(",");
    let feed: Option<RawFeed> = get_json(
        inv,
        Upstream::Hypersnap,
        &v2(
            "feed/parent_urls",
            &[("parent_urls", &variants), ("limit", &limit.to_string()), ("with_recasts", "false")],
        ),
        true,
    )
    .await?;
    Ok(feed.map(|f| f.casts).unwrap_or_default())
}

#[derive(Deserialize)]
struct UserResponse {
    user: Option<RawUser>,
}

/// By fname, falling back to search for ENS-style names (which Hypersnap's
/// `by_username` does not resolve), accepting only an exact username match.
pub async fn user_by_username(inv: &Invocation, username: &str) -> Result<Option<RawUser>, UpstreamError> {
    let found: Option<UserResponse> =
        get_json(inv, Upstream::Hypersnap, &v2("user/by_username", &[("username", username)]), true).await?;
    if let Some(user) = found.and_then(|r| r.user) {
        return Ok(Some(user));
    }
    let search: Option<RawUserSearch> =
        get_json(inv, Upstream::Hypersnap, &v2("user/search", &[("q", username), ("limit", "10")]), true).await?;
    Ok(search
        .map(RawUserSearch::into_users)
        .unwrap_or_default()
        .into_iter()
        .find(|u| u.username.as_deref().is_some_and(|name| name.eq_ignore_ascii_case(username))))
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

/// A cast with up to five levels of replies.
pub async fn conversation(inv: &Invocation, hash: &str) -> Result<Option<RawCast>, UpstreamError> {
    let found: Option<ConversationResponse> = get_json(
        inv,
        Upstream::Hypersnap,
        &v2("cast/conversation", &[("identifier", hash), ("type", "hash"), ("reply_depth", "5"), ("limit", "50")]),
        true,
    )
    .await?;
    Ok(found.and_then(|r| r.conversation).and_then(|c| c.cast))
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
