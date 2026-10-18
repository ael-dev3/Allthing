// SPDX-License-Identifier: AGPL-3.0-or-later
//! Quoted casts in listings. A feed payload carries the casts that its casts
//! quote ([`crate::api::Annotations::quotes`]), so each card can show its
//! quote at once. These helpers decide which quotes a build still has to
//! read, which carried ones to keep, and what a quoted cast keeps.

use std::collections::{BTreeMap, HashSet};

use crate::model::Cast;

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

/// Keep only the quoted casts that one of `casts` still quotes, as when a
/// payload's quotes are carried into its next build.
pub fn retain_quoted<'a>(quotes: &mut BTreeMap<String, Cast>, casts: impl IntoIterator<Item = &'a Cast>) {
    let wanted: HashSet<&str> = casts.into_iter().flat_map(Cast::quoted).collect();
    quotes.retain(|hash, _| wanted.contains(hash.as_str()));
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
}
