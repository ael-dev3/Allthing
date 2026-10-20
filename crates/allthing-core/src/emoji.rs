// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's emoji: a pack of Pepe the Frog emotes from 7TV, FrankerFaceZ
//! and emoji.gg, which Allthing serves itself (`web/emoji/v1/`, credited
//! with their sources in `web/emoji/CREDITS.md`). A cast writes one as a
//! shortcode, `:pepe_no:`, which Allthing shows as the emote; other
//! Farcaster apps show the shortcode as text.
//!
//! Some of the animated ones are stickers too: larger GIFs
//! (`web/stickers/v1/`, credited in `web/stickers/CREDITS.md`) that a cast
//! carries as an image embed, which every Farcaster app shows.

/// Where the pack's files are served, below the site's base path. They are
/// cached for a year, so a changed file goes in a folder of its own (`v2`).
pub const DIR: &str = "/assets/emoji/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Emoji {
    /// Its shortcode, without the colons.
    pub code: &'static str,
    /// What Pepe does in it, after "Pepe": "holds up a NO sign".
    pub what: &'static str,
    /// Animated, with a still (`.still.webp`) for readers who ask for less
    /// motion.
    pub animated: bool,
    /// Its size in pixels, twice what it shows at in cast text.
    pub width: u16,
    pub height: u16,
}

impl Emoji {
    /// Its file, below the site's base path.
    pub fn file(&self) -> String {
        format!("{DIR}/{}.webp", self.code)
    }

    /// A still of it: its file, unless it is animated.
    pub fn still(&self) -> String {
        if self.animated { format!("{DIR}/{}.still.webp", self.code) } else { self.file() }
    }

    /// What it shows, for a tooltip or a picker: "Pepe holds up a NO sign".
    pub fn label(&self) -> String {
        format!("Pepe {}", self.what)
    }
}

const fn e(code: &'static str, what: &'static str, animated: bool, width: u16, height: u16) -> Emoji {
    Emoji { code, what, animated, width, height }
}

