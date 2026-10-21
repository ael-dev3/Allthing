// SPDX-License-Identifier: AGPL-3.0-or-later
//! Casts the reader handed to the Farcaster app, shown where they will be
//! while the reader posts them there: a reply above its post's replies, a
//! cast or a quote above the feed (a channel's, for a cast in it), and on
//! the reader's own profile. A clock says Farcaster doesn't have it yet;
//! once the node has it, a check, and a link to it (`Writer::watch`). Not
//! posted in time, it goes, and the draft is back in the composer. A cast
//! posted from here with the reader's key shows the same way at once, and
//! after a reload for ten minutes (`Writer::cast_posted`). Once the page
//! lists a cast itself, or it's deleted, its card goes. Not in the static
//! copy.

use std::collections::HashSet;

use allthing_core::message::Sent;
use leptos::prelude::*;

use crate::components::chips::UserChip;
use crate::components::signin::use_session;
use crate::components::text::CastText;
use crate::components::writer::{Outgoing, use_writer};
use crate::util::cast_href;

const CLOCK: &str = "M8 4.6V8l2.4 1.6";
const CHECK: &str = "M3.5 8.5l3 3 6-7";

/// Where on the page.
#[derive(Clone)]
pub enum Place {
    /// Above a post's replies: replies to it, or to any of its replies.
    Replies(HashSet<String>),
    /// Above a feed: casts and quotes, or on a channel's page, the casts
    /// that go to it.
    Feed(Option<String>),
    /// Above `fid`'s casts on their profile, when it is the reader's own:
    /// casts and quotes, or on its Replies tab (`replies`), replies.
    Profile { fid: u64, replies: Signal<bool> },
}

impl Place {
    fn holds(&self, cast: &Outgoing, reader: u64) -> bool {
        match (self, &cast.sent) {
            (Place::Replies(hashes), Sent::Reply { parent, .. }) => hashes.contains(parent),
            (Place::Feed(None), Sent::Cast { .. } | Sent::Quote { .. }) => true,
            (Place::Feed(Some(channel)), Sent::Cast { .. }) => cast.channel.as_ref() == Some(channel),
            (Place::Profile { fid, replies }, Sent::Cast { .. } | Sent::Quote { .. }) => {
                *fid == reader && !replies.get()
            }
            (Place::Profile { fid, replies }, Sent::Reply { .. }) => *fid == reader && replies.get(),
            _ => false,
        }
    }
}

/// The reader's casts that belong at `place` and that the page doesn't
/// list itself (`listed`, by hash) or that aren't deleted: replies in the
/// order they went, casts newest first, as the lists below them.
#[component]
pub fn OutgoingCasts(place: Place, #[prop(into)] listed: Signal<HashSet<String>>) -> impl IntoView {
    let (Some(writer), session) = (use_writer(), use_session()) else { return ().into_any() };
    let shown = Memo::new(move |_| {
        let Some(reader) = writer.reader() else { return Vec::new() };
        let mut casts = writer
            .outgoing()
            .into_iter()
            .filter(|cast| place.holds(cast, reader))
            .filter(|cast| {
                cast.hash
                    .as_ref()
                    .is_none_or(|hash| !listed.with(|listed| listed.contains(hash)) && !writer.deleted(hash))
            })
            .collect::<Vec<_>>();
        if !matches!(place, Place::Replies(_)) {
            casts.reverse();
        }
        casts
    });
    (move || {
        let user = session.user.get()?;
        let casts = shown.get();
        (!casts.is_empty()).then(|| {
            view! {
                <div class="outgoing-casts">
                    {casts.into_iter().map(|cast| view! { <OutgoingCard cast user=user.clone()/> }).collect_view()}
                </div>
            }
        })
    })
    .into_any()
}

#[component]
fn OutgoingCard(cast: Outgoing, user: allthing_core::model::UserSummary) -> impl IntoView {
    let posted = cast.hash.is_some();
    view! {
        <article class="cast-card outgoing" class:posted=posted>
            <header class="cast-meta">
                <UserChip user show_name=true card=None/>
                <span class="sep" aria-hidden="true">"·"</span>
                {match cast.hash {
                    Some(hash) => view! {
                        <a class="when outgoing-state" href=cast_href(&hash)>
                            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=CHECK/></svg>
                            "Posted"
                        </a>
                    }
                    .into_any(),
                    None => view! {
                        <span class="outgoing-state">
                            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.2"/><path d=CLOCK/></svg>
                            "Posting in Farcaster"
                        </span>
                    }
                    .into_any(),
                }}
            </header>
            {cast.replying_to.map(|name| view! { <p class="replying-to">"replying to @"{name}</p> })}
            {(!cast.text.is_empty()).then(|| view! { <div class="cast-text"><CastText text=cast.text mentions=Vec::new()/></div> })}
            {cast.image.map(|src| view! { <img class="outgoing-image" src=src alt="" loading="lazy"/> })}
            {cast.quoting.map(|name| view! { <p class="outgoing-quote">"quoting @"{name}</p> })}
        </article>
    }
}
