// SPDX-License-Identifier: AGPL-3.0-or-later
//! What a profile card says about an account beyond its picture and name: a
//! short bio, follower counts and when it registered.
//!
//! Hypersnap sends each cast's author as a full user object, so feeds carry
//! these facts for their authors in the payload (`Annotations::users`) at no
//! extra request. Bios are cut short there to keep payloads small.

use serde::{Deserialize, Serialize};

use crate::model::User;
use crate::text::{Token, tokenize};
use crate::time::{duration_since, format_date};

/// Characters of bio a card shows; a longer bio is cut between words.
pub const CARD_BIO_CHARS: usize = 160;

/// The day (UTC) the ID registry moved to OP Mainnet, 7 Nov 2023. Accounts
/// registered before it report that day as their registration: it is the
/// earliest date onchain data has for them.
pub const ID_REGISTRY_MIGRATION_DAY: std::ops::Range<i64> = 1_699_315_200..1_699_401_600;

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
    /// Unix seconds of the onchain ID registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<i64>,
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
        }
    }
}

/// How profiles state an account's registration: a short date and a longer
/// note for its tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    /// "17 Oct 2026", or "before Nov 2023" for accounts registered before
    /// the ID registry's migration.
    pub date: String,
    pub note: String,
}

impl Registered {
    pub fn at(ts: i64, now: i64) -> Registered {
        if ID_REGISTRY_MIGRATION_DAY.contains(&ts) {
            Registered {
                date: "before Nov 2023".into(),
                note: "Registered before the ID registry moved to OP Mainnet on 7 Nov 2023, which is the earliest \
                       date onchain data reports."
                    .into(),
            }
        } else {
            Registered { date: format_date(ts), note: format!("Registered onchain {} ago", duration_since(ts, now)) }
        }
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

    #[test]
    fn registration_on_the_migration_day_reads_before_nov_2023() {
        let now = 1_792_235_681;
        // dwr's reported registration, 2023-11-07T19:42:51Z.
        let migrated = Registered::at(1_699_386_171, now);
        assert_eq!(migrated.date, "before Nov 2023");
        assert!(migrated.note.contains("7 Nov 2023"));
        assert_eq!(Registered::at(ID_REGISTRY_MIGRATION_DAY.start, now).date, "before Nov 2023");
        assert_eq!(Registered::at(ID_REGISTRY_MIGRATION_DAY.end - 1, now).date, "before Nov 2023");
        // The days around it are real registration dates.
        assert_eq!(Registered::at(ID_REGISTRY_MIGRATION_DAY.end, now).date, "8 Nov 2023");
        assert_eq!(Registered::at(ID_REGISTRY_MIGRATION_DAY.start - 1, now).date, "6 Nov 2023");
        let recent = Registered::at(1_759_222_921, now);
        assert_eq!(recent.date, "30 Sep 2025");
        assert_eq!(recent.note, "Registered onchain 1 year ago");
    }

    #[test]
    fn cards_of_full_profiles_cut_the_bio() {
        let user: User = serde_json::from_str(&format!(
            r#"{{"fid":3,"username":"dwr","bio":"{}","follower_count":10,"following_count":2,"registered_at":1699386171}}"#,
            "gm ".repeat(100)
        ))
        .unwrap();
        let card = UserCard::of(&user);
        assert_eq!((card.followers, card.following, card.registered_at), (10, 2, Some(1_699_386_171)));
        assert!(card.bio.chars().count() <= CARD_BIO_CHARS + 1 && card.bio.ends_with('…'));
    }

    #[test]
    fn cards_serialize_compactly_and_read_back() {
        let card = UserCard { bio: "gm".into(), followers: 5, following: 0, registered_at: None };
        let json = serde_json::to_string(&card).unwrap();
        assert_eq!(json, r#"{"bio":"gm","followers":5}"#);
        assert_eq!(serde_json::from_str::<UserCard>(&json).unwrap(), card);
        assert_eq!(serde_json::to_string(&UserCard::default()).unwrap(), "{}");
    }
}
