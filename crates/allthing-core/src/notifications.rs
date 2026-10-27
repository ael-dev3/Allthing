// SPDX-License-Identifier: AGPL-3.0-or-later
//! A signed-in reader's notifications (`/api/v1/notifications`): who liked,
//! recasted, replied to, mentioned and quoted them, and who followed them,
//! grouped as the Farcaster app groups them ("alice and 5 others liked your
//! cast"), with the page's tabs and the bell's unread count.
//!
//! Hypersnap's notifications name likes, follows and replies only
//! (`normalize::notifications`). A recast comes as one more like, the same
//! item to the byte, which [`sort_reactions`] tells apart by the cast's
//! recasts as the hub lists them; a mention or a quote comes as a reply,
//! which [`classify`] tells apart by the cast. Quotes that neither reply to
//! nor mention the reader aren't there at all: the Worker reads those of
//! the reader's latest casts on its own.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::api::{API_PREFIX, Annotations, Meta};
use crate::embeds::embed_label;
use crate::format::full_number;
use crate::model::{Cast, Embed, UserSummary};
use crate::moderation::References;
use crate::params::percent_encode;
use crate::text::truncate_title;

/// Where the app reads them.
pub const NOTIFICATIONS_PATH: &str = "/api/v1/notifications";

/// The most people a group shows; its count says how many there are.
pub const MAX_ACTORS: usize = 5;

/// An excerpt's length, as [`truncate_title`] counts it: two lines of a
/// row on a phone, about.
pub const EXCERPT_CHARS: usize = 140;

/// The longest cursor passed back to Hypersnap: base64 of where its next
/// page starts (a time and a hash, about 120 characters).
const MAX_CURSOR: usize = 512;

/// What someone did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Like,
    Recast,
    Reply,
    Mention,
    Quote,
    Follow,
}

impl Kind {
    /// What the people of a group did, after their names.
    pub fn verb(self) -> &'static str {
        match self {
            Kind::Like => "liked your cast",
            Kind::Recast => "recasted your cast",
            Kind::Reply => "replied to your cast",
            Kind::Mention => "mentioned you",
            Kind::Quote => "quoted your cast",
            Kind::Follow => "followed you",
        }
    }

    /// Which a cast that is several things to the reader shows as: a reply
    /// before a quote, a quote before a mention.
    fn rank(self) -> u8 {
        match self {
            Kind::Reply => 3,
            Kind::Quote => 2,
            Kind::Mention => 1,
            _ => 0,
        }
    }
}

/// One thing someone did that concerns the reader.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub kind: Kind,
    pub actor: UserSummary,
    /// Unix seconds.
    pub ts: i64,
    /// For a like or a recast, the reader's cast; for a reply, a mention or
    /// a quote, the cast that is one. `None` for a follow.
    pub cast: Option<Cast>,
    /// The cast mentions the reader: a quote that does shows under Mentions.
    pub mentions_you: bool,
}

/// Whether `cast` quotes one of `me`'s casts.
pub fn quotes(cast: &Cast, me: u64) -> bool {
    cast.embeds.iter().any(|embed| matches!(embed, Embed::Cast { fid: Some(fid), .. } if *fid == me))
}

/// Whether `cast` mentions `me`.
pub fn mentions(cast: &Cast, me: u64) -> bool {
    cast.mentions.iter().any(|m| m.fid == me)
}

/// What a cast Hypersnap calls a reply is to the reader `me`, given who
/// wrote the cast it replies to (`parent_author`): a reply to one of their
/// casts (one that also mentions them is still a reply), a quote of one, or
/// a cast that mentions them. Anything else is a reply, as Hypersnap says.
pub fn classify(cast: &Cast, parent_author: Option<u64>, me: u64) -> Kind {
    if parent_author == Some(me) {
        Kind::Reply
    } else if quotes(cast, me) {
        Kind::Quote
    } else if mentions(cast, me) {
        Kind::Mention
    } else {
        Kind::Reply
    }
}

/// What the hub says of a cast's recasts: who recasted it and when, and
/// down to when that is complete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recasts {
    /// (fid, Unix seconds), newest first, each account once.
    pub by: Vec<(u64, i64)>,
    /// Recasts older than this may be missing; `None` when none are.
    pub since: Option<i64>,
}

impl Recasts {
    /// From a read of the newest recasts, `per_shard` of each shard's
    /// (`reactionsByCast` with `reverse`). The newest `per_shard` of them
    /// all are certainly the cast's newest, whichever shard they came from;
    /// a read that found fewer has them all.
    pub fn read(mut by: Vec<(u64, i64)>, per_shard: usize) -> Recasts {
        by.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        let mut seen = HashSet::new();
        by.retain(|(fid, _)| seen.insert(*fid));
        let since = (per_shard > 0 && by.len() >= per_shard).then(|| by[per_shard - 1].1);
        Recasts { by, since }
    }

