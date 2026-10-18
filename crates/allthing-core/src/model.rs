// SPDX-License-Identifier: AGPL-3.0-or-later
//! The normalized, provider-independent Farcaster model. The Worker builds
//! these from upstream JSON; the browser app only ever sees these shapes.

use serde::{Deserialize, Serialize};

use crate::time::{duration_since, format_date};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSummary {
    pub fid: u64,
    /// fname or ENS-style name ("dwr", "balajis.eth"); `fid:<n>` when the
    /// account has neither.
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pfp_url: Option<String>,
}

impl UserSummary {
    /// The display name, falling back to the username.
    pub fn name(&self) -> &str {
        if self.display_name.is_empty() { &self.username } else { &self.display_name }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub fid: u64,
    pub username: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub pfp_url: Option<String>,
    #[serde(default)]
    pub bio: String,
    #[serde(default)]
    pub follower_count: u64,
    #[serde(default)]
    pub following_count: u64,
    /// Unix seconds of the onchain ID registration the provider reports.
    #[serde(default)]
    pub registered_at: Option<i64>,
    /// Unix seconds at which the account got its first username (fname),
    /// read only when `registered_at` is the ID registry's migration day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_username_at: Option<i64>,
    #[serde(default)]
    pub custody_address: Option<String>,
    #[serde(default)]
    pub eth_addresses: Vec<String>,
    #[serde(default)]
    pub sol_addresses: Vec<String>,
    #[serde(default)]
    pub primary_eth: Option<String>,
    #[serde(default)]
    pub primary_sol: Option<String>,
}

impl User {
    pub fn summary(&self) -> UserSummary {
        UserSummary {
            fid: self.fid,
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            pfp_url: self.pfp_url.clone(),
        }
    }

    pub fn name(&self) -> &str {
        if self.display_name.is_empty() { &self.username } else { &self.display_name }
    }

    /// Whether the reported registration is the ID registry's move to OP
    /// Mainnet, which every account registered before it reports.
    pub fn registered_before_migration(&self) -> bool {
        self.registered_at.is_some_and(|ts| ID_REGISTRY_MIGRATION_DAY.contains(&ts))
    }

    /// When the account joined Farcaster, as well as it is known.
    pub fn joined(&self) -> Option<Joined> {
        Joined::from_dates(self.registered_at, self.first_username_at)
    }
}

/// The day (UTC) the ID registry moved to OP Mainnet, 7 Nov 2023. Accounts
/// registered before it report that day as their registration: it is the
/// earliest date onchain data has for them.
pub const ID_REGISTRY_MIGRATION_DAY: std::ops::Range<i64> = 1_699_315_200..1_699_401_600;

/// When an account joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Joined {
    /// Its onchain ID registration.
    Registered(i64),
    /// Registered before the ID registry's migration: the date of its first
    /// username, which the old sign-up registered along with the ID.
    FirstUsername(i64),
    /// Registered before the migration, with no earlier date known.
    BeforeMigration,
}

impl Joined {
    /// When an account joined, from the onchain registration its provider
    /// reports and, for one registered before the ID registry's migration,
    /// when it got its first username, if that is known.
    pub fn from_dates(registered_at: Option<i64>, first_username_at: Option<i64>) -> Option<Joined> {
        let registered = registered_at?;
        if !ID_REGISTRY_MIGRATION_DAY.contains(&registered) {
            return Some(Joined::Registered(registered));
        }
        Some(match first_username_at {
            Some(ts) if ts < ID_REGISTRY_MIGRATION_DAY.start => Joined::FirstUsername(ts),
            _ => Joined::BeforeMigration,
        })
    }

    /// The date profiles show: "17 Jun 2021", or "before Nov 2023".
    pub fn date(self) -> String {
        match self {
            Joined::Registered(ts) | Joined::FirstUsername(ts) => format_date(ts),
            Joined::BeforeMigration => "before Nov 2023".to_owned(),
        }
    }

