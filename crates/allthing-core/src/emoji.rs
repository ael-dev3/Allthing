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

use crate::model::{Cast, UserSummary};

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
    /// Where a picker shows it.
    pub group: Group,
}

/// A run of the pack, under its own heading in a picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// Close matches for the five Pepes Allthing was asked for first, and
    /// their nearest kin.
    Classics,
    Reactions,
    Happy,
    Love,
    Sad,
    Party,
    Hello,
    Food,
    Doing,
    DressedUp,
    Signs,
}

impl Group {
    /// Every group, in a picker's order.
    pub const ALL: [Group; 11] = [
        Group::Classics,
        Group::Reactions,
        Group::Happy,
        Group::Love,
        Group::Sad,
        Group::Party,
        Group::Hello,
        Group::Food,
        Group::Doing,
        Group::DressedUp,
        Group::Signs,
    ];

    /// Its heading.
    pub fn label(self) -> &'static str {
        match self {
            Group::Classics => "Classics",
            Group::Reactions => "Reactions",
            Group::Happy => "Happy",
            Group::Love => "Love",
            Group::Sad => "Sad and scared",
            Group::Party => "Music and party",
            Group::Hello => "Hi and bye",
            Group::Food => "Food and drink",
            Group::Doing => "Doing things",
            Group::DressedUp => "Dressed up",
            Group::Signs => "Signs",
        }
    }

    /// Its emoji, in the pack's order.
    pub fn emoji(self) -> impl Iterator<Item = &'static Emoji> {
        PACK.iter().filter(move |e| e.group == self)
    }
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

const fn e(code: &'static str, what: &'static str, animated: bool, width: u16, height: u16, group: Group) -> Emoji {
    Emoji { code, what, animated, width, height, group }
}