    fn of(&self, fid: u64) -> Option<i64> {
        self.by.iter().find(|(by, _)| *by == fid).map(|(_, at)| *at)
    }

    /// Whether the read says for certain that `fid` made no recast at `at`
    /// or later.
    fn rules_out(&self, at: i64) -> bool {
        self.since.is_none_or(|since| at >= since)
    }
}

/// Tell recasts from likes. Hypersnap lists a recast as one more like, the
/// same item to the byte, so an account that liked and recasted a cast
/// shows twice. For the casts whose recasts were read (`recasts`, by
/// hash), the item nearest in time to the account's recast is the recast.
/// Otherwise, of an account's two items on one cast, the later is taken
/// for it: people mostly like, then recast, seconds apart. A single item
/// stays a like, unless the read shows it was a recast. (Farcaster keeps
/// one like and one recast per account and cast, so more than two items
/// are repeats, which grouping counts once.)
pub fn sort_reactions(events: &mut [Event], recasts: &HashMap<String, Recasts>) {
    let mut items: HashMap<(&str, u64), Vec<usize>> = HashMap::new();
    for (i, event) in events.iter().enumerate() {
        if event.kind == Kind::Like
            && let Some(cast) = &event.cast
        {
            items.entry((cast.hash.as_str(), event.actor.fid)).or_default().push(i);
        }
    }
    let mut recast: Vec<usize> = Vec::new();
    for ((hash, fid), mut found) in items {
        found.sort_by_key(|&i| std::cmp::Reverse(events[i].ts));
        let read = recasts.get(hash);
        match read.and_then(|read| read.of(fid)) {
            // Nearest the recast's time; of two as near, the later.
            Some(at) => recast.extend(found.iter().copied().min_by_key(|&i| (events[i].ts - at).abs())),
            None if found.len() >= 2 && !read.is_some_and(|read| read.rules_out(events[found[1]].ts)) => {
                recast.push(found[0]);
            }
            None => {}
        }
    }
    for i in recast {
        events[i].kind = Kind::Recast;
    }
}

/// The casts whose recasts are worth reading to tell them from likes, at
/// most `limit`, the most recently liked first: those liked on the page,
/// but for the ones whose count of recasts (`known`, by hash, from the
/// reader's casts read alongside) is the number of accounts that show
/// twice on them: none recasted, or each recaster liked it too, here.
pub fn recasts_to_read(events: &[Event], known: &HashMap<String, u64>, limit: usize) -> Vec<String> {
    let mut casts: Vec<&str> = Vec::new();
    let mut items: HashMap<(&str, u64), u32> = HashMap::new();
    for event in events.iter().filter(|event| event.kind == Kind::Like) {
        let Some(cast) = &event.cast else { continue };
        if !casts.contains(&cast.hash.as_str()) {
            casts.push(&cast.hash);
        }
        *items.entry((cast.hash.as_str(), event.actor.fid)).or_default() += 1;
    }
    casts
        .into_iter()
        .filter(|hash| {
            let pairs = items.iter().filter(|((on, _), n)| on == hash && **n >= 2).count() as u64;
            known.get(*hash) != Some(&pairs)
        })
        .take(limit)
        .map(str::to_owned)
        .collect()
}

/// The reader's casts whose quotes are read: the newest `limit` of their
/// latest `casts`, those that aren't replies first, as they are the ones
/// people quote.
pub fn quoted_casts(casts: &[Cast], limit: usize) -> Vec<&Cast> {
    let mut newest: Vec<&Cast> = casts.iter().collect();
    newest.sort_by_key(|cast| (cast.is_reply(), std::cmp::Reverse(cast.ts)));
    newest.truncate(limit);
    newest
}

/// Events read on top of Hypersnap's page (the quotes of the reader's
/// latest casts) that belong on it: those at least as new as its oldest
/// item, or all of them when it is the last page (`more` is false) or has
/// none. The pages after it don't read them, so older ones would sort
/// above notifications newer than they are.
pub fn within_page(extra: Vec<Event>, oldest: Option<i64>, more: bool) -> Vec<Event> {
    match oldest {
        Some(oldest) if more => extra.into_iter().filter(|event| event.ts >= oldest).collect(),
        _ => extra,
    }
}

/// The cast a group is about: the one a row opens, and its start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub excerpt: String,
}

impl Target {
    pub fn of(cast: &Cast) -> Target {
        Target { hash: cast.hash.clone(), excerpt: excerpt(cast) }
    }
}

/// The start of a cast, as a row shows it under its sentence: its text,
/// cut between words (an emoji whole or not at all), or for a cast with
/// none its first embed in a word or two.
pub fn excerpt(cast: &Cast) -> String {
    let text = truncate_title(&cast.text, EXCERPT_CHARS);
    if !text.trim().is_empty() {
        return text;
    }
    embed_label(&cast.embeds).or_else(|| cast.media().next().map(|_| "Image".to_owned())).unwrap_or_default()
}

