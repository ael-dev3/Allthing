// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reader-selected account filtering. Identity comes from FIDs, structured
//! mentions and explicit account references, never from a display name.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use crate::model::{Cast, Embed, UserSummary};
use crate::notifications::{Group, GroupTime};
use crate::text::Token;

/// A finite payload can carry every account mentioned in normal casts and
/// notification pages. Oversized metadata is marked incomplete rather than
/// silently dropping an identity and accidentally showing protected content.
const MAX_REFERENCES: usize = 512;
const MAX_QUOTE_NODES: usize = 128;

/// Compact account identities retained when a cast becomes an excerpt or a
/// notification's displayed actors are truncated. Usernames are unresolved
/// explicit references only; known structured identities use their stable FID.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct References {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fids: Vec<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usernames: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub incomplete: bool,
}

impl References {
    pub fn is_empty(&self) -> bool {
        self.fids.is_empty() && self.usernames.is_empty() && !self.incomplete
    }

    pub fn add_fid(&mut self, fid: u64) {
        if fid != 0 {
            insert_bounded(&mut self.fids, fid, &mut self.incomplete);
        }
    }

    pub fn add_username(&mut self, username: &str) {
        if let Some(username) = normalize_username(username) {
            if let Some(fid) = crate::params::fid_username(&username) {
                self.add_fid(fid);
            } else {
                insert_bounded(&mut self.usernames, username, &mut self.incomplete);
            }
        }
    }

    pub fn add_user(&mut self, user: &UserSummary) {
        if user.fid != 0 { self.add_fid(user.fid) } else { self.add_username(&user.username) }
    }

    pub fn extend(&mut self, other: &References) {
        for &fid in &other.fids {
            self.add_fid(fid);
        }
        for username in &other.usernames {
            self.add_username(username);
        }
        self.incomplete |= other.incomplete;
    }

    /// Explicit @handles and links to profiles on Farcaster's two official
    /// hosts. Email addresses, arbitrary URLs and plain names do not count.
    pub fn of_text(text: &str) -> Self {
        Self::text_with_resolved(text, &[])
    }

    /// A cast's own identity references, including normalized reply/recast
    /// provenance. [`FilterSet::hides_cast`] also walks hydrated quoted casts.
    pub fn of_cast(cast: &Cast) -> Self {
        let mut references = Self::text_with_resolved(&cast.text, &cast.mentions);
        references.extend(&cast.references);
        references.add_user(&cast.author);
        for mention in &cast.mentions {
            if mention.fid != 0 { references.add_fid(mention.fid) } else { references.add_username(&mention.username) }
        }
        for embed in &cast.embeds {
            match embed {
                Embed::Cast { fid: Some(fid), .. } => references.add_fid(*fid),
                Embed::Image { url } | Embed::Video { url } | Embed::Link { url, .. } => {
                    references.add_profile_url(url)
                }
                _ => {}
            }
        }
        references
    }

    /// Retain all identities reachable through the quotes that can display
    /// inline. A cycle is visited once. The node guard fails closed while any
    /// filters are active, rather than exposing media from an uninspected node.
    pub fn with_quotes(cast: &Cast, quotes: &BTreeMap<String, Cast>) -> Self {
        let mut references = Self::of_cast(cast);
        let mut pending = vec![(cast, 0)];
        let mut visited = HashSet::new();
        visited.insert(cast.hash.as_str());
        while let Some((current, depth)) = pending.pop() {
            if depth >= crate::quotes::QUOTE_DEPTH {
                continue;
            }
            for hash in current.quoted() {
                if visited.contains(hash) {
                    continue;
                }
                if visited.len() >= MAX_QUOTE_NODES {
                    references.incomplete = true;
                    return references;
                }
                visited.insert(hash);
                if let Some(quote) = quotes.get(hash) {
                    references.extend(&Self::of_cast(quote));
                    pending.push((quote, depth + 1));
                }
            }
        }
        references
    }