/// The pack, in the order a picker shows it: group by group, in
/// `Group::ALL`'s order, close matches for the five Pepes Allthing was asked
/// for first, then in each other group the animated before the still.
pub const PACK: &[Emoji] = &[
    e("pepe_no", "holds up a NO sign", false, 73, 64, Group::Classics),
    e("pepe_smug", "looks smug", false, 64, 64, Group::Classics),
    e("pepe_blush", "blushes", false, 64, 64, Group::Classics),
    e("pepe_heart_hug", "hugs a big heart", false, 82, 64, Group::Classics),
    e("pepe_sit", "sits, a small blob", false, 64, 64, Group::Classics),
    e("pepe_nopers", "shakes his head no", true, 64, 64, Group::Classics),
    e("pepe_shy", "blushes and looks away", true, 64, 64, Group::Classics),
    e("pepe_love", "holds a heart", true, 64, 64, Group::Classics),
    e("pepe_hug", "hugs", true, 82, 64, Group::Classics),
    e("pepe_sit_clap", "sits and claps", true, 64, 64, Group::Classics),
    e("pepe_comfy", "snuggles under a blanket", true, 64, 64, Group::Classics),
    e("pepe_smirk", "smirks", false, 64, 64, Group::Classics),
    e("pepe_yes", "nods yes", true, 64, 64, Group::Reactions),
    e("pepe_think", "thinks", true, 64, 64, Group::Reactions),
    e("pepe_side_eye", "gives a side-eye", true, 64, 64, Group::Reactions),
    e("pepe_clap", "claps", true, 64, 64, Group::Reactions),
    e("pepe_wow", "says wow", true, 64, 64, Group::Reactions),
    e("pepe_point", "points and laughs", true, 64, 64, Group::Reactions),
    e("pepe_salute", "salutes", true, 64, 64, Group::Reactions),
    e("pepe_shrug", "shrugs", true, 64, 64, Group::Reactions),
    e("pepe_thanks", "says thanks", true, 64, 64, Group::Reactions),
    e("pepe_nerd", "pushes up his glasses", true, 64, 64, Group::Reactions),
    e("pepe_zip", "zips his mouth", true, 64, 64, Group::Reactions),
    e("pepe_facepalm", "facepalms", true, 64, 64, Group::Reactions),
    e("pepe_shocked", "is shocked", true, 64, 64, Group::Reactions),
    e("pepe_hmm", "goes hmm", true, 64, 64, Group::Reactions),
    e("pepe_sweat", "sweats", true, 64, 64, Group::Reactions),
    e("pepe_stare", "stares", true, 64, 64, Group::Reactions),
    e("pepe_confused", "is confused", true, 64, 64, Group::Reactions),
    e("pepe_noted", "takes notes", true, 98, 64, Group::Reactions),
    e("pepe_sigh", "sighs", true, 64, 64, Group::Reactions),
    e("pepe_weird_clap", "claps, unimpressed", true, 64, 64, Group::Reactions),
    e("pepe_yikes", "says yikes", true, 64, 64, Group::Reactions),
    e("pepe_sus", "looks sus", true, 84, 64, Group::Reactions),
    e("pepe_plot", "plots, rubbing his hands", true, 64, 64, Group::Reactions),
    e("pepe_saved", "wipes his brow, saved", true, 64, 64, Group::Reactions),
    e("pepe_yep", "stares, eyes wide: yep", true, 66, 64, Group::Reactions),
    e("pepe_sure", "agrees: sure", true, 64, 64, Group::Reactions),
    e("pepe_okay", "says okay", false, 84, 64, Group::Reactions),
    e("pepe_like", "gives a thumbs up", false, 74, 64, Group::Reactions),
    e("pepe_feels_weird", "feels weird", false, 68, 64, Group::Reactions),
    e("pepe_amazed", "is amazed", false, 64, 64, Group::Reactions),
    e("pepe_big_brain", "has a big brain", false, 64, 64, Group::Reactions),
    e("pepe_what", "asks what?", false, 56, 64, Group::Reactions),
    e("pepe_woke", "wide awake in bed", false, 64, 64, Group::Reactions),
    e("pepe_laugh", "laughs", true, 64, 64, Group::Happy),
    e("pepe_giggle", "giggles", true, 64, 64, Group::Happy),
    e("pepe_money", "throws money", true, 64, 64, Group::Happy),
    e("pepe_cool", "wears sunglasses", true, 64, 64, Group::Happy),
    e("pepe_glad", "is glad", true, 64, 64, Group::Happy),
    e("pepe_smile", "smiles", true, 64, 64, Group::Happy),
    e("pepe_cheer", "cheers with pom-poms", true, 64, 64, Group::Happy),
    e("pepe_hype", "gets hyped", true, 64, 64, Group::Happy),
    e("pepe_flex", "flexes", true, 64, 64, Group::Happy),
    e("pepe_lmao", "laughs his head off", true, 64, 64, Group::Happy),
    e("pepe_win", "wins", true, 64, 64, Group::Happy),
    e("pepe_proud", "is proud, with a medal", true, 64, 64, Group::Happy),
    e("pepe_sit_happy", "sits back in a chair, happy", true, 64, 64, Group::Happy),
    e("pepe_summer", "in shades in the sun", true, 64, 64, Group::Happy),
    e("pepe_happy", "looks happy", false, 86, 64, Group::Happy),
    e("pepe_wide_happy", "smiles wide", false, 160, 64, Group::Happy),
    e("pepe_feels_good", "feels good", false, 86, 64, Group::Happy),
    e("pepe_stonks", "says stonks", false, 66, 64, Group::Happy),
    e("pepe_star", "is a star", false, 64, 64, Group::Happy),
    e("pepe_rainbow", "under a rainbow", false, 68, 64, Group::Happy),
    e("pepe_huggers", "opens his arms", true, 64, 64, Group::Love),
    e("pepe_kiss", "blows kisses", true, 64, 64, Group::Love),
    e("pepe_wink", "winks", true, 64, 64, Group::Love),
    e("pepe_flower", "offers a flower", true, 64, 64, Group::Love),
    e("pepe_pat", "pats a heart", true, 64, 64, Group::Love),
    e("pepe_gift", "gives a gift", true, 74, 64, Group::Love),
    e("pepe_pray", "prays", true, 64, 64, Group::Love),
    e("pepe_headpat", "gets a head pat", true, 64, 64, Group::Love),
    e("pepe_cat", "hugs a cat", true, 64, 64, Group::Love),
    e("pepe_friends", "hugs a friend", true, 64, 64, Group::Love),
    e("pepe_cute", "wears a bow", false, 72, 64, Group::Love),
    e("pepe_hands", "cries into his hands", true, 64, 64, Group::Sad),
    e("pepe_scared", "is scared", true, 64, 64, Group::Sad),
    e("pepe_strong", "cries, but stays strong", true, 64, 64, Group::Sad),
    e("pepe_cry", "cries", true, 64, 64, Group::Sad),
    e("pepe_rain", "stands under an umbrella", true, 64, 64, Group::Sad),
    e("pepe_meltdown", "melts down", true, 64, 64, Group::Sad),
    e("pepe_agony", "writhes in agony", true, 64, 64, Group::Sad),
    e("pepe_sad_dance", "dances sadly", true, 72, 64, Group::Sad),
    e("pepe_nervous", "shakes nervously", true, 64, 64, Group::Sad),
    e("pepe_tremble", "trembles, eyes wide", true, 64, 64, Group::Sad),
    e("pepe_sadge", "looks sad", false, 84, 64, Group::Sad),
    e("pepe_sad", "looks sad, a small one", false, 64, 64, Group::Sad),
    e("pepe_monkas", "looks nervous", false, 64, 64, Group::Sad),
    e("pepe_monkaw", "looks nervous, close up", false, 64, 64, Group::Sad),
    e("pepe_madge", "looks mad", false, 84, 64, Group::Sad),
    e("pepe_feels_bad", "feels bad", false, 64, 64, Group::Sad),
    e("pepe_smadge", "cries, cross", false, 84, 64, Group::Sad),
    e("pepe_despair", "in grey despair", false, 84, 64, Group::Sad),
    e("pepe_pls", "dances on the spot", true, 46, 64, Group::Party),
    e("pepe_jam", "jams with headphones", true, 66, 64, Group::Party),
    e("pepe_bounce", "bounces", true, 64, 64, Group::Party),
    e("pepe_spin", "spins", true, 64, 64, Group::Party),
    e("pepe_shake", "shakes", true, 64, 64, Group::Party),
    e("pepe_headbang", "headbangs", true, 64, 64, Group::Party),
    e("pepe_birthday", "blows a party horn", true, 64, 64, Group::Party),
    e("pepe_party", "parties", true, 74, 64, Group::Party),
    e("pepe_dance", "dances with his arms out", true, 64, 64, Group::Party),
    e("pepe_music", "listens to music", true, 64, 64, Group::Party),
    e("pepe_dj", "DJs", true, 64, 64, Group::Party),
    e("pepe_guitar", "plays the guitar", true, 64, 64, Group::Party),
    e("pepe_piano", "plays the piano", true, 64, 64, Group::Party),
    e("pepe_sing", "sings", true, 64, 64, Group::Party),
    e("pepe_vibe", "vibes slowly", true, 64, 64, Group::Party),
    e("pepe_party_hat", "in a party hat, confetti flying", true, 64, 64, Group::Party),
    e("pepe_torch", "holds a torch high", true, 64, 64, Group::Party),
    e("pepe_balloon", "holds a balloon", false, 94, 64, Group::Party),
    e("pepe_vanish", "vanishes", true, 118, 64, Group::Hello),
    e("pepe_hey", "waves hey", true, 64, 64, Group::Hello),
    e("pepe_bye", "waves bye", true, 64, 64, Group::Hello),
    e("pepe_exit", "heads for the exit", true, 64, 64, Group::Hello),
    e("pepe_hi", "says hi", true, 64, 64, Group::Hello),
    e("pepe_gn", "says good night", true, 64, 64, Group::Hello),
    e("pepe_gm", "says good morning", true, 64, 64, Group::Hello),
    e("pepe_wave", "waves", true, 64, 64, Group::Hello),
    e("pepe_arrive", "arrives", true, 64, 64, Group::Hello),
    e("pepe_leave", "leaves", true, 64, 64, Group::Hello),
    e("pepe_peek", "peeks in", true, 64, 64, Group::Hello),
    e("pepe_sneak", "sneaks by", true, 64, 64, Group::Hello),
    e("pepe_afk", "is AFK, asleep at the keyboard", true, 64, 64, Group::Hello),
    e("pepe_lurk", "lurks at the window", true, 64, 64, Group::Hello),
    e("pepe_tea", "sips tea", true, 64, 64, Group::Food),
    e("pepe_eat", "eats", true, 64, 64, Group::Food),
    e("pepe_cookie", "eats a cookie", true, 64, 64, Group::Food),
    e("pepe_pizza", "eats pizza", true, 64, 64, Group::Food),
    e("pepe_coffee", "drinks coffee", true, 64, 64, Group::Food),
    e("pepe_popcorn", "eats popcorn", true, 64, 64, Group::Food),
    e("pepe_yum", "licks his lips", true, 64, 64, Group::Food),
    e("pepe_chef", "cooks", true, 64, 64, Group::Food),
    e("pepe_ice_cream", "eats ice cream", true, 110, 64, Group::Food),
    e("pepe_burger", "eats a burger", true, 64, 64, Group::Food),
    e("pepe_noodles", "slurps noodles", true, 64, 64, Group::Food),
    e("pepe_sushi", "eats sushi", true, 64, 64, Group::Food),
    e("pepe_juice", "sips a juice box", true, 64, 64, Group::Food),
    e("pepe_banana", "eats a banana", true, 64, 64, Group::Food),
    e("pepe_watermelon", "eats watermelon", true, 64, 64, Group::Food),
    e("pepe_cake", "holds a birthday cake", false, 64, 64, Group::Food),
    e("pepe_donut", "sits in a donut", false, 76, 64, Group::Food),
    e("pepe_bread", "the baker holds a loaf", false, 90, 64, Group::Food),
    e("pepe_toast", "as a slice of bread", false, 78, 64, Group::Food),
    e("pepe_egg", "in an egg", false, 64, 64, Group::Food),
    e("pepe_cheese", "with a wedge of cheese", false, 82, 64, Group::Food),
    e("pepe_blanket", "wraps up in a blanket", true, 64, 64, Group::Doing),
    e("pepe_smash", "smashes the keyboard", true, 64, 64, Group::Doing),
    e("pepe_sleep", "sleeps", true, 64, 64, Group::Doing),
    e("pepe_read", "reads", true, 94, 64, Group::Doing),
    e("pepe_typing", "types", true, 64, 64, Group::Doing),
    e("pepe_writing", "writes", true, 98, 64, Group::Doing),
    e("pepe_phone", "talks on the phone", true, 62, 64, Group::Doing),
    e("pepe_tub", "takes a bath", true, 64, 64, Group::Doing),
    e("pepe_run", "runs", true, 64, 64, Group::Doing),
    e("pepe_talk", "talks", true, 64, 64, Group::Doing),
    e("pepe_chat", "types in chat", true, 64, 64, Group::Doing),
    e("pepe_gamer", "plays games", true, 64, 64, Group::Doing),
    e("pepe_build", "builds, in a hard hat", true, 64, 64, Group::Doing),
    e("pepe_rocket", "rides a rocket", true, 64, 64, Group::Doing),
    e("pepe_swim", "swims", true, 64, 64, Group::Doing),
    e("pepe_drive", "drives", true, 64, 64, Group::Doing),
    e("pepe_fly", "flies a plane", true, 64, 64, Group::Doing),
    e("pepe_chess", "plays chess", true, 74, 64, Group::Doing),
    e("pepe_paint", "paints", true, 64, 64, Group::Doing),
    e("pepe_chill", "chills in an armchair", true, 64, 64, Group::Doing),
    e("pepe_fish", "catches a fish", true, 102, 64, Group::Doing),
    e("pepe_plant", "waters a plant", true, 102, 64, Group::Doing),
    e("pepe_pogo", "bounces on a pogo stick", true, 64, 64, Group::Doing),
    e("pepe_work", "works at his desk", true, 64, 64, Group::Doing),
    e("pepe_candle", "holds up a lit candle", true, 64, 64, Group::Doing),
    e("pepe_bike", "rides a bike", true, 64, 64, Group::Doing),
    e("pepe_yoga", "does yoga", true, 64, 64, Group::Doing),
    e("pepe_fishing", "fishes in a helmet", true, 90, 64, Group::Doing),
    e("pepe_cloud", "floats on a cloud", true, 66, 64, Group::Doing),
    e("pepe_bedge", "lies in bed", false, 64, 64, Group::Doing),
    e("pepe_cozy", "snuggles in a hood", false, 64, 64, Group::Doing),
    e("pepe_meditate", "meditates", false, 64, 64, Group::Doing),
    e("pepe_study", "studies", false, 66, 64, Group::Doing),
    e("pepe_camera", "takes a photo", false, 76, 64, Group::Doing),
    e("pepe_selfie", "takes a selfie", false, 64, 64, Group::Doing),
    e("pepe_boat", "sails", false, 64, 64, Group::Doing),
    e("pepe_business", "in a suit with a briefcase", false, 64, 64, Group::Doing),
    e("pepe_gym", "does pull-ups", false, 62, 64, Group::Doing),
    e("pepe_poof", "vanishes in a puff, dressed as a samurai", true, 64, 64, Group::DressedUp),
    e("pepe_detective", "investigates", true, 64, 64, Group::DressedUp),
    e("pepe_ninja", "is a ninja", true, 64, 64, Group::DressedUp),
    e("pepe_wizard", "casts a spell", true, 64, 64, Group::DressedUp),
    e("pepe_clown", "is a clown", true, 64, 64, Group::DressedUp),
    e("pepe_pumpkin", "in a pumpkin hat", true, 64, 64, Group::DressedUp),
    e("pepe_winter", "in a bobble hat in the snow", true, 64, 64, Group::DressedUp),
    e("pepe_snow", "in a Santa hat in the snow", true, 64, 64, Group::DressedUp),
    e("pepe_cowboy", "in a cowboy hat", true, 64, 64, Group::DressedUp),
    e("pepe_pirate", "the pirate, eye patch and hat", true, 64, 64, Group::DressedUp),
    e("pepe_knight", "in a knight's helmet", true, 64, 64, Group::DressedUp),
    e("pepe_samurai", "dressed as a samurai", false, 64, 64, Group::DressedUp),
    e("pepe_king", "wears a crown", false, 64, 64, Group::DressedUp),
    e("pepe_angel", "is an angel", false, 74, 64, Group::DressedUp),
    e("pepe_doctor", "is a doctor", false, 64, 64, Group::DressedUp),
    e("pepe_bee", "is a bee", false, 64, 64, Group::DressedUp),
    e("pepe_bunny", "wears bunny ears", false, 64, 64, Group::DressedUp),
    e("pepe_shark", "is a shark", false, 64, 64, Group::DressedUp),
    e("pepe_dino", "is a dino", false, 64, 64, Group::DressedUp),
    e("pepe_skeleton", "in a skeleton costume", false, 64, 64, Group::DressedUp),
    e("pepe_witch", "in a witch's hat", false, 64, 64, Group::DressedUp),
    e("pepe_vampire", "as a vampire", false, 64, 64, Group::DressedUp),
    e("pepe_santa", "dressed as Santa", false, 64, 64, Group::DressedUp),
    e("pepe_viking", "in a horned helmet", false, 76, 64, Group::DressedUp),
    e("pepe_firefighter", "in a firefighter's helmet", false, 64, 64, Group::DressedUp),
    e("pepe_farmer", "in a straw hat", false, 64, 64, Group::DressedUp),
    e("pepe_artist", "the artist, beret and palette", false, 72, 64, Group::DressedUp),
    e("pepe_boba", "holds a BOBA sign", true, 64, 64, Group::Signs),
    e("pepe_cutie", "holds a CUTIE sign", true, 64, 64, Group::Signs),
    e("pepe_gg", "holds a GG sign", true, 64, 64, Group::Signs),
    e("pepe_drama", "holds a DRAMA sign", true, 64, 64, Group::Signs),
    e("pepe_l", "holds an L sign", true, 64, 64, Group::Signs),
    e("pepe_w", "holds a W sign", true, 64, 64, Group::Signs),
    e("pepe_live", "holds a LIVE sign", true, 64, 64, Group::Signs),
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

    /// It moving in the picker: an animated WebP made from its GIF, the
    /// same size and about a third of the bytes.
    pub fn tile(&self) -> String {
        format!("{STICKER_DIR}/{}.tile.webp", self.code)
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
/// Who made each Pepe, by its shortcode, as `web/emoji/CREDITS.md` credits
/// them (a sticker is the same emote, larger, by the same maker): for the
/// picker, which names the maker of the one pointed at. A test keeps this
/// and the credits the same.
pub const MAKERS: &[(&str, &str)] = &[
    ("pepe_no", "lalalexuss"),
    ("pepe_smug", "kickeristic"),
    ("pepe_blush", "n4chtc4leb"),
    ("pepe_heart_hug", "justrogan"),
    ("pepe_sit", "semmys1"),
    ("pepe_nopers", "a_duck_king"),
    ("pepe_shy", "laden"),
    ("pepe_love", "poplar_ixy"),
    ("pepe_hug", "harott"),
    ("pepe_sit_clap", "justkaye"),
    ("pepe_comfy", "zjawa"),
    ("pepe_smirk", "naatiszu"),
    ("pepe_yes", "fabby"),
    ("pepe_think", "wily_wolf"),
    ("pepe_side_eye", "da_infamous_dre"),
    ("pepe_clap", "rodrirangel2004"),
    ("pepe_wow", "mouzah_"),
    ("pepe_point", "zer0osu"),
    ("pepe_salute", "dexmlk"),
    ("pepe_shrug", "kushalatv"),
    ("pepe_thanks", "kinkanexo"),
    ("pepe_nerd", "cnys_"),
    ("pepe_zip", "𝙔𝘽𝙉"),
    ("pepe_facepalm", "marrryanx"),
    ("pepe_shocked", "woodywizzard"),
    ("pepe_hmm", "letsiii"),
    ("pepe_sweat", "kushalatv"),
    ("pepe_stare", "qwert0p"),
    ("pepe_confused", "kushalatv"),
    ("pepe_noted", "moonieywca"),
    ("pepe_sigh", "kushalatv"),
    ("pepe_weird_clap", "tanoshi13"),
    ("pepe_yikes", "woodywizzard"),
    ("pepe_sus", "bodybuffalo28"),
    ("pepe_plot", "esuesuesuuu"),
    ("pepe_saved", "khosi"),
    ("pepe_yep", "jackiechain"),
    ("pepe_sure", "andreimonty"),
    ("pepe_okay", "joonwi"),
    ("pepe_like", "shaderpc"),
    ("pepe_feels_weird", "clyvere"),
    ("pepe_amazed", "itsfuntum"),
    ("pepe_big_brain", "kushalatv"),
    ("pepe_what", "sour_appl"),
    ("pepe_woke", "aftonxy"),
    ("pepe_laugh", "mistrzu__"),
    ("pepe_giggle", "islabacate"),
    ("pepe_money", "elperroluis"),
    ("pepe_cool", "gvaste"),
    ("pepe_glad", "databaka"),
    ("pepe_smile", "evilmessy"),
    ("pepe_cheer", "ayyybubu"),
    ("pepe_hype", "moonretealoud"),
    ("pepe_flex", "fabby"),
    ("pepe_lmao", "neverdiepool"),
    ("pepe_win", "soaral"),
    ("pepe_proud", "calvinjcandy"),
    ("pepe_sit_happy", "a7med_madx"),
    ("pepe_summer", "ayyybubu"),
    ("pepe_happy", "brian6932"),
    ("pepe_wide_happy", "soda_"),
    ("pepe_feels_good", "rubydota"),
    ("pepe_stonks", "brian6932"),
    ("pepe_star", "on_a_stack"),
    ("pepe_rainbow", "stayster"),
    ("pepe_huggers", "khosi"),
    ("pepe_kiss", "justrogan"),
    ("pepe_wink", "kushalatv"),
    ("pepe_flower", "ogjacobik"),
    ("pepe_pat", "ssebas_b"),
    ("pepe_gift", "viba"),
    ("pepe_pray", "soyaka__"),
    ("pepe_headpat", "brian6932"),
    ("pepe_cat", "basedtype"),
    ("pepe_friends", "marc"),
    ("pepe_cute", "vinn700"),
    ("pepe_hands", "corolado"),
    ("pepe_scared", "idkfran_"),
    ("pepe_strong", "redlof"),
    ("pepe_cry", "hauvikk"),
    ("pepe_rain", "obscurecharmling"),
    ("pepe_meltdown", "supernoahtv"),
    ("pepe_agony", "mustikiing"),
    ("pepe_sad_dance", "freakshowz88"),
    ("pepe_nervous", "mistrzu__"),
    ("pepe_tremble", "yoim5th"),
    ("pepe_sadge", "mauriplss"),
    ("pepe_sad", "zjawa"),
    ("pepe_monkas", "corolado"),
    ("pepe_monkaw", "justrogan"),
    ("pepe_madge", "askhp"),
    ("pepe_feels_bad", "trippycolour"),
    ("pepe_smadge", "khosi"),
    ("pepe_despair", "kaitorius"),
    ("pepe_pls", "zjawa"),
    ("pepe_jam", "shakothewacko"),
    ("pepe_bounce", "imsub"),
    ("pepe_spin", "vinn700"),
    ("pepe_shake", "jotkbw"),
    ("pepe_headbang", "atmos"),
    ("pepe_birthday", "peeporowen"),
    ("pepe_party", "lithamsterlaze"),
    ("pepe_dance", "bahkiroff"),
    ("pepe_music", "xluks_"),
    ("pepe_dj", "rsnowwolf"),
    ("pepe_guitar", "on_a_stack"),
    ("pepe_piano", "sblectric"),
    ("pepe_sing", "maltesaa"),
    ("pepe_vibe", "mistrzu__"),
    ("pepe_party_hat", "dewwicc"),
    ("pepe_torch", "robo02wwe"),
    ("pepe_balloon", "renderjoose"),
    ("pepe_vanish", "shmovy"),
    ("pepe_hey", "justrogan"),
    ("pepe_bye", "phayp"),
    ("pepe_exit", "benjaminyvr"),
    ("pepe_hi", "lemaikeru"),
    ("pepe_gn", "0_nekopara_0"),
    ("pepe_gm", "santanamp1"),
    ("pepe_wave", "clockpip"),
    ("pepe_arrive", "sundizzle"),
    ("pepe_leave", "boopiskat"),
    ("pepe_peek", "tanoshi13"),
    ("pepe_sneak", "brackstein"),
    ("pepe_afk", "reaihax"),
    ("pepe_lurk", "tpur"),
    ("pepe_tea", "kushalatv"),
    ("pepe_eat", "*DeletedUser"),
    ("pepe_cookie", "johnnycyan"),
    ("pepe_pizza", "khosi"),
    ("pepe_coffee", "v2lue"),
    ("pepe_popcorn", "element7991"),
    ("pepe_yum", "𝙔𝘽𝙉"),
    ("pepe_chef", "oshgay"),
    ("pepe_ice_cream", "crisrymer"),
    ("pepe_burger", "kushalatv"),
    ("pepe_noodles", "dreiinfinity"),
    ("pepe_sushi", "umukei"),
    ("pepe_juice", "thisisdara"),
    ("pepe_banana", "giyo"),
    ("pepe_watermelon", "pufomint"),
    ("pepe_cake", "kigrath"),
    ("pepe_donut", "4w3official"),
    ("pepe_bread", "ashleyidk"),
    ("pepe_toast", "erdna2013"),
    ("pepe_egg", "kushalatv"),
    ("pepe_cheese", "tustinbear"),
    ("pepe_blanket", "henri_with_i"),
    ("pepe_smash", "boopiskat"),
    ("pepe_sleep", "supernoahtv"),
    ("pepe_read", "falorni"),
    ("pepe_typing", "NimaAMD"),
    ("pepe_writing", "sund"),
    ("pepe_phone", "johhmar"),
    ("pepe_tub", "undiegnome"),
    ("pepe_run", "maximus6216"),
    ("pepe_talk", "hyru"),
    ("pepe_chat", "laden"),
    ("pepe_gamer", "yoshkinkrot"),
    ("pepe_build", "gvsyn"),
    ("pepe_rocket", "xyzsee"),
    ("pepe_swim", "stel_phone"),
    ("pepe_drive", "cactuardo"),
    ("pepe_fly", "sulu_18"),
    ("pepe_chess", "justrogan"),
    ("pepe_paint", "on_a_stack"),
    ("pepe_chill", "Sadrasb"),
    ("pepe_fish", "ritual___"),
    ("pepe_plant", "ayyybubu"),
    ("pepe_pogo", "fabby"),
    ("pepe_work", "ypy555"),
    ("pepe_candle", "baykakabay"),
    ("pepe_bike", "virmolian"),
    ("pepe_yoga", "stonko_"),
    ("pepe_fishing", "slashzxmictlan"),
    ("pepe_cloud", "kat_the_lad"),
    ("pepe_bedge", "paruna"),
    ("pepe_cozy", "alencarrg"),
    ("pepe_meditate", "vioo"),
    ("pepe_study", "hanna_dul_set"),
    ("pepe_camera", "juanca137"),
    ("pepe_selfie", "kushalatv"),
    ("pepe_boat", "fumtastic_"),
    ("pepe_business", "clarobollo"),
    ("pepe_gym", "ste_ndhal_"),
    ("pepe_poof", "𝙔𝘽𝙉"),
    ("pepe_detective", "poduszkowiec_"),
    ("pepe_ninja", "ggently"),
    ("pepe_wizard", "frikiii"),
    ("pepe_clown", "corolado"),
    ("pepe_pumpkin", "letsiii"),
    ("pepe_winter", "ayyybubu"),
    ("pepe_snow", "eternal_pestilence"),
    ("pepe_cowboy", "leli_h"),
    ("pepe_pirate", "mic_stargazer"),
    ("pepe_knight", "turtoise"),
    ("pepe_samurai", "𝙔𝘽𝙉"),
    ("pepe_king", "NimaAMD"),
    ("pepe_angel", "hanomgwow"),
    ("pepe_doctor", "kushalatv"),
    ("pepe_bee", "amelieea"),
    ("pepe_bunny", "jadedzilla"),
    ("pepe_shark", "manorefantazio"),
    ("pepe_dino", "crash_0715"),
    ("pepe_skeleton", "xrennisx"),
    ("pepe_witch", "johnyowan"),
    ("pepe_vampire", "sylwekm10"),
    ("pepe_santa", "scoo0p"),
    ("pepe_viking", "LastDudeStanding"),
    ("pepe_firefighter", "rementin"),
    ("pepe_farmer", "khosi"),
    ("pepe_artist", "tiirazziil"),
    ("pepe_boba", "onlyjohno"),
    ("pepe_cutie", "peepoghost"),
    ("pepe_gg", "s3sh"),
    ("pepe_drama", "corolado"),
    ("pepe_l", "meicadiewurst"),
    ("pepe_w", "meicadiewurst"),
    ("pepe_live", "tjbirrd"),
];

/// Who made `code`'s emote, as its credit names them; nobody for one whose
/// maker's account is gone (7TV's `*DeletedUser`).
pub fn maker(code: &str) -> Option<&'static str> {
    MAKERS.iter().find(|(c, _)| *c == code).map(|(_, by)| *by).filter(|by| !by.starts_with('*'))
}

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
/// name starts with it, then those with a later word of the name starting
/// with it, then those with a word of the description starting with it,
/// each in the pack's order: "eat" finds "eats a cookie", not "sweats".
/// Colons, case and the `pepe_` every name starts with don't count, and
/// spaces match underscores. An empty search, or one still typing "pepe_",
/// matches all.
pub fn search(query: &str) -> Vec<&'static Emoji> {
    let query = query.trim().trim_matches(':').to_lowercase().replace(' ', "_");
    let query = match query.strip_prefix("pepe_") {
        Some(rest) => rest,
        None if query.len() >= 3 && "pepe_".starts_with(&query) => "",
        None => &query,
    };
    // A word's start: just after an underscore, as both are written here.
    let word = format!("_{query}");
    let mut found: Vec<(u8, &'static Emoji)> = PACK
        .iter()
        .filter_map(|emoji| {
            let name = emoji.code.strip_prefix("pepe_").unwrap_or(emoji.code);
            let rank = if name.starts_with(query) {
                0
            } else if name.contains(&word) {
                1
            } else if format!("_{}", emoji.what.to_lowercase().replace(' ', "_")).contains(&word) {
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

/// The emoji a cast is, when its text is one emoji and nothing else and it
/// carries no embeds: how Allthing sends an emoji reaction, as a reply to
/// the cast (Farcaster's own reactions are only likes and recasts).
pub fn reaction(cast: &Cast) -> Option<&'static Emoji> {
    if !cast.embeds.is_empty() || cast.hidden() {
        return None;
    }
    reaction_text(&cast.text)
}

/// The emoji `text` is, when it is one emoji and nothing else.
pub fn reaction_text(text: &str) -> Option<&'static Emoji> {
    let text = text.trim();
    shortcode_at(text).filter(|(_, len)| *len == text.len()).map(|(emoji, _)| emoji)
}

/// A reply that is one emoji (an emoji reaction) as the hub event follower
/// keeps it for the cast it replies to (`activity`), and as a listing's
/// notes carry it for its casts (`api::Annotations::reactions`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReactionReply {
    /// The reply's hash, lowercase `0x` hex.
    pub hash: String,
    /// The emoji's code (`pepe_no`).
    pub emoji: String,
    /// Whose reply it is.
    pub fid: u64,
    /// When it was merged, Unix seconds.
    pub ts: i64,
}

/// One emoji's reactions to a cast, and who reacted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reacted {
    pub emoji: &'static Emoji,
    /// Each account once, in the order they reacted.
    pub by: Vec<UserSummary>,
    /// Every reply that is this emoji, in the order they came: whose it is
    /// (fid) and its hash. Taking a reaction back deletes its replies.
    pub replies: Vec<(u64, String)>,
}

impl Reacted {
    /// The hashes of `fid`'s replies that are this emoji.
    pub fn replies_of(&self, fid: u64) -> Vec<String> {
        self.replies.iter().filter(|(by, _)| *by == fid).map(|(_, hash)| hash.clone()).collect()
    }
}

/// The emoji reactions among a cast's `replies`: each emoji with whom it
/// came from, an account counted once for each emoji, most first, then the
/// earliest first.
pub fn reactions<'a>(replies: impl IntoIterator<Item = &'a Cast>) -> Vec<Reacted> {
    let mut replies: Vec<&Cast> = replies.into_iter().collect();
    replies.sort_by_key(|c| c.ts);
    gather(replies.into_iter().filter_map(|cast| Some((reaction(cast)?, cast.author.clone(), cast.hash.clone()))))
}