/// The pack, in the order a picker shows it: close matches for the five
/// Pepes Allthing was asked for first, then the rest, animated before still.
pub const PACK: &[Emoji] = &[
    e("pepe_no", "holds up a NO sign", false, 73, 64),
    e("pepe_smug", "looks smug", false, 64, 64),
    e("pepe_blush", "blushes", false, 64, 64),
    e("pepe_heart_hug", "hugs a big heart", false, 82, 64),
    e("pepe_sit", "sits, a small blob", false, 64, 64),
    e("pepe_nopers", "shakes his head no", true, 64, 64),
    e("pepe_shy", "blushes and looks away", true, 64, 64),
    e("pepe_love", "holds a heart", true, 64, 64),
    e("pepe_hug", "hugs", true, 82, 64),
    e("pepe_sit_clap", "sits and claps", true, 64, 64),
    e("pepe_comfy", "snuggles under a blanket", true, 64, 64),
    e("pepe_smirk", "smirks", false, 64, 64),
    e("pepe_laugh", "laughs", true, 64, 64),
    e("pepe_giggle", "giggles", true, 64, 64),
    e("pepe_pls", "dances", true, 46, 64),
    e("pepe_jam", "jams with headphones", true, 66, 64),
    e("pepe_bounce", "bounces", true, 64, 64),
    e("pepe_spin", "spins", true, 64, 64),
    e("pepe_shake", "shakes", true, 64, 64),
    e("pepe_yes", "nods yes", true, 64, 64),
    e("pepe_hands", "cries into his hands", true, 64, 64),
    e("pepe_vanish", "vanishes", true, 118, 64),
    e("pepe_blanket", "wraps up in a blanket", true, 64, 64),
    e("pepe_huggers", "opens his arms", true, 64, 64),
    e("pepe_kiss", "blows kisses", true, 64, 64),
    e("pepe_wink", "winks", true, 64, 64),
    e("pepe_headbang", "headbangs", true, 64, 64),
    e("pepe_tea", "sips tea", true, 64, 64),
    e("pepe_boba", "holds a BOBA sign", true, 64, 64),
    e("pepe_eat", "eats", true, 64, 64),
    e("pepe_cookie", "eats a cookie", true, 64, 64),
    e("pepe_pizza", "eats pizza", true, 64, 64),
    e("pepe_money", "throws money", true, 64, 64),
    e("pepe_smash", "smashes the keyboard", true, 64, 64),
    e("pepe_hey", "waves hey", true, 64, 64),
    e("pepe_think", "thinks", true, 64, 64),
    e("pepe_scared", "is scared", true, 64, 64),
    e("pepe_sleep", "sleeps", true, 64, 64),
    e("pepe_bye", "waves bye", true, 64, 64),
    e("pepe_read", "reads", true, 94, 64),
    e("pepe_typing", "types", true, 64, 64),
    e("pepe_writing", "writes", true, 98, 64),
    e("pepe_phone", "talks on the phone", true, 62, 64),
    e("pepe_tub", "takes a bath", true, 64, 64),
    e("pepe_birthday", "blows a party horn", true, 64, 64),
    e("pepe_side_eye", "gives a side-eye", true, 64, 64),
    e("pepe_cool", "wears sunglasses", true, 64, 64),
    e("pepe_run", "runs", true, 64, 64),
    e("pepe_clap", "claps", true, 64, 64),
    e("pepe_glad", "is glad", true, 64, 64),
    e("pepe_smile", "smiles", true, 64, 64),
    e("pepe_wow", "says wow", true, 64, 64),
    e("pepe_point", "points and laughs", true, 64, 64),
    e("pepe_cutie", "holds a CUTIE sign", true, 64, 64),
    e("pepe_flower", "offers a flower", true, 64, 64),
    e("pepe_cheer", "cheers with pom-poms", true, 64, 64),
    e("pepe_strong", "cries, but stays strong", true, 64, 64),
    e("pepe_salute", "salutes", true, 64, 64),
    e("pepe_shrug", "shrugs", true, 64, 64),
    e("pepe_exit", "heads for the exit", true, 64, 64),
    e("pepe_hi", "says hi", true, 64, 64),
    e("pepe_gn", "says good night", true, 64, 64),
    e("pepe_gm", "says good morning", true, 64, 64),
    e("pepe_gg", "holds a GG sign", true, 64, 64),
    e("pepe_thanks", "says thanks", true, 64, 64),
    e("pepe_pat", "pats a heart", true, 64, 64),
    e("pepe_drama", "holds a DRAMA sign", true, 64, 64),
    e("pepe_wave", "waves", true, 64, 64),
    e("pepe_arrive", "arrives", true, 64, 64),
    e("pepe_leave", "leaves", true, 64, 64),
    e("pepe_party", "parties", true, 74, 64),
    e("pepe_dance", "dances", true, 64, 64),
    e("pepe_coffee", "drinks coffee", true, 64, 64),
    e("pepe_cry", "cries", true, 64, 64),
    e("pepe_talk", "talks", true, 64, 64),
    e("pepe_nerd", "pushes up his glasses", true, 64, 64),
    e("pepe_popcorn", "eats popcorn", true, 64, 64),
    e("pepe_chat", "types in chat", true, 64, 64),
    e("pepe_rain", "stands under an umbrella", true, 64, 64),
    e("pepe_music", "listens to music", true, 64, 64),
    e("pepe_gamer", "plays games", true, 64, 64),
    e("pepe_gift", "gives a gift", true, 74, 64),
    e("pepe_meltdown", "melts down", true, 64, 64),
    e("pepe_agony", "writhes in agony", true, 64, 64),
    e("pepe_sad_dance", "dances sadly", true, 72, 64),
    e("pepe_yum", "licks his lips", true, 64, 64),
    e("pepe_zip", "zips his mouth", true, 64, 64),
    e("pepe_poof", "vanishes in a puff, dressed as a samurai", true, 64, 64),
    e("pepe_sadge", "looks sad", false, 84, 64),
    e("pepe_sad", "looks sad, a small one", false, 64, 64),
    e("pepe_monkas", "looks nervous", false, 64, 64),
    e("pepe_monkaw", "looks nervous, close up", false, 64, 64),
    e("pepe_happy", "looks happy", false, 86, 64),
    e("pepe_wide_happy", "smiles wide", false, 160, 64),
    e("pepe_okay", "says okay", false, 84, 64),
    e("pepe_madge", "looks mad", false, 84, 64),
    e("pepe_bedge", "lies in bed", false, 64, 64),
    e("pepe_cute", "wears a bow", false, 72, 64),
    e("pepe_cozy", "snuggles in a hood", false, 64, 64),
    e("pepe_cake", "holds a birthday cake", false, 64, 64),
    e("pepe_feels_good", "feels good", false, 86, 64),
    e("pepe_feels_bad", "feels bad", false, 64, 64),
    e("pepe_samurai", "dressed as a samurai", false, 64, 64),
];

