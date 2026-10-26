// SPDX-License-Identifier: AGPL-3.0-or-later
//! Drafts the composer keeps in the browser, as the Farcaster app's
//! composer does: one for each thing it is for (a reply to a cast, a quote
//! of one, a cast in a channel or on the front page), so starting another
//! never loses one, and a reload doesn't either. Each holds what the
//! composer had (its text, its uploaded files' URLs, its GIF or sticker,
//! and for a reply, what it showed of the cast it answers) and when it last
//! changed. They last 14 days, and the 30 newest are kept.

use serde::{Deserialize, Serialize};

use crate::compose::ParentPreview;
use crate::scheduled::SavedDraft;

/// Where the browser keeps them (`localStorage`).
pub const DRAFTS_KEY: &str = "allthing-drafts";
/// How long one is kept since it last changed, in seconds.
pub const KEEP: i64 = 14 * 86_400;
/// How many are kept: past this, the oldest go.
pub const MAX_DRAFTS: usize = 30;

/// A draft the browser keeps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptDraft {
    /// What it is for ([`key_of`]).
    pub key: String,
    pub draft: SavedDraft,
    /// For a reply, what its composer showed of the cast it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<ParentPreview>,
    /// When it last changed, in Unix seconds.
    pub updated_at: i64,
}

impl KeptDraft {
    /// What it is, for the list of drafts: "Reply to @alice", "Quote of
    /// @bob", "Cast in /books" or "Cast".
    pub fn label(&self) -> String {
        match (&self.draft.reply, &self.draft.quote, &self.draft.channel) {
            (Some(to), _, _) => format!("Reply to @{}", to.username),
            (None, Some(of), _) => format!("Quote of @{}", of.username),
            (None, None, Some(channel)) => format!("Cast in /{channel}"),
            (None, None, None) => "Cast".to_owned(),
        }
    }
}

/// What a draft is for, as it is kept: `reply:<hash>`, `quote:<hash>`, or
/// `cast:<channel>` (`cast:home` for the front page's).
pub fn key_of(draft: &SavedDraft) -> String {
    match (&draft.reply, &draft.quote) {
        (Some(to), _) => format!("reply:{}", to.hash.to_ascii_lowercase()),
        (None, Some(of)) => format!("quote:{}", of.hash.to_ascii_lowercase()),
        (None, None) => format!("cast:{}", draft.channel.as_deref().unwrap_or("home")),
    }
}

/// Whether a draft holds nothing worth keeping: no text, no files, no GIF.
pub fn is_empty(draft: &SavedDraft) -> bool {
    draft.text.trim().is_empty() && draft.media.is_empty() && draft.gif.is_none()
}

/// The browser's drafts, newest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Drafts {
    #[serde(default)]
    pub drafts: Vec<KeptDraft>,
}

impl Drafts {
    /// As the browser keeps them, at `now` (Unix seconds): those older than
    /// [`KEEP`] gone, and any that can't be read left out.
    pub fn parse(json: &str, now: i64) -> Drafts {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            drafts: Vec<serde_json::Value>,
        }
        let raw: Raw = serde_json::from_str(json).unwrap_or(Raw { drafts: Vec::new() });
        let mut drafts: Vec<KeptDraft> = raw
            .drafts
            .into_iter()
            .filter_map(|kept| serde_json::from_value::<KeptDraft>(kept).ok())
            .filter(|kept| now - kept.updated_at < KEEP && !is_empty(&kept.draft))
            .collect();
        drafts.sort_by_key(|kept| std::cmp::Reverse(kept.updated_at));
        Drafts { drafts }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The draft kept for `key`.
    pub fn get(&self, key: &str) -> Option<&KeptDraft> {
        self.drafts.iter().find(|kept| kept.key == key)
    }

    /// Keep `kept` in the place of the one for its key, or, if it holds
    /// nothing, let that one go; the newest [`MAX_DRAFTS`] stay. Newest
    /// first, `kept` goes in by when it changed, ahead of any as new (no
    /// sort, which would cost the app's first part its code).
    pub fn put(&mut self, kept: KeptDraft) {
        self.drafts.retain(|other| other.key != kept.key);
        if !is_empty(&kept.draft) {
            let at = self.drafts.partition_point(|other| other.updated_at > kept.updated_at);
            self.drafts.insert(at, kept);
            self.drafts.truncate(MAX_DRAFTS);
        }
    }