    /// Structured mentions resolve an alias to a FID. A later holder of a
    /// muted account's former username must not be hidden by the old alias.
    pub fn text_with_resolved(text: &str, mentions: &[crate::model::Mention]) -> Self {
        let mut references = Self::default();
        let resolved: BTreeSet<String> = mentions
            .iter()
            .filter(|mention| mention.fid != 0)
            .filter_map(|mention| normalize_username(&mention.username))
            .collect();
        for token in crate::text::tokenize(text, &[]) {
            match token {
                Token::Url(url) => {
                    if let Some(username) = profile_username(&url)
                        && !resolved.contains(&username)
                    {
                        references.add_username(&username);
                    }
                }
                Token::Text(text) => {
                    // Unlike the compose helper this has no ten-mention cap:
                    // an explicit handle beyond the protocol mention limit
                    // still matters to a reader who requested full hiding.
                    for (at, _) in text.match_indices('@') {
                        let previous = text[..at].chars().next_back();
                        if previous.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '/' | '@' | '.' | '-')) {
                            continue;
                        }
                        let rest = &text[at + 1..];
                        let stop = rest.find(|c: char| !name_char(c)).unwrap_or(rest.len());
                        let name = rest[..stop].trim_end_matches('.');
                        if let Some(username) = normalize_username(name)
                            && !resolved.contains(&username)
                        {
                            references.add_username(&username);
                        }
                    }
                }
                _ => {}
            }
        }
        references
    }

    pub fn add_profile_url(&mut self, url: &str) {
        if let Some(username) = profile_username(url) {
            self.add_username(&username);
        }
    }
}

fn insert_bounded<T: Ord>(items: &mut Vec<T>, item: T, incomplete: &mut bool) {
    match items.binary_search(&item) {
        Ok(_) => {}
        Err(at) if items.len() < MAX_REFERENCES => items.insert(at, item),
        Err(_) => *incomplete = true,
    }
}

fn name_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(c, '_' | '.' | '-')
        || (!c.is_ascii() && !c.is_whitespace() && !c.is_control() && !"，。！？：；、（）【】“”‘’—–…«»‹›".contains(c))
}

/// Normalize an actual handle, not a display name or a URL. FID fallback
/// handles are supported for accounts without a username.
pub fn normalize_username(value: &str) -> Option<String> {
    let value = value.trim().strip_prefix('@').unwrap_or(value.trim()).to_lowercase();
    if let Some(fid) = crate::params::fid_username(&value) {
        return (fid != 0).then(|| format!("fid:{fid}"));
    }
    let first = value.chars().next()?;
    let last = value.chars().next_back()?;
    (value.chars().count() <= 253
        && !matches!(first, '.' | '-' | '_')
        && !matches!(last, '.' | '-' | '_')
        && value.chars().all(name_char))
    .then_some(value)
}

fn profile_username(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    let (authority, path) = rest.split_once('/')?;
    if !["farcaster.xyz", "warpcast.com", "www.farcaster.xyz", "www.warpcast.com"]
        .iter()
        .any(|host| authority.eq_ignore_ascii_case(host))
    {
        return None;
    }
    let path = path.split(['?', '#']).next()?;
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let name = segments.next()?;
    // Official FID profile URLs have no username that can be reassigned.
    if name == "~" {
        if segments.next()? != "profiles" {
            return None;
        }
        let fid: u64 = segments.next()?.parse().ok()?;
        return (fid != 0 && segments.next().is_none()).then(|| format!("fid:{fid}"));
    }
    if ["miniapps", "mini-apps", "frames", "channel", "channels", "settings", "search", "invite", "login", "signup"]
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
    {
        return None;
    }
    // /name and /name/0xhash are identity-bearing; arbitrary routes are not.
    if let Some(hash) = segments.next()
        && (!cast_hash_link(hash) || segments.next().is_some())
    {
        return None;
    }
    let decoded = crate::params::percent_decode(name)?;
    normalize_username(&decoded)
}

fn cast_hash_link(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) else { return false };
    // The Farcaster app also shares eight-digit cast hash prefixes.
    (8..=40).contains(&hex.len()) && hex.len().is_multiple_of(2) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The union of the reader's active mutes and blocks. Their storage semantics
/// differ, but either requests the same complete content hiding in Allthing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterSet {
    fids: BTreeSet<u64>,
    usernames: BTreeSet<String>,
}

impl FilterSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, fid: u64, username: &str) {
        if fid != 0 {
            self.fids.insert(fid);
        }
        if let Some(username) = normalize_username(username) {
            self.usernames.insert(username);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fids.is_empty() && self.usernames.is_empty()
    }

    pub fn hides_user(&self, user: &UserSummary) -> bool {
        if user.fid != 0 {
            self.fids.contains(&user.fid)
        } else {
            normalize_username(&user.username).is_some_and(|name| {
                crate::params::fid_username(&name).is_some_and(|fid| self.fids.contains(&fid))
                    || self.usernames.contains(&name)
            })
        }
    }

    pub fn hides_references(&self, references: &References) -> bool {
        if self.is_empty() {
            return false;
        }
        references.incomplete
            || references.fids.iter().any(|fid| self.fids.contains(fid))
            || references.usernames.iter().any(|name| {
                crate::params::fid_username(name).is_some_and(|fid| self.fids.contains(&fid))
                    || self.usernames.contains(name)
            })
    }

