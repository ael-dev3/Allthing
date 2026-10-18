// SPDX-License-Identifier: AGPL-3.0-or-later
//! Profile cards. Hovering a profile picture or name, or resting keyboard
//! focus on it, shows a small card about the account; on touch screens a tap
//! shows it instead of opening the profile, and the card's "View profile"
//! link opens it. A mouse click still goes straight to the profile.
//!
//! The bio, counts and join date come with the page's payload, or from what
//! this visit has already shown of the account: a card never asks for the
//! profile itself. Its vote, its Farcaster app label and its wallet activity
//! load when it opens, as on the profile, and from the same sources.

use allthing_core::model::UserSummary;
use allthing_core::profile::UserCard;
use leptos::prelude::*;

use crate::api;
use crate::components::chips::{Avatar, Count};
use crate::components::facts::{JoinedFact, LabelFact, VoteFact, WalletShares};
use crate::components::popover::Popover;
use crate::components::text::CastText;
use crate::labels::spam_label;
use crate::util::user_href;

/// A card is at most this tall, room permitting; a taller one scrolls.
const CARD_MAX_PX: f64 = 440.0;

/// `children` are the links to the profile (its picture, name or handle)
/// that show the card.
#[component]
pub fn ProfileHover(
    user: UserSummary,
    /// What the page's payload says about the account, if anything.
    #[prop(optional_no_strip)]
    card: Option<UserCard>,
    /// Classes for the element around the links.
    #[prop(into, optional)]
    class: String,
    /// The links sit in a container that clips what overflows it (a
    /// channel's header): the card is placed against the viewport.
    #[prop(optional)]
    fixed: bool,
    /// The links sit in a dialog's scrolling list: the card is placed
    /// against the viewport, and keyboard focus is left to the dialog.
    #[prop(optional)]
    in_dialog: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let label = format!("Profile: {} (@{})", user.name(), user.username);
    view! {
        <Popover
            label
            hover=true
            links=true
            fixed=fixed || in_dialog
            no_focus_open=in_dialog
            max_px=CARD_MAX_PX
            class
            panel_class="profile-card"
            trigger=move || children()
            content=move || view! { <ProfileCard user=user.clone() card=card.clone()/> }
        />
    }
}

#[component]
fn ProfileCard(user: UserSummary, card: Option<UserCard>) -> impl IntoView {
    let href = user_href(&user.username);
    let fid = user.fid;
    // What the payload says, completed with what this visit saw elsewhere:
    // an account from another page (who liked a cast), or the first
    // username's date that only its profile knows.
    let seen = api::seen_card(fid);
    let card = match card {
        Some(card) => Some(card.filled_from(seen.as_ref())),
        None => seen,
    };
    let joined = card.as_ref().and_then(UserCard::joined);
    let label = LocalResource::new(move || spam_label(fid));
    view! {
        <div class="profile-card-head">
            <a class="profile-card-picture" href=href.clone() tabindex="-1" aria-hidden="true">
                <Avatar url=user.pfp_url.clone() size=44 large=true/>
            </a>
            <a class="profile-card-id" href=href.clone()>
                <strong class="profile-card-name">{user.name().to_owned()}</strong>
                <span class="profile-card-handle">"@"{user.username.clone()}</span>
            </a>
        </div>
        {match card {
            Some(card) => view! {
                {(!card.bio.is_empty()).then(|| view! {
                    <div class="profile-card-bio"><CastText text=card.bio.clone() mentions=Vec::new()/></div>
                })}
                <p class="profile-card-counts">
                    <Count n=card.followers label="followers"/>
                    <span class="sep" aria-hidden="true">"·"</span>
                    <Count n=card.following label="following"/>
                </p>
            }
            .into_any(),
            None => view! { <p class="profile-card-more muted small">"The bio and follower counts are on the profile."</p> }
                .into_any(),
        }}
        <dl class="facts">
            {joined.map(|joined| view! { <JoinedFact joined/> })}
            <VoteFact fid/>
            <LabelFact label/>
        </dl>
        <div class="profile-card-wallets">
            <p class="profile-card-label">"Wallet activity"</p>
            <WalletShares fid/>
        </div>
        <a class="popover-foot" href=href>"View profile →"</a>
    }
}