/// Notifications grouped as the Farcaster app groups them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub kind: Kind,
    /// Newest first, at most [`MAX_ACTORS`].
    pub actors: Vec<UserSummary>,
    /// How many people the group has, those in `actors` and the rest.
    pub count: u32,
    /// The newest event's time, Unix seconds.
    pub ts: i64,
    /// The cast it is about; none for follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cast: Option<Target>,
    /// A quote that mentions the reader, which Mentions shows too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mentions_you: bool,
    /// All actors, including those beyond the five displayed, and the full
    /// target cast's account references before its text becomes an excerpt.
    #[serde(default, skip_serializing_if = "References::is_empty")]
    pub references: References,
}

/// Where a group's row goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opens<'a> {
    /// The cast with this hash.
    Cast(&'a str),
    /// The profile of the account with this username: a follower's.
    Profile(&'a str),
}

impl Group {
    /// The newest person in the group.
    pub fn first(&self) -> Option<&UserSummary> {
        self.actors.first()
    }

    /// What follows the first person's name: " and 5 others liked your
    /// cast".
    pub fn rest(&self) -> String {
        let verb = self.kind.verb();
        match self.count.saturating_sub(1) {
            0 => format!(" {verb}"),
            1 => format!(" and 1 other {verb}"),
            others => format!(" and {} others {verb}", full_number(u64::from(others))),
        }
    }

    /// "alice and 5 others liked your cast", with the first person's
    /// display name (or username).
    pub fn sentence(&self) -> String {
        let name = self.first().map_or("Someone", UserSummary::name);
        format!("{name}{}", self.rest())
    }

    /// What its row opens: the cast, or for follows the newest follower's
    /// profile.
    pub fn opens(&self) -> Option<Opens<'_>> {
        match &self.cast {
            Some(cast) => Some(Opens::Cast(&cast.hash)),
            None => self.first().map(|user| Opens::Profile(&user.username)),
        }
    }
}

/// What makes events one group.
#[derive(PartialEq, Eq, Hash)]
enum Key {
    /// Likes, or recasts, of one cast.
    Reactions(Kind, String),
    /// A reply, a mention or a quote: the cast, one item each.
    Item(String),
    /// Follows on one UTC day.
    Day(i64),
}

impl Key {
    fn of(event: &Event) -> Option<Key> {
        let hash = || event.cast.as_ref().map(|cast| cast.hash.clone());
        Some(match event.kind {
            Kind::Like | Kind::Recast => Key::Reactions(event.kind, hash()?),
            Kind::Reply | Kind::Mention | Kind::Quote => Key::Item(hash()?),
            Kind::Follow => Key::Day(event.ts.div_euclid(86_400)),
        })
    }
}

/// Group `events` as the Farcaster app does: likes of one cast together,
/// and recasts of one cast, follows on one UTC day, and each reply,
/// mention and quote on its own (a cast that is two of them shows as the
/// first of reply, quote, mention). Each person counts once in a group, and
/// the reader's own doings (`me`) aren't there. Newest first, by each
/// group's newest event, as are the people in each.
pub fn group(mut events: Vec<Event>, me: u64) -> Vec<Group> {
    events.retain(|event| event.actor.fid != me);
    // Stable: events of the same second keep the order they came in.
    events.sort_by_key(|event| std::cmp::Reverse(event.ts));
    let mut groups: Vec<(Group, HashSet<u64>)> = Vec::new();
    let mut by_key: HashMap<Key, usize> = HashMap::new();
    for event in events {
        let Some(key) = Key::of(&event) else { continue };
        let at = match by_key.get(&key) {
            Some(&at) => at,
            None => {
                by_key.insert(key, groups.len());
                let group = Group {
                    kind: event.kind,
                    actors: Vec::new(),
                    count: 0,
                    ts: event.ts,
                    cast: event.cast.as_ref().map(Target::of),
                    mentions_you: false,
                    references: References::default(),
                };
                groups.push((group, HashSet::new()));
                groups.len() - 1
            }
        };
        let (group, people) = &mut groups[at];
        group.references.add_user(&event.actor);
        if let Some(cast) = &event.cast {
            group.references.extend(&References::of_cast(cast));
        }
        if event.kind.rank() > group.kind.rank() {
            group.kind = event.kind;
        }
        group.mentions_you |= event.mentions_you;
        if people.insert(event.actor.fid) {
            group.count += 1;
            if group.actors.len() < MAX_ACTORS {
                group.actors.push(event.actor);
            }
        }
    }
    // Made in the order of their newest events, which came first.
    groups.into_iter().map(|(group, _)| group).collect()
}