/// Where stickers are served, below the site's base path; cached for a
/// year, like the emoji, and embedded in casts for good, so a file never
/// changes in place.
pub const STICKER_DIR: &str = "/assets/stickers/v1";
/// Where a cast's sticker is embedded from, for every Farcaster app to load.
pub const STICKER_ORIGIN: &str = "https://allthing.social";
/// Every sticker's height in pixels (7TV's 4x).
pub const STICKER_HEIGHT: u16 = 128;

/// A sticker: one of the pack's animated emoji as a 128 px GIF, which a
/// cast embeds as an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sticker {
    /// The emoji's shortcode, without the colons.
    pub code: &'static str,
    /// Its width in pixels.
    pub width: u16,
}

impl Sticker {
    /// The emoji it is.
    pub fn emoji(&self) -> &'static Emoji {
        find(self.code).unwrap_or(&PACK[0])
    }

    /// Its GIF, below the site's base path.
    pub fn file(&self) -> String {
        format!("{STICKER_DIR}/{}.gif", self.code)
    }

    /// A still of it, for readers who ask for less motion.
    pub fn still(&self) -> String {
        format!("{STICKER_DIR}/{}.still.webp", self.code)
    }

    /// What a cast embeds: its GIF at allthing.social.
    pub fn embed(&self) -> String {
        format!("{STICKER_ORIGIN}{}", self.file())
    }

    /// What it shows: "Pepe jams with headphones".
    pub fn label(&self) -> String {
        self.emoji().label()
    }
}

const fn s(code: &'static str, width: u16) -> Sticker {
    Sticker { code, width }
}

/// The stickers, in the order a picker shows them.
pub const STICKERS: &[Sticker] = &[
    s("pepe_jam", 132),
    s("pepe_pls", 92),
    s("pepe_sad_dance", 144),
    s("pepe_laugh", 128),
    s("pepe_clap", 128),
    s("pepe_nopers", 128),
    s("pepe_yes", 128),
    s("pepe_hands", 128),
    s("pepe_cry", 128),
    s("pepe_shy", 128),
    s("pepe_comfy", 128),
    s("pepe_huggers", 128),
    s("pepe_wink", 128),
    s("pepe_cool", 128),
    s("pepe_side_eye", 128),
    s("pepe_wow", 128),
    s("pepe_wave", 128),
    s("pepe_gm", 128),
    s("pepe_gn", 128),
    s("pepe_gg", 128),
    s("pepe_thanks", 128),
    s("pepe_party", 148),
    s("pepe_cheer", 128),
    s("pepe_popcorn", 128),
    s("pepe_coffee", 128),
    s("pepe_typing", 128),
    s("pepe_rain", 128),
    s("pepe_birthday", 128),
    s("pepe_headbang", 128),
    s("pepe_sleep", 128),
    s("pepe_money", 128),
    s("pepe_gift", 148),
];

/// The sticker whose shortcode is `code`.
pub fn sticker(code: &str) -> Option<&'static Sticker> {
    STICKERS.iter().find(|s| s.code == code)
}

/// The sticker a cast's embed is, if it is one of Allthing's.
pub fn sticker_of(url: &str) -> Option<&'static Sticker> {
    let file = url.strip_prefix(STICKER_ORIGIN)?.strip_prefix(STICKER_DIR)?.strip_prefix('/')?;
    sticker(file.strip_suffix(".gif")?)
}

/// How many recently used stickers a picker keeps: a row or two.
pub const RECENT_STICKERS: usize = 8;

/// The recently used stickers in `kept` (shortcodes, most recent first,
/// separated by spaces), leaving out any that are no longer stickers.
pub fn recent_stickers(kept: &str) -> Vec<&'static Sticker> {
    let mut recent: Vec<&'static Sticker> = Vec::new();
    for sticker in kept.split_whitespace().filter_map(sticker) {
        if !recent.contains(&sticker) && recent.len() < RECENT_STICKERS {
            recent.push(sticker);
        }
    }
    recent
}

