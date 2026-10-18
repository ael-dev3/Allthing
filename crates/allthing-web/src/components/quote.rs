// SPDX-License-Identifier: AGPL-3.0-or-later
//! Quoted casts, shown inside the cast that quotes them.

use allthing_core::model::Cast;
use allthing_core::text::truncate_title;
use leptos::prelude::*;

use crate::components::chips::{TimeAgo, UserChip};
use crate::components::media::MediaGrid;
use crate::components::text::CastText;
use crate::nav::card_click;
use crate::util::cast_href;

/// How much of a quoted cast's text a feed card shows.
pub const FEED_QUOTE_CHARS: usize = 280;
/// How much of it the post page shows.
pub const POST_QUOTE_CHARS: usize = 400;

/// A link to a quoted cast that isn't shown as a card: one the payload
/// doesn't carry, or a quote inside a quote.
pub fn quote_chip(hash: &str) -> AnyView {
    view! { <a class="quote-chip" href=cast_href(hash)>"↳ quotes a cast"</a> }.into_any()
}

/// A quoted cast as a small card inside the cast that quotes it: author,
/// time, text and media. A click on the card, away from its links and media,
/// opens the quoted cast, not the post around it; the time is its link for
/// keyboards and screen readers. A hidden quoted cast (spam, a mini app)
/// says so instead of showing its content, and a quote inside the quote
/// stays a link.
#[component]
pub fn QuoteCard(
    cast: Cast,
    /// Images open the media viewer instead of the quoted cast.
    #[prop(optional)]
    open_on_click: bool,
    /// How much of the text to show.
    #[prop(default = FEED_QUOTE_CHARS)]
    max_chars: usize,
) -> impl IntoView {
    let href = cast_href(&cast.hash);
    let click = card_click(href.clone());
    let body = if cast.hidden() {
        let reason = if cast.spam { "spam" } else { "mini app" };
        view! { <p class="quote-hidden">"Quoted cast hidden: "{reason}</p> }.into_any()
    } else {
        let text = truncate_title(&cast.text, max_chars);
        view! {
            {(!text.is_empty()).then(|| view! {
                <div class="quote-text"><CastText text mentions=cast.mentions.clone()/></div>
            })}
            <MediaGrid cast=cast.clone() open_on_click/>
            {cast.quoted().map(quote_chip).collect_view()}
        }
        .into_any()
    };
    view! {
        <blockquote class="quote" on:click=click>
            <div class="cast-meta">
                <UserChip user=cast.author.clone() show_name=true/>
                <span class="sep" aria-hidden="true">"·"</span>
                <a class="when" href=href><span class="visually-hidden">"Open quoted cast, "</span><TimeAgo ts=cast.ts/></a>
            </div>
            {body}
        </blockquote>
    }
}