/// The page's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    All,
    /// Mentions, and quotes that mention the reader.
    Mentions,
    Replies,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::All, Tab::Mentions, Tab::Replies];

    /// From the page's `?tab=`; anything else is All.
    pub fn parse(value: Option<&str>) -> Tab {
        match value.map(str::trim) {
            Some("mentions") => Tab::Mentions,
            Some("replies") => Tab::Replies,
            _ => Tab::All,
        }
    }

    /// Its `?tab=` value; All, the page's own, has none.
    pub fn query(self) -> Option<&'static str> {
        match self {
            Tab::All => None,
            Tab::Mentions => Some("mentions"),
            Tab::Replies => Some("replies"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Mentions => "Mentions",
            Tab::Replies => "Replies",
        }
    }

    pub fn shows(self, group: &Group) -> bool {
        match self {
            Tab::All => true,
            Tab::Mentions => group.kind == Kind::Mention || (group.kind == Kind::Quote && group.mentions_you),
            Tab::Replies => group.kind == Kind::Reply,
        }
    }

    /// What the tab says with nothing to show.
    pub fn empty(self) -> &'static str {
        match self {
            Tab::All => {
                "Nothing yet. When people like, recast, reply to, mention or quote your casts, or follow you, it shows here."
            }
            Tab::Mentions => "No mentions yet. Casts that mention you, and quotes of your casts that do, show here.",
            Tab::Replies => "No replies yet. Replies to your casts show here.",
        }
    }
}

/// How many groups are newer than `seen_at` (Unix seconds): the bell's
/// count. A group with someone new in it counts again.
pub fn unread(groups: &[Group], seen_at: i64) -> usize {
    groups.iter().filter(|group| group.ts > seen_at).count()
}

/// What the bell reads of a page of notifications: each group's newest
/// time, and the page's, and nothing else. Its reader is a fraction of the
/// whole payload's, which only the page's own code needs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Times {
    #[serde(default)]
    pub groups: Vec<GroupTime>,
    #[serde(default)]
    pub newest: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct GroupTime {
    pub ts: i64,
    /// The same compact references used by notification rows, so a hidden
    /// notification cannot leak back through the unread badge.
    #[serde(default)]
    pub references: References,
    /// Backward compatibility for cached payloads predating references.
    #[serde(default)]
    pub actors: Vec<UserSummary>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub cast: Option<Target>,
}

impl Times {
    /// [`unread`], from the times alone.
    pub fn unread(&self, seen_at: i64) -> usize {
        self.groups.iter().filter(|group| group.ts > seen_at).count()
    }
}

/// The bell's badge: the count, "9+" past nine, none at zero.
pub fn badge(unread: usize) -> Option<String> {
    match unread {
        0 => None,
        1..=9 => Some(unread.to_string()),
        _ => Some("9+".to_owned()),
    }
}

/// The bell's name for screen readers, with the exact count.
pub fn bell_label(unread: usize) -> String {
    format!("Notifications, {unread} unread")
}

/// What `/api/v1/notifications` answers: one page of the reader's
/// notifications, grouped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationsPayload {
    /// Newest first.
    pub groups: Vec<Group>,
    /// The cursor for the next, older page, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// The newest group's time (Unix seconds), 0 with none: what opening
    /// the page marks as read.
    #[serde(default)]
    pub newest: i64,
    /// The profile cards of the people the groups show (`users`), and the
    /// verdicts on the links of their casts (`links`), carried to the next
    /// build.
    #[serde(default)]
    pub notes: Annotations,
    pub meta: Meta,
}

/// `/api/v1/notifications`, or a later page of them from `cursor`.
pub fn notifications_path(cursor: Option<&str>) -> String {
    match cursor {
        Some(cursor) => format!("{API_PREFIX}/notifications?cursor={}", percent_encode(cursor)),
        None => NOTIFICATIONS_PATH.to_owned(),
    }
}

