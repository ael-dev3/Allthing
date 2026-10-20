// SPDX-License-Identifier: AGPL-3.0-or-later
//! Profile cards. Hovering a profile picture or name, or resting keyboard
//! focus on it, shows a small card about the account; on touch screens a tap
//! shows it instead of opening the profile, and the card's "View profile"
//! link opens it. A mouse click still goes straight to the profile.
//!
//! The bio, counts and join date come with the page's payload, or from what
//! this visit has already shown of the account: a card never asks for the
//! profile itself. Its vote and its wallet activity come from the same
//! sources as the profile's, preloaded for the authors of feed cards in view
//! and the people of an opened likes, recasts or replies preview; a card
//! whose facts are still on their way waits for them, briefly, out of sight.
//! One that shows before they come keeps its size when they do: its vote
//! row is always there ("Unknown" if it can't be read), and its wallet
//! section is one height whatever it says (`WalletShares`'s `compact`).

use std::time::Duration;

use allthing_core::api::{vote_path, wallets_path};
use allthing_core::model::UserSummary;
use allthing_core::profile::UserCard;
use allthing_core::vote::VotePayload;
use allthing_core::wallet::WalletActivity;
use leptos::prelude::*;

use crate::api;
use crate::components::chips::{Avatar, Count};
use crate::components::facts::{JoinedFact, VoteFact, WALLETS_WHAT, WalletShares, WalletsHelp, vote_data, wallet_data};
use crate::components::popover::Popover;
use crate::components::text::CastText;
use crate::util::user_href;

/// A card is at most this tall, room permitting; a taller one scrolls.
const CARD_MAX_PX: f64 = 440.0;
/// How long a card waits, out of sight, for a vote or wallet activity still
/// on its way, before it shows anyway.
const SETTLE: Duration = Duration::from_millis(500);

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
    /// The links sit in another popover's panel (a list of who liked a
    /// cast): the card opens beside that panel, against the viewport.
    #[prop(optional)]
    beside: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let label = format!("Profile: {} (@{})", user.name(), user.username);
    view! {
        <Popover
            label
            hover=true
            links=true
            fixed=fixed || in_dialog || beside
            beside
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
    // The vote and wallets: kept already when preloaded. Otherwise the card
    // shows once the reads its rows render are in, as they render them, or
    // after `SETTLE` at most.
    let kept = api::ready::<VotePayload>(&vote_path(fid)).is_some()
        && api::ready::<WalletActivity>(&wallets_path(fid)).is_some();
    let (vote, wallets) = (vote_data(fid), wallet_data(fid));
    let waited = RwSignal::new(kept);
    if !kept {
        set_timeout(
            move || {
                let _ = waited.try_set(true);
            },
            SETTLE,
        );
    }
    let settled = move || waited.get() || (vote.get().is_some() && wallets.get().is_some());
    view! {
        <div class="profile-card-body" class:settling=move || !settled()>
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
                <VoteFact fid data=vote always=true/>
            </dl>
            <div class="profile-card-wallets">
                <p class="profile-card-label" aria-description=WALLETS_WHAT>"Wallet activity " <WalletsHelp/></p>
                <WalletShares fid data=wallets compact=true/>
            </div>
            <a class="popover-foot" href=href>"View profile →"</a>
        </div>
    }
}
