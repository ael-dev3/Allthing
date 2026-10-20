// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cast text with links, mentions, channel references and emoji.

use allthing_core::emoji::Emoji;
use allthing_core::model::Mention;
use allthing_core::text::{Token, paragraphs, tokenize};
use leptos::prelude::*;

use crate::util::{channel_href, href, user_href};

/// A cast of this many emoji or fewer, and nothing else, shows them large.
const JUMBO_MAX: usize = 6;

/// "example.com/very/long/pa…" for a URL's visible text.
fn display_url(url: &str) -> String {
    let bare = url.split_once("://").map_or(url, |(_, rest)| rest);
    let bare = bare.strip_prefix("www.").unwrap_or(bare).trim_end_matches('/');
    if bare.chars().count() > 48 {
        let head: String = bare.chars().take(47).collect();
        format!("{head}…")
    } else {
        bare.to_owned()
    }
}

fn line(text: &str, mentions: &[Mention]) -> Vec<AnyView> {
    tokenize(text, mentions)
        .into_iter()
        .map(|token| match token {
            Token::Text(t) => t.into_any(),
            Token::Url(url) => view! {
                <a class="ext" href=url.clone() target="_blank" rel="noopener noreferrer nofollow ugc">{display_url(&url)}</a>
            }
            .into_any(),
            Token::Mention(name) => view! { <a class="mention" href=user_href(&name)>"@"{name.clone()}</a> }.into_any(),
            Token::Channel(id) => view! { <a class="channel-ref" href=channel_href(&id)>"/"{id.clone()}</a> }.into_any(),
            Token::Emoji(emoji) => emoji_view(emoji),
        })
        .collect()
}

/// One of the pack's emoji, in the text: a still of it for readers who ask
/// for less motion. Its alt text is its shortcode, so copied text keeps it,
/// and what it shows is read out as its description (no native tooltip).
fn emoji_view(emoji: &'static Emoji) -> AnyView {
    view! {
        <picture class="emoji">
            {emoji.animated.then(|| view! { <source media="(prefers-reduced-motion: reduce)" srcset=href(&emoji.still())/> })}
            <img
                src=href(&emoji.file())
                alt=format!(":{}:", emoji.code)
                aria-description=emoji.label()
                width=emoji.width
                height=emoji.height
                loading="lazy"
                decoding="async"
            />
        </picture>
    }
    .into_any()
}

/// Whether `text` is nothing but a few of the pack's emoji.
fn only_emoji(text: &str, mentions: &[Mention]) -> bool {
    let tokens = tokenize(text, mentions);
    let emoji = tokens.iter().filter(|t| matches!(t, Token::Emoji(_))).count();
    (1..=JUMBO_MAX).contains(&emoji)
        && tokens.iter().all(|t| match t {
            Token::Emoji(_) => true,
            Token::Text(text) => text.trim().is_empty(),
            _ => false,
        })
}

/// Paragraphs of linked text; single newlines become line breaks. A cast of
/// a few emoji and nothing else shows them large.
#[component]
pub fn CastText(text: String, mentions: Vec<Mention>) -> impl IntoView {
    let jumbo = only_emoji(&text, &mentions);
    paragraphs(&text)
        .into_iter()
        .map(|paragraph| {
            let lines: Vec<AnyView> = paragraph
                .split('\n')
                .enumerate()
                .flat_map(|(i, l)| {
                    let mut parts = if i > 0 { vec![view! { <br/> }.into_any()] } else { Vec::new() };
                    parts.extend(line(l, &mentions));
                    parts
                })
                .collect();
            view! { <p class:jumbo=jumbo>{lines}</p> }
        })
        .collect_view()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_urls_for_display() {
        assert_eq!(display_url("https://www.example.com/"), "example.com");
        assert_eq!(display_url("https://a.com/x"), "a.com/x");
        assert!(display_url(&format!("https://a.com/{}", "x".repeat(80))).ends_with('…'));
    }
}