/// `kept` with `sticker` as the most recent.
pub fn remember_sticker(kept: &str, sticker: &Sticker) -> String {
    let mut codes = vec![sticker.code];
    codes.extend(recent_stickers(kept).iter().map(|s| s.code).filter(|code| *code != sticker.code));
    codes.truncate(RECENT_STICKERS);
    codes.join(" ")
}

/// The stickers that match a picker's search, best first, as `search`
/// ranks their emoji; all of them, in their own order, when it matches all.
pub fn search_stickers(query: &str) -> Vec<&'static Sticker> {
    let found = search(query);
    if found.len() == PACK.len() {
        return STICKERS.iter().collect();
    }
    found.into_iter().filter_map(|emoji| sticker(emoji.code)).collect()
}

/// The emoji whose shortcode is `code`, without its colons, in any case.
pub fn find(code: &str) -> Option<&'static Emoji> {
    PACK.iter().find(|e| e.code.eq_ignore_ascii_case(code))
}

/// The emoji whose shortcode `rest` starts with (":pepe_no:…"), and the
/// shortcode's length in bytes.
pub fn shortcode_at(rest: &str) -> Option<(&'static Emoji, usize)> {
    let inner = rest.strip_prefix(':')?;
    let end = inner.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
    if end == 0 || !inner[end..].starts_with(':') {
        return None;
    }
    find(&inner[..end]).map(|emoji| (emoji, end + 2))
}

/// The pack's emoji that match a picker's search, best first: those whose
/// name starts with it, then those whose name has it, then those whose
/// description has it, each in the pack's order. Colons, case and the
/// `pepe_` every name starts with don't count, and spaces match
/// underscores. An empty search, or one still typing "pepe_", matches all.
pub fn search(query: &str) -> Vec<&'static Emoji> {
    let query = query.trim().trim_matches(':').to_lowercase().replace(' ', "_");
    let query = match query.strip_prefix("pepe_") {
        Some(rest) => rest,
        None if query.len() >= 3 && "pepe_".starts_with(&query) => "",
        None => &query,
    };
    let mut found: Vec<(u8, &'static Emoji)> = PACK
        .iter()
        .filter_map(|emoji| {
            let name = emoji.code.strip_prefix("pepe_").unwrap_or(emoji.code);
            let rank = if name.starts_with(query) {
                0
            } else if name.contains(query) {
                1
            } else if emoji.what.to_lowercase().replace(' ', "_").contains(query) {
                2
            } else {
                return None;
            };
            Some((rank, emoji))
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, emoji)| emoji).collect()
}

/// How many recently used emoji a picker keeps.
pub const RECENT_MAX: usize = 16;

/// The recently used emoji in `kept` (shortcodes, most recent first,
/// separated by spaces), leaving out any no longer in the pack.
pub fn recent(kept: &str) -> Vec<&'static Emoji> {
    let mut recent: Vec<&'static Emoji> = Vec::new();
    for emoji in kept.split_whitespace().filter_map(find) {
        if !recent.contains(&emoji) && recent.len() < RECENT_MAX {
            recent.push(emoji);
        }
    }
    recent
}

/// `kept` with `emoji` as the most recent.
pub fn remember(kept: &str, emoji: &Emoji) -> String {
    let mut codes = vec![emoji.code];
    codes.extend(recent(kept).iter().map(|e| e.code).filter(|code| *code != emoji.code));
    codes.truncate(RECENT_MAX);
    codes.join(" ")
}

/// `emoji`'s shortcode in `text` in place of the selection from `start` to
/// `end` (in UTF-16 code units, as a text box counts them), with a space
/// before it unless it starts the text or follows one, and after it unless
/// one follows. Also where the caret goes: just after what was put in.
pub fn insert(text: &str, start: usize, end: usize, emoji: &Emoji) -> (String, usize) {
    let (start, end) = (byte_at(text, start.min(end)), byte_at(text, start.max(end)));
    let (before, after) = (&text[..start], &text[end..]);
    let mut out = before.to_owned();
    if !before.is_empty() && !before.ends_with(char::is_whitespace) {
        out.push(' ');
    }
    out.push(':');
    out.push_str(emoji.code);
    out.push(':');
    if !after.starts_with(char::is_whitespace) {
        out.push(' ');
    }
    let caret = out.encode_utf16().count();
    out.push_str(after);
    (out, caret)
}

