// SPDX-License-Identifier: AGPL-3.0-or-later
//! Profile cards. Hovering a profile picture or name, or resting keyboard
//! focus on it, shows a small card about the account; on touch screens a tap
//! shows it instead of opening the profile, and the card's "View profile"
//! link opens it. A mouse click still goes straight to the profile.
//!
//! The bio, counts and registration come with the page's payload (or a
//! profile this visit already loaded); the vote and the Farcaster app's
//! label load only when a card opens.

use allthing_core::model::UserSummary;
use allthing_core::profile::UserCard;
use leptos::prelude::*;

use crate::api;
use crate::components::chips::{Avatar, Count};
use crate::components::facts::{LabelFact, RegisteredFact, VoteFact};
use crate::components::popover::Popover;
use crate::components::text::CastText;
use crate::labels::spam_label;
use crate::util::user_href;

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
    /// The links sit in a dialog's scrolling list: the card is placed
    /// against the viewport so the list doesn't cut it off, and keyboard
    /// focus is left to the dialog.
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
            fixed=in_dialog
            no_focus_open=in_dialog
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
    // Not in the page's payload (who liked a cast, say): what an earlier
    // feed or the profile itself told this visit. A card never asks the API.
    let card = card
        .or_else(|| api::seen_card(user.fid))
        .or_else(|| api::cached_profile(&user.username).map(|profile| UserCard::of(&profile)));
    let registered = card.as_ref().and_then(|c| c.registered_at);
    let fid = user.fid;
    let label = LocalResource::new(move || spam_label(fid));
    view! {
        <div class="profile-card-head">
            <a class="profile-card-picture" href=href.clone() tabindex="-1" aria-hidden="true">
                <Avatar url=user.pfp_url.clone() size=48 large=true/>
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
            None => view! { <p class="profile-card-more muted small">"The bio and follower counts are on the profile."</p> }.into_any(),
        }}
        <dl class="facts">
            {registered.map(|ts| view! { <RegisteredFact ts/> })}
            <VoteFact fid/>
            <LabelFact label/>
        </dl>
        <a class="popover-foot" href=href>"View profile →"</a>
    }
}
