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

/// How many characters one of the pack's emoji counts as in an excerpt:
/// about the room its image takes in a line of text.
const EMOJI_CHARS: usize = 3;

/// The first `max` characters of a cast, whitespace-collapsed, cut at a word
/// boundary. A shortcode of the pack's emoji counts as the room its image
/// takes, and is never cut: the emoji comes whole or not at all.
pub fn truncate_title(text: &str, max: usize) -> String {
    let flat = collapse_whitespace(text);
    let spans = emoji_spans(&flat);
    // Each character or whole shortcode, as where it ends and its room.
    let mut units: Vec<(usize, usize)> = Vec::with_capacity(flat.len());
    let mut next = spans.iter().peekable();
    let mut i = 0;
    while i < flat.len() {
        match next.peek() {
            Some(&&(start, end, _)) if start == i => {
                next.next();
                units.push((end, EMOJI_CHARS));
                i = end;
            }
            _ => {
                i += flat[i..].chars().next().map_or(1, char::len_utf8);
                units.push((i, 1));
            }
        }
    }
    let mut room = 0;
    let fit = units
        .iter()
        .take_while(|(_, width)| {
            room += width;
            room <= max
        })
        .count();
    if fit == units.len() {
        return flat;
    }
    let end = fit.checked_sub(1).map_or(0, |last| units[last].0);
    let cut = &flat[..end];
    // A cut before a space, or beside an emoji, already falls between words.
    let between = flat[end..].starts_with(' ') || spans.iter().any(|&(start, stop, _)| stop == end || start == end);
    let base = match cut.rfind(' ') {
        Some(space) if !between => {
            let before: usize = units.iter().take_while(|(e, _)| *e <= space).map(|(_, width)| width).sum();
            if before * 10 > max * 6 { &cut[..space] } else { cut }
        }
        _ => cut,
    };
    let trimmed = base.trim_end_matches(|c: char| c.is_whitespace() || ".,;:!?…-".contains(c));
    // A shortcode at the end keeps its closing colon.
    let shortcode = spans.iter().map(|&(_, stop, _)| stop).rfind(|&stop| stop <= base.len()).unwrap_or(0);
    format!("{}…", &base[..trimmed.len().max(shortcode)])
}

/// The start of `text` for a box that shows its first few lines: as
/// written, newlines kept, at most `max` characters, cut between words and
/// ended with "…". A shortcode of the pack's emoji is never cut.
pub fn text_start(text: &str, max: usize) -> String {
    let text = text.trim();
    let Some((end, _)) = text.char_indices().nth(max) else {
        return text.to_owned();
    };
    let mut cut = &text[..end];
    if let Some(&(start, _, _)) = emoji_spans(text).iter().find(|&&(start, stop, _)| start < end && end < stop) {
        cut = &text[..start];
    }
    // Inside a word, back to the last one's end, unless that loses more
    // than a fifth.
    let inside = !text[cut.len()..].starts_with(char::is_whitespace);
    if let Some(space) = cut.rfind(char::is_whitespace).filter(|&space| inside && space * 5 > cut.len() * 4) {
        cut = &cut[..space];
    }
    format!("{}…", cut.trim_end())
}

/// The link `rest` starts with, if any, as `tokenize` reads one.
fn url_at(rest: &str) -> Option<&str> {
    let scheme_len = url_scheme_len(rest)?;
    let end = rest.find(|c: char| c.is_whitespace() || "<>\"'".contains(c)).unwrap_or(rest.len());
    let href = trim_url(&rest[..end]);
    (href.len() > scheme_len).then_some(href)
}

/// Where `text` has shortcodes of the pack's emoji, in bytes: those
/// `tokenize` reads as emoji, so none inside a link.
fn emoji_spans(text: &str) -> Vec<(usize, usize, &'static crate::emoji::Emoji)> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if let Some(url) = url_at(rest) {
            i += url.len();
        } else if let Some((emoji, len)) = crate::emoji::shortcode_at(rest) {
            spans.push((i, i + len, emoji));
            i += len;
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    spans
}

