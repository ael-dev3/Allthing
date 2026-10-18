// SPDX-License-Identifier: AGPL-3.0-or-later
//! Raw shapes of the Neynar v2 Farcaster API, as served by Hypersnap (fewer
//! fields) and Neynar, and their normalization into [`crate::model`].
//!
//! Only the fields Allthing reads are declared; serde skips the rest, which
//! keeps parsing cheap inside the Worker's CPU budget.

use serde::Deserialize;
use serde_json::Value;

use crate::channels::{
    canonical_parent_url, channel_id_for_url, channel_ref_from_url, channel_url_for_id, short_parent_url_label,
};
use crate::embeds::{MediaKind, classify_url};
use crate::model::{Cast, Channel, ChannelRef, Embed, Mention, ThreadNode, User, UserSummary};
use crate::params::normalize_hash;
use crate::text::splice_mentions;
use crate::time::parse_rfc3339;

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
    pub member_count: Option<u64>,
    pub follower_count: Option<u64>,
    /// Unix seconds from Neynar, an ISO string from Hypersnap.
    pub created_at: Option<Value>,
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

/// Only the fields a [`UserSummary`] shows. Reaction lists hold up to 100
/// full user objects (about 1.2 KB each, with bios and addresses); reading
/// just these halves their parse time.
#[derive(Debug, Default, Deserialize)]
pub struct RawUserSummary {
    pub fid: u64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub pfp_url: Option<String>,
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
/// after another, so they are put in time order here rather than reversed.
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
        custody_address: non_empty(raw.custody_address.as_deref()).map(|a| a.to_ascii_lowercase()),
        eth_addresses: verified
            .map(|v| v.eth_addresses.iter().map(|a| a.to_ascii_lowercase()).collect())
            .unwrap_or_default(),
        sol_addresses: verified.map(|v| v.sol_addresses.clone()).unwrap_or_default(),
        primary_eth: primary.and_then(|p| non_empty(p.eth_address.as_deref())).map(|a| a.to_ascii_lowercase()),
        primary_sol: primary.and_then(|p| non_empty(p.sol_address.as_deref())),
    }
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
    Channel {
        id: named_id.clone().unwrap_or_else(|| canonical_parent_url(&parent_url)),
        name: display_name.or(named_id).unwrap_or_else(|| short_parent_url_label(&parent_url)),
        description: non_empty(raw.description.as_deref()),
        image_url: non_empty(raw.image_url.as_deref()),
        member_count: raw.member_count.or(raw.follower_count),
        created_at: unix_seconds(raw.created_at.as_ref()),
        parent_url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let legacy: Value = serde_json::from_str(&fixture("hypersnap-channel-legacy.json")).unwrap();
        let ch = channel(&serde_json::from_value::<RawChannel>(legacy["channel"].clone()).unwrap());
        assert_eq!(ch.id, "farcaster");
        let neynar: RawChannel = serde_json::from_str(
            r#"{"id":"Dev","name":"Developers","url":"https://warpcast.com/~/channel/dev","created_at":1700000000,"follower_count":12}"#,
        )
        .unwrap();
        let ch = channel(&neynar);
        assert_eq!(
            (ch.id.as_str(), ch.name.as_str(), ch.member_count, ch.created_at),
            ("dev", "Developers", Some(12), Some(1_700_000_000))
        );
        let list: RawChannelList = serde_json::from_str(&fixture("hypersnap-channel-trending.json")).unwrap();
        assert!(!list.channels.is_empty());
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
