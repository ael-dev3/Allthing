// SPDX-License-Identifier: AGPL-3.0-or-later
//! Spam Allthing leaves out, by the cast's own content: links to apps on
//! `*.vercel.app`, anything about gig-bot, and DEGEN reward or airdrop bait
//! (`?degen=1` links, "Just unlocked my DEGEN rewards"). These are the
//! owner's content rules, matched on text, links and mentions, not on any
//! account label.

use crate::embeds::host_of;
use crate::text::{Token, tokenize};

/// Bait phrases, matched case-insensitively with any `$` removed.
const DEGEN_BAIT: &[&str] =
    &["unlocked my degen", "degen reward", "degen airdrop", "claim your degen", "claim degen", "free degen"];

/// gig-bot as it is written in text, usernames and hosts.
const GIG_BOT: &[&str] = &["gig-bot", "gigbot", "gig_bot"];

fn mentions_gig_bot(lower: &str) -> bool {
    GIG_BOT.iter().any(|name| lower.contains(name))
}

/// A link that is spam by itself: an app on `vercel.app`, a gig-bot host, or
/// a `degen=1` query parameter.
pub fn url_is_spam(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let host = host_of(&lower).unwrap_or_default();
    if host == "vercel.app" || host.ends_with(".vercel.app") || mentions_gig_bot(&host) {
        return true;
    }
    let query = lower.split_once('?').map_or("", |(_, q)| q);
    let query = query.split('#').next().unwrap_or("");
    query.split('&').any(|pair| pair == "degen=1")
}

/// Whether a cast is spam, from its text (with mentions spelled out), the
/// URLs of its embeds and the usernames it mentions. Links written in the
/// text count like embeds.
pub fn is_spam<'a>(
    text: &str,
    embed_urls: impl IntoIterator<Item = &'a str>,
    mentioned: impl IntoIterator<Item = &'a str>,
) -> bool {
    if embed_urls.into_iter().any(url_is_spam) {
        return true;
    }
    if mentioned.into_iter().any(|username| mentions_gig_bot(&username.to_ascii_lowercase())) {
        return true;
    }
    let lower = text.to_lowercase();
    if mentions_gig_bot(&lower) {
        return true;
    }
    let plain = lower.replace('$', "");
    if DEGEN_BAIT.iter().any(|bait| plain.contains(bait)) {
        return true;
    }
    tokenize(text, &[]).iter().any(|token| matches!(token, Token::Url(url) if url_is_spam(url)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> bool {
        is_spam(t, [], [])
    }

    #[test]
    fn links_to_vercel_apps_are_spam() {
        assert!(is_spam("play now", ["https://gig-bot-2-0.vercel.app/?degen=1"], []));
        assert!(is_spam("", ["https://warpletbox.vercel.app/embed"], []));
        assert!(is_spam("", ["https://vercel.app/x"], []));
        assert!(text("try https://some-game.vercel.app today"));
        assert!(!is_spam("", ["https://vercel.com/blog/post"], []));
        assert!(!is_spam("", ["https://notvercel.app/"], []));
    }

    #[test]
    fn anything_about_gig_bot_is_spam() {
        assert!(text("Earned 50 on Gig-Bot today"));
        assert!(text("gigbot pays out"));
        assert!(is_spam("hello @someone", [], ["gig-bot"]));
        assert!(is_spam("", ["https://gig-bot.xyz/claim"], []));
        assert!(!text("I played a gig last night, what a bot of a crowd"));
    }

    #[test]
    fn degen_reward_and_airdrop_bait_is_spam() {
        assert!(text("Just unlocked my DEGEN rewards 🎩"));
        assert!(text("Just unlocked my $DEGEN rewards"));
        assert!(text("The $DEGEN airdrop is live, claim your DEGEN here"));
        assert!(text("Claim $DEGEN now"));
        assert!(is_spam("check this", ["https://example.com/app?ref=3&degen=1"], []));
        assert!(text("go to https://example.com/?degen=1#top"));
        // Ordinary talk about DEGEN stays.
        assert!(!text("DEGEN is up today, and I tipped 100 $DEGEN to @dwr"));
        assert!(!is_spam("", ["https://example.com/?degen=10"], []));
        assert!(!is_spam("", ["https://degen.tips/"], []));
    }

    #[test]
    fn ordinary_casts_are_not_spam() {
        assert!(!text("gm farcaster"));
        assert!(!is_spam("new post", ["https://paragraph.xyz/@me/post", "https://i.imgur.com/a.png"], ["dwr"]));
    }
}
