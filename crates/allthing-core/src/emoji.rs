// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's emoji: a pack of Pepe the Frog emotes from 7TV, FrankerFaceZ
//! and emoji.gg, which Allthing serves itself (`web/emoji/v1/`, credited
//! with their sources in `web/emoji/CREDITS.md`). A cast writes one as a
//! shortcode, `:pepe_no:`, which Allthing shows as the emote; other
//! Farcaster apps show the shortcode as text.

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
