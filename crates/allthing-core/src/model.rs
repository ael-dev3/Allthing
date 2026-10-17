// SPDX-License-Identifier: AGPL-3.0-or-later
//! The normalized, provider-independent Farcaster model. The Worker builds
//! these from upstream JSON; the browser app only ever sees these shapes.

use serde::{Deserialize, Serialize};

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
}

impl Cast {
    pub fn is_reply(&self) -> bool {
        self.parent_hash.is_some()
    }

    /// Images and videos, in the order they were attached.
    pub fn media(&self) -> impl Iterator<Item = &Embed> {
        self.embeds.iter().filter(|embed| embed.is_media())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Route key, same meaning as [`ChannelRef::id`].
    pub id: String,
    pub name: String,
    pub parent_url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub member_count: Option<u64>,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadNode {
    pub cast: Cast,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ThreadNode>,
}
