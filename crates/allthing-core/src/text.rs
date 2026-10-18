// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cast text: mention splicing, excerpts, and tokenizing into links.

use crate::model::Mention;

/// Farcaster stores mentions out of band: the text omits them and
/// `mentions_positions` give UTF-8 byte offsets where "@username" belongs.
/// Hypersnap passes those through as zero-width ranges; Neynar returns text
/// with mentions already spliced in (non-zero-width ranges). Handle both.
pub fn splice_mentions(text: &str, mentions: &[Mention], ranges: &[(u32, u32)]) -> String {
    if mentions.is_empty() || ranges.len() != mentions.len() {
        return text.to_owned();
    }
    if ranges.iter().any(|(start, end)| start != end) {
        return text.to_owned();
    }
    let mut inserts: Vec<(usize, &Mention)> =
        ranges.iter().zip(mentions).map(|((start, _), mention)| (*start as usize, mention)).collect();
    inserts.sort_by_key(|(at, _)| *at);

    let mut out = String::with_capacity(text.len() + mentions.len() * 12);
    let mut cursor = 0;
    for (at, mention) in inserts {
        let mut position = at.clamp(cursor, text.len());
        while !text.is_char_boundary(position) {
            position -= 1;
        }
        let position = position.max(cursor);
        out.push_str(&text[cursor..position]);
        out.push('@');
        out.push_str(&mention.username);
        cursor = position;
    }
    out.push_str(&text[cursor..]);
    out
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `max` characters of a cast, whitespace-collapsed, cut at a word
/// boundary.
pub fn truncate_title(text: &str, max: usize) -> String {
    let flat = collapse_whitespace(text);
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    let base = match cut.rfind(' ') {
        Some(space) if cut[..space].chars().count() * 10 > max * 6 => &cut[..space],
        _ => cut.as_str(),
    };
    let base = base.trim_end_matches(|c: char| c.is_whitespace() || ".,;:!?…-".contains(c));
    format!("{base}…")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Text(String),
    Url(String),
    /// A mention the cast actually carries, without the "@".
    Mention(String),
    /// A "/channel" reference, lowercased, without the "/".
    Channel(String),
}

/// Length of the "http://" or "https://" prefix `rest` starts with, if any.
fn url_scheme_len(rest: &str) -> Option<usize> {
    let prefix = |p: &str| rest.len() >= p.len() && rest.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes());
    if prefix("https://") {
        Some(8)
    } else if prefix("http://") {
        Some(7)
    } else {
        None
    }
}

fn trim_url(raw: &str) -> &str {
    let mut url = raw;
    loop {
        match url.chars().next_back() {
            Some(c) if ".,!?;:'\"".contains(c) => url = &url[..url.len() - 1],
            Some(')') => {
                // Keep a closing paren the URL itself opened, e.g. wikipedia links.
                if url.matches('(').count() >= url.matches(')').count() {
                    return url;
                }
                url = &url[..url.len() - 1];
            }
            _ => return url,
        }
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

/// Split cast text into plain text, URLs, mentions and /channel references.
/// Only "@names" that the cast actually mentions become mention tokens.
pub fn tokenize(text: &str, mentions: &[Mention]) -> Vec<Token> {
    let known: Vec<String> = mentions.iter().map(|m| m.username.to_lowercase()).collect();
    let mut tokens: Vec<Token> = Vec::new();
    let push_text = |tokens: &mut Vec<Token>, value: &str| {
        if value.is_empty() {
            return;
        }
        if let Some(Token::Text(prev)) = tokens.last_mut() {
            prev.push_str(value);
        } else {
            tokens.push(Token::Text(value.to_owned()));
        }
    };

    let mut last = 0;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let ch = rest.chars().next().unwrap_or(' ');
        let prev = text[..i].chars().next_back();

        if let Some(scheme_len) = url_scheme_len(rest) {
            let end = rest.find(|c: char| c.is_whitespace() || "<>\"'".contains(c)).unwrap_or(rest.len());
            let href = trim_url(&rest[..end]);
            if href.len() > scheme_len {
                push_text(&mut tokens, &text[last..i]);
                tokens.push(Token::Url(href.to_owned()));
                i += href.len();
                last = i;
                continue;
            }
        } else if ch == '@' {
            let name_end = rest[1..].find(|c: char| !is_name_char(c)).map_or(rest.len(), |n| n + 1);
            let name = rest[1..name_end].trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
            if name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
                let whole = &rest[..1 + name.len()];
                push_text(&mut tokens, &text[last..i]);
                if known.iter().any(|k| k.eq_ignore_ascii_case(name)) {
                    tokens.push(Token::Mention(name.to_owned()));
                } else {
                    push_text(&mut tokens, whole);
                }
                i += whole.len();
                last = i;
                continue;
            }
        } else if ch == '/' && prev.is_none_or(|p| p.is_whitespace() || p == '(') {
            let run_end =
                rest[1..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).map_or(rest.len(), |n| n + 1);
            let name = &rest[1..run_end];
            let next = rest[run_end..].chars().next();
            let blocked = next.is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '-'));
            if !name.is_empty() && name.len() <= 32 && name.starts_with(|c: char| c.is_ascii_alphanumeric()) && !blocked
            {
                push_text(&mut tokens, &text[last..i]);
                tokens.push(Token::Channel(name.to_ascii_lowercase()));
                i += run_end;
                last = i;
                continue;
            }
        }
        i += ch.len_utf8();
    }
    push_text(&mut tokens, &text[last..]);
    tokens
}