/// `text` as plain runs and the pack's emoji, with nothing linked, for an
/// excerpt inside a link or a button: only `Token::Text` and
/// `Token::Emoji`, with the text as written.
pub fn text_and_emoji(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut last = 0;
    for (start, end, emoji) in emoji_spans(text) {
        if start > last {
            tokens.push(Token::Text(text[last..start].to_owned()));
        }
        tokens.push(Token::Emoji(emoji));
        last = end;
    }
    if last < text.len() {
        tokens.push(Token::Text(text[last..].to_owned()));
    }
    tokens
}

/// `text` with what lies between `from` and `to` (offsets in UTF-16 units,
/// as a text box counts them) replaced by `put`, and the offset just after
/// what was put. Offsets past the end are the end.
pub fn splice(text: &str, from: usize, to: usize, put: &str) -> (String, usize) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let (from, to) = (from.min(to).min(units.len()), to.max(from).min(units.len()));
    let mut out = String::from_utf16_lossy(&units[..from]);
    out.push_str(put);
    let at = from + put.encode_utf16().count();
    out.push_str(&String::from_utf16_lossy(&units[to..]));
    (out, at)
}

/// What lies between `from` and `to` in `text`, offsets in UTF-16 units.
pub fn between(text: &str, from: usize, to: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let (from, to) = (from.min(to).min(units.len()), to.max(from).min(units.len()));
    String::from_utf16_lossy(&units[from..to])
}

/// `text` as the composer's text box shows it while it is written: runs of
/// text, and the pack's emoji, each with its shortcode as written, which the
/// box keeps so that the draft is what was typed. The same shortcodes are
/// emoji as in a cast's text (not inside a link).
pub fn written_emoji(text: &str) -> Vec<(Option<&'static crate::emoji::Emoji>, &str)> {
    let mut parts = Vec::new();
    let mut last = 0;
    for (start, end, emoji) in emoji_spans(text) {
        if start > last {
            parts.push((None, &text[last..start]));
        }
        parts.push((Some(emoji), &text[start..end]));
        last = end;
    }
    if last < text.len() {
        parts.push((None, &text[last..]));
    }
    parts
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Text(String),
    Url(String),
    /// A mention the cast actually carries, without the "@".
    Mention(String),
    /// A "/channel" reference, lowercased, without the "/".
    Channel(String),
    /// A shortcode of Allthing's emoji pack, ":pepe_no:".
    Emoji(&'static crate::emoji::Emoji),
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

/// Where `text` names an account as "@name": each mention's byte range,
/// with the name in lowercase. An "@" right after a letter, a digit, "_"
/// or "/" (an email address, a link) names no one.
fn mention_spans(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = text[i..].find('@').map(|n| n + i) {
        let prev = text[..at].chars().next_back();
        let rest = &text[at + 1..];
        let end = rest.find(|c: char| !is_name_char(c)).unwrap_or(rest.len());
        let name = rest[..end].trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
        let free = !prev.is_some_and(|p| p.is_alphanumeric() || p == '_' || p == '/');
        if free && name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            out.push((at, at + 1 + name.len(), name.to_ascii_lowercase()));
            i = at + 1 + name.len();
        } else {
            i = at + 1;
        }
    }
    out
}

/// The accounts a draft names as "@name", in lowercase, each once and in
/// order, as many as a cast can mention: the names to look up before it is
/// posted.
pub fn mention_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (_, _, name) in mention_spans(text) {
        if !names.contains(&name) && names.len() < crate::message::MAX_MENTIONS {
            names.push(name);
        }
    }
    names
}

/// A draft's text as Farcaster stores a cast's: each "@name" whose account
/// `fid_of` knows taken out, and listed with that account and the byte
/// offset where it stood, in order (as many as a cast can mention). Names
/// it doesn't know stay in the text as they are.
pub fn split_mentions(text: &str, fid_of: impl Fn(&str) -> Option<u64>) -> (String, Vec<(u64, u32)>) {
    let mut out = String::with_capacity(text.len());
    let mut mentions = Vec::new();
    let mut last = 0;
    for (start, end, name) in mention_spans(text) {
        if mentions.len() == crate::message::MAX_MENTIONS {
            break;
        }
        let Some(fid) = fid_of(&name) else { continue };
        out.push_str(&text[last..start]);
        mentions.push((fid, out.len() as u32));
        last = end;
    }
    out.push_str(&text[last..]);
    (out, mentions)
}