    /// Where the date comes from, for its tooltip (`now` in Unix seconds).
    pub fn note(self, now: i64) -> String {
        const MIGRATION: &str =
            "the ID registry moved to OP Mainnet on 7 Nov 2023, which is the earliest date onchain data has for it";
        match self {
            Joined::Registered(ts) => format!("Registered onchain {} ago.", duration_since(ts, now)),
            Joined::FirstUsername(ts) => format!(
                "Got its first username {} ago. The account was registered before {MIGRATION}.",
                duration_since(ts, now)
            ),
            Joined::BeforeMigration => format!("Registered before {MIGRATION}."),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Embed {
    Image {
        url: String,
    },
    Video {
        url: String,
    },
    Link {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// A quoted cast.
    Cast {
        hash: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fid: Option<u64>,
    },
}

impl Embed {
    pub fn url(&self) -> Option<&str> {
        match self {
            Embed::Image { url } | Embed::Video { url } | Embed::Link { url, .. } => Some(url),
            Embed::Cast { .. } => None,
        }
    }

    pub fn is_media(&self) -> bool {
        matches!(self, Embed::Image { .. } | Embed::Video { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mention {
    pub username: String,
    pub fid: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRef {
    /// Route key: a channel id ("farcaster") or a canonical parent URL.
    pub id: String,
    /// Short display label ("farcaster", "erc20:0x4ed4…efed").
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cast {
    /// 0x-prefixed, lowercase.
    pub hash: String,
    pub author: UserSummary,
    /// Text with mentions spliced in as "@username".
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<Mention>,
    /// Unix seconds.
    pub ts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<ChannelRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub recasts: u64,
    #[serde(default)]
    pub replies: u64,
    /// The cast embeds a mini app or frame. Allthing does not run them, so
    /// such casts are left out of listings, and kept in threads only as an
    /// emptied node when they have replies.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mini_app: bool,
    /// The cast is spam by Allthing's content rules (see [`crate::spam`]),
    /// left out like a mini app cast.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spam: bool,
}

impl Cast {
    /// Left out of listings: a mini app or spam.
    pub fn hidden(&self) -> bool {
        self.mini_app || self.spam
    }

    /// Drop a hidden cast's content, keeping its place in a thread.
    pub fn strip_hidden(&mut self) {
        self.text.clear();
        self.mentions.clear();
        self.embeds.clear();
    }

    pub fn is_reply(&self) -> bool {
        self.parent_hash.is_some()
    }

    /// The URLs of its link embeds (not media, not quoted casts).
    pub fn link_urls(&self) -> impl Iterator<Item = &str> {
        self.embeds.iter().filter_map(|embed| match embed {
            Embed::Link { url, .. } => Some(url.as_str()),
            _ => None,
        })
    }

    /// Images and videos, in the order they were attached.
    pub fn media(&self) -> impl Iterator<Item = &Embed> {
        self.embeds.iter().filter(|embed| embed.is_media())
    }

    /// The hashes of the casts it quotes, in the order they were attached.
    pub fn quoted(&self) -> impl Iterator<Item = &str> {
        self.embeds.iter().filter_map(|embed| match embed {
            Embed::Cast { hash, .. } => Some(hash.as_str()),
            _ => None,
        })
    }
}

/// A link a channel's hosts put on it, such as "Join" or "Subscribe".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalLink {
    /// An http(s) URL.
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
}

/// A channel, or any parent-URL feed. Hypersnap supplies the id, name, feed
/// URL and caster count; the Farcaster app's channel API, where it can be
/// read, the rest. Everything after `parent_url` is optional, so payloads
/// cached before a field existed stay readable.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Route key, same meaning as [`ChannelRef::id`].
    pub id: String,
    /// A display name ("Farcaster") when the Farcaster app gives one, else
    /// the id or a short label for the parent URL.
    pub name: String,
    pub parent_url: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The accounts the description mentions, spliced into it as "@username".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub description_mentions: Vec<Mention>,
    /// The channel's picture.
    #[serde(default)]
    pub image_url: Option<String>,
    /// The channel's banner, usually 3:1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_image_url: Option<String>,
    /// Members, as the Farcaster app counts them.
    #[serde(default)]
    pub member_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follower_count: Option<u64>,
    /// How many accounts have cast in the feed (Hypersnap's `member_count`).
    /// Not members: a channel's casters need not be members, nor its members
    /// casters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub casters: Option<u64>,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<UserSummary>,
    /// Moderators other than the lead, in the channel's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moderators: Vec<UserSummary>,
    /// The cast the channel's hosts pinned, unless it is a mini app or spam.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<Cast>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_link: Option<ExternalLink>,
}

/// One of a channel's headline numbers, saying what it counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelCount {
    Followers(u64),
    Members(u64),
    /// Accounts that have cast in the feed.
    Casters(u64),
}

impl ChannelCount {
    pub fn value(self) -> u64 {
        match self {
            ChannelCount::Followers(n) | ChannelCount::Members(n) | ChannelCount::Casters(n) => n,
        }
    }

    fn nouns(self) -> (&'static str, &'static str) {
        match self {
            ChannelCount::Followers(_) => ("follower", "followers"),
            ChannelCount::Members(_) => ("member", "members"),
            ChannelCount::Casters(_) => ("caster", "casters"),
        }
    }

    /// The noun for this count: "member" or "members".
    pub fn noun(self) -> &'static str {
        let (one, many) = self.nouns();
        if self.value() == 1 { one } else { many }
    }

    /// "475 members".
    pub fn label(self) -> String {
        let (one, many) = self.nouns();
        crate::format::plural(self.value(), one, many)
    }

    /// What the number means and where it comes from, for a tooltip.
    pub fn describe(self) -> &'static str {
        match self {
            ChannelCount::Followers(_) => "Accounts following the channel, from the Farcaster app.",
            ChannelCount::Members(_) => "Members of the channel, from the Farcaster app.",
            ChannelCount::Casters(_) => "Accounts that have cast in this channel, counted by Hypersnap.",
        }
    }
}

impl Channel {
    /// The numbers worth showing, most telling first: followers and members
    /// when the Farcaster app reported them, otherwise the accounts that have
    /// cast there. Hypersnap's caster count is never presented as members.
    pub fn counts(&self) -> Vec<ChannelCount> {
        let reported: Vec<ChannelCount> =
            [self.follower_count.map(ChannelCount::Followers), self.member_count.map(ChannelCount::Members)]
                .into_iter()
                .flatten()
                .collect();
        if reported.is_empty() { self.casters.map(ChannelCount::Casters).into_iter().collect() } else { reported }
    }

    /// Whether the channel has a display name of its own ("Farcaster" for
    /// /farcaster), rather than its id or a feed's label.
    pub fn has_display_name(&self) -> bool {
        crate::channels::is_channel_id(&self.id) && self.name != self.id
    }

    /// The page title: the display name ("Farcaster"), else "/" and the id
    /// or the feed's label ("/dev", "/erc20:0x4ed4…efed").
    pub fn heading(&self) -> String {
        if self.has_display_name() { self.name.clone() } else { format!("/{}", self.name) }
    }

    /// The line under the title: "/id" under a display name, and a feed's
    /// parent URL under its label.
    pub fn subheading(&self) -> Option<String> {
        if self.has_display_name() {
            Some(format!("/{}", self.id))
        } else {
            (self.name != self.id).then(|| self.id.clone())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadNode {
    pub cast: Cast,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ThreadNode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: &str, name: &str) -> Channel {
        Channel {
            id: id.into(),
            name: name.into(),
            parent_url: format!("https://warpcast.com/~/channel/{id}"),
            ..Default::default()
        }
    }

    fn user(registered_at: Option<i64>, first_username_at: Option<i64>) -> User {
        User {
            fid: 3,
            username: "dwr".into(),
            display_name: String::new(),
            pfp_url: None,
            bio: String::new(),
            follower_count: 0,
            following_count: 0,
            registered_at,
            first_username_at,
            custody_address: None,
            eth_addresses: Vec::new(),
            sol_addresses: Vec::new(),
            primary_eth: None,
            primary_sol: None,
        }
    }

    #[test]
    fn counts_never_call_casters_members() {
        // Hypersnap alone: its number counts casters, and says so.
        let hypersnap = Channel { casters: Some(15_664), ..channel("dev", "dev") };
        assert_eq!(hypersnap.counts(), [ChannelCount::Casters(15_664)]);
        assert_eq!(hypersnap.counts()[0].label(), "15,664 casters");
        // With the Farcaster app's numbers, those are shown instead.
        let app = Channel { follower_count: Some(183_421), member_count: Some(475), ..hypersnap.clone() };
        assert_eq!(app.counts(), [ChannelCount::Followers(183_421), ChannelCount::Members(475)]);
        assert_eq!(app.counts().iter().map(|c| c.label()).collect::<Vec<_>>(), ["183,421 followers", "475 members"]);
        assert_eq!(ChannelCount::Members(1).label(), "1 member");
        assert_eq!(ChannelCount::Followers(1).noun(), "follower");
        assert_eq!(ChannelCount::Casters(1).noun(), "caster");
        assert!(channel("dev", "dev").counts().is_empty());
    }

    #[test]
    fn display_names_are_only_for_named_channels() {
        let farcaster = channel("farcaster", "Farcaster");
        assert!(farcaster.has_display_name());
        assert_eq!((farcaster.heading(), farcaster.subheading().as_deref()), ("Farcaster".into(), Some("/farcaster")));
        let dev = channel("dev", "dev");
        assert!(!dev.has_display_name());
        assert_eq!((dev.heading(), dev.subheading()), ("/dev".into(), None));
        let feed = Channel { id: "https://example.com/a".into(), name: "example.com/a".into(), ..Default::default() };
        assert!(!feed.has_display_name());
        assert_eq!(
            (feed.heading(), feed.subheading().as_deref()),
            ("/example.com/a".into(), Some("https://example.com/a"))
        );
    }

    #[test]
    fn payloads_cached_before_the_rich_fields_stay_readable() {
        let old: Channel = serde_json::from_str(
            r#"{"id":"dev","name":"dev","parent_url":"https://warpcast.com/~/channel/dev","description":null,"image_url":null,"member_count":9,"created_at":null}"#,
        )
        .unwrap();
        assert_eq!((old.member_count, old.lead, old.moderators.len(), old.pinned), (Some(9), None, 0, None));
        // Empty rich fields stay out of the JSON.
        let json = serde_json::to_string(&channel("dev", "dev")).unwrap();
        assert!(!json.contains("moderators") && !json.contains("pinned") && !json.contains("casters"), "{json}");
    }

    #[test]
    fn accounts_from_before_the_migration_join_with_their_first_username() {
        // 2023-11-07T19:42:51Z, the migration, and dwr's first fname on 2021-06-17.
        let migrated = 1_699_386_171;
        assert_eq!(user(Some(migrated), Some(1_623_888_000)).joined(), Some(Joined::FirstUsername(1_623_888_000)));
        assert_eq!(user(Some(migrated), None).joined(), Some(Joined::BeforeMigration));
        // A username newer than the migration says nothing about when it joined.
        assert_eq!(user(Some(migrated), Some(1_700_000_000)).joined(), Some(Joined::BeforeMigration));
        // Registered since: the onchain date stands, whatever the username's.
        assert_eq!(user(Some(1_715_817_600), Some(1_715_900_000)).joined(), Some(Joined::Registered(1_715_817_600)));
        assert_eq!(user(None, Some(1_623_888_000)).joined(), None);
        assert!(user(Some(migrated), None).registered_before_migration());
        assert!(!user(Some(1_699_401_600), None).registered_before_migration());
        // The same reading for anything that knows the two dates.
        assert_eq!(Joined::from_dates(Some(migrated), Some(1_623_888_000)), Some(Joined::FirstUsername(1_623_888_000)));
        assert_eq!(Joined::from_dates(Some(ID_REGISTRY_MIGRATION_DAY.start), None), Some(Joined::BeforeMigration));
        assert_eq!(
            Joined::from_dates(Some(ID_REGISTRY_MIGRATION_DAY.end), None),
            Some(Joined::Registered(1_699_401_600))
        );
        assert_eq!(Joined::from_dates(None, None), None);
    }

    #[test]
    fn join_dates_read_as_profiles_say_them() {
        let now = 1_792_235_681;
        let first = Joined::FirstUsername(1_623_888_000);
        assert_eq!(first.date(), "17 Jun 2021");
        assert_eq!(
            first.note(now),
            "Got its first username 5 years ago. The account was registered before the ID registry moved to OP \
             Mainnet on 7 Nov 2023, which is the earliest date onchain data has for it."
        );
        assert_eq!(Joined::BeforeMigration.date(), "before Nov 2023");
        assert_eq!(
            Joined::BeforeMigration.note(now),
            "Registered before the ID registry moved to OP Mainnet on 7 Nov 2023, which is the earliest date onchain \
             data has for it."
        );
        let registered = Joined::Registered(1_715_817_600);
        assert_eq!(
            (registered.date(), registered.note(now)),
            ("16 May 2024".into(), "Registered onchain 2 years ago.".into())
        );
    }
}
