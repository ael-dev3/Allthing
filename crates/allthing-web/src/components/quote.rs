// SPDX-License-Identifier: AGPL-3.0-or-later
//! Quoted casts, shown inside the cast that quotes them.

use allthing_core::model::Cast;
use allthing_core::profile::UserCard;
use allthing_core::text::truncate_title;
use leptos::prelude::*;

use crate::Notes;
use crate::components::chips::{TimeAgo, UserChip};
use crate::components::media::MediaGrid;
use crate::components::text::CastText;
use crate::nav::card_click;
use crate::util::cast_href;

/// How much of a quoted cast's text a feed card shows.
pub const FEED_QUOTE_CHARS: usize = 280;
/// How much of it the post page shows.
pub const POST_QUOTE_CHARS: usize = 400;
/// Levels of quotes shown inline: a quote, a quote of it, and a quote of
/// that. A quote beyond them is a link. The listing payload carries this many.
pub use allthing_core::quotes::QUOTE_DEPTH;

/// The quotes a cast makes, one level down from `depth` (0 for a feed's
/// cast): each carried quote as a card, its own quotes nested in turn, and a
/// link where the payload does not carry the quote or the depth is spent.
pub fn nested_quotes(cast: &Cast, notes: Option<&Notes>, depth: usize) -> Vec<AnyView> {
    cast.quoted()
        .map(|hash| match notes.and_then(|n| n.quotes.get(hash).map(|q| (n, q))) {
            Some((notes, quoted)) if depth < QUOTE_DEPTH => {
                let author_card = notes.users.get(&quoted.author.fid).cloned();
                view! {
                    <QuoteCard cast=quoted.clone() author_card notes=Some(notes.clone()) depth=depth + 1/>
                }
                .into_any()
            }
            _ => quote_chip(hash),
        })
        .collect()
}

/// A link to a quoted cast that isn't shown as a card: one the payload
/// doesn't carry, or a quote inside a quote.
pub fn quote_chip(hash: &str) -> AnyView {
    view! { <a class="quote-chip" href=cast_href(hash)>"↳ quotes a cast"</a> }.into_any()
}

/// A quoted cast as a small card inside the cast that quotes it: author,
/// time, text and media. A click on the card, away from its links and media,
/// opens the quoted cast, not the post around it; the time is its link for
/// keyboards and screen readers, and its media opens in the media viewer. A
/// hidden quoted cast (spam, a mini app) says so instead of showing its
/// content, and a quote inside the quote stays a link.
#[component]
pub fn QuoteCard(
    cast: Cast,
    /// How much of the text to show.
    #[prop(default = FEED_QUOTE_CHARS)]
    max_chars: usize,
    /// What the payload says about the quoted cast's author, for their
    /// profile card.
    #[prop(optional_no_strip)]
    author_card: Option<UserCard>,
    /// The payload's quotes, so a quote inside this one shows inline.
    #[prop(optional_no_strip)]
    notes: Option<Notes>,
    /// This card's level: 1 for a quote of a listed cast.
    #[prop(default = 1)]
    depth: usize,
) -> impl IntoView {
    let href = cast_href(&cast.hash);
    let click = card_click(href.clone());
    let body = if crate::miniapps::hides(&cast) {
        // No label on the cast itself: a mini app is a format, said as such.
        let note = if cast.spam { "Quoted cast hidden" } else { "Quoted cast hidden: mini app" };
        view! { <p class="quote-hidden">{note}</p> }.into_any()
    } else {
        let text = truncate_title(&cast.text, max_chars);
        view! {
            {(!text.is_empty()).then(|| view! {
                <div class="quote-text"><CastText text mentions=cast.mentions.clone()/></div>
            })}
            <MediaGrid cast=cast.clone()/>
            {nested_quotes(&cast, notes.as_ref(), depth)}
        }
        .into_any()
    };
    view! {
        <blockquote class="quote" on:click=click>
            <div class="cast-meta">
                <span class="byline">
                    <UserChip user=cast.author.clone() show_name=true card=author_card/>
                    <a class="when" href=href><span class="visually-hidden">"Open quoted cast, "</span><TimeAgo ts=cast.ts/></a>
                </span>
            </div>
            {body}
        </blockquote>
    }
}