/// Split cast text into plain text, URLs, mentions, /channel references and
/// emoji shortcodes. Only "@names" that the cast actually mentions become
/// mention tokens, and only shortcodes of the pack emoji.
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

        if let Some(href) = url_at(rest) {
            push_text(&mut tokens, &text[last..i]);
            tokens.push(Token::Url(href.to_owned()));
            i += href.len();
            last = i;
            continue;
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
        } else if ch == ':'
            && let Some((emoji, len)) = crate::emoji::shortcode_at(rest)
        {
            push_text(&mut tokens, &text[last..i]);
            tokens.push(Token::Emoji(emoji));
            i += len;
            last = i;
            continue;
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
    fn emoji_shortcodes_of_the_pack_become_emoji() {
        let no = crate::emoji::find("pepe_no").unwrap();
        let gm = crate::emoji::find("pepe_gm").unwrap();
        assert_eq!(
            tokenize("gm :pepe_gm: frens :pepe_no::PEPE_NO:", &[]),
            vec![
                Token::Text("gm ".into()),
                Token::Emoji(gm),
                Token::Text(" frens ".into()),
                Token::Emoji(no),
                Token::Emoji(no),
            ]
        );
        // Unknown ones, times, and a link's colons stay as they are.
        assert_eq!(
            tokenize("at 10:30:45 :smile: :pepe_nope:", &[]),
            vec![Token::Text("at 10:30:45 :smile: :pepe_nope:".into())]
        );
        // A link keeps its colons (bar a trailing one, as for any link).
        assert_eq!(
            tokenize("https://x.com/:pepe_no:", &[]),
            vec![Token::Url("https://x.com/:pepe_no".into()), Token::Text(":".into())]
        );
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
    fn a_text_start_keeps_its_lines_and_cuts_between_words() {
        assert_eq!(text_start("  two\nlines \n", 600), "two\nlines");
        let long = format!("first line\n{}tail", "word ".repeat(30));
        let start = text_start(&long, 40);
        assert_eq!(start, "first line\nword word word word word word…");
        assert!(start.chars().count() <= 41);
        // One long word is cut where it must.
        assert_eq!(text_start(&"x".repeat(50), 10), format!("{}…", "x".repeat(10)));
        // A shortcode the cut falls in goes whole: no ":pepe_" left over.
        let cut = text_start(&format!("{} :pepe_gm: and more", "y".repeat(30)), 34);
        assert_eq!(cut, format!("{}…", "y".repeat(30)));
        assert_eq!(text_start(":pepe_gm: hi", 9), ":pepe_gm:…");
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
        // A cut already between words keeps the last one.
        assert_eq!(truncate_title("aaa bbb ccc ddd", 11), "aaa bbb ccc…");
    }

    #[test]
    fn excerpts_keep_shortcodes_whole() {
        // Each emoji counts as three characters, not its shortcode's length.
        assert_eq!(truncate_title("hello :pepe_heart_hug:", 9), "hello :pepe_heart_hug:");
        assert_eq!(truncate_title("gm :pepe_gm: frens :PEPE_NO:", 16), "gm :pepe_gm: frens :PEPE_NO:");
        // One that doesn't fit is left out whole, not cut.
        assert_eq!(truncate_title("gm :pepe_gm: frens :PEPE_NO:", 15), "gm :pepe_gm: frens…");
        // One at the end keeps its closing colon.
        assert_eq!(truncate_title("one two :pepe_gm: three four", 11), "one two :pepe_gm:…");
        assert_eq!(truncate_title(&":pepe_gm:".repeat(50), 30), format!("{}…", ":pepe_gm:".repeat(10)));
        // Whatever the length, no shortcode is cut.
        let text = "ab :pepe_heart_hug: cd :pepe_no:ef :pepe_sit_clap: gh";
        for max in 0..30 {
            let excerpt = truncate_title(text, max);
            let partial = text_and_emoji(&excerpt).iter().any(|t| matches!(t, Token::Text(t) if t.contains(":pepe")));
            assert!(!partial, "{max}: {excerpt}");
        }
    }

    #[test]
    fn text_and_emoji_links_nothing() {
        let gm = crate::emoji::find("pepe_gm").unwrap();
        // Mentions and channels stay as written, unlinked.
        assert_eq!(
            text_and_emoji("/Base @Bob :pepe_gm:x"),
            vec![Token::Text("/Base @Bob ".into()), Token::Emoji(gm), Token::Text("x".into())]
        );
        assert_eq!(text_and_emoji(":PEPE_GM:"), vec![Token::Emoji(gm)]);
        // A link keeps its colons, as in cast text; other words stay text.
        assert_eq!(
            text_and_emoji("see https://x.com/:pepe_gm: at 10:30 :smile:"),
            vec![Token::Text("see https://x.com/:pepe_gm: at 10:30 :smile:".into())]
        );
        assert!(text_and_emoji("").is_empty());
    }

    #[test]
    fn splices_count_as_a_text_box_does() {
        // "é" is one unit, "🐸" two.
        assert_eq!(splice("gm 🐸 fren", 3, 5, ":pepe_gm:"), ("gm :pepe_gm: fren".to_owned(), 12));
        assert_eq!(splice("ab", 2, 2, "\n"), ("ab\n".to_owned(), 3));
        assert_eq!(splice("ab", 9, 1, "x"), ("ax".to_owned(), 2));
        assert_eq!(splice("", 0, 0, ""), (String::new(), 0));
        assert_eq!(between("é 🐸 x", 2, 4), "🐸");
        assert_eq!(between("abc", 3, 1), "bc");
    }

    #[test]
    fn written_emoji_keep_their_shortcodes_as_typed() {
        let gm = crate::emoji::find("pepe_gm").unwrap();
        let text = "gm :PEPE_GM::pepe_gm:\nhttps://x.com/:pepe_gm: :nope:";
        let parts = written_emoji(text);
        assert_eq!(
            parts,
            vec![
                (None, "gm "),
                (Some(gm), ":PEPE_GM:"),
                (Some(gm), ":pepe_gm:"),
                (None, "\nhttps://x.com/:pepe_gm: :nope:"),
            ]
        );
        // Put back together, it is the text as typed.
        assert_eq!(parts.iter().map(|(_, part)| *part).collect::<String>(), text);
        assert!(written_emoji("").is_empty());
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

    #[test]
    fn a_drafts_mentions_are_found_and_taken_out() {
        let text = "gm @Alice and @bob.eth, cc @alice. mail me@example.com or see https://x.com/@carol @nobody";
        assert_eq!(mention_names(text), ["alice", "bob.eth", "nobody"]);
        let fids = |name: &str| match name {
            "alice" => Some(1),
            "bob.eth" => Some(2),
            _ => None,
        };
        let (out, mentions) = split_mentions(text, fids);
        assert_eq!(out, "gm  and , cc . mail me@example.com or see https://x.com/@carol @nobody");
        assert_eq!(mentions, [(1, 3), (2, 8), (1, 13)]);
        // The offsets are where splice_mentions puts the names back.
        let named = |fid, username: &str| Mention { fid, username: username.into() };
        let back = splice_mentions(
            &out,
            &[named(1, "alice"), named(2, "bob.eth"), named(1, "alice")],
            &mentions.iter().map(|(_, at)| (*at, *at)).collect::<Vec<_>>(),
        );
        assert_eq!(back, "gm @alice and @bob.eth, cc @alice. mail me@example.com or see https://x.com/@carol @nobody");
        // UTF-8 offsets are in bytes; a cast mentions ten at most.
        assert_eq!(split_mentions("héllo @alice", fids), ("héllo ".to_owned(), vec![(1, 7)]));
        let crowd = "@a @b @c @d @e @f @g @h @i @j @k @l";
        assert_eq!(mention_names(crowd).len(), 10);
        assert_eq!(split_mentions(crowd, |_| Some(9)).1.len(), 10);
    }
}