/// A cast's emoji reactions as a listing carries them (`ReactionReply`,
/// kept by the cron), gathered as [`reactions`] gathers replies. A listing
/// has no names for them: each account comes by its fid alone.
pub fn reactions_kept(kept: &[ReactionReply]) -> Vec<Reacted> {
    let mut kept: Vec<&ReactionReply> = kept.iter().collect();
    kept.sort_by_key(|r| r.ts);
    gather(kept.into_iter().filter_map(|reply| {
        let account =
            UserSummary { fid: reply.fid, username: String::new(), display_name: String::new(), pfp_url: None };
        Some((find(&reply.emoji)?, account, reply.hash.clone()))
    }))
}

/// Reactions in the order they came, as `(emoji, whose, reply hash)`: each
/// emoji with whom it came from, an account counted once for each emoji,
/// most first, then the earliest first.
fn gather(replies: impl Iterator<Item = (&'static Emoji, UserSummary, String)>) -> Vec<Reacted> {
    let mut out: Vec<Reacted> = Vec::new();
    for (emoji, account, hash) in replies {
        let at = match out.iter().position(|r| r.emoji == emoji) {
            Some(at) => at,
            None => {
                out.push(Reacted { emoji, by: Vec::new(), replies: Vec::new() });
                out.len() - 1
            }
        };
        out[at].replies.push((account.fid, hash));
        if !out[at].by.iter().any(|u| u.fid == account.fid) {
            out[at].by.push(account);
        }
    }
    // Stable: equal counts keep the order they first came in.
    out.sort_by_key(|r| std::cmp::Reverse(r.by.len()));
    out
}

/// Who reacted, for a chip's label: up to three accounts by name, then how
/// many others ("@alice, @bob and 3 others"). The reader is "you": first
/// when their reaction is from this visit (`yours_now`), and in place of a
/// name a listing doesn't carry. Accounts with no name count among the
/// others, or alone ("2 accounts").
pub fn who_reacted(by: &[UserSummary], reader: Option<u64>, yours_now: bool) -> String {
    let mut named: Vec<String> = Vec::new();
    let mut unnamed = 0;
    for account in by {
        if !account.username.is_empty() {
            named.push(format!("@{}", account.username));
        } else if Some(account.fid) == reader {
            named.insert(0, "you".into());
        } else {
            unnamed += 1;
        }
    }
    let shown: Vec<&str> = named.iter().take(3).map(String::as_str).collect();
    let others = named.len() - shown.len() + unnamed;
    let list = match (shown.is_empty(), others) {
        (true, 0) => String::new(),
        (true, 1) => "1 account".into(),
        (true, n) => format!("{n} accounts"),
        (false, 0) => shown.join(", "),
        (false, 1) => format!("{} and 1 other", shown.join(", ")),
        (false, n) => format!("{} and {n} others", shown.join(", ")),
    };
    match (yours_now, list.is_empty()) {
        (true, true) => "you".into(),
        (true, false) if shown.is_empty() => format!("you and {}", list.replace("account", "other")),
        (true, false) => format!("you, {list}"),
        (false, _) => list,
    }
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

    fn reply(fid: u64, text: &str, ts: i64) -> Cast {
        Cast {
            hash: format!("0x{ts:040x}"),
            author: UserSummary { fid, username: format!("user{fid}"), display_name: String::new(), pfp_url: None },
            text: text.into(),
            mentions: vec![],
            ts,
            parent_hash: Some("0x01".into()),
            parent_url: None,
            channel: None,
            embeds: vec![],
            likes: 0,
            recasts: 0,
            replies: 0,
            mini_app: false,
            references: Default::default(),
            spam: false,
        }
    }

    #[test]
    fn a_reaction_is_one_emoji_and_nothing_else() {
        let heart = find("pepe_heart_hug").unwrap();
        assert_eq!(reaction(&reply(1, ":pepe_heart_hug:", 1)), Some(heart));
        assert_eq!(reaction(&reply(1, "  :PEPE_HEART_HUG:\n", 1)), Some(heart));
        for not in [":pepe_heart_hug: gm", "gm", ":pepe_heart_hug::pepe_no:", ":not_a_pepe:", "", ":pepe_heart_hug"] {
            assert_eq!(reaction(&reply(1, not, 1)), None, "{not:?}");
        }
        let mut with_image = reply(1, ":pepe_no:", 1);
        with_image.embeds.push(crate::model::Embed::Image { url: "https://example.com/a.png".into() });
        assert_eq!(reaction(&with_image), None);
        let mut spam = reply(1, ":pepe_no:", 1);
        spam.spam = true;
        assert_eq!(reaction(&spam), None);
    }

    #[test]
    fn reactions_count_each_account_once_per_emoji_most_first() {
        let replies = [
            reply(1, ":pepe_no:", 10),
            reply(2, ":pepe_heart_hug:", 11),
            reply(3, ":pepe_heart_hug:", 12),
            reply(2, ":pepe_heart_hug:", 13),
            reply(4, "not a reaction", 14),
            reply(2, ":pepe_no:", 15),
            reply(5, ":pepe_pls:", 9),
        ];
        let found = reactions(&replies);
        let summary: Vec<(&str, Vec<u64>)> =
            found.iter().map(|r| (r.emoji.code, r.by.iter().map(|u| u.fid).collect())).collect();
        // Two accounts each for the NO sign and the heart, the NO sign
        // first, as it came first; then the one that came before both.
        assert_eq!(summary, [("pepe_no", vec![1, 2]), ("pepe_heart_hug", vec![2, 3]), ("pepe_pls", vec![5])]);
        // Each reply is kept, an account's two hearts both: taking the
        // reaction back deletes them all.
        let heart = &found[1];
        assert_eq!(heart.replies_of(2), [format!("0x{:040x}", 11), format!("0x{:040x}", 13)]);
        assert_eq!(heart.replies_of(3), [format!("0x{:040x}", 12)]);
        assert!(heart.replies_of(9).is_empty());
        assert!(reactions(std::iter::empty()).is_empty());
    }

    #[test]
    fn kept_reactions_gather_as_replies_do_with_no_names() {
        let kept = |hash: &str, emoji: &str, fid: u64, ts: i64| ReactionReply {
            hash: hash.into(),
            emoji: emoji.into(),
            fid,
            ts,
        };
        // Out of order, an account twice, and an emoji no longer in the pack.
        let found = reactions_kept(&[
            kept("0x3", "pepe_heart_hug", 2, 12),
            kept("0x1", "pepe_no", 1, 10),
            kept("0x2", "pepe_heart_hug", 3, 11),
            kept("0x4", "pepe_heart_hug", 2, 13),
            kept("0x5", "pepe_gone_from_the_pack", 4, 14),
        ]);
        let summary: Vec<(&str, Vec<u64>, usize)> =
            found.iter().map(|r| (r.emoji.code, r.by.iter().map(|u| u.fid).collect(), r.replies.len())).collect();
        assert_eq!(summary, [("pepe_heart_hug", vec![3, 2], 3), ("pepe_no", vec![1], 1)]);
        assert!(found.iter().flat_map(|r| &r.by).all(|u| u.username.is_empty()));
        assert_eq!(found[0].replies_of(2), ["0x3", "0x4"]);
        assert!(reactions_kept(&[]).is_empty());
    }

    #[test]
    fn who_reacted_names_up_to_three_and_counts_the_rest() {
        let named = |fid: u64, name: &str| UserSummary {
            fid,
            username: name.into(),
            display_name: String::new(),
            pfp_url: None,
        };
        let unnamed = |fid: u64| named(fid, "");
        // A post page names everyone, the reader too.
        let four = [named(1, "ann"), named(2, "bob"), named(3, "cat"), named(4, "dan")];
        assert_eq!(who_reacted(&four[..2], Some(2), false), "@ann, @bob");
        assert_eq!(who_reacted(&four, None, false), "@ann, @bob, @cat and 1 other");
        assert_eq!(who_reacted(&four[..1], None, true), "you, @ann");
        assert_eq!(who_reacted(&[], None, true), "you");
        // A listing names no one: the reader is "you", the rest are counted.
        assert_eq!(who_reacted(&[unnamed(7)], None, false), "1 account");
        assert_eq!(who_reacted(&[unnamed(7), unnamed(8)], None, false), "2 accounts");
        assert_eq!(who_reacted(&[unnamed(7), unnamed(8)], Some(8), false), "you and 1 other");
        assert_eq!(who_reacted(&[unnamed(7)], Some(7), false), "you");
        assert_eq!(who_reacted(&[unnamed(7), unnamed(8)], None, true), "you and 2 others");
        assert_eq!(who_reacted(&[unnamed(7)], None, true), "you and 1 other");
    }

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
        // Named "no…" first, then "nods yes"; not the piano, nor the dino.
        assert_eq!(codes(&search("no")), ["pepe_no", "pepe_nopers", "pepe_noted", "pepe_noodles", "pepe_yes"]);
        // A word's start, in a name or a description; not its middle.
        assert_eq!(codes(&search("clap")), ["pepe_clap", "pepe_sit_clap", "pepe_weird_clap"]);
        assert!(codes(&search("eat")).contains(&"pepe_cookie"));
        assert!(!codes(&search("eat")).contains(&"pepe_sweat"));
        assert_eq!(codes(&search("spell")), ["pepe_wizard"]);
        assert_eq!(codes(&search(":PEPE_NO:"))[0], "pepe_no");
        assert_eq!(codes(&search("heart hug")), ["pepe_heart_hug"]);
        assert_eq!(codes(&search("big heart")), ["pepe_heart_hug"]);
        assert_eq!(codes(&search("hug")), ["pepe_hug", "pepe_huggers", "pepe_heart_hug", "pepe_cat", "pepe_friends"]);
        assert!(search("zzz").is_empty());
    }

    #[test]
    fn the_pack_runs_group_by_group_with_the_five_first() {
        // Each group is one run of the pack, in `Group::ALL`'s order.
        let mut runs: Vec<Group> = Vec::new();
        for e in PACK {
            if runs.last() != Some(&e.group) {
                assert!(!runs.contains(&e.group), "{:?} comes in two runs", e.group);
                runs.push(e.group);
            }
        }
        assert_eq!(runs, Group::ALL);
        let first: Vec<&str> = PACK.iter().take(5).map(|e| e.code).collect();
        assert_eq!(first, ["pepe_no", "pepe_smug", "pepe_blush", "pepe_heart_hug", "pepe_sit"]);
        assert_eq!(Group::ALL.iter().map(|g| g.emoji().count()).sum::<usize>(), PACK.len());
        let labels: BTreeSet<&str> = Group::ALL.iter().map(|g| g.label()).collect();
        assert_eq!(labels.len(), Group::ALL.len());
    }

    #[test]
    fn every_emoji_says_what_it_shows_in_its_own_words() {
        // Two alike would be two buttons with one name.
        let whats: BTreeSet<&str> = PACK.iter().map(|e| e.what).collect();
        assert_eq!(whats.len(), PACK.len());
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
    /// a still and a moving tile of the same size, and a credit with its
    /// source; every file is one of theirs.
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
            let tile = fs::read(dir.join("v1").join(format!("{}.tile.webp", sticker.code))).unwrap();
            assert_eq!(webp_size(&tile), (u32::from(sticker.width), u32::from(STICKER_HEIGHT)), "{}", sticker.code);
            // Animated (VP8X's animation flag), and lighter than the GIF.
            assert!(&tile[12..16] == b"VP8X" && tile[20] & 0x02 != 0, "{} doesn't move", sticker.code);
            assert!(tile.len() < gif.len(), "{}'s tile is {} bytes", sticker.code, tile.len());
            expected.insert(format!("{}.gif", sticker.code));
            expected.insert(format!("{}.still.webp", sticker.code));
            expected.insert(format!("{}.tile.webp", sticker.code));
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

    /// The picker's makers are the credits' (`MAKERS`): one for each Pepe
    /// in the pack, as the emoji's credits name them, and the stickers'.
    #[test]
    fn makers_are_as_credited() {
        let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web");
        let credited = |file: &str| -> Vec<(String, String)> {
            let text = fs::read_to_string(web.join(file)).unwrap();
            text.lines()
                .filter(|line| line.starts_with("| `"))
                .map(|line| {
                    let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
                    let source = cells.iter().position(|c| c.starts_with("https://")).unwrap();
                    (cells[0].trim_matches('`').trim_matches(':').to_owned(), cells[source - 1].to_owned())
                })
                .collect()
        };
        let ours: Vec<(String, String)> = MAKERS.iter().map(|(code, by)| (code.to_string(), by.to_string())).collect();
        assert_eq!(ours, credited("emoji/CREDITS.md"));
        let codes: BTreeSet<&str> = PACK.iter().map(|e| e.code).collect();
        assert_eq!(MAKERS.iter().map(|(code, _)| *code).collect::<BTreeSet<_>>(), codes);
        for (code, by) in credited("stickers/CREDITS.md") {
            assert_eq!(MAKERS.iter().find(|(c, _)| *c == code).map(|(_, b)| *b), Some(by.as_str()), "{code}");
        }
        assert_eq!(maker("pepe_no"), Some("lalalexuss"));
        assert_eq!(maker("pepe_eat"), None, "its maker's account is gone");
        assert_eq!(maker("not_a_pepe"), None);
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
            // Its terms, as the credits explain them, are its source's.
            let terms = if credit.contains("| https://7tv.app/emotes/") {
                "7TV public"
            } else if credit.contains("| https://www.frankerfacez.com/emoticon/") {
                "FFZ public"
            } else if credit.contains("| https://emoji.gg/emoji/") {
                "emoji.gg free"
            } else {
                panic!("no source for {}", e.code)
            };
            assert_eq!(credit.rsplit('|').nth(1).map(str::trim), Some(terms), "{}", e.code);
            assert!(credits.contains(&format!("- **{terms}**: ")), "the terms {terms} aren't explained");
        }
        assert_eq!(files, expected, "files that aren't in the pack, or missing");
    }
}
