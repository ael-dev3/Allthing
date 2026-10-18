// SPDX-License-Identifier: AGPL-3.0-or-later
//! What a profile card says about an account beyond its picture and names:
//! the start of its bio, its follower counts and when it joined.
//!
//! Hypersnap sends each cast's author as a full user object, so payloads
//! carry these facts for the accounts they show
//! ([`crate::api::Annotations::users`]) without another request. Bios are
//! cut short there to keep payloads small.

use serde::{Deserialize, Serialize};

use crate::model::{Joined, User, UserSummary};
use crate::text::{Token, tokenize};

/// Characters of bio a card shows; a longer bio is cut between words.
pub const CARD_BIO_CHARS: usize = 160;

/// A profile card's facts about one account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCard {
    /// The start of the bio ([`bio_excerpt`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bio: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub followers: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub following: u64,
    /// Unix seconds of the onchain ID registration the provider reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<i64>,
    /// Unix seconds at which the account got its first username. Only
    /// profiles read it ([`User::first_username_at`]), so cards made from
    /// cast authors never have it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_username_at: Option<i64>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl UserCard {
    /// The card of a full profile.
    pub fn of(user: &User) -> UserCard {
        UserCard {
            bio: bio_excerpt(&user.bio, CARD_BIO_CHARS),
            followers: user.follower_count,
            following: user.following_count,
            registered_at: user.registered_at,
            first_username_at: user.first_username_at,
        }
    }

    /// When the account joined, read as its profile reads it.
    pub fn joined(&self) -> Option<Joined> {
        Joined::from_dates(self.registered_at, self.first_username_at)
    }

    /// This card, completed with what `other` (an earlier card of the same
    /// account) knows that it doesn't: when the account got its first
    /// username, which only its profile reads.
    pub fn filled_from(mut self, other: Option<&UserCard>) -> UserCard {
        if self.first_username_at.is_none() {
            self.first_username_at = other.and_then(|o| o.first_username_at);
        }
        self
    }
}

/// What a profile can show before its payload arrives, from what the visit
/// has seen of the account: its picture and names, from a cast it wrote,
/// and its card, if a payload carried one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfilePreview {
    /// The account as far as it is known: no addresses; the card's bio (its
    /// start) and counts.
    pub user: User,
    /// Whether the bio and counts are known. Without a card the profile
    /// shows placeholders for them, never an empty bio or zero followers.
    pub has_card: bool,
    /// When the account joined, if the card already says what the profile
    /// will: a card without the date of the first username of an account
    /// registered before the ID registry's migration would say "before Nov
    /// 2023", which the profile then corrects, so it says nothing yet.
    pub joined: Option<Joined>,
}

impl ProfilePreview {
    pub fn new(author: &UserSummary, card: Option<&UserCard>) -> ProfilePreview {
        let user = User {
            fid: author.fid,
            username: author.username.clone(),
            display_name: author.display_name.clone(),
            pfp_url: author.pfp_url.clone(),
            bio: card.map(|c| c.bio.clone()).unwrap_or_default(),
            follower_count: card.map_or(0, |c| c.followers),
            following_count: card.map_or(0, |c| c.following),
            registered_at: card.and_then(|c| c.registered_at),
            first_username_at: card.and_then(|c| c.first_username_at),
            custody_address: None,
            eth_addresses: Vec::new(),
            sol_addresses: Vec::new(),
            primary_eth: None,
            primary_sol: None,
        };
        let joined = card.and_then(UserCard::joined).filter(|j| *j != Joined::BeforeMigration);
        ProfilePreview { user, has_card: card.is_some(), joined }
    }
}

