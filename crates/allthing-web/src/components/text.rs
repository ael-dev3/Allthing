// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cast text with links, mentions and channel references.

use allthing_core::model::Mention;
use allthing_core::text::{Token, paragraphs, tokenize};
use leptos::prelude::*;

use crate::util::{channel_href, user_href};

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
        })
        .collect()
}

/// Paragraphs of linked text; single newlines become line breaks.
#[component]
pub fn CastText(text: String, mentions: Vec<Mention>) -> impl IntoView {
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
            view! { <p>{lines}</p> }
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
