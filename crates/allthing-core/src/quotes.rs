// SPDX-License-Identifier: AGPL-3.0-or-later
//! Quoted casts in listings. A feed payload carries the casts that its casts
//! quote ([`crate::api::Annotations::quotes`]), so each card can show its
//! quote at once. These helpers decide which quotes a build still has to
//! read, which carried ones to keep, and what a quoted cast keeps.

use std::collections::{BTreeMap, HashSet};

use crate::embeds::single_image;
use crate::model::Cast;

/// Levels of quotes carried and displayed inline. A deeper quote stays a link.
pub const QUOTE_DEPTH: usize = 3;

/// The hashes that `casts` quote and `known` lacks: each once, in the order
/// the casts list them, and at most `limit`.
pub fn missing<'a>(
    casts: impl IntoIterator<Item = &'a Cast>,
    known: &BTreeMap<String, Cast>,
    limit: usize,
) -> Vec<String> {
    let mut seen = HashSet::new();
    casts
        .into_iter()
        .flat_map(Cast::quoted)
        .filter(|hash| !known.contains_key(*hash) && seen.insert(*hash))
        .take(limit)
        .map(str::to_owned)
        .collect()
}

/// Keep the quotes still reachable from `casts`, including their nested
/// quotes within [`QUOTE_DEPTH`], as a payload carries them into its next
/// build. Unrelated quotes and those beyond the display depth are released.
pub fn retain_quoted<'a>(quotes: &mut BTreeMap<String, Cast>, casts: impl IntoIterator<Item = &'a Cast>) {
    let mut frontier: Vec<String> = casts.into_iter().flat_map(Cast::quoted).map(str::to_owned).collect();
    let mut wanted = HashSet::new();
    for _ in 0..QUOTE_DEPTH {
        let mut next = Vec::new();
        for hash in frontier {
            if wanted.insert(hash.clone())
                && let Some(quote) = quotes.get(&hash)
            {
                next.extend(quote.quoted().map(str::to_owned));
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    quotes.retain(|hash, _| wanted.contains(hash));
}

/// A quoted cast as a listing carries it. A hidden one (spam, a mini app)
/// keeps its author and time but not its content, so its card can say that
/// it is hidden without showing it.
pub fn as_quote(mut cast: Cast) -> Cast {
    if cast.hidden() {
        cast.strip_hidden();
    }
    cast
}

/// The images a listing's card shows alone, each at its own shape
/// ([`crate::embeds::single_image`]): the cast's, and those of the casts it
/// shows as quote cards, within [`QUOTE_DEPTH`] (a hidden quote shows none,
/// nor its own quotes).
pub fn single_images<'a>(cast: &'a Cast, quotes: &'a BTreeMap<String, Cast>) -> Vec<&'a str> {
    let mut images: Vec<&str> = single_image(cast).into_iter().collect();
    let mut level = vec![cast];
    for _ in 0..QUOTE_DEPTH {
        level =
            level.iter().flat_map(|c| c.quoted()).filter_map(|hash| quotes.get(hash)).filter(|q| !q.hidden()).collect();
        images.extend(level.iter().filter_map(|quote| single_image(quote)));
    }
    images
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Embed, UserSummary};

    fn cast(hash: &str, quotes: &[&str]) -> Cast {
        Cast {
            hash: hash.into(),
            author: UserSummary { fid: 1, username: "a".into(), display_name: String::new(), pfp_url: None },
            text: format!("cast {hash}"),
            mentions: vec![],
            ts: 1_700_000_000,
            parent_hash: None,
            parent_url: None,
            channel: None,
            embeds: quotes.iter().map(|q| Embed::Cast { hash: (*q).into(), fid: Some(2) }).collect(),
            likes: 0,
            recasts: 0,
            replies: 0,
            mini_app: false,
            spam: false,
        }
    }

    #[test]
    fn lists_each_unknown_quote_once_in_order_up_to_a_limit() {
        let mut linking = cast("0x04", &[]);
        linking.embeds = vec![
            Embed::Link { url: "https://example.com".into(), image_url: None, title: None },
            Embed::Cast { hash: "0xq4".into(), fid: None },
        ];
        let casts = [cast("0x01", &["0xq1", "0xq2"]), cast("0x02", &["0xq2", "0xq3"]), cast("0x03", &[]), linking];
        let known = BTreeMap::from([("0xq3".to_owned(), cast("0xq3", &[]))]);
        assert_eq!(missing(&casts, &known, 100), ["0xq1", "0xq2", "0xq4"]);
        assert_eq!(missing(&casts, &known, 2), ["0xq1", "0xq2"]);
        assert!(missing(&casts, &known, 0).is_empty());
        assert!(missing(&casts[2..3], &BTreeMap::new(), 100).is_empty());
        assert_eq!(casts[0].quoted().collect::<Vec<_>>(), ["0xq1", "0xq2"]);
    }

    #[test]
    fn carried_quotes_stay_only_while_quoted() {
        let mut quotes = BTreeMap::from([
            ("0xq1".to_owned(), cast("0xq1", &[])),
            ("0xgone".to_owned(), cast("0xgone", &[])),
            ("0xq2".to_owned(), cast("0xq2", &[])),
        ]);
        retain_quoted(&mut quotes, &[cast("0x01", &["0xq1"]), cast("0x02", &["0xq2", "0xnew"])]);
        assert_eq!(quotes.keys().collect::<Vec<_>>(), ["0xq1", "0xq2"]);
        retain_quoted(&mut quotes, &[] as &[Cast]);
        assert!(quotes.is_empty());
    }

    #[test]
    fn carried_nested_quotes_stay_within_the_display_depth() {
        let mut quotes = BTreeMap::from([
            ("0xa".into(), cast("0xa", &["0xb"])),
            ("0xb".into(), cast("0xb", &["0xc"])),
            ("0xc".into(), cast("0xc", &["0xd"])),
            ("0xd".into(), cast("0xd", &[])),
            ("0xold".into(), cast("0xold", &["0xolder"])),
            ("0xolder".into(), cast("0xolder", &[])),
        ]);
        retain_quoted(&mut quotes, &[cast("0xroot", &["0xa"])]);
        assert_eq!(quotes.keys().map(String::as_str).collect::<Vec<_>>(), ["0xa", "0xb", "0xc"]);
        // Removing the root releases the whole graph, including descendants.
        retain_quoted(&mut quotes, &[] as &[Cast]);
        assert!(quotes.is_empty());
    }

    #[test]
    fn hidden_quotes_keep_who_and_when_but_not_their_content() {
        let mut spam = cast("0xq1", &["0xq9"]);
        spam.spam = true;
        spam.embeds.push(Embed::Image { url: "https://example.com/a.png".into() });
        let quote = as_quote(spam);
        assert!(quote.spam && quote.hidden());
        assert!(quote.text.is_empty() && quote.embeds.is_empty());
        assert_eq!((quote.author.username.as_str(), quote.ts), ("a", 1_700_000_000));

        let mut mini_app = cast("0xq2", &[]);
        mini_app.mini_app = true;
        assert!(as_quote(mini_app).text.is_empty());

        let shown = cast("0xq3", &["0xq9"]);
        assert_eq!(as_quote(shown.clone()), shown);
    }

    fn with(mut cast: Cast, embeds: &[Embed]) -> Cast {
        cast.embeds.extend(embeds.iter().cloned());
        cast
    }

    fn image(url: &str) -> Embed {
        Embed::Image { url: url.into() }
    }

    #[test]
    fn a_single_image_is_the_only_medium_and_an_image() {
        let photo = image("https://imagedelivery.net/a/photo/original");
        let video = Embed::Video { url: "https://stream.farcaster.xyz/v/manifest/video.m3u8".into() };
        let link = Embed::Link { url: "https://example.com".into(), image_url: None, title: None };
        let sticker = image("https://allthing.social/assets/stickers/v1/pepe_jam.gif");
        assert_eq!(
            single_image(&with(cast("0x1", &[]), std::slice::from_ref(&photo))),
            Some("https://imagedelivery.net/a/photo/original")
        );
        // A link, a quote or a sticker beside it doesn't count; another image or a video does.
        assert!(single_image(&with(cast("0x2", &["0xq"]), &[link.clone(), photo.clone(), sticker.clone()])).is_some());
        assert_eq!(single_image(&with(cast("0x3", &[]), &[photo.clone(), image("https://i.imgur.com/b.png")])), None);
        assert_eq!(single_image(&with(cast("0x4", &[]), &[photo.clone(), video.clone()])), None);
        assert_eq!(single_image(&with(cast("0x5", &[]), &[video])), None);
        assert_eq!(single_image(&with(cast("0x6", &[]), &[sticker, link])), None);
        assert_eq!(single_image(&cast("0x7", &[])), None);
    }

    #[test]
    fn a_card_waits_for_its_own_single_image_and_its_shown_quotes() {
        let a = |name: &str| image(&format!("https://imagedelivery.net/a/{name}/original"));
        let mut hidden = with(cast("0xhidden", &["0xbelow"]), &[a("hidden")]);
        hidden.spam = true;
        let quotes = BTreeMap::from([
            ("0xq1".to_owned(), with(cast("0xq1", &["0xq2", "0xhidden"]), &[a("q1")])),
            ("0xq2".to_owned(), with(cast("0xq2", &["0xq3"]), &[a("q2a"), a("q2b")])),
            ("0xq3".to_owned(), with(cast("0xq3", &["0xq4"]), &[a("q3")])),
            // A fourth level is a link, not a card.
            ("0xq4".to_owned(), with(cast("0xq4", &[]), &[a("q4")])),
            ("0xhidden".to_owned(), hidden),
            ("0xbelow".to_owned(), with(cast("0xbelow", &[]), &[a("below")])),
        ]);
        let card = with(cast("0xcard", &["0xq1", "0xunknown"]), &[a("own")]);
        assert_eq!(
            single_images(&card, &quotes),
            [
                "https://imagedelivery.net/a/own/original",
                "https://imagedelivery.net/a/q1/original",
                "https://imagedelivery.net/a/q3/original"
            ]
        );
        assert!(single_images(&cast("0xplain", &[]), &quotes).is_empty());
    }
}