    /// Let the draft for `key` go; whether there was one.
    pub fn remove(&mut self, key: &str) -> bool {
        let before = self.drafts.len();
        self.drafts.retain(|kept| kept.key != key);
        self.drafts.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduled::{SavedCast, SavedGif, SavedMedia};

    fn reply(hash: &str, text: &str, at: i64) -> KeptDraft {
        let draft = SavedDraft {
            text: text.into(),
            reply: Some(SavedCast {
                hash: hash.into(),
                fid: 42,
                username: "alice".into(),
                excerpt: "What next?".into(),
                ..SavedCast::default()
            }),
            ..SavedDraft::default()
        };
        KeptDraft { key: key_of(&draft), draft, preview: None, updated_at: at }
    }

    #[test]
    fn each_draft_is_kept_for_what_it_is_for() {
        let quote = SavedDraft {
            quote: Some(SavedCast {
                hash: "0xAB".into(),
                fid: 7,
                username: "bob".into(),
                excerpt: String::new(),
                ..SavedCast::default()
            }),
            ..SavedDraft::default()
        };
        let books = SavedDraft { channel: Some("books".into()), ..SavedDraft::default() };
        assert_eq!(key_of(&reply("0xCD", "", 0).draft), "reply:0xcd");
        assert_eq!(key_of(&quote), "quote:0xab");
        assert_eq!(key_of(&books), "cast:books");
        assert_eq!(key_of(&SavedDraft::default()), "cast:home");
        let label = |draft: SavedDraft| KeptDraft { key: key_of(&draft), draft, preview: None, updated_at: 0 }.label();
        assert_eq!(label(reply("0x1", "", 0).draft), "Reply to @alice");
        assert_eq!(label(quote), "Quote of @bob");
        assert_eq!(label(books), "Cast in /books");
        assert_eq!(label(SavedDraft::default()), "Cast");
    }

    #[test]
    fn one_draft_a_target_the_newest_first_thirty_at_most_and_none_empty() {
        let mut drafts = Drafts::default();
        drafts.put(reply("0x1", "first", 100));
        drafts.put(reply("0x2", "second", 200));
        // The same reply again takes its place, first.
        drafts.put(reply("0x1", "first, changed", 300));
        let keys: Vec<&str> = drafts.drafts.iter().map(|kept| kept.key.as_str()).collect();
        assert_eq!(keys, ["reply:0x1", "reply:0x2"]);
        assert_eq!(drafts.get("reply:0x1").unwrap().draft.text, "first, changed");
        // Emptied, it goes; a GIF or a file alone is worth keeping.
        drafts.put(reply("0x1", "  ", 400));
        assert!(drafts.get("reply:0x1").is_none());
        let mut gif = reply("0x3", "", 500);
        gif.draft.gif =
            Some(SavedGif { title: "cat".into(), embed: "e".into(), tile: "t".into(), width: 1, height: 1 });
        drafts.put(gif);
        let mut file = reply("0x4", "", 600);
        file.draft.media =
            vec![SavedMedia { url: "https://allthing.social/m/1/a.png".into(), media_type: "image/png".into() }];
        drafts.put(file);
        assert_eq!(drafts.drafts.len(), 3);
        // Thirty at most: the oldest go.
        for n in 0..40 {
            drafts.put(reply(&format!("0x{:02x}", n + 16), "many", 1_000 + n));
        }
        assert_eq!(drafts.drafts.len(), MAX_DRAFTS);
        assert_eq!(drafts.drafts[0].updated_at, 1_039);
        assert_eq!(drafts.drafts.last().unwrap().updated_at, 1_010);
        assert!(drafts.remove("reply:0x1a"));
        assert!(!drafts.remove("reply:0x1a"));
        // One older than the newest goes in its place by when it changed,
        // ahead of one as new.
        drafts.put(reply("0xolder", "older", 1_020));
        let at = drafts.drafts.iter().position(|kept| kept.key == "reply:0xolder").unwrap();
        assert_eq!((drafts.drafts[at - 1].updated_at, drafts.drafts[at + 1].updated_at), (1_021, 1_020));
        assert_eq!(drafts.drafts[at + 1].key, "reply:0x24");
    }

    #[test]
    fn kept_drafts_last_fourteen_days_and_survive_a_bad_read() {
        let mut drafts = Drafts::default();
        drafts.put(reply("0x1", "old", 1_000));
        drafts.put(reply("0x2", "recent", 1_000 + KEEP - 10));
        let json = drafts.to_json();
        let read = Drafts::parse(&json, 1_000 + KEEP);
        assert_eq!(read.drafts.iter().map(|kept| kept.draft.text.as_str()).collect::<Vec<_>>(), ["recent"]);
        assert_eq!(Drafts::parse(&json, 1_001).drafts.len(), 2);
        assert_eq!(Drafts::parse("not json", 0), Drafts::default());
        // One that can't be read is left out, and the rest kept.
        let one = r#"{"drafts":[{"key":"x"},{"key":"reply:0x2","draft":{"text":"kept"},"updated_at":3}]}"#;
        assert_eq!(
            Drafts::parse(one, 4).drafts.iter().map(|kept| kept.key.as_str()).collect::<Vec<_>>(),
            ["reply:0x2"]
        );
        // What a reply showed of its cast comes back with it.
        let mut shown = reply("0x9", "with its cast", 5);
        shown.preview =
            Some(ParentPreview { name: "Alice".into(), ts: 4, text: "What next?".into(), ..ParentPreview::default() });
        drafts.put(shown.clone());
        assert_eq!(Drafts::parse(&drafts.to_json(), 6).get("reply:0x9"), Some(&shown));
    }
}