/// The start of a bio, for a card: whitespace collapsed to single spaces, at
/// most `max` characters, cut between words and ended with "…". A link is
/// never cut, so what remains still links where it should; a first word too
/// long to fit is cut only when it holds no link.
pub fn bio_excerpt(bio: &str, max: usize) -> String {
    let mut out = String::new();
    let mut length = 0;
    for word in bio.split_whitespace() {
        let chars = word.chars().count();
        let space = usize::from(!out.is_empty());
        if length + space + chars <= max {
            if space == 1 {
                out.push(' ');
            }
            out.push_str(word);
            length += space + chars;
            continue;
        }
        if out.is_empty() {
            if links_somewhere(word) {
                // A long link alone: keep it whole (bios are at most 256 bytes).
                return word.to_owned();
            }
            out = word.chars().take(max).collect();
        }
        return with_ellipsis(out);
    }
    out
}

/// Whether `word` holds a URL, a /channel or an @name, which a cut would
/// turn into a different link.
fn links_somewhere(word: &str) -> bool {
    word.contains("://") || word.contains('/') || word.contains('@')
}

fn with_ellipsis(text: String) -> String {
    let base = text.trim_end_matches(|c: char| c.is_whitespace() || ".,;:!?…-".contains(c));
    // An ellipsis right after a URL would become part of it.
    let ends_in_url = matches!(tokenize(base, &[]).last(), Some(Token::Url(_)));
    format!("{base}{}…", if ends_in_url { " " } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ID_REGISTRY_MIGRATION_DAY;

    #[test]
    fn short_bios_stay_whole_on_one_line() {
        assert_eq!(bio_excerpt("", CARD_BIO_CHARS), "");
        assert_eq!(bio_excerpt("  \n ", CARD_BIO_CHARS), "");
        assert_eq!(
            bio_excerpt("Author of the Network State.\nFounder of the Network School.", CARD_BIO_CHARS),
            "Author of the Network State. Founder of the Network School."
        );
    }

    #[test]
    fn long_bios_are_cut_between_words() {
        let bio = "word ".repeat(60);
        let excerpt = bio_excerpt(&bio, CARD_BIO_CHARS);
        assert!(excerpt.ends_with("word…"), "{excerpt}");
        assert!(excerpt.chars().count() <= CARD_BIO_CHARS + 1);
        // Trailing punctuation gives way to the ellipsis.
        assert_eq!(bio_excerpt("one two, three four", 9), "one two…");
        // Text without spaces is cut by characters.
        assert_eq!(bio_excerpt(&"字".repeat(200), 10), format!("{}…", "字".repeat(10)));
    }

    #[test]
    fn links_are_never_cut() {
        let excerpt = bio_excerpt("building https://example.com/a/very/long/path/to/a/page today", 30);
        assert_eq!(excerpt, "building…");
        // A link that fits keeps a space before the ellipsis, which would
        // otherwise read as part of the URL.
        let excerpt = bio_excerpt("see https://example.com and more words here", 24);
        assert_eq!(excerpt, "see https://example.com …");
        assert!(tokenize(&excerpt, &[]).contains(&Token::Url("https://example.com".into())));
        let link = format!("https://example.com/{}", "x".repeat(200));
        assert_eq!(bio_excerpt(&format!("{link} after"), 50), link);
        assert_eq!(bio_excerpt(&format!("/{} and", "c".repeat(60)), 20), format!("/{}", "c".repeat(60)));
    }

    fn dwr(registered_at: Option<i64>, first_username_at: Option<i64>) -> User {
        let mut user: User = serde_json::from_str(&format!(
            r#"{{"fid":3,"username":"dwr","bio":"{}","follower_count":10,"following_count":2}}"#,
            "gm ".repeat(100)
        ))
        .unwrap();
        user.registered_at = registered_at;
        user.first_username_at = first_username_at;
        user
    }

    #[test]
    fn cards_of_full_profiles_cut_the_bio_and_keep_the_join_date() {
        // dwr's reported registration (the migration) and his first fname.
        let card = UserCard::of(&dwr(Some(1_699_386_171), Some(1_623_888_000)));
        assert_eq!((card.followers, card.following), (10, 2));
        assert!(card.bio.chars().count() <= CARD_BIO_CHARS + 1 && card.bio.ends_with('…'));
        assert_eq!(card.joined(), Some(Joined::FirstUsername(1_623_888_000)));
        // As on the profile: without the first username's date, before Nov 2023.
        let card = UserCard::of(&dwr(Some(1_699_386_171), None));
        assert_eq!(card.joined(), Some(Joined::BeforeMigration));
        assert_eq!(UserCard::of(&dwr(None, None)).joined(), None);
        assert_eq!(UserCard::of(&dwr(Some(1_759_222_921), None)).joined(), Some(Joined::Registered(1_759_222_921)));
    }

    #[test]
    fn a_card_takes_the_first_username_date_an_earlier_card_knew() {
        let migrated = ID_REGISTRY_MIGRATION_DAY.start + 3600;
        let feed = UserCard { followers: 12, registered_at: Some(migrated), ..Default::default() };
        let profile = UserCard::of(&dwr(Some(migrated), Some(1_623_888_000)));
        let filled = feed.clone().filled_from(Some(&profile));
        assert_eq!(filled.joined(), Some(Joined::FirstUsername(1_623_888_000)));
        // The newer card's own facts stay.
        assert_eq!(filled.followers, 12);
        assert_eq!(feed.clone().filled_from(None), feed);
        assert_eq!(profile.clone().filled_from(Some(&UserCard::default())), profile);
    }

    fn author() -> UserSummary {
        UserSummary {
            fid: 3,
            username: "dwr".into(),
            display_name: "Dan Romero".into(),
            pfp_url: Some("https://example.com/dwr.png".into()),
        }
    }

    #[test]
    fn a_preview_shows_the_author_and_the_card_it_has() {
        let card = UserCard {
            bio: "Working on Farcaster…".into(),
            followers: 500_000,
            following: 2_000,
            registered_at: Some(1_759_222_921),
            first_username_at: None,
        };
        let preview = ProfilePreview::new(&author(), Some(&card));
        assert!(preview.has_card);
        assert_eq!(preview.user.summary(), author());
        assert_eq!(preview.user.bio, "Working on Farcaster…");
        assert_eq!((preview.user.follower_count, preview.user.following_count), (500_000, 2_000));
        assert_eq!(preview.joined, Some(Joined::Registered(1_759_222_921)));
        assert!(preview.user.eth_addresses.is_empty() && preview.user.sol_addresses.is_empty());
    }

    #[test]
    fn a_preview_without_a_card_claims_no_bio_or_counts() {
        let preview = ProfilePreview::new(&author(), None);
        assert!(!preview.has_card);
        assert_eq!(preview.user.summary(), author());
        assert_eq!(preview.joined, None);
    }

    #[test]
    fn a_preview_leaves_out_a_join_date_the_profile_would_correct() {
        let migrated = ID_REGISTRY_MIGRATION_DAY.start + 3600;
        // A cast author's card: registered "on" the migration day, no first username.
        let card = UserCard { registered_at: Some(migrated), ..Default::default() };
        assert_eq!(card.joined(), Some(Joined::BeforeMigration));
        assert_eq!(ProfilePreview::new(&author(), Some(&card)).joined, None);
        // A card from a profile seen earlier knows the first username's date.
        let card = UserCard { registered_at: Some(migrated), first_username_at: Some(1_623_888_000), ..card };
        assert_eq!(ProfilePreview::new(&author(), Some(&card)).joined, Some(Joined::FirstUsername(1_623_888_000)));
    }

    #[test]
    fn cards_serialize_compactly_and_read_back() {
        let card = UserCard { bio: "gm".into(), followers: 5, ..Default::default() };
        let json = serde_json::to_string(&card).unwrap();
        assert_eq!(json, r#"{"bio":"gm","followers":5}"#);
        assert_eq!(serde_json::from_str::<UserCard>(&json).unwrap(), card);
        assert_eq!(serde_json::to_string(&UserCard::default()).unwrap(), "{}");
    }
}