/// The byte offset in `text` of the UTF-16 offset `at`, at a character's
/// start; past the end, the end.
fn byte_at(text: &str, at: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= at {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    #[test]
    fn shortcodes_are_unique_lowercase_words() {
        let codes: BTreeSet<&str> = PACK.iter().map(|e| e.code).collect();
        assert_eq!(codes.len(), PACK.len());
        for e in PACK {
            assert!(e.code.starts_with("pepe_"), "{}", e.code);
            assert!(e.code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{}", e.code);
            assert!(!e.what.is_empty() && e.height == 64, "{}", e.code);
        }
    }

    #[test]
    fn finds_a_shortcode_in_any_case_and_only_a_whole_one() {
        assert_eq!(find("pepe_no").map(|e| e.code), Some("pepe_no"));
        assert_eq!(find("PEPE_No").map(|e| e.code), Some("pepe_no"));
        assert_eq!(find("pepe_nope"), None);
        assert_eq!(shortcode_at(":pepe_no: yes").map(|(e, n)| (e.code, n)), Some(("pepe_no", 9)));
        assert_eq!(shortcode_at(":pepe_no"), None);
        assert_eq!(shortcode_at(":pepe_no :"), None);
        assert_eq!(shortcode_at("::"), None);
        assert_eq!(shortcode_at(":30:45"), None);
        assert_eq!(shortcode_at(":smile:"), None);
    }

    #[test]
    fn paths_and_labels() {
        let no = find("pepe_no").unwrap();
        assert_eq!(no.file(), "/assets/emoji/v1/pepe_no.webp");
        assert_eq!(no.still(), no.file());
        assert_eq!(no.label(), "Pepe holds up a NO sign");
        let laugh = find("pepe_laugh").unwrap();
        assert!(laugh.animated);
        assert_eq!(laugh.still(), "/assets/emoji/v1/pepe_laugh.still.webp");
    }

    fn codes(found: &[&Emoji]) -> Vec<&'static str> {
        found.iter().map(|e| e.code).collect()
    }

    #[test]
    fn a_search_finds_names_first_then_descriptions() {
        assert_eq!(search("").len(), PACK.len());
        assert_eq!(search("  ").len(), PACK.len());
        // Still typing "pepe_" or ":pepe_": everything.
        assert_eq!(search("pep").len(), PACK.len());
        assert_eq!(search(":pepe_").len(), PACK.len());
        // Named "no…" first, then "nods yes".
        assert_eq!(codes(&search("no")), ["pepe_no", "pepe_nopers", "pepe_yes"]);
        assert_eq!(codes(&search(":PEPE_NO:"))[0], "pepe_no");
        assert_eq!(codes(&search("heart hug")), ["pepe_heart_hug"]);
        assert_eq!(codes(&search("big heart")), ["pepe_heart_hug"]);
        assert_eq!(codes(&search("hug")), ["pepe_hug", "pepe_huggers", "pepe_heart_hug"]);
        assert!(search("zzz").is_empty());
    }

    #[test]
    fn recent_emoji_are_kept_newest_first_and_known() {
        let no = find("pepe_no").unwrap();
        let jam = find("pepe_jam").unwrap();
        assert_eq!(remember("", no), "pepe_no");
        assert_eq!(remember("pepe_no", jam), "pepe_jam pepe_no");
        assert_eq!(remember("pepe_jam pepe_no", no), "pepe_no pepe_jam");
        // Codes no longer in the pack, and repeats, are dropped.
        assert_eq!(codes(&recent("pepe_gone pepe_no  PEPE_NO pepe_jam")), ["pepe_no", "pepe_jam"]);
        let many: Vec<&str> = PACK.iter().map(|e| e.code).collect();
        assert_eq!(recent(&many.join(" ")).len(), RECENT_MAX);
        assert_eq!(remember(&many.join(" "), jam).split(' ').count(), RECENT_MAX);
    }

    #[test]
    fn inserting_spaces_the_shortcode_and_puts_the_caret_after_it() {
        let no = find("pepe_no").unwrap();
        assert_eq!(insert("", 0, 0, no), (":pepe_no: ".to_owned(), 10));
        assert_eq!(insert("gm", 2, 2, no), ("gm :pepe_no: ".to_owned(), 13));
        assert_eq!(insert("gm ", 3, 3, no), ("gm :pepe_no: ".to_owned(), 13));
        // In the middle, with a space after already.
        assert_eq!(insert("a b", 1, 1, no), ("a :pepe_no: b".to_owned(), 11));
        // In place of a selection, either way round.
        assert_eq!(insert("say no!", 4, 6, no), ("say :pepe_no: !".to_owned(), 14));
        assert_eq!(insert("say no!", 6, 4, no), ("say :pepe_no: !".to_owned(), 14));
        // Offsets in UTF-16, past a character outside the BMP and an accent.
        assert_eq!(insert("🐸é", 3, 3, no), ("🐸é :pepe_no: ".to_owned(), 14));
        assert_eq!(insert("🐸é", 2, 2, no), ("🐸 :pepe_no: é".to_owned(), 13));
        // Past the end: at the end.
        assert_eq!(insert("gm", 99, 99, no), ("gm :pepe_no: ".to_owned(), 13));
        assert_eq!(insert("gm\n", 3, 3, no), ("gm\n:pepe_no: ".to_owned(), 13));
    }

    #[test]
    fn stickers_are_animated_emoji_of_the_pack() {
        let codes: BTreeSet<&str> = STICKERS.iter().map(|s| s.code).collect();
        assert_eq!(codes.len(), STICKERS.len());
        for sticker in STICKERS {
            let emoji = find(sticker.code).unwrap_or_else(|| panic!("{} isn't in the pack", sticker.code));
            assert!(emoji.animated, "{}", sticker.code);
            assert_eq!(sticker.emoji(), emoji);
        }
        let jam = sticker("pepe_jam").unwrap();
        assert_eq!(jam.file(), "/assets/stickers/v1/pepe_jam.gif");
        assert_eq!(jam.still(), "/assets/stickers/v1/pepe_jam.still.webp");
        assert_eq!(jam.label(), "Pepe jams with headphones");
    }

    #[test]
    fn a_casts_sticker_is_known_by_its_embed() {
        let jam = sticker("pepe_jam").unwrap();
        assert_eq!(jam.embed(), "https://allthing.social/assets/stickers/v1/pepe_jam.gif");
        assert_eq!(sticker_of(&jam.embed()), Some(jam));
        for other in [
            "http://allthing.social/assets/stickers/v1/pepe_jam.gif",
            "https://allthing.social.example/assets/stickers/v1/pepe_jam.gif",
            "https://allthing.social/assets/stickers/v1/pepe_jam.still.webp",
            "https://allthing.social/assets/stickers/v2/pepe_jam.gif",
            "https://allthing.social/assets/stickers/v1/pepe_no.gif",
            "https://allthing.social/assets/emoji/v1/pepe_jam.webp",
            "https://example.com/pepe_jam.gif",
        ] {
            assert_eq!(sticker_of(other), None, "{other}");
        }
        // Every embed fits a cast's 256 bytes.
        assert!(STICKERS.iter().all(|s| s.embed().len() <= 256));
    }

    #[test]
    fn a_sticker_search_ranks_as_the_emoji_search() {
        let codes: Vec<&str> = search_stickers("dance").iter().map(|s| s.code).collect();
        assert_eq!(codes, ["pepe_sad_dance", "pepe_pls"]);
        let all: Vec<&str> = search_stickers("").iter().map(|s| s.code).collect();
        assert_eq!(all, STICKERS.iter().map(|s| s.code).collect::<Vec<_>>());
        assert!(search_stickers("no").iter().all(|s| s.code != "pepe_no"));
    }

    #[test]
    fn recent_stickers_are_kept_apart_newest_first() {
        let jam = sticker("pepe_jam").unwrap();
        let gm = sticker("pepe_gm").unwrap();
        assert_eq!(remember_sticker("", jam), "pepe_jam");
        assert_eq!(remember_sticker("pepe_jam", gm), "pepe_gm pepe_jam");
        assert_eq!(remember_sticker("pepe_gm pepe_jam", jam), "pepe_jam pepe_gm");
        // Emoji that aren't stickers, and repeats, are dropped.
        let kept: Vec<&str> = recent_stickers("pepe_no pepe_gm pepe_gm pepe_jam").iter().map(|s| s.code).collect();
        assert_eq!(kept, ["pepe_gm", "pepe_jam"]);
        let all: Vec<&str> = STICKERS.iter().map(|s| s.code).collect();
        assert_eq!(recent_stickers(&all.join(" ")).len(), RECENT_STICKERS);
        assert_eq!(remember_sticker(&all.join(" "), gm).split(' ').count(), RECENT_STICKERS);
    }

    /// Every sticker has its GIF, 128 px tall at the width the table says,
    /// and a still of the same size, and a credit with its source; every
    /// file is one of theirs.
    #[test]
    fn every_sticker_has_its_files_and_a_credit() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/stickers");
        let credits = fs::read_to_string(dir.join("CREDITS.md")).unwrap();
        let files: BTreeSet<String> =
            fs::read_dir(dir.join("v1")).unwrap().map(|f| f.unwrap().file_name().into_string().unwrap()).collect();
        let mut expected = BTreeSet::new();
        for sticker in STICKERS {
            let gif = fs::read(dir.join("v1").join(format!("{}.gif", sticker.code))).unwrap();
            assert!(gif.starts_with(b"GIF89a"), "{}", sticker.code);
            let size = (u16::from_le_bytes([gif[6], gif[7]]), u16::from_le_bytes([gif[8], gif[9]]));
            assert_eq!(size, (sticker.width, STICKER_HEIGHT), "{}", sticker.code);
            assert!(gif.len() <= 300_000, "{} is {} bytes", sticker.code, gif.len());
            let still = fs::read(dir.join("v1").join(format!("{}.still.webp", sticker.code))).unwrap();
            assert_eq!(webp_size(&still), (u32::from(sticker.width), u32::from(STICKER_HEIGHT)), "{}", sticker.code);
            expected.insert(format!("{}.gif", sticker.code));
            expected.insert(format!("{}.still.webp", sticker.code));
            let credit = credits.lines().find(|l| l.starts_with(&format!("| `{}` |", sticker.code)));
            let credit = credit.unwrap_or_else(|| panic!("no credit for {}", sticker.code));
            assert!(credit.contains("| https://7tv.app/emotes/"), "no source for {}", sticker.code);
        }
        assert_eq!(files, expected, "files that aren't stickers, or missing");
    }

    /// The width and height a WebP says it has: its canvas (VP8X), or its
    /// one frame's (VP8, VP8L).
    fn webp_size(bytes: &[u8]) -> (u32, u32) {
        assert!(bytes.len() > 30 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP");
        let le24 = |b: &[u8]| u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16;
        match &bytes[12..16] {
            b"VP8X" => (le24(&bytes[24..27]) + 1, le24(&bytes[27..30]) + 1),
            b"VP8 " => {
                let w = u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3fff);
                let h = u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3fff);
                (w, h)
            }
            b"VP8L" => {
                let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
                ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
            }
            other => panic!("not a WebP this reads: {other:?}"),
        }
    }

    /// Every emoji has its file (and still), at the size the table says,
    /// and a credit with where it came from; every file is one of theirs.
    #[test]
    fn every_emoji_has_its_files_and_a_credit() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/emoji");
        let credits = fs::read_to_string(dir.join("CREDITS.md")).unwrap();
        let files: BTreeSet<String> =
            fs::read_dir(dir.join("v1")).unwrap().map(|f| f.unwrap().file_name().into_string().unwrap()).collect();
        let mut expected = BTreeSet::new();
        for e in PACK {
            let file = format!("{}.webp", e.code);
            let bytes = fs::read(dir.join("v1").join(&file)).unwrap_or_else(|_| panic!("no file for {}", e.code));
            assert_eq!(webp_size(&bytes), (u32::from(e.width), u32::from(e.height)), "{}", e.code);
            assert!(bytes.len() <= 150_000, "{} is {} bytes", e.code, bytes.len());
            expected.insert(file);
            if e.animated {
                let still = format!("{}.still.webp", e.code);
                assert!(files.contains(&still), "no still for {}", e.code);
                expected.insert(still);
            }
            let credit = credits.lines().find(|l| l.starts_with(&format!("| `:{}:` |", e.code)));
            let credit = credit.unwrap_or_else(|| panic!("no credit for {}", e.code));
            assert!(credit.contains("| https://"), "no source for {}", e.code);
        }
        assert_eq!(files, expected, "files that aren't in the pack, or missing");
    }
}