/// Split text into paragraphs on blank lines; single newlines stay inside.
pub fn paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(current.join("\n"));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        out.push(current.join("\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mention(username: &str, fid: u64) -> Mention {
        Mention { username: username.into(), fid }
    }

    #[test]
    fn splices_mentions_at_utf8_byte_offsets() {
        let text = "Nothing held back! 🌴\n\n tipped  576 UCI.";
        let out = splice_mentions(text, &[mention("specktacular", 1), mention("dylanb.eth", 2)], &[(25, 25), (33, 33)]);
        assert_eq!(out, "Nothing held back! 🌴\n\n@specktacular tipped @dylanb.eth 576 UCI.");
    }

    #[test]
    fn splices_out_of_order_and_at_the_end() {
        let out = splice_mentions("Good Night ", &[mention("b", 2), mention("a", 1)], &[(11, 11), (0, 0)]);
        assert_eq!(out, "@aGood Night @b");
    }

    #[test]
    fn leaves_inline_mentions_and_mismatches_alone() {
        let text = "hi @dwr how are you";
        assert_eq!(splice_mentions(text, &[mention("dwr", 3)], &[(3, 7)]), text);
        assert_eq!(splice_mentions("hi", &[mention("x", 1)], &[]), "hi");
        // An offset inside a multi-byte character snaps back to its start.
        assert_eq!(splice_mentions("é!", &[mention("x", 1)], &[(1, 1)]), "@xé!");
    }

    #[test]
    fn excerpts() {
        assert_eq!(truncate_title("hello\n\n  world", 120), "hello world");
        let long = format!("{}end", "word ".repeat(40));
        let excerpt = truncate_title(&long, 120);
        assert!(excerpt.ends_with('…'));
        assert!(excerpt.chars().count() <= 121);
        assert!(!excerpt.ends_with(" …"));
        assert_eq!(truncate_title(&"🎉".repeat(130), 120), format!("{}…", "🎉".repeat(120)));
    }

    #[test]
    fn tokenizes_urls_mentions_and_channels() {
        let tokens = tokenize("gm @dwr, see https://example.com/a?b=1. join /farcaster now", &[mention("dwr", 3)]);
        assert_eq!(
            tokens,
            vec![
                Token::Text("gm ".into()),
                Token::Mention("dwr".into()),
                Token::Text(", see ".into()),
                Token::Url("https://example.com/a?b=1".into()),
                Token::Text(". join ".into()),
                Token::Channel("farcaster".into()),
                Token::Text(" now".into()),
            ]
        );
    }

    #[test]
    fn ignores_unknown_mentions_and_paths_in_words() {
        let tokens = tokenize("email me@example.com and/or @nobody", &[]);
        assert_eq!(tokens, vec![Token::Text("email me@example.com and/or @nobody".into())]);
        assert_eq!(tokenize("/a_b /x/y", &[]), vec![Token::Text("/a_b /x/y".into())]);
        assert_eq!(tokenize(&format!("/{}", "a".repeat(33)), &[]).len(), 1);
    }

    #[test]
    fn keeps_balanced_parentheses_in_urls() {
        let tokens = tokenize("(see https://en.wikipedia.org/wiki/Foo_(bar))", &[]);
        assert_eq!(tokens[1], Token::Url("https://en.wikipedia.org/wiki/Foo_(bar)".into()));
        assert_eq!(tokens[2], Token::Text(")".into()));
    }

    #[test]
    fn links_dotted_mentions_and_channels_in_parens() {
        let tokens = tokenize("thanks @balajis.eth.", &[mention("balajis.eth", 37)]);
        assert_eq!(
            tokens,
            vec![Token::Text("thanks ".into()), Token::Mention("balajis.eth".into()), Token::Text(".".into())]
        );
        let tokens = tokenize("(/Dev) HTTPS://X.COM", &[]);
        assert_eq!(
            tokens,
            vec![
                Token::Text("(".into()),
                Token::Channel("dev".into()),
                Token::Text(") ".into()),
                Token::Url("HTTPS://X.COM".into()),
            ]
        );
    }

    #[test]
    fn splits_paragraphs() {
        assert_eq!(paragraphs("a\nb\n\n\nc\n"), vec!["a\nb".to_string(), "c".to_string()]);
        assert_eq!(paragraphs("a\n \nb"), vec!["a".to_string(), "b".to_string()]);
        assert!(paragraphs("\n\n").is_empty());
    }
}
