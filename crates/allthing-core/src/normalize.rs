// SPDX-License-Identifier: AGPL-3.0-or-later
//! Raw shapes of the Neynar v2 Farcaster API, as served by Hypersnap (fewer
//! fields) and Neynar, and of the Farcaster app's public channel API, and
//! their normalization into [`crate::model`].
//!
//! Only the fields Allthing reads are declared; serde skips the rest, which
//! keeps parsing cheap inside the Worker's CPU budget.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::channels::{
    canonical_parent_url, channel_id_for_url, channel_ref_from_url, channel_url_for_id, short_parent_url_label,
};
use crate::embeds::{MediaKind, classify_url};
use crate::model::{Cast, Channel, ChannelRef, Embed, ExternalLink, Mention, ThreadNode, User, UserSummary};
use crate::params::{hash_from_param, normalize_hash};
use crate::profile::{CARD_BIO_CHARS, UserCard, bio_excerpt};
use crate::text::splice_mentions;
use crate::time::parse_rfc3339;
use crate::website::Website;

#[derive(Debug, Default, Deserialize)]
pub struct RawBio {
    pub text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawProfile {
    pub bio: Option<RawBio>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawPrimary {
    pub eth_address: Option<String>,
    pub sol_address: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawVerified {
    #[serde(default)]
    pub eth_addresses: Vec<String>,
    #[serde(default)]
    pub sol_addresses: Vec<String>,
    pub primary: Option<RawPrimary>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawUser {
    pub fid: u64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub pfp_url: Option<String>,
    pub profile: Option<RawProfile>,
    pub follower_count: Option<u64>,
    pub following_count: Option<u64>,
    pub registered_at: Option<String>,
    pub custody_address: Option<String>,
    pub verified_addresses: Option<RawVerified>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawCastId {
    pub fid: Option<u64>,
    pub hash: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawEmbeddedCastAuthor {
    pub fid: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawEmbeddedCast {
    pub hash: Option<String>,
    pub author: Option<RawEmbeddedCastAuthor>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawOgImage {
    pub url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawHtml {
    #[serde(rename = "ogImage")]
    pub og_image: Option<Vec<RawOgImage>>,
    #[serde(rename = "ogTitle")]
    pub og_title: Option<String>,
    #[serde(rename = "fcFrame", alias = "fc:frame")]
    pub fc_frame: Option<Value>,
    #[serde(rename = "fcMiniapp", alias = "fc:miniapp")]
    pub fc_miniapp: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawEmbedMetadata {
    pub content_type: Option<String>,
    pub html: Option<RawHtml>,
    /// Neynar's parsed frame, for frame embeds.
    pub frame: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawEmbed {
    pub url: Option<String>,
    pub cast_id: Option<RawCastId>,
    pub cast: Option<RawEmbeddedCast>,
    pub metadata: Option<RawEmbedMetadata>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawReactions {
    pub likes_count: Option<u64>,
    pub recasts_count: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawReplies {
    pub count: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawChannelId {
    pub id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawCast {
    pub hash: String,
    pub text: Option<String>,
    pub timestamp: Option<String>,
    pub author: Option<RawUser>,
    pub parent_hash: Option<String>,
    pub parent_url: Option<String>,
    pub root_parent_url: Option<String>,
    pub channel: Option<RawChannelId>,
    pub embeds: Option<Vec<RawEmbed>>,
    pub reactions: Option<RawReactions>,
    pub replies: Option<RawReplies>,
    pub mentioned_profiles: Option<Vec<RawUser>>,
    pub mentioned_profiles_ranges: Option<Vec<RawRange>>,
    pub direct_replies: Option<Vec<RawCast>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawChannel {
    pub id: String,
    pub name: Option<String>,
    pub url: Option<String>,
    pub parent_url: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    /// Hypersnap: how many distinct accounts have cast in the feed, not its
    /// members (it said /dev had 15,664; the Farcaster app counts 475).
    /// Neynar: members.
    pub member_count: Option<u64>,
    /// Neynar only.
    pub follower_count: Option<u64>,
    /// Unix seconds from Neynar, an ISO string from Hypersnap.
    pub created_at: Option<Value>,
}

/// `api.farcaster.xyz/v1/channel?channelId=<id>`, the Farcaster app's public
/// channel API, answers `{ "result": { "channel": … } }` (HTTP 400 for an
/// unknown channel).
#[derive(Debug, Default, Deserialize)]
pub struct RawAppChannelResponse {
    pub result: Option<RawAppChannelResult>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawAppChannelResult {
    pub channel: Option<RawAppChannel>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawExternalLink {
    pub title: Option<String>,
    pub url: Option<String>,
}

/// The Farcaster app's record of a channel: what its hosts set (name,
/// description, pictures, link, pinned cast) and who they are, with true
/// member and follower counts. Hypersnap knows none of it.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawAppChannel {
    pub id: String,
    /// A display name ("Farcaster").
    pub name: Option<String>,
    /// Like cast text, the description leaves mentions out:
    /// `descriptionMentions` are their fids and
    /// `descriptionMentionsPositions` the UTF-8 byte offsets where they belong.
    pub description: Option<String>,
    pub description_mentions: Option<Vec<u64>>,
    pub description_mentions_positions: Option<Vec<u32>>,
    /// The picture.
    pub image_url: Option<String>,
    /// The banner, which many channels don't have.
    pub header_image_url: Option<String>,
    /// Unix seconds.
    pub created_at: Option<Value>,
    pub follower_count: Option<u64>,
    pub member_count: Option<u64>,
    pub lead_fid: Option<u64>,
    /// Usually includes the lead.
    pub moderator_fids: Option<Vec<u64>>,
    /// Absent on most channels.
    pub pinned_cast_hash: Option<String>,
    /// `{ "title": "", "url": "" }` when unset.
    pub external_link: Option<RawExternalLink>,
}

/// At most this many accounts are looked up for a channel record, the most a
/// `user/bulk` call takes.
const APP_CHANNEL_FIDS: usize = 100;

impl RawAppChannel {
    /// The accounts the record names, to read in one `user/bulk` call: the
    /// lead, the moderators, then anyone the description mentions.
    pub fn fids(&self) -> Vec<u64> {
        let mut out: Vec<u64> = Vec::new();
        let named = self.lead_fid.into_iter().chain(self.moderator_fids.iter().flatten().copied());
        for fid in named.chain(self.description_mentions.iter().flatten().copied()) {
            if fid > 0 && !out.contains(&fid) && out.len() < APP_CHANNEL_FIDS {
                out.push(fid);
            }
        }
        out
    }

    /// The pinned cast's hash, normalized, when it is a well-formed one.
    pub fn pinned_hash(&self) -> Option<String> {
        self.pinned_cast_hash.as_deref().and_then(hash_from_param)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct RawNext {
    pub cursor: Option<String>,
}

/// `feed/*` responses.
#[derive(Debug, Default, Deserialize)]
pub struct RawFeed {
    #[serde(default)]
    pub casts: Vec<RawCast>,
    pub next: Option<RawNext>,
}

impl RawFeed {
    /// The cursor for the next page, if there is more.
    pub fn cursor(&self) -> Option<&str> {
        self.next.as_ref().and_then(|n| n.cursor.as_deref()).filter(|c| !c.is_empty())
    }

    /// A page with no casts that still has a cursor for more. Hypersnap
    /// sends some channels' newest casts after a page or two like this,
    /// each pointing to the next; the end of a feed has no cursor.
    pub fn is_empty_with_more(&self) -> bool {
        self.casts.is_empty() && self.cursor().is_some()
    }
}

/// `channel/trending` wraps entries as `{ channel }` on Neynar.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RawChannelEntry {
    Wrapped { channel: RawChannel },
    Plain(RawChannel),
}

impl RawChannelEntry {
    pub fn into_channel(self) -> RawChannel {
        match self {
            RawChannelEntry::Wrapped { channel } | RawChannelEntry::Plain(channel) => channel,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct RawChannelList {
    #[serde(default)]
    pub channels: Vec<RawChannelEntry>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawUserList {
    #[serde(default)]
    pub users: Vec<RawUser>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawSearchResult {
    #[serde(default)]
    pub users: Vec<RawUser>,
}

/// The fname server's transfer log for an account (`/transfers?fid=`):
/// each username it was given or gave up, with Unix seconds.
#[derive(Debug, Default, Deserialize)]
pub struct RawTransfers {
    #[serde(default)]
    pub transfers: Vec<RawTransfer>,
}

#[derive(Debug, Deserialize)]
pub struct RawTransfer {
    pub timestamp: i64,
    /// The FID the username went to (0 when it was given up).
    #[serde(default)]
    pub to: u64,
}

/// When `fid` first got a username.
pub fn first_username_at(raw: &RawTransfers, fid: u64) -> Option<i64> {
    raw.transfers.iter().filter(|t| t.to == fid && t.timestamp > 0).map(|t| t.timestamp).min()
}

/// The hub's `userDataByFid`: the user-data messages an account has signed
/// (its picture, names, bio, website and the rest), the latest of each type.
#[derive(Debug, Default, Deserialize)]
pub struct RawUserData {
    #[serde(default)]
    pub messages: Vec<RawUserDataMessage>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawUserDataMessage {
    pub data: Option<RawUserDataFields>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawUserDataFields {
    #[serde(rename = "userDataBody")]
    pub body: Option<RawUserDataBody>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawUserDataBody {
    /// `"USER_DATA_TYPE_URL"` and the like, or the protobuf's number for it
    /// from a node that writes enums as numbers.
    #[serde(rename = "type")]
    pub kind: Option<Value>,
    pub value: Option<String>,
}

impl RawUserDataBody {
    /// Whether this is the account's URL (`USER_DATA_TYPE_URL`, 5).
    fn is_url(&self) -> bool {
        match &self.kind {
            Some(Value::String(kind)) => kind == "USER_DATA_TYPE_URL",
            Some(Value::Number(kind)) => kind.as_u64() == Some(5),
            _ => false,
        }
    }
}

/// The website an account's user data sets: the value of its URL message
/// (the last, should a node list more than one), if it is an address a
/// profile can link to ([`Website::parse`]). An empty value is a website
/// the account cleared; no URL message, one never set.
pub fn website(raw: &RawUserData) -> Option<Website> {
    let bodies = raw.messages.iter().filter_map(|message| message.data.as_ref()?.body.as_ref());
    let url = bodies.rev().find(|body| body.is_url())?;
    Website::parse(url.value.as_deref()?)
}

/// `user/search`: Hypersnap answers `{ users }`, Neynar `{ result: { users } }`.
#[derive(Debug, Default, Deserialize)]
pub struct RawUserSearch {
    #[serde(default)]
    pub users: Vec<RawUser>,
    pub result: Option<RawSearchResult>,
}

impl RawUserSearch {
    pub fn into_users(self) -> Vec<RawUser> {
        if self.users.is_empty() { self.result.map(|r| r.users).unwrap_or_default() } else { self.users }
    }
}

/// Only the fields a [`UserSummary`] and a profile card ([`UserCard`]) show.
/// A reaction list holds up to 100 full user objects (about 1.2 KB each,
/// most of it addresses and verifications), and a batch of previews reads up
/// to 12 lists: declaring just these keeps parsing them cheap.
#[derive(Debug, Default, Deserialize)]
pub struct RawUserSummary {
    pub fid: u64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub pfp_url: Option<String>,
    pub profile: Option<RawProfile>,
    pub follower_count: Option<u64>,
    pub following_count: Option<u64>,
    pub registered_at: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawReaction {
    pub reaction_timestamp: Option<String>,
    pub user: Option<RawUserSummary>,
}

/// `reactions/cast`: who liked or recasted a cast.
#[derive(Debug, Default, Deserialize)]
pub struct RawReactionList {
    #[serde(default)]
    pub reactions: Vec<RawReaction>,
}

fn username_or_fid(fid: u64, username: Option<&str>) -> String {
    match username.map(str::trim) {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => format!("fid:{fid}"),
    }
}

fn username_for(raw: &RawUser) -> String {
    username_or_fid(raw.fid, raw.username.as_deref())
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

fn summary(fid: u64, username: Option<&str>, display_name: Option<&str>, pfp_url: Option<&str>) -> UserSummary {
    UserSummary {
        fid,
        username: username_or_fid(fid, username),
        display_name: display_name.map(str::trim).unwrap_or_default().to_owned(),
        pfp_url: non_empty(pfp_url),
    }
}

pub fn user_summary(raw: &RawUser) -> UserSummary {
    summary(raw.fid, raw.username.as_deref(), raw.display_name.as_deref(), raw.pfp_url.as_deref())
}

/// Who reacted, newest first: at most `limit` people, and whether more
/// reacted. Hypersnap lists each shard's reactions oldest first, one shard
/// after another, so they are put in time order here: reversing the list
/// would rank the last shard's oldest above the first shard's newest.
pub fn reactors(raw: &[RawReaction], limit: usize) -> (Vec<UserSummary>, bool) {
    let mut timed: Vec<(i64, &RawUserSummary)> = raw
        .iter()
        .filter_map(|r| {
            let at = r.reaction_timestamp.as_deref().and_then(parse_rfc3339).unwrap_or(i64::MIN);
            Some((at, r.user.as_ref()?))
        })
        .collect();
    // Stable, so reactions in the same second keep the provider's order,
    // and the later one in that order counts as the newer.
    timed.sort_by_key(|(at, _)| *at);
    let users: Vec<UserSummary> = timed
        .iter()
        .rev()
        .take(limit)
        .map(|(_, u)| summary(u.fid, u.username.as_deref(), u.display_name.as_deref(), u.pfp_url.as_deref()))
        .collect();
    let more = timed.len() > users.len();
    (users, more)
}

pub fn user(raw: &RawUser) -> User {
    let verified = raw.verified_addresses.as_ref();
    let primary = verified.and_then(|v| v.primary.as_ref());
    User {
        fid: raw.fid,
        username: username_for(raw),
        display_name: raw.display_name.as_deref().map(str::trim).unwrap_or_default().to_owned(),
        pfp_url: non_empty(raw.pfp_url.as_deref()),
        bio: raw.profile.as_ref().and_then(|p| p.bio.as_ref()).and_then(|b| b.text.clone()).unwrap_or_default(),
        follower_count: raw.follower_count.unwrap_or(0),
        following_count: raw.following_count.unwrap_or(0),
        registered_at: raw.registered_at.as_deref().and_then(parse_rfc3339),
        first_username_at: None,
        // Not in the provider's user object: a profile reads it apart.
        website: None,
        custody_address: non_empty(raw.custody_address.as_deref()).map(|a| a.to_ascii_lowercase()),
        eth_addresses: verified
            .map(|v| v.eth_addresses.iter().map(|a| a.to_ascii_lowercase()).collect())
            .unwrap_or_default(),
        sol_addresses: verified.map(|v| v.sol_addresses.clone()).unwrap_or_default(),
        primary_eth: primary.and_then(|p| non_empty(p.eth_address.as_deref())).map(|a| a.to_ascii_lowercase()),
        primary_sol: primary.and_then(|p| non_empty(p.sol_address.as_deref())),
    }
}

/// A profile card's facts from a provider user object, such as the author
/// object that comes with each cast. `None` when it carries none of them,
/// rather than a card claiming no followers.
pub fn user_card(raw: &RawUser) -> Option<UserCard> {
    card(raw.profile.as_ref(), raw.follower_count, raw.following_count, raw.registered_at.as_deref())
}

/// Profile cards for the people a reactions list names (`listed`), from the
/// user objects the reactions come with: no request of their own.
pub fn reactor_cards(raw: &[RawReaction], listed: &[UserSummary]) -> BTreeMap<u64, UserCard> {
    raw.iter()
        .filter_map(|reaction| reaction.user.as_ref())
        .filter(|user| listed.iter().any(|u| u.fid == user.fid))
        .filter_map(|user| {
            let card =
                card(user.profile.as_ref(), user.follower_count, user.following_count, user.registered_at.as_deref());
            Some((user.fid, card?))
        })
        .collect()
}

fn card(
    profile: Option<&RawProfile>,
    followers: Option<u64>,
    following: Option<u64>,
    registered_at: Option<&str>,
) -> Option<UserCard> {
    let bio = profile.and_then(|p| p.bio.as_ref()).and_then(|b| b.text.as_deref());
    if bio.is_none() && followers.is_none() && following.is_none() && registered_at.is_none() {
        return None;
    }
    Some(UserCard {
        bio: bio.map(|b| bio_excerpt(b, CARD_BIO_CHARS)).unwrap_or_default(),
        followers: followers.unwrap_or(0),
        following: following.unwrap_or(0),
        registered_at: registered_at.and_then(parse_rfc3339),
        first_username_at: None,
    })
}

/// Profile cards for `users`, by fid.
pub fn user_cards<'a>(users: impl IntoIterator<Item = &'a RawUser>) -> BTreeMap<u64, UserCard> {
    users.into_iter().filter_map(|user| Some((user.fid, user_card(user)?))).collect()
}

/// Profile cards for the authors of `casts` and of the replies under them,
/// from the user objects the casts come with, so they cost no request.
pub fn author_cards<'a>(casts: impl IntoIterator<Item = &'a RawCast>) -> BTreeMap<u64, UserCard> {
    let mut cards = BTreeMap::new();
    let mut pending: Vec<&RawCast> = casts.into_iter().collect();
    while let Some(cast) = pending.pop() {
        if let Some(author) = &cast.author
            && !cards.contains_key(&author.fid)
            && let Some(card) = user_card(author)
        {
            cards.insert(author.fid, card);
        }
        pending.extend(cast.direct_replies.iter().flatten());
    }
    cards
}

pub fn embed(raw: &RawEmbed) -> Option<Embed> {
    let quoted = raw.cast_id.as_ref().and_then(|id| Some((id.hash.as_deref()?, id.fid))).or_else(|| {
        let cast = raw.cast.as_ref()?;
        Some((cast.hash.as_deref()?, cast.author.as_ref().and_then(|a| a.fid)))
    });
    if let Some((hash, fid)) = quoted {
        return Some(Embed::Cast { hash: normalize_hash(hash), fid });
    }
    let url = non_empty(raw.url.as_deref())?;
    let metadata = raw.metadata.as_ref();
    Some(match classify_url(&url, metadata.and_then(|m| m.content_type.as_deref())) {
        MediaKind::Image => Embed::Image { url },
        MediaKind::Video => Embed::Video { url },
        MediaKind::Link => {
            let html = metadata.and_then(|m| m.html.as_ref());
            Embed::Link {
                image_url: html
                    .and_then(|h| h.og_image.as_ref())
                    .and_then(|images| images.first())
                    .and_then(|image| non_empty(image.url.as_deref())),
                title: html.and_then(|h| non_empty(h.og_title.as_deref())),
                url,
            }
        }
    })
}

/// A raw embed that is a mini app or frame, by its URL or its metadata
/// (`fc:frame` / `fc:miniapp` meta tags, as Neynar-shaped APIs report them).
fn embed_is_mini_app(raw: &RawEmbed) -> bool {
    if raw.url.as_deref().is_some_and(crate::embeds::is_mini_app_url) {
        return true;
    }
    let Some(metadata) = &raw.metadata else { return false };
    metadata.frame.as_ref().is_some_and(|f| !f.is_null())
        || metadata.html.as_ref().is_some_and(|h| h.fc_frame.is_some() || h.fc_miniapp.is_some())
}

/// Mini app links pasted into the text count too.
fn text_has_mini_app(text: &str) -> bool {
    crate::text::tokenize(text, &[])
        .iter()
        .any(|t| matches!(t, crate::text::Token::Url(url) if crate::embeds::is_mini_app_url(url)))
}

pub fn cast(raw: &RawCast) -> Cast {
    let mentions: Vec<Mention> = raw
        .mentioned_profiles
        .iter()
        .flatten()
        .map(|user| Mention { username: username_for(user), fid: user.fid })
        .collect();
    let ranges: Vec<(u32, u32)> = raw.mentioned_profiles_ranges.iter().flatten().map(|r| (r.start, r.end)).collect();
    let channel_url = non_empty(raw.root_parent_url.as_deref()).or_else(|| non_empty(raw.parent_url.as_deref()));
    let channel = match raw.channel.as_ref().and_then(|c| non_empty(c.id.as_deref())) {
        Some(id) => Some(ChannelRef { label: id.clone(), id }),
        None => channel_ref_from_url(channel_url.as_deref()),
    };
    let text = splice_mentions(raw.text.as_deref().unwrap_or(""), &mentions, &ranges);
    let embed_urls: Vec<&str> = raw.embeds.iter().flatten().filter_map(|e| e.url.as_deref()).collect();
    let spam = crate::spam::is_spam(&text, embed_urls, mentions.iter().map(|m| m.username.as_str()));
    Cast {
        hash: normalize_hash(&raw.hash),
        author: raw.author.as_ref().map_or_else(
            || UserSummary { fid: 0, username: "fid:0".into(), display_name: String::new(), pfp_url: None },
            user_summary,
        ),
        text,
        mentions,
        ts: raw.timestamp.as_deref().and_then(parse_rfc3339).unwrap_or(0),
        parent_hash: non_empty(raw.parent_hash.as_deref()).map(|h| normalize_hash(&h)),
        parent_url: non_empty(raw.parent_url.as_deref()),
        channel,
        embeds: raw.embeds.iter().flatten().filter_map(embed).collect(),
        likes: raw.reactions.as_ref().and_then(|r| r.likes_count).unwrap_or(0),
        recasts: raw.reactions.as_ref().and_then(|r| r.recasts_count).unwrap_or(0),
        replies: raw.replies.as_ref().and_then(|r| r.count).unwrap_or(0),
        mini_app: raw.embeds.iter().flatten().any(embed_is_mini_app)
            || text_has_mini_app(raw.text.as_deref().unwrap_or("")),
        spam,
    }
}

pub fn thread(raw: &RawCast) -> ThreadNode {
    ThreadNode { cast: cast(raw), children: raw.direct_replies.iter().flatten().map(thread).collect() }
}

fn unix_seconds(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => parse_rfc3339(s),
        _ => None,
    }
}

pub fn channel(raw: &RawChannel) -> Channel {
    // Hypersnap uses the parent URL itself as the id of URL-keyed feeds.
    let id_is_url = raw.id.contains(':') || raw.id.contains('/');
    let parent_url = non_empty(raw.parent_url.as_deref())
        .or_else(|| non_empty(raw.url.as_deref()))
        .unwrap_or_else(|| if id_is_url { raw.id.clone() } else { channel_url_for_id(&raw.id) });
    let named_id = if id_is_url { channel_id_for_url(&parent_url) } else { Some(raw.id.to_ascii_lowercase()) };
    // Neynar's `name` is a display name ("Farcaster"); Hypersnap echoes the id.
    let display_name = if id_is_url { None } else { non_empty(raw.name.as_deref()) };
    // Hypersnap's `member_count` counts casters. Only Neynar reports
    // followers, and its `member_count` counts members.
    let neynar = raw.follower_count.is_some();
    Channel {
        id: named_id.clone().unwrap_or_else(|| canonical_parent_url(&parent_url)),
        name: display_name.or(named_id).unwrap_or_else(|| short_parent_url_label(&parent_url)),
        description: non_empty(raw.description.as_deref()),
        image_url: non_empty(raw.image_url.as_deref()),
        member_count: raw.member_count.filter(|_| neynar),
        follower_count: raw.follower_count,
        casters: raw.member_count.filter(|_| !neynar),
        created_at: unix_seconds(raw.created_at.as_ref()),
        parent_url,
        ..Channel::default()
    }
}

/// The URL, trimmed, if it has one of `schemes`. Links take http(s); images
/// only https, the one scheme the Content Security Policy allows for them.
fn url_with_scheme(value: Option<&str>, schemes: &[&str]) -> Option<String> {
    let url = non_empty(value)?;
    let (scheme, _) = url.split_once("://")?;
    schemes.iter().any(|s| s.eq_ignore_ascii_case(scheme)).then_some(url)
}

/// Fill `channel` in from the Farcaster app's record of it. `users` are the
/// accounts the record names ([`RawAppChannel::fids`]), as far as they could
/// be read: a lead or moderator who couldn't be read is left out, and a
/// mention is spliced in as "@fid:N". A record for another channel changes
/// nothing.
pub fn enrich_channel(channel: &mut Channel, app: &RawAppChannel, users: &[RawUser]) {
    if !app.id.eq_ignore_ascii_case(&channel.id) {
        return;
    }
    let user = |fid: u64| users.iter().find(|u| u.fid == fid);
    if let Some(name) = non_empty(app.name.as_deref()) {
        channel.name = name;
    }
    let mentioned = app.description_mentions.as_deref().unwrap_or_default();
    let positions = app.description_mentions_positions.as_deref().unwrap_or_default();
    let mentions: Vec<Mention> = if mentioned.len() == positions.len() {
        mentioned
            .iter()
            .map(|&fid| Mention { username: user(fid).map_or_else(|| format!("fid:{fid}"), username_for), fid })
            .collect()
    } else {
        Vec::new()
    };
    let ranges: Vec<(u32, u32)> = positions.iter().map(|&at| (at, at)).collect();
    // Splice before trimming: the positions count from the very start.
    let description = splice_mentions(app.description.as_deref().unwrap_or_default(), &mentions, &ranges);
    if let Some(description) = non_empty(Some(description.as_str())) {
        channel.description = Some(description);
        channel.description_mentions = mentions;
    }
    if let Some(url) = url_with_scheme(app.image_url.as_deref(), &["https"]) {
        channel.image_url = Some(url);
    }
    channel.header_image_url = url_with_scheme(app.header_image_url.as_deref(), &["https"]);
    channel.follower_count = app.follower_count;
    channel.member_count = app.member_count;
    if let Some(created) = unix_seconds(app.created_at.as_ref()) {
        channel.created_at = Some(created);
    }
    channel.lead = app.lead_fid.and_then(user).map(user_summary);
    let mut moderators: Vec<UserSummary> = Vec::new();
    for fid in app.moderator_fids.iter().flatten().filter(|fid| Some(**fid) != app.lead_fid) {
        if let Some(moderator) = user(*fid).filter(|_| moderators.iter().all(|m| m.fid != *fid)) {
            moderators.push(user_summary(moderator));
        }
    }
    channel.moderators = moderators;
    channel.external_link = app.external_link.as_ref().and_then(|link| {
        Some(ExternalLink {
            url: url_with_scheme(link.url.as_deref(), &["https", "http"])?,
            title: link.title.as_deref().map(str::trim).unwrap_or_default().to_owned(),
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChannelCount;

    #[test]
    fn a_feed_page_with_no_casts_but_a_cursor_is_empty_with_more() {
        let feed = |json: &str| serde_json::from_str::<RawFeed>(json).unwrap();
        assert!(feed(r#"{"casts":[],"next":{"cursor":"5b5b33312c3130342c313136"}}"#).is_empty_with_more());
        assert!(feed(r#"{"next":{"cursor":"5b5b"}}"#).is_empty_with_more());
        // The end of a feed, or a channel with no casts.
        assert!(!feed(r#"{"casts":[],"next":{"cursor":""}}"#).is_empty_with_more());
        assert!(!feed(r#"{"casts":[],"next":{"cursor":null}}"#).is_empty_with_more());
        assert!(!feed(r#"{"casts":[],"next":null}"#).is_empty_with_more());
        assert!(!feed(r#"{"casts":[]}"#).is_empty_with_more());
        // A page with casts.
        assert!(
            !feed(r#"{"casts":[{"hash":"0x01","text":"gm","author":{"fid":2}}],"next":{"cursor":"5b5b"}}"#)
                .is_empty_with_more()
        );
        assert_eq!(feed(r#"{"casts":[],"next":{"cursor":"5b5b"}}"#).cursor(), Some("5b5b"));
        assert_eq!(feed(r#"{"casts":[],"next":{"cursor":""}}"#).cursor(), None);
    }

    #[test]
    fn the_first_username_is_the_earliest_given_to_the_account() {
        let raw: RawTransfers = serde_json::from_str(
            r#"{"transfers":[
                {"id":9,"timestamp":1718064000,"username":"300000","owner":"0x1","from":0,"to":300000},
                {"id":8,"timestamp":1718063000,"username":"gemagentana","owner":"0x1","from":300000,"to":0},
                {"id":7,"timestamp":1707177600,"username":"gemagentana","owner":"0x1","from":0,"to":300000}
            ]}"#,
        )
        .unwrap();
        assert_eq!(first_username_at(&raw, 300_000), Some(1_707_177_600));
        assert_eq!(first_username_at(&raw, 3), None);
        assert_eq!(first_username_at(&RawTransfers::default(), 3), None);
    }

    #[test]
    fn the_website_is_the_user_datas_url() {
        // dwr's user data, as Hypersnap's /v1/userDataByFid sends it.
        let raw: RawUserData = serde_json::from_str(&fixture("hypersnap-user-data.json")).unwrap();
        let dwr = Website { href: "https://danromero.org".into(), label: "danromero.org".into() };
        assert_eq!(website(&raw), Some(dwr));
        let one = |kind: &str, value: &str| {
            serde_json::from_str::<RawUserData>(&format!(
                r#"{{"messages":[{{"data":{{"type":"MESSAGE_TYPE_USER_DATA_ADD","fid":2,"timestamp":1,
                    "userDataBody":{{"type":{kind},"value":"{value}"}}}}}}],"nextPageToken":""}}"#
            ))
            .unwrap()
        };
        // Cleared (as fid 2's is), never set (fid 99 has no URL message), or
        // not a web address.
        assert_eq!(website(&one(r#""USER_DATA_TYPE_URL""#, "")), None);
        assert_eq!(website(&one(r#""USER_DATA_TYPE_LOCATION""#, "geo:34.05,-118.24")), None);
        assert_eq!(website(&RawUserData::default()), None);
        assert_eq!(website(&one(r#""USER_DATA_TYPE_URL""#, "javascript:alert(1)")), None);
        // Another type's value is never taken for it.
        assert_eq!(website(&one(r#""USER_DATA_TYPE_BIO""#, "https://example.com")), None);
        // A node that writes the type as the protobuf's number.
        assert_eq!(website(&one("5", "senpi.ai")).map(|w| w.href).as_deref(), Some("https://senpi.ai"));
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn flags_spam_casts_and_listings_leave_them_out() {
        let parse = |json: &str| cast(&serde_json::from_str::<RawCast>(json).unwrap());
        let vercel = parse(r#"{"hash":"a1","text":"gm","embeds":[{"url":"https://gig-bot-2-0.vercel.app/?degen=1"}]}"#);
        let degen = parse(r#"{"hash":"a2","text":"Just unlocked my DEGEN rewards 🎩 https://example.com/x?degen=1"}"#);
        let mention = parse(
            r#"{"hash":"a3","text":"thanks  for the gig","mentioned_profiles":[{"fid":9,"username":"gig-bot"}],"mentioned_profiles_ranges":[{"start":7,"end":8}]}"#,
        );
        let fine =
            parse(r#"{"hash":"a4","text":"I tipped 100 $DEGEN today","embeds":[{"url":"https://paragraph.xyz/p"}]}"#);
        assert!(vercel.spam && degen.spam && mention.spam);
        assert!(!fine.spam && !fine.hidden());
        let kept = crate::thread::without_hidden(vec![vercel, degen, mention, fine]);
        assert_eq!(kept.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), ["0xa4"]);
    }

    #[test]
    fn flags_mini_app_casts() {
        let parse = |json: &str| cast(&serde_json::from_str::<RawCast>(json).unwrap());
        assert!(parse(r#"{"hash":"ab","text":"play","embeds":[{"url":"https://farcaster.xyz/miniapps/Z6gEN6rYltaz/swarmhood"}]}"#).mini_app);
        assert!(parse(r#"{"hash":"ab","text":"try https://warpcast.com/~/frames/x now"}"#).mini_app);
        assert!(
            parse(r#"{"hash":"ab","embeds":[{"url":"https://game.example","metadata":{"html":{"fcFrame":"vNext"}}}]}"#)
                .mini_app
        );
        assert!(
            parse(r#"{"hash":"ab","embeds":[{"url":"https://game.example","metadata":{"frame":{"version":"next"}}}]}"#)
                .mini_app
        );
        assert!(
            !parse(r#"{"hash":"ab","text":"see https://example.com","embeds":[{"url":"https://i.imgur.com/a.png"}]}"#)
                .mini_app
        );
    }

    #[test]
    fn normalizes_a_hypersnap_user() {
        let raw: RawUserList = serde_json::from_str(&fixture("hypersnap-user-bulk.json")).unwrap();
        let dwr = user(&raw.users[0]);
        assert_eq!(dwr.fid, 3);
        assert_eq!(dwr.username, "dwr");
        assert_eq!(dwr.display_name, "Dan Romero");
        assert_eq!(dwr.registered_at, Some(parse_rfc3339("2023-11-07T19:42:51.000Z").unwrap()));
        assert_eq!(dwr.eth_addresses.len(), 2);
        assert_eq!(dwr.sol_addresses.len(), 1);
        assert_eq!(dwr.primary_eth.as_deref(), Some("0x187c7b0393ebe86378128f2653d0930e33218899"));
        assert!(dwr.follower_count > 1_000_000);
        assert_eq!(user(&raw.users[1]).username, "balajis.eth");
    }

    #[test]
    fn reads_profile_cards_from_cast_authors() {
        let raw: RawFeed = serde_json::from_str(&fixture("hypersnap-feed-trending.json")).unwrap();
        let author = raw.casts[0].author.as_ref().unwrap();
        assert_eq!(
            user_card(author),
            Some(UserCard {
                bio: "The swarm that wears the hood".into(),
                followers: 449,
                following: 0,
                registered_at: parse_rfc3339("2026-09-30T09:02:01.000Z"),
                first_username_at: None,
            })
        );
        // An author object without any of the facts gives no card, rather
        // than a card claiming zero followers.
        let bare: RawUser = serde_json::from_str(r#"{"fid":1,"username":"x"}"#).unwrap();
        assert_eq!(user_card(&bare), None);
        let bio_only: RawUser = serde_json::from_str(r#"{"fid":1,"profile":{"bio":{"text":"  gm\n gm "}}}"#).unwrap();
        assert_eq!(user_card(&bio_only).map(|c| c.bio), Some("gm gm".into()));

        // One card per author, each bio cut to a card's length.
        let cards = author_cards(&raw.casts);
        assert_eq!(cards.keys().copied().collect::<Vec<_>>(), [3, 37, 1725, 4167, 3_353_201]);
        assert!(cards.values().all(|c| c.bio.chars().count() <= CARD_BIO_CHARS + 1));
        assert_eq!((cards[&3].followers, cards[&3].following), (1_221_547, 77));
        // dwr reports the ID registry's migration as his registration.
        assert_eq!(cards[&3].joined(), Some(crate::model::Joined::BeforeMigration));
        assert_eq!(user_cards([author, &bare]).keys().copied().collect::<Vec<_>>(), [3_353_201]);
    }

    #[test]
    fn reads_profile_cards_through_a_conversation() {
        let value: Value = serde_json::from_str(&fixture("hypersnap-conversation.json")).unwrap();
        let root: RawCast = serde_json::from_value(value["conversation"]["cast"].clone()).unwrap();
        let tree = thread(&root);
        let fids: std::collections::BTreeSet<u64> =
            crate::thread::flatten(&tree).iter().map(|c| c.author.fid).collect();
        let cards = author_cards([&root]);
        assert_eq!(cards.keys().copied().collect::<std::collections::BTreeSet<_>>(), fids);
        let balajis = &cards[&37];
        assert!(balajis.bio.starts_with("Author of the Network State. Founder"));
        assert!(balajis.followers > 500_000);
        // A reply two levels down.
        assert_eq!(cards[&3_318_053].bio, "Universal Caster Income Bot");
    }

    #[test]
    fn blank_usernames_fall_back_to_the_fid() {
        let raw: RawUser = serde_json::from_str(r#"{"fid":1,"username":"  "}"#).unwrap();
        assert_eq!(user_summary(&raw).username, "fid:1");
        let reaction =
            RawReaction { reaction_timestamp: None, user: Some(RawUserSummary { fid: 2, ..Default::default() }) };
        assert_eq!(reactors(&[reaction], 10).0[0].username, "fid:2");
    }

    #[test]
    fn lists_reactors_newest_first_across_shards() {
        // Recorded from Hypersnap: two shards' likes, each oldest first.
        let raw: RawReactionList = serde_json::from_str(&fixture("hypersnap-reactions-likes.json")).unwrap();
        assert_eq!(raw.reactions.len(), 13);
        let (newest, more) = reactors(&raw.reactions, 5);
        let names: Vec<&str> = newest.iter().map(|u| u.username.as_str()).collect();
        // Reversing the list would put the second shard's 08:10 and 07:42
        // ahead of the first shard's 08:18.
        assert_eq!(names, ["fid:1051393", "chiilla", "manaminnn", "meromeron", "shakil4"]);
        assert!(more);
        assert_eq!(newest[1].display_name, "chii_chiilla");
        assert!(newest[1].pfp_url.as_deref().is_some_and(|url| url.starts_with("https://")));

        let (all, more) = reactors(&raw.reactions, 100);
        assert_eq!((all.len(), more), (13, false));
        assert_eq!(all.last().map(|u| u.username.as_str()), Some("shiory"));
    }

    #[test]
    fn a_reactions_list_carries_the_cards_of_the_people_it_names() {
        let raw: RawReactionList = serde_json::from_str(&fixture("hypersnap-reactions-likes.json")).unwrap();
        let (newest, _) = reactors(&raw.reactions, 5);
        let cards = reactor_cards(&raw.reactions, &newest);
        // The five listed, not the others, each with what its user object says.
        let listed: Vec<u64> = newest.iter().map(|u| u.fid).collect();
        assert_eq!(cards.len(), 5);
        assert!(cards.keys().all(|fid| listed.contains(fid)));
        let chiilla = newest.iter().find(|u| u.username == "chiilla").unwrap();
        assert!(cards[&chiilla.fid].followers > 0 && cards[&chiilla.fid].registered_at.is_some());

        let with_bio: RawReactionList = serde_json::from_str(
            r#"{"reactions":[{"reaction_timestamp":"2026-10-17T10:00:00.000Z","user":{"fid":7,"username":"seven",
                "profile":{"bio":{"text":"Reads conversations."}},"follower_count":12,"following_count":3,
                "registered_at":"2024-02-05T12:56:57.000Z","verified_addresses":{"eth_addresses":["0x01"]}}}]}"#,
        )
        .unwrap();
        let (listed, _) = reactors(&with_bio.reactions, 10);
        let card = &reactor_cards(&with_bio.reactions, &listed)[&7];
        assert_eq!((card.bio.as_str(), card.followers, card.following), ("Reads conversations.", 12, 3));
        assert_eq!(card.registered_at, Some(1_707_137_817));
    }

    #[test]
    fn reactions_without_a_user_are_left_out() {
        let raw: RawReactionList = serde_json::from_str(
            r#"{"reactions":[
                {"reaction_timestamp":"2026-10-17T10:00:00.000Z","user":{"fid":1,"username":"a"}},
                {"reaction_timestamp":"2026-10-17T11:00:00.000Z"},
                {"reaction_timestamp":"2026-10-17T10:00:00.000Z","user":{"fid":2,"username":"b"}}
            ]}"#,
        )
        .unwrap();
        let (users, more) = reactors(&raw.reactions, 1);
        // Same second: the later-listed one is the newer.
        assert_eq!(users.iter().map(|u| u.fid).collect::<Vec<_>>(), [2]);
        assert!(more);
        let (both, more) = reactors(&raw.reactions, 2);
        assert_eq!(both.iter().map(|u| u.username.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert!(!more);
        assert_eq!(reactors(&[], 10), (vec![], false));
    }

    #[test]
    fn normalizes_a_trending_feed() {
        let raw: RawFeed = serde_json::from_str(&fixture("hypersnap-feed-trending.json")).unwrap();
        assert!(!raw.casts.is_empty());
        for c in raw.casts.iter().map(cast) {
            assert!(c.hash.starts_with("0x") && c.hash.len() == 42, "{}", c.hash);
            assert!(c.ts > 1_600_000_000);
            assert!(c.author.fid > 0);
        }
    }

    #[test]
    fn normalizes_a_conversation_tree() {
        let value: Value = serde_json::from_str(&fixture("hypersnap-conversation.json")).unwrap();
        let root: RawCast = serde_json::from_value(value["conversation"]["cast"].clone()).unwrap();
        let tree = thread(&root);
        assert!(!tree.children.is_empty());
        assert!(tree.children.iter().all(|child| child.cast.parent_hash.as_deref() == Some(tree.cast.hash.as_str())));
    }

    #[test]
    fn normalizes_embeds() {
        let raw: RawEmbed =
            serde_json::from_str(r#"{"cast_id":{"fid":5283,"hash":"C55E0F3BC79FE65E54A02835DBC7896D8BD593AD"}}"#)
                .unwrap();
        assert_eq!(
            embed(&raw),
            Some(Embed::Cast { hash: "0xc55e0f3bc79fe65e54a02835dbc7896d8bd593ad".into(), fid: Some(5283) })
        );
        let raw: RawEmbed = serde_json::from_str(r#"{"url":"https://imagedelivery.net/a/b/original"}"#).unwrap();
        assert!(matches!(embed(&raw), Some(Embed::Image { .. })));
        // A reader's files, as Allthing keeps them: a picture, and a video
        // the browser plays as it is.
        let kept = |ext: &str| {
            let raw: RawEmbed = serde_json::from_str(&format!(
                r#"{{"url":"https://allthing.social/m/1234/0123456789abcdef0123456789abcdef.{ext}"}}"#
            ))
            .unwrap();
            embed(&raw)
        };
        assert!(matches!(kept("png"), Some(Embed::Image { .. })));
        assert!(matches!(kept("mp4"), Some(Embed::Video { url }) if !crate::embeds::is_hls(&url)));
        assert!(matches!(kept("mov"), Some(Embed::Video { .. })));
        let raw: RawEmbed = serde_json::from_str(r#"{"url":"https://example.com","metadata":{"html":{"ogTitle":"Hi","ogImage":[{"url":"https://example.com/og.png"}]}}}"#).unwrap();
        assert_eq!(
            embed(&raw),
            Some(Embed::Link {
                url: "https://example.com".into(),
                image_url: Some("https://example.com/og.png".into()),
                title: Some("Hi".into())
            })
        );
        assert_eq!(embed(&RawEmbed::default()), None);
    }

    #[test]
    fn splices_hypersnap_mentions() {
        let raw: RawCast = serde_json::from_str(
            r#"{"hash":"ab","text":"gm  and ","timestamp":"2026-10-17T00:00:00Z",
                "mentioned_profiles":[{"fid":3,"username":"dwr"},{"fid":37,"username":"balajis.eth"}],
                "mentioned_profiles_ranges":[{"start":3,"end":3},{"start":8,"end":8}]}"#,
        )
        .unwrap();
        let c = cast(&raw);
        assert_eq!(c.text, "gm @dwr and @balajis.eth");
        assert_eq!(c.hash, "0xab");
        assert!(!c.mini_app);
        assert_eq!(c.mentions.len(), 2);
    }

    #[test]
    fn normalizes_channels() {
        let raw: Value = serde_json::from_str(&fixture("hypersnap-channel.json")).unwrap();
        let ch = channel(&serde_json::from_value::<RawChannel>(raw["channel"].clone()).unwrap());
        assert!(!ch.id.is_empty());
        assert!(!ch.parent_url.is_empty());
        // Hypersnap's `member_count` counts casters, so it is never a member count.
        assert_eq!((ch.member_count, ch.follower_count, ch.casters), (None, None, Some(2023)));
        assert_eq!(ch.counts(), [ChannelCount::Casters(2023)]);
        let legacy: Value = serde_json::from_str(&fixture("hypersnap-channel-legacy.json")).unwrap();
        let ch = channel(&serde_json::from_value::<RawChannel>(legacy["channel"].clone()).unwrap());
        assert_eq!((ch.id.as_str(), ch.casters, ch.member_count), ("farcaster", Some(94_568), None));
        let neynar: RawChannel = serde_json::from_str(
            r#"{"id":"Dev","name":"Developers","url":"https://warpcast.com/~/channel/dev","created_at":1700000000,"follower_count":12}"#,
        )
        .unwrap();
        let ch = channel(&neynar);
        assert_eq!(
            (ch.id.as_str(), ch.name.as_str(), ch.follower_count, ch.member_count, ch.casters, ch.created_at),
            ("dev", "Developers", Some(12), None, None, Some(1_700_000_000))
        );
        // Neynar's member count, which comes with followers, counts members.
        let neynar: RawChannel = serde_json::from_str(r#"{"id":"dev","follower_count":12,"member_count":3}"#).unwrap();
        assert_eq!(channel(&neynar).counts(), [ChannelCount::Followers(12), ChannelCount::Members(3)]);
        let list: RawChannelList = serde_json::from_str(&fixture("hypersnap-channel-trending.json")).unwrap();
        assert!(!list.channels.is_empty());
    }

    fn app_channel(id: &str) -> RawAppChannel {
        let raw: RawAppChannelResponse =
            serde_json::from_str(&fixture(&format!("farcaster-channel-{id}.json"))).unwrap();
        raw.result.and_then(|r| r.channel).unwrap()
    }

    fn users(json: &str) -> Vec<RawUser> {
        serde_json::from_str::<RawUserList>(json).unwrap().users
    }

    #[test]
    fn reads_the_farcaster_apps_channel_records() {
        let dev = app_channel("dev");
        assert_eq!(dev.fids(), [191, 2172, 834, 725, 6841, 7657, 864_314]);
        assert_eq!(dev.pinned_hash(), None);
        let farcaster = app_channel("farcaster");
        assert_eq!(farcaster.fids(), [1]);
        assert_eq!(farcaster.pinned_hash().as_deref(), Some("0x50184b62b4c52fd76628713562e1eda3dccaf63d"));
        // The lead first, each account once, and no malformed hashes.
        let odd: RawAppChannel = serde_json::from_str(
            r#"{"id":"x","leadFid":2,"moderatorFids":[1,2,2,0],"descriptionMentions":[3,1],"pinnedCastHash":"0x12"}"#,
        )
        .unwrap();
        assert_eq!((odd.fids(), odd.pinned_hash()), (vec![2, 1, 3], None));
    }

    #[test]
    fn fills_a_hypersnap_channel_in_from_the_app() {
        // Hypersnap's /dev: its legacy feed, and 15,664 distinct casters.
        let raw: RawChannel = serde_json::from_str(
            r#"{"id":"chain://eip155:1/erc721:0x7dd4e31f1530ac682c8ea4d8016e95773e08d8b0",
                "parent_url":"chain://eip155:1/erc721:0x7dd4e31f1530ac682c8ea4d8016e95773e08d8b0",
                "created_at":"2026-10-15T12:07:42.000Z","member_count":15664}"#,
        )
        .unwrap();
        let mut dev = channel(&raw);
        assert_eq!((dev.id.as_str(), dev.member_count, dev.casters), ("dev", None, Some(15_664)));
        // Three of the seven accounts could be read.
        let read = users(
            r#"{"users":[{"fid":834,"username":"gabrielayuso.eth","display_name":"Gabriel Ayuso"},
                {"fid":191,"username":"manan","display_name":"Manan","pfp_url":"https://example.com/m.png"},
                {"fid":2172,"username":"quest"}]}"#,
        );
        enrich_channel(&mut dev, &app_channel("dev"), &read);
        assert_eq!(dev.name, "dev");
        assert!(!dev.has_display_name());
        assert_eq!(dev.description.as_deref(), Some("Cultivating curiosity for software developers"));
        assert_eq!((dev.member_count, dev.follower_count, dev.casters), (Some(475), Some(183_421), Some(15_664)));
        assert_eq!(dev.counts(), [ChannelCount::Followers(183_421), ChannelCount::Members(475)]);
        assert_eq!(
            dev.lead.as_ref().map(|u| (u.name(), u.pfp_url.as_deref())),
            Some(("Manan", Some("https://example.com/m.png")))
        );
        // In the channel's order, without the lead, and without those not read.
        assert_eq!(
            dev.moderators.iter().map(|m| m.username.as_str()).collect::<Vec<_>>(),
            ["quest", "gabrielayuso.eth"]
        );
        assert!(dev.image_url.as_deref().is_some_and(|u| u.starts_with("https://ipfs.")));
        // No banner, and an empty link is no link.
        assert_eq!((dev.header_image_url.as_deref(), dev.external_link.as_ref()), (None, None));
        assert_eq!(dev.created_at, Some(1_689_121_073));
        assert_eq!(dev.parent_url, "chain://eip155:1/erc721:0x7dd4e31f1530ac682c8ea4d8016e95773e08d8b0");
    }

    #[test]
    fn keeps_the_banner_and_names_without_the_accounts() {
        let mut farcaster = Channel { id: "farcaster".into(), name: "farcaster".into(), ..Channel::default() };
        // `user/bulk` failed: the lead is left out rather than guessed.
        enrich_channel(&mut farcaster, &app_channel("farcaster"), &[]);
        assert_eq!(farcaster.name, "Farcaster");
        assert!(farcaster.has_display_name());
        assert!(farcaster.header_image_url.as_deref().is_some_and(|u| u.starts_with("https://imagedelivery.net/")));
        assert_eq!(farcaster.counts(), [ChannelCount::Followers(445_632), ChannelCount::Members(1)]);
        assert_eq!((farcaster.lead.as_ref(), farcaster.moderators.len()), (None, 0));
    }

    #[test]
    fn splices_mentions_into_descriptions() {
        let app = app_channel("books");
        let mut books = Channel { id: "books".into(), ..Channel::default() };
        enrich_channel(&mut books, &app, &users(r#"{"users":[{"fid":129,"username":"phil"}]}"#));
        assert!(books.description.as_deref().unwrap().contains(" grow. DM @phil for an invite "));
        assert_eq!(books.description_mentions, [Mention { username: "phil".into(), fid: 129 }]);
        assert_eq!(books.lead.as_ref().map(|u| u.username.as_str()), Some("phil"));
        // Unread, a mention still reads as one, and the text stays whole.
        let mut unread = Channel { id: "books".into(), ..Channel::default() };
        enrich_channel(&mut unread, &app, &[]);
        assert!(unread.description.as_deref().unwrap().contains(" DM @fid:129 for an invite "));
    }

    #[test]
    fn keeps_only_web_links_and_https_images() {
        let mut ch = Channel { id: "x".into(), name: "x".into(), ..Channel::default() };
        let unsafe_urls: RawAppChannel = serde_json::from_str(
            r#"{"id":"x","imageUrl":"http://example.com/p.png","headerImageUrl":"javascript:alert(1)",
                "externalLink":{"title":"Join","url":"javascript:alert(1)"}}"#,
        )
        .unwrap();
        enrich_channel(&mut ch, &unsafe_urls, &[]);
        assert_eq!(
            (ch.image_url.as_ref(), ch.header_image_url.as_ref(), ch.external_link.as_ref()),
            (None, None, None)
        );
        let link: RawAppChannel =
            serde_json::from_str(r#"{"id":"x","externalLink":{"title":" Join ","url":" http://example.com/join "}}"#)
                .unwrap();
        enrich_channel(&mut ch, &link, &[]);
        assert_eq!(
            ch.external_link,
            Some(ExternalLink { url: "http://example.com/join".into(), title: "Join".into() })
        );
        // Another channel's record changes nothing.
        let other: RawAppChannel = serde_json::from_str(r#"{"id":"y","name":"Y","memberCount":5}"#).unwrap();
        enrich_channel(&mut ch, &other, &[]);
        assert_eq!((ch.name.as_str(), ch.member_count), ("x", None));
    }

    #[test]
    fn reads_both_search_shapes() {
        let hypersnap: RawUserSearch = serde_json::from_str(&fixture("hypersnap-user-search.json")).unwrap();
        assert!(hypersnap.into_users().iter().any(|u| u.username.as_deref() == Some("balajis.eth")));
        let neynar: RawUserSearch =
            serde_json::from_str(r#"{"result":{"users":[{"fid":37,"username":"balajis.eth"}]}}"#).unwrap();
        assert_eq!(neynar.into_users()[0].fid, 37);
    }
}