/// A cursor from `?cursor=`, if it is one Hypersnap could have given: base64
/// (standard or URL-safe), of a sane length.
pub fn cursor_from_param(value: &str) -> Option<String> {
    let value = value.trim();
    let base64 = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_');
    (!value.is_empty() && value.len() <= MAX_CURSOR && value.bytes().all(base64)).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Mention;

    const ME: u64 = 539_854;

    fn person(fid: u64, name: &str) -> UserSummary {
        UserSummary { fid, username: name.to_owned(), display_name: String::new(), pfp_url: None }
    }

    fn cast(hash: &str, author: u64, text: &str) -> Cast {
        Cast {
            hash: hash.to_owned(),
            author: person(author, "author"),
            text: text.to_owned(),
            mentions: Vec::new(),
            ts: 1_792_000_000,
            parent_hash: None,
            parent_url: None,
            channel: None,
            embeds: Vec::new(),
            likes: 0,
            recasts: 0,
            replies: 0,
            mini_app: false,
            references: Default::default(),
            spam: false,
        }
    }

    fn event(kind: Kind, fid: u64, ts: i64, on: Option<&str>) -> Event {
        let author = if matches!(kind, Kind::Like | Kind::Recast) { ME } else { fid };
        Event {
            kind,
            actor: person(fid, &format!("user{fid}")),
            ts,
            cast: on.map(|hash| cast(hash, author, &format!("text of {hash}"))),
            mentions_you: false,
        }
    }

    #[test]
    fn replies_are_told_from_mentions_and_quotes() {
        let mut mention = cast("0xm", 7, "gm @ael");
        mention.mentions.push(Mention { username: "0xael.eth".into(), fid: ME });
        let mut quote = cast("0xq", 7, "look");
        quote.embeds.push(Embed::Cast { hash: "0xmine".into(), fid: Some(ME) });
        let mut other_quote = cast("0xo", 7, "look");
        other_quote.embeds.push(Embed::Cast { hash: "0xtheirs".into(), fid: Some(3) });

        // A reply to the reader's cast, even one that mentions them.
        assert_eq!(classify(&cast("0xr", 7, "agreed"), Some(ME), ME), Kind::Reply);
        assert_eq!(classify(&mention, Some(ME), ME), Kind::Reply);
        // In someone else's thread, or on its own: a mention.
        assert_eq!(classify(&mention, Some(3), ME), Kind::Mention);
        assert_eq!(classify(&mention, None, ME), Kind::Mention);
        // A quote of one of theirs, mentioning them or not.
        assert_eq!(classify(&quote, None, ME), Kind::Quote);
        let mut both = quote.clone();
        both.mentions = mention.mentions.clone();
        assert_eq!(classify(&both, Some(3), ME), Kind::Quote);
        assert!(mentions(&both, ME) && quotes(&both, ME));
        // Quoting someone else's cast isn't a quote of theirs; what isn't
        // anything else stays the reply Hypersnap says it is.
        assert_eq!(classify(&other_quote, None, ME), Kind::Reply);
    }

    #[test]
    fn groups_likes_and_recasts_by_cast() {
        let events = vec![
            event(Kind::Like, 1, 100, Some("0xa")),
            event(Kind::Like, 2, 300, Some("0xa")),
            event(Kind::Recast, 2, 301, Some("0xa")),
            event(Kind::Like, 3, 200, Some("0xb")),
            // The same person twice: once in the group.
            event(Kind::Like, 1, 90, Some("0xa")),
        ];
        let groups = group(events, ME);
        let shape: Vec<(Kind, &str, Vec<u64>, u32, i64)> = groups
            .iter()
            .map(|g| {
                let hash = g.cast.as_ref().map_or("", |c| c.hash.as_str());
                (g.kind, hash, g.actors.iter().map(|a| a.fid).collect(), g.count, g.ts)
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Kind::Recast, "0xa", vec![2], 1, 301),
                (Kind::Like, "0xa", vec![2, 1], 2, 300),
                (Kind::Like, "0xb", vec![3], 1, 200),
            ]
        );
        assert_eq!(groups[1].sentence(), "user2 and 1 other liked your cast");
        assert_eq!(groups[0].sentence(), "user2 recasted your cast");
        assert_eq!(groups[1].cast.as_ref().unwrap().excerpt, "text of 0xa");
        assert_eq!(groups[1].opens(), Some(Opens::Cast("0xa")));
    }

    #[test]
    fn a_group_shows_five_people_and_counts_them_all() {
        let events: Vec<Event> = (1..=1205).map(|fid| event(Kind::Like, fid, fid as i64, Some("0xa"))).collect();
        let groups = group(events, ME);
        assert_eq!(groups.len(), 1);
        let likes = &groups[0];
        assert_eq!(likes.actors.iter().map(|a| a.fid).collect::<Vec<_>>(), [1205, 1204, 1203, 1202, 1201]);
        assert_eq!((likes.count, likes.ts), (1205, 1205));
        assert_eq!(likes.rest(), " and 1,204 others liked your cast");
        let named =
            Group { actors: vec![UserSummary { display_name: "Alice".into(), ..person(1, "alice") }], ..likes.clone() };
        assert_eq!(named.sentence(), "Alice and 1,204 others liked your cast");
    }

    #[test]
    fn follows_group_by_utc_day() {
        let day = 1_792_540_800; // 2026-10-21T00:00:00Z
        let events = vec![
            event(Kind::Follow, 1, day + 36_000, None),
            event(Kind::Follow, 2, day + 100, None),
            event(Kind::Follow, 3, day - 100, None),
            event(Kind::Follow, 4, day - 80_000, None),
            event(Kind::Like, 5, day + 50_000, Some("0xa")),
        ];
        let groups = group(events, ME);
        let shape: Vec<(Kind, Vec<u64>)> =
            groups.iter().map(|g| (g.kind, g.actors.iter().map(|a| a.fid).collect())).collect();
        assert_eq!(shape, [(Kind::Like, vec![5]), (Kind::Follow, vec![1, 2]), (Kind::Follow, vec![3, 4])]);
        assert_eq!(groups[1].sentence(), "user1 and 1 other followed you");
        // A follow opens the newest follower's profile.
        assert_eq!(groups[1].opens(), Some(Opens::Profile("user1")));
        assert_eq!(groups[1].cast, None);
    }

    #[test]
    fn replies_mentions_and_quotes_are_one_item_each() {
        let mut quoted = event(Kind::Quote, 8, 500, Some("0xq"));
        quoted.mentions_you = true;
        let events = vec![
            event(Kind::Reply, 7, 400, Some("0xr1")),
            event(Kind::Reply, 7, 300, Some("0xr2")),
            event(Kind::Mention, 9, 200, Some("0xm")),
            // The same cast from the notifications, as a mention, and from
            // the quotes read: one quote that mentions the reader.
            event(Kind::Mention, 8, 500, Some("0xq")),
            event(Kind::Quote, 8, 500, Some("0xq")),
            quoted,
            // A reply that quotes too is a reply.
            event(Kind::Quote, 7, 100, Some("0xboth")),
            event(Kind::Reply, 7, 100, Some("0xboth")),
        ];
        let groups = group(events, ME);
        let shape: Vec<(Kind, &str, bool)> =
            groups.iter().map(|g| (g.kind, g.cast.as_ref().unwrap().hash.as_str(), g.mentions_you)).collect();
        assert_eq!(
            shape,
            [
                (Kind::Quote, "0xq", true),
                (Kind::Reply, "0xr1", false),
                (Kind::Reply, "0xr2", false),
                (Kind::Mention, "0xm", false),
                (Kind::Reply, "0xboth", false),
            ]
        );
        assert!(groups.iter().all(|g| g.count == 1));
        assert_eq!(groups[0].sentence(), "user8 quoted your cast");
        assert_eq!(groups[3].sentence(), "user9 mentioned you");
        assert_eq!(groups[1].sentence(), "user7 replied to your cast");
    }

    #[test]
    fn the_readers_own_doings_are_left_out() {
        let events = vec![
            event(Kind::Like, ME, 100, Some("0xa")),
            event(Kind::Reply, ME, 100, Some("0xb")),
            event(Kind::Quote, ME, 100, Some("0xc")),
            event(Kind::Like, 2, 50, Some("0xa")),
        ];
        let groups = group(events, ME);
        assert_eq!(groups.len(), 1);
        assert_eq!((groups[0].count, groups[0].ts), (1, 50));
        assert!(group(Vec::new(), ME).is_empty());
    }

    #[test]
    fn tabs_show_their_kinds() {
        let mut quote_mentioning = event(Kind::Quote, 2, 10, Some("0xq1"));
        quote_mentioning.mentions_you = true;
        let groups = group(
            vec![
                event(Kind::Like, 1, 60, Some("0xa")),
                event(Kind::Reply, 2, 50, Some("0xr")),
                event(Kind::Mention, 3, 40, Some("0xm")),
                event(Kind::Quote, 4, 30, Some("0xq0")),
                quote_mentioning,
                event(Kind::Follow, 5, 20, None),
            ],
            ME,
        );
        let shown = |tab: Tab| groups.iter().filter(|g| tab.shows(g)).map(|g| g.kind).collect::<Vec<_>>();
        assert_eq!(shown(Tab::All).len(), 6);
        assert_eq!(shown(Tab::Mentions), [Kind::Mention, Kind::Quote]);
        assert_eq!(shown(Tab::Replies), [Kind::Reply]);
        assert_eq!(Tab::parse(Some("mentions")), Tab::Mentions);
        assert_eq!(Tab::parse(Some("replies")), Tab::Replies);
        assert_eq!(Tab::parse(Some("likes")), Tab::All);
        assert_eq!(Tab::parse(None), Tab::All);
        for tab in Tab::ALL {
            assert_eq!(Tab::parse(tab.query()), tab);
        }
    }

    #[test]
    fn counts_what_came_since_the_reader_last_looked() {
        let groups = group((1..=12).map(|n| event(Kind::Follow, n, n as i64 * 86_400, None)).collect::<Vec<_>>(), ME);
        assert_eq!(unread(&groups, 0), 12);
        assert_eq!(unread(&groups, 10 * 86_400), 2);
        assert_eq!(unread(&groups, 12 * 86_400), 0);
        // The bell reads the times alone, from the same payload.
        let payload = NotificationsPayload {
            groups: groups.clone(),
            next: Some("eyJ".into()),
            newest: 12 * 86_400,
            notes: Annotations::default(),
            meta: Meta::default(),
        };
        let times: Times = serde_json::from_str(&serde_json::to_string(&payload).unwrap()).unwrap();
        assert_eq!((times.groups.len(), times.newest), (12, 12 * 86_400));
        assert_eq!(times.unread(10 * 86_400), unread(&groups, 10 * 86_400));
        assert_eq!(serde_json::from_str::<Times>("{}").unwrap(), Times::default());
        assert_eq!(badge(0), None);
        assert_eq!(badge(3).as_deref(), Some("3"));
        assert_eq!(badge(9).as_deref(), Some("9"));
        assert_eq!(badge(10).as_deref(), Some("9+"));
        assert_eq!(bell_label(14), "Notifications, 14 unread");
        assert_eq!(bell_label(0), "Notifications, 0 unread");
    }

    #[test]
    fn a_recast_hypersnap_lists_as_a_like_is_found_by_its_time() {
        // Recorded on 21 October 2026: warpcast-com liked 0xd506… at
        // 09:01:08 and recasted it at 09:01:09; eth3 did both on 0x6fe6…
        // in the same second; Hypersnap listed all four as likes.
        let mut events = vec![
            event(Kind::Like, 434_449, 1_792_573_269, Some("0xd506")),
            event(Kind::Like, 434_449, 1_792_573_268, Some("0xd506")),
            event(Kind::Like, 635_889, 1_792_495_098, Some("0x6fe6")),
            event(Kind::Like, 635_889, 1_792_495_098, Some("0x6fe6")),
            event(Kind::Like, 340_885, 1_792_490_000, Some("0x6fe6")),
        ];
        let recasts = HashMap::from([
            ("0xd506".to_owned(), Recasts::read(vec![(434_449, 1_792_573_269)], 10)),
            ("0x6fe6".to_owned(), Recasts::read(vec![(635_889, 1_792_495_098)], 10)),
        ]);
        sort_reactions(&mut events, &recasts);
        let kinds: Vec<(u64, i64, Kind)> = events.iter().map(|e| (e.actor.fid, e.ts, e.kind)).collect();
        assert_eq!(
            kinds,
            [
                (434_449, 1_792_573_269, Kind::Recast),
                (434_449, 1_792_573_268, Kind::Like),
                (635_889, 1_792_495_098, Kind::Recast),
                (635_889, 1_792_495_098, Kind::Like),
                (340_885, 1_792_490_000, Kind::Like),
            ]
        );
        let groups = group(events, ME);
        let counts: Vec<(Kind, &str, u32)> =
            groups.iter().map(|g| (g.kind, g.cast.as_ref().unwrap().hash.as_str(), g.count)).collect();
        assert_eq!(
            counts,
            [
                (Kind::Recast, "0xd506", 1),
                (Kind::Like, "0xd506", 1),
                (Kind::Recast, "0x6fe6", 1),
                (Kind::Like, "0x6fe6", 2)
            ]
        );
    }

    #[test]
    fn a_recast_only_is_a_recast_and_unread_casts_fall_back_to_pairs() {
        let mut events = vec![
            // Read: recasted only.
            event(Kind::Like, 1, 1000, Some("0xread")),
            // Read, and no recast by them: two items are two likes' repeat.
            event(Kind::Like, 2, 900, Some("0xread")),
            event(Kind::Like, 2, 905, Some("0xread")),
            // Not read: of a pair, the later.
            event(Kind::Like, 3, 800, Some("0xunread")),
            event(Kind::Like, 3, 806, Some("0xunread")),
            event(Kind::Like, 4, 700, Some("0xunread")),
        ];
        let recasts = HashMap::from([("0xread".to_owned(), Recasts::read(vec![(1, 1000)], 10))]);
        sort_reactions(&mut events, &recasts);
        let kinds: Vec<(u64, i64, Kind)> = events.iter().map(|e| (e.actor.fid, e.ts, e.kind)).collect();
        assert_eq!(
            kinds,
            [
                (1, 1000, Kind::Recast),
                (2, 900, Kind::Like),
                (2, 905, Kind::Like),
                (3, 800, Kind::Like),
                (3, 806, Kind::Recast),
                (4, 700, Kind::Like),
            ]
        );
    }

    #[test]
    fn a_full_read_is_certain_only_down_to_its_page() {
        // Ten a shard read, twelve found: the newest ten are certain.
        let by: Vec<(u64, i64)> = (1..=12).map(|n| (n, n as i64 * 100)).collect();
        let read = Recasts::read(by, 10);
        assert_eq!(read.by[0], (12, 1200));
        assert_eq!(read.since, Some(300));
        assert!(read.rules_out(300) && !read.rules_out(299));
        // Fewer than a page: all of them, and each account once, newest.
        let read = Recasts::read(vec![(5, 10), (6, 30), (5, 20)], 10);
        assert_eq!((read.by, read.since), (vec![(6, 30), (5, 20)], None));
        // An older pair under a full read's page still falls back to pairs.
        let mut events = vec![event(Kind::Like, 99, 50, Some("0xbusy")), event(Kind::Like, 99, 55, Some("0xbusy"))];
        let full = Recasts::read((1..=10).map(|n| (n, 1000 + n as i64)).collect(), 10);
        sort_reactions(&mut events, &HashMap::from([("0xbusy".to_owned(), full)]));
        assert_eq!(events.iter().map(|e| e.kind).collect::<Vec<_>>(), [Kind::Like, Kind::Recast]);
    }

    #[test]
    fn reads_the_recasts_only_of_casts_that_may_hide_some() {
        let events = vec![
            event(Kind::Like, 1, 900, Some("0xpaired")),
            event(Kind::Like, 1, 901, Some("0xpaired")),
            event(Kind::Like, 2, 800, Some("0xnone")),
            event(Kind::Like, 3, 700, Some("0xunknown")),
            event(Kind::Like, 4, 600, Some("0xmore")),
            event(Kind::Like, 4, 601, Some("0xmore")),
            event(Kind::Reply, 5, 950, Some("0xreply")),
            event(Kind::Follow, 6, 990, None),
        ];
        let known = HashMap::from([
            // One account twice, one recast: that was it.
            ("0xpaired".to_owned(), 1),
            ("0xnone".to_owned(), 0),
            // Two recasts, one account twice: someone recasted without a like here.
            ("0xmore".to_owned(), 2),
        ]);
        assert_eq!(recasts_to_read(&events, &known, 6), ["0xunknown", "0xmore"]);
        assert_eq!(recasts_to_read(&events, &known, 1), ["0xunknown"]);
        assert_eq!(recasts_to_read(&events, &HashMap::new(), 6), ["0xpaired", "0xnone", "0xunknown", "0xmore"]);
    }

    #[test]
    fn quotes_are_read_for_the_newest_casts_that_arent_replies() {
        let mut casts: Vec<Cast> = (0..6).map(|n| Cast { ts: n, ..cast(&format!("0x{n}"), ME, "") }).collect();
        casts[5].parent_hash = Some("0xparent".into());
        casts[4].parent_hash = Some("0xparent".into());
        let hashes = |limit| quoted_casts(&casts, limit).iter().map(|c| c.hash.clone()).collect::<Vec<_>>();
        assert_eq!(hashes(4), ["0x3", "0x2", "0x1", "0x0"]);
        assert_eq!(hashes(6), ["0x3", "0x2", "0x1", "0x0", "0x5", "0x4"]);
        assert!(quoted_casts(&[], 4).is_empty());
    }

    #[test]
    fn quotes_read_on_top_keep_to_the_page() {
        let extra = vec![event(Kind::Quote, 1, 500, Some("0xnew")), event(Kind::Quote, 2, 50, Some("0xold"))];
        let kept = |oldest, more| within_page(extra.clone(), oldest, more).iter().map(|e| e.ts).collect::<Vec<_>>();
        assert_eq!(kept(Some(100), true), [500]);
        assert_eq!(kept(Some(100), false), [500, 50]);
        assert_eq!(kept(None, true), [500, 50]);
    }

    #[test]
    fn excerpts_say_what_a_cast_without_text_has() {
        let mut image = cast("0xi", ME, "  ");
        image.embeds.push(Embed::Image { url: "https://imagedelivery.net/a/b/original".into() });
        assert_eq!(excerpt(&image), "Image");
        let mut video = cast("0xv", ME, "");
        video.embeds.push(Embed::Video { url: "https://stream.farcaster.xyz/v/1/manifest.m3u8".into() });
        assert_eq!(excerpt(&video), "Video");
        assert_eq!(excerpt(&cast("0xe", ME, "")), "");
        let long = cast("0xl", ME, &"word ".repeat(60));
        assert!(excerpt(&long).ends_with('…') && excerpt(&long).chars().count() <= EXCERPT_CHARS + 1);
        // An emoji comes whole.
        assert_eq!(excerpt(&cast("0xp", ME, "Casting from Allthing :pepe_smug:")), "Casting from Allthing :pepe_smug:");
    }

    #[test]
    fn paths_and_cursors() {
        assert_eq!(notifications_path(None), "/api/v1/notifications");
        let cursor = "eyJiZWZvcmVfdGltZXN0YW1wIjoxODMwMjk1MDJ9";
        assert_eq!(notifications_path(Some(cursor)), format!("/api/v1/notifications?cursor={cursor}"));
        assert_eq!(notifications_path(Some("a+b/c=")), "/api/v1/notifications?cursor=a%2Bb%2Fc%3D");
        assert_eq!(cursor_from_param(&format!(" {cursor} ")).as_deref(), Some(cursor));
        assert_eq!(cursor_from_param("a+b/c=").as_deref(), Some("a+b/c="));
        for bad in ["", "   ", "a b", "<script>", "a&fid=3", &"a".repeat(MAX_CURSOR + 1)] {
            assert_eq!(cursor_from_param(bad), None, "{bad}");
        }
    }

    #[test]
    fn payloads_round_trip_without_empty_fields() {
        let payload = NotificationsPayload {
            groups: group(vec![event(Kind::Follow, 1, 10, None), event(Kind::Like, 2, 20, Some("0xa"))], ME),
            next: None,
            newest: 20,
            notes: Annotations::default(),
            meta: Meta::default(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("next") && !json.contains("mentions_you") && !json.contains("\"notes\":{\""));
        assert!(json.contains(r#""kind":"like""#) && json.contains(r#""kind":"follow""#), "{json}");
        assert_eq!(serde_json::from_str::<NotificationsPayload>(&json).unwrap(), payload);
    }
}