    pub fn hides_text(&self, text: &str) -> bool {
        !self.is_empty() && self.hides_references(&References::of_text(text))
    }

    pub fn hides_cast(&self, cast: &Cast, quotes: &BTreeMap<String, Cast>) -> bool {
        !self.is_empty() && self.hides_references(&References::with_quotes(cast, quotes))
    }

    pub fn hides_group(&self, group: &Group) -> bool {
        if self.is_empty() {
            return false;
        }
        // Old cached excerpts cannot prove that no protected account appears
        // beyond their text/actor cutoff. Wait for full reference metadata.
        if group.references.is_empty()
            && (group.cast.is_some() || group.actors.is_empty() || group.count as usize > group.actors.len())
        {
            return true;
        }
        self.hides_references(&group.references) || group.actors.iter().any(|actor| self.hides_user(actor))
    }

    pub fn hides_group_time(&self, group: &GroupTime) -> bool {
        if self.is_empty() {
            return false;
        }
        if group.references.is_empty()
            && (group.cast.is_some()
                || group.actors.is_empty()
                || group.count.is_none_or(|count| count as usize > group.actors.len()))
        {
            return true;
        }
        self.hides_references(&group.references) || group.actors.iter().any(|actor| self.hides_user(actor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Mention;
    use crate::notifications::{Event, Kind, Times, group};

    fn user(fid: u64, username: &str) -> UserSummary {
        UserSummary { fid, username: username.into(), display_name: "Alice".into(), pfp_url: None }
    }

    fn cast(hash: &str, fid: u64, text: &str) -> Cast {
        serde_json::from_value(serde_json::json!({
            "hash": hash, "author": user(fid, &format!("user{fid}")), "text": text, "ts": 100
        }))
        .unwrap()
    }

    fn filters() -> FilterSet {
        let mut filters = FilterSet::new();
        filters.insert(88, "@Alice.ETH");
        filters
    }

    #[test]
    fn explicit_handles_use_whole_tokens_and_never_plain_names_or_emails() {
        let filter = filters();
        for text in ["@alice.eth", "Hello (@ALICE.ETH).", "@alice.eth!", "a message\n@alice.eth", "@alice.eth—hello"]
        {
            assert!(filter.hides_text(text), "{text}");
        }
        for text in [
            "Alice",
            "alice.eth",
            "@alice.eth2",
            "@alice.eth-extra",
            "@alice.eth_other",
            "other@alice.eth",
            "first.last@alice.eth",
            "https://example.com/@alice.eth",
            "https://alice.eth",
            "@@alice.eth",
        ] {
            assert!(!filter.hides_text(text), "{text}");
        }
        assert!(filter.hides_text("@alice.eth/example"));
    }

    #[test]
    fn handles_after_the_protocol_mention_limit_still_hide_the_whole_post() {
        let text = (0..30).map(|n| format!("@other{n} ")).collect::<String>() + "@alice.eth";
        assert!(filters().hides_text(&text));
    }

    #[test]
    fn profiles_and_cast_links_require_exact_official_hosts_and_real_routes() {
        let filter = filters();
        for text in [
            "https://farcaster.xyz/alice.eth",
            "http://warpcast.com/ALICE.ETH?x=1",
            "(https://www.farcaster.xyz/%61lice.eth/)",
            "https://farcaster.xyz/alice.eth/0x1234abcd",
            "https://warpcast.com/~/profiles/88",
            "HTTPS://FARCASTER.XYZ/alice.eth#casts",
        ] {
            assert!(filter.hides_text(text), "{text}");
        }
        for text in [
            "https://farcaster.xyz.evil.test/alice.eth",
            "https://farcaster.xyz@evil.test/alice.eth",
            "https://evil.test/?next=https://farcaster.xyz/alice.eth",
            "https://farcaster.xyz/alice.eth2",
            "https://farcaster.xyz/~/channel/alice.eth",
            "https://farcaster.xyz/miniapps/alice.eth",
            "https://farcaster.xyz/alice.eth/something",
            "https://farcaster.xyz/alice.eth%2Fevil",
            "https://farcaster.xyz/~/profiles/888",
            "https://farcaster.xyz/alice.eth/0xNOTAHASH",
        ] {
            assert!(!filter.hides_text(text), "{text}");
        }
    }

    #[test]
    fn known_fids_survive_renames_without_hiding_a_reassigned_alias() {
        let filter = filters();
        assert!(filter.hides_user(&user(88, "new-name")));
        assert!(!filter.hides_user(&user(99, "alice.eth")));
        assert!(filter.hides_user(&user(0, "ALICE.ETH")));
        assert!(filter.hides_user(&user(0, "fid:88")));
        let mut mention = cast("0xm", 1, "hello @alice.eth");
        mention.mentions.push(Mention { fid: 99, username: "alice.eth".into() });
        assert!(!filter.hides_cast(&mention, &BTreeMap::new()));
        mention.mentions[0] = Mention { fid: 88, username: "new-name".into() };
        mention.text = "hello @new-name".into();
        assert!(filter.hides_cast(&mention, &BTreeMap::new()));
    }

    #[test]
    fn author_mentions_parents_recasters_and_embedded_media_are_filtered_before_display() {
        let filter = filters();
        let empty = BTreeMap::new();
        assert!(filter.hides_cast(&cast("0xauthor", 88, ""), &empty));
        let mut normal = cast("0xnormal", 1, "Alice is just a display name here");
        assert!(!filter.hides_cast(&normal, &empty));
        normal.mentions.push(Mention { fid: 88, username: "renamed".into() });
        assert!(filter.hides_cast(&normal, &empty));
        normal.mentions.clear();
        normal.references.add_fid(88);
        normal.embeds.push(Embed::Image { url: "https://example.com/private-photo.jpg".into() });
        assert!(filter.hides_cast(&normal, &empty));
        normal.references = References::default();
        normal.embeds.push(Embed::Cast { hash: "0xunhydrated".into(), fid: Some(88) });
        assert!(filter.hides_cast(&normal, &empty));
    }

    #[test]
    fn nested_quotes_hide_the_entire_outer_post_and_cycles_terminate() {
        let mut outer = cast("0xouter", 1, "ordinary post with attached media");
        outer.embeds.push(Embed::Cast { hash: "0xmiddle".into(), fid: Some(2) });
        let mut middle = cast("0xmiddle", 2, "ordinary quote");
        middle.embeds.push(Embed::Cast { hash: "0xinner".into(), fid: Some(3) });
        let mut inner = cast("0xinner", 3, "hello @alice.eth");
        inner.embeds.push(Embed::Cast { hash: "0xouter".into(), fid: Some(1) });
        let mut quotes = BTreeMap::from([(middle.hash.clone(), middle), (inner.hash.clone(), inner)]);
        assert!(filters().hides_cast(&outer, &quotes));
        quotes.get_mut("0xinner").unwrap().text = "entirely unrelated".into();
        assert!(!filters().hides_cast(&outer, &quotes));
    }

    #[test]
    fn normalization_keeps_reply_recast_and_inline_nested_quote_identities() {
        use crate::normalize::{RawCast, cast as normalize};
        for provenance in [
            serde_json::json!({"parent_author":{"fid":88}}),
            serde_json::json!({"recasted_by":{"fid":88,"username":"renamed"}}),
            serde_json::json!({"recast_by":[{"fid":2},{"fid":88}]}),
            serde_json::json!({"embeds":[{"cast":{"hash":"1","author":{"fid":2},"text":"x",
                "embeds":[{"cast":{"hash":"2","author":{"fid":3},"text":"hello @alice.eth"}}]}}]}),
            serde_json::json!({"embeds":[{"cast":{"hash":"1","author":{"fid":2},
                "mentioned_profiles":[{"fid":88,"username":"renamed"}]}}]}),
        ] {
            let mut raw = serde_json::json!({"hash":"1","author":{"fid":1,"username":"one"},"text":"ordinary"});
            raw.as_object_mut().unwrap().extend(provenance.as_object().unwrap().clone());
            let normalized = normalize(&serde_json::from_value::<RawCast>(raw).unwrap());
            assert!(filters().hides_cast(&normalized, &BTreeMap::new()), "{provenance}");
        }
    }

    #[test]
    fn all_group_actors_and_full_text_reach_rows_and_the_unread_badge() {
        let mut events: Vec<Event> = (1..=7)
            .map(|fid| Event {
                kind: Kind::Like,
                actor: user(fid, &format!("person{fid}")),
                ts: 100 - fid as i64,
                cast: Some(cast("0xmine", 500, "ordinary excerpt")),
                mentions_you: false,
            })
            .collect();
        events.last_mut().unwrap().actor = user(88, "renamed");
        let mut groups = group(events, 500);
        assert_eq!(groups[0].actors.len(), 5);
        assert_eq!(groups[0].count, 7);
        assert!(filters().hides_group(&groups[0]));
        let times: Times = serde_json::from_value(serde_json::json!({"groups":groups,"newest":100})).unwrap();
        assert!(filters().hides_group_time(&times.groups[0]));
        groups = group(
            vec![Event {
                kind: Kind::Mention,
                actor: user(2, "two"),
                ts: 100,
                cast: Some(cast("0xmention", 2, &("unrelated words ".repeat(20) + "@alice.eth"))),
                mentions_you: true,
            }],
            500,
        );
        assert!(!groups[0].cast.as_ref().unwrap().excerpt.contains("alice"));
        assert!(filters().hides_group(&groups[0]));
    }

    #[test]
    fn notification_reference_metadata_preserves_resolved_rename_safety() {
        let mut target = cast("0xmention", 2, "hello @alice.eth");
        target.mentions.push(Mention { fid: 99, username: "alice.eth".into() });
        let groups = group(
            vec![Event { kind: Kind::Mention, actor: user(2, "two"), ts: 100, cast: Some(target), mentions_you: true }],
            500,
        );
        assert!(!filters().hides_group(&groups[0]));
        let times: Times = serde_json::from_value(serde_json::json!({"groups":groups})).unwrap();
        assert!(!filters().hides_group_time(&times.groups[0]));
    }

    #[test]
    fn old_cached_notifications_without_complete_references_stay_hidden_until_refresh() {
        let times: Times = serde_json::from_value(serde_json::json!({"groups":[
            {"ts":100,"actors":[user(88,"renamed")]},
            {"ts":101,"actors":[user(2,"two")],"cast":{"hash":"0xm","excerpt":"hello @alice.eth"}},
            {"ts":102,"actors":[user(2,"two")],"cast":{"hash":"0xn","excerpt":"unrelated"}},
            {"ts":103,"actors":[user(2,"two")],"count":2},
            {"ts":104,"actors":[user(2,"two")],"count":1},
            {"ts":105}
        ]}))
        .unwrap();
        assert!(filters().hides_group_time(&times.groups[0]));
        assert!(filters().hides_group_time(&times.groups[1]));
        // Even an ordinary visible excerpt cannot prove that text/actors
        // outside its cutoff do not involve a selected account.
        assert!(filters().hides_group_time(&times.groups[2]));
        assert!(filters().hides_group_time(&times.groups[3]));
        assert!(!filters().hides_group_time(&times.groups[4]));
        assert!(filters().hides_group_time(&times.groups[5]));
        for group in &times.groups {
            assert!(!FilterSet::default().hides_group_time(group));
        }
        let follow: Group = serde_json::from_value(serde_json::json!({
            "kind":"follow","ts":100,"actors":[user(2,"two")],"count":1
        }))
        .unwrap();
        assert!(!filters().hides_group(&follow));
        let mut truncated = follow.clone();
        truncated.count = 7;
        assert!(filters().hides_group(&truncated));
        assert!(!FilterSet::default().hides_group(&truncated));
        let mut cast_group = follow;
        cast_group.kind = Kind::Like;
        cast_group.cast =
            Some(crate::notifications::Target { hash: "0xlegacy".into(), excerpt: "A short ordinary start…".into() });
        assert!(filters().hides_group(&cast_group));
        assert!(!FilterSet::default().hides_group(&cast_group));
    }

    #[test]
    fn oversized_reference_lists_do_not_silently_leak_protected_content() {
        let mut references = References::default();
        for fid in 1000..2000 {
            references.add_fid(fid);
        }
        assert_eq!(references.fids.len(), MAX_REFERENCES);
        assert!(references.incomplete);
        assert!(filters().hides_references(&references));
        assert!(!FilterSet::default().hides_references(&references));
        let mut copy = References::default();
        copy.extend(&references);
        assert_eq!(references, copy);
    }

    #[test]
    fn unicode_ens_handles_and_fid_links_are_normalized_without_name_guessing() {
        let mut filter = FilterSet::new();
        filter.insert(88, "🌴.eth");
        assert!(filter.hides_text("hey @🌴.eth"));
        assert!(filter.hides_text("https://farcaster.xyz/%F0%9F%8C%B4.eth"));
        assert!(filter.hides_text("https://farcaster.xyz/~/profiles/88"));
        assert!(!filter.hides_text("a 🌴.eth display name"));
        assert_eq!(normalize_username("@ALICE.ETH"), Some("alice.eth".into()));
        for invalid in ["", "@", ".eth", "alice/name", "alice?x", "alice:name", "hello world", "alice\nname"] {
            assert_eq!(normalize_username(invalid), None, "{invalid}");
        }
    }
}
