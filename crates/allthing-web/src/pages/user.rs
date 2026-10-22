// SPDX-License-Identifier: AGPL-3.0-or-later
//! A profile: who the account is, how it voted, where its wallets are
//! active, and its casts.

use std::sync::Arc;

use allthing_core::api::{UserPayload, user_path, wallets_path};
use allthing_core::format::short_address;
use allthing_core::listing::{replies_only, top_level};
use allthing_core::model::{Joined, User};
use allthing_core::params::username_from_param;
use allthing_core::profile::UserCard;
use allthing_core::ranking::{Sort, sorted};
use allthing_core::wallet::WalletActivity;
use leptos::prelude::*;
use leptos::wasm_split::{SplitLoaderError, wasm_split};
use leptos_router::hooks::{use_params_map, use_query_map};

use crate::Notes;
use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons};
use crate::components::chips::{Avatar, Count};
use crate::components::facts::{JoinedFact, NO_WALLETS, VoteFact, WALLETS_WHAT, WalletShares, WalletsHelp};
use crate::components::favorite::FavoriteButton;
use crate::components::layout::{Columns, ErrorPanel, NotFound, SortTabs, StaleNotice};
use crate::components::popover::Popover;
use crate::components::show_less::ShowLessButton;
use crate::components::text::CastText;
use crate::util::{set_title, user_href, with_query};
use wasm_bindgen::JsCast;

/// The page, whose code is fetched the first time it's needed
/// (`crate::split`): as it opens, or ahead of it.
#[wasm_split(user, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_user))]
pub fn user_page() -> Result<AnyView, SplitLoaderError> {
    Ok(view! { <UserPage/> }.into_any())
}

#[component]
fn UserPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let name = Memo::new(move |_| {
        params.read().get("name").or_else(|| query.read().get("name")).and_then(|raw| username_from_param(&raw))
    });
    let replies_tab = Memo::new(move |_| query.read().get("tab").as_deref() == Some("replies"));
    let sort = Memo::new(move |_| Sort::parse(query.read().get("sort").as_deref(), Sort::New));
    let data = LocalResource::new(move || {
        let name = name.get();
        async move {
            match name {
                Some(name) => Some(api::get::<UserPayload>(&user_path(&name)).await),
                None => None,
            }
        }
    });
    Effect::new(move |_| {
        let user = match data.get() {
            Some(Some(Ok(fetched))) => fetched.data.user.clone(),
            None | Some(None) => match name.get().and_then(|name| api::seen_person(&name)) {
                Some(preview) => preview.user,
                None => return,
            },
            Some(Some(Err(_))) => return,
        };
        set_title(&format!("{} (@{})", user.name(), user.username));
    });

    move || {
        let Some(username) = name.get() else {
            return view! { <NotFound/> }.into_any();
        };
        match data.get() {
            None | Some(None) => match api::seen_person(&username) {
                // An account a page showed: its header at once, from what the
                // visit knows; its casts once the payload arrives.
                Some(preview) => {
                    let path = user_href(&preview.user.username);
                    // The casts' placeholders only if they're slow to come.
                    view! {
                        <Columns class="profile-columns">
                            <ProfileHeader user=preview.user has_card=preview.has_card joined=preview.joined/>
                            <ProfileTabs path replies_tab sort/>
                            <div class="later"><CastSkeletons count=3/></div>
                        </Columns>
                    }
                    .into_any()
                }
                None => view! {
                    <div class="profile skeleton" aria-busy="true"><span class="sk sk-avatar"></span><span class="sk sk-title"></span><span class="sk sk-title short"></span></div>
                    <CastSkeletons count=3/>
                }
                .into_any(),
            },
            Some(Some(Err(failure))) => view! { <ErrorPanel failure/> }.into_any(),
            Some(Some(Ok(Fetched { data, stale, .. }))) => {
                let payload: Arc<UserPayload> = data;
                let user = payload.user.clone();
                // The casts here are the account's own: their profile card
                // is the profile itself.
                let mut notes = payload.notes.clone();
                notes.users.insert(user.fid, UserCard::of(&user));
                let notes: Notes = Arc::new(notes);
                let path = user_href(&user.username);
                let pool = payload.pool.clone();
                // The reader's own casts above, until the list has them.
                let listed = Signal::stored(pool.iter().map(|cast| cast.hash.to_ascii_lowercase()).collect());
                // Switching sort or tab reorders the list, not the page.
                let casts = Signal::derive(move || {
                    if replies_tab.get() { replies_only(&pool) } else { sorted(&top_level(&pool), sort.get()) }
                });
                view! {
                    <Columns class="profile-columns">
                        <ProfileHeader user=user.clone() has_card=true joined=user.joined() addresses_known=true/>
                        <ProfileTabs path replies_tab sort/>
                        <StaleNotice stale/>
                        {crate::components::outgoing_profile(user.fid, replies_tab.into(), listed)}
                        {move || view! { <CastList casts notes=notes.clone() show_parent=replies_tab.get()/> }}
                    </Columns>
                }
                .into_any()
            }
        }
    }
}

/// The profile's Casts and Replies tabs, and the casts' sort.
#[component]
fn ProfileTabs(path: String, replies_tab: Memo<bool>, sort: Memo<Sort>) -> impl IntoView {
    let casts_link = path.clone();
    let replies_link = with_query(&path, &[("tab", Some("replies".to_owned()))]);
    view! {
        <nav class="tabs" aria-label="Profile sections">
            <a href=casts_link data-noscroll="" class:selected=move || !replies_tab.get() aria-current=move || (!replies_tab.get()).then_some("page")>"Casts"</a>
            <a href=replies_link data-noscroll="" class:selected=move || replies_tab.get() aria-current=move || replies_tab.get().then_some("page")>"Replies"</a>
        </nav>
        {move || (!replies_tab.get()).then(|| view! {
            <div class="listing-head"><SortTabs path=path.clone() current=sort.get() default_sort=Sort::New/></div>
        })}
    }
}

/// Who the account is. `has_card` says whether the bio and counts are
/// known: a profile previewed from a feed, before its payload, may only
/// know the picture and names, and shows placeholders for the rest.
#[component]
fn ProfileHeader(
    user: User,
    has_card: bool,
    joined: Option<Joined>,
    /// Whether `user` has its verified addresses: a profile's payload
    /// does, a preview from a feed doesn't.
    #[prop(optional)]
    addresses_known: bool,
) -> impl IntoView {
    view! {
        <section class="profile" aria-label="Profile" aria-busy=(!has_card).then_some("true")>
            <div class="profile-top">
                <Avatar url=user.pfp_url.clone() size=88 large=true/>
                <div class="profile-id">
                    <div class="profile-name-row">
                        <h1 class="profile-name">{user.name().to_owned()}</h1>
                        {follow_button(user.fid, &user.username)}
                    </div>
                    // "· fid 99" keeps together: a long handle can wrap, but
                    // not the dot from the fid or the fid from its number.
                    <p class="handle">"@"{user.username.clone()}<span class="muted">{format!(" ·\u{a0}fid\u{a0}{}", user.fid)}</span></p>
                    {if has_card {
                        view! {
                            <p class="counts">
                                {followers(user.fid, user.follower_count)}
                                <span class="sep" aria-hidden="true">"·"</span>
                                <Count n=user.following_count label="following"/>
                            </p>
                        }
                        .into_any()
                    } else {
                        view! { <p class="counts"><span class="sk sk-meta"></span></p> }.into_any()
                    }}
                </div>
                {corner_buttons(user.fid, &user.username)}
            </div>
            {if !has_card {
                view! { <div class="bio"><span class="sk sk-title"></span></div> }.into_any()
            } else if user.bio.trim().is_empty() {
                view! { <p class="bio muted">"No bio."</p> {website(&user)} }.into_any()
            } else {
                view! { <div class="bio"><CastText text=user.bio.clone() mentions=Vec::new()/></div> {website(&user)} }.into_any()
            }}
            // Its facts keep their places as they come: the vote's row is
            // always there, and the wallets' block keeps one height.
            <dl class="facts">
                {joined.map(|joined| view! { <JoinedFact joined/> })}
                <VoteFact fid=user.fid always=true/>
            </dl>
            <WalletTile user=user.clone() addresses_known/>
        </section>
    }
}

const LINK: &str = "M6.8 9.2l2.4-2.4M7.4 4.6l1-1a2.6 2.6 0 0 1 3.7 3.7l-1 1M8.6 11.4l-1 1a2.6 2.6 0 0 1-3.7-3.7l1-1";

/// The account's website, under its bio, as the profile's payload gives it:
/// a link out, on one line. It comes with the bio, whose whole text the
/// header waits for too. Checked again here (`Website::checked`), so that a
/// payload can only link to a web address, and says where the link goes.
fn website(user: &User) -> Option<AnyView> {
    let website = user.website.as_ref()?.checked()?;
    Some(
        view! {
            <p class="profile-website">
                <a href=website.href target="_blank" rel="noopener noreferrer nofollow ugc">
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=LINK/></svg>
                    <span class="visually-hidden">"Website: "</span>
                    <span class="profile-website-label">{website.label}</span>
                </a>
            </p>
        }
        .into_any(),
    )
}

/// Follow, for a signed-in reader who can (not in the static copy).
#[cfg(not(feature = "static-host"))]
fn follow_button(fid: u64, username: &str) -> AnyView {
    view! { <crate::components::writer::FollowButton fid username=username.to_owned()/> }.into_any()
}

#[cfg(feature = "static-host")]
fn follow_button(_: u64, _: &str) -> AnyView {
    ().into_any()
}

/// At the header's top right, or in a row of their own under it on a
/// phone: Favorite and Show less on anyone's profile but the reader's own.
/// Their own (signed in, the same fid) has Cast in the site header instead,
/// or floating at the bottom right on a phone (`components::compose`).
#[cfg(not(feature = "static-host"))]
fn corner_buttons(fid: u64, username: &str) -> AnyView {
    let session = crate::components::signin::use_session();
    let username = username.to_owned();
    // Redrawn only when signing in or out makes it the reader's own or not.
    let mine = Memo::new(move |_| session.user.with(|u| u.as_ref().is_some_and(|u| u.fid == fid)));
    (move || {
        if mine.get() {
            ().into_any()
        } else {
            view! {
                <span class="leaning-buttons">
                    <FavoriteButton fid username=username.clone()/>
                    <ShowLessButton fid username=username.clone()/>
                </span>
            }
            .into_any()
        }
    })
    .into_any()
}

/// The static copy has no one signed in, and no composer: Favorite and
/// Show less on every profile.
#[cfg(feature = "static-host")]
fn corner_buttons(fid: u64, username: &str) -> AnyView {
    view! {
        <span class="leaning-buttons">
            <FavoriteButton fid username=username.to_owned()/>
            <ShowLessButton fid username=username.to_owned()/>
        </span>
    }
    .into_any()
}

/// The follower count, with the reader's follow or unfollow here.
#[cfg(not(feature = "static-host"))]
fn followers(fid: u64, n: u64) -> AnyView {
    view! { <crate::components::writer::FollowerCount fid n/> }.into_any()
}

#[cfg(feature = "static-host")]
fn followers(_: u64, n: u64) -> AnyView {
    view! { <Count n label="followers"/> }.into_any()
}

/// Wallet activity by chain, loaded separately because it queries many RPCs,
/// with the account's verified addresses one click away. It sits with the
/// profile's facts, and keeps its height as they come: one block whatever
/// it says, or, for an account with no verified wallets, which it can tell
/// without asking (from the profile, or from activity a card already
/// read), the line that says so.
#[component]
fn WalletTile(user: User, #[prop(optional)] addresses_known: bool) -> impl IntoView {
    let fid = user.fid;
    let addresses: Vec<(&'static str, String)> = user
        .eth_addresses
        .iter()
        .map(|a| ("EVM", a.clone()))
        .chain(user.sol_addresses.iter().map(|a| ("SOL", a.clone())))
        .collect();
    let read_before = api::ready::<WalletActivity>(&wallets_path(fid));
    let none = if addresses_known {
        addresses.is_empty()
    } else {
        read_before.as_ref().is_some_and(|read| read.data.addresses.is_empty())
    };
    // What the endpoint would say, kept as its answer: the cards of this
    // account's casts, and profile cards opened here, don't ask either.
    if addresses_known && none && read_before.is_none() {
        let activity = WalletActivity {
            fid,
            addresses: Vec::new(),
            chains: Vec::new(),
            total: 0,
            sources: Vec::new(),
            fetched_at: js_sys::Date::now() as i64,
        };
        api::know(&wallets_path(fid), &activity);
    }
    view! {
        <section class="wallets profile-wallets" aria-label="Wallet activity by chain">
            <div class="wallets-head">
                <h2>"Wallet activity " <WalletsHelp/></h2>
                {(!addresses.is_empty()).then(|| view! { <WalletsButton addresses=addresses.clone()/> })}
            </div>
            <p class="wallets-what muted small">{WALLETS_WHAT}</p>
            {if none {
                view! { <p class="muted small share-message">{NO_WALLETS}</p> }.into_any()
            } else {
                view! { <WalletShares fid/> }.into_any()
            }}
        </section>
    }
}

/// "Wallets": every verified address, each copyable in one click.
#[component]
fn WalletsButton(addresses: Vec<(&'static str, String)>) -> impl IntoView {
    let count = addresses.len();
    let addresses = StoredValue::new(addresses);
    view! {
        <Popover
            label="Verified wallets"
            trigger_label=format!("Show {count} verified wallet{}", if count == 1 { "" } else { "s" })
            trigger_class="wallets-button"
            // Hover and keyboard focus open it on screens that hover; a tap
            // or click still toggles it. Fixed, so the profile card's
            // overflow doesn't cut the list off at its bottom edge.
            hover=true
            fixed=true
            trigger=move || view! { "Wallets" <span class="badge">{count}</span> }
            content=move || view! {
                <ul class="wallet-list">
                    {addresses.get_value().into_iter().map(|(kind, address)| view! { <WalletRow kind address/> }).collect_view()}
                </ul>
            }
        />
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CopyState {
    Idle,
    Pending,
    Copied,
    Failed,
}

#[component]
fn WalletRow(kind: &'static str, address: String) -> impl IntoView {
    let state = RwSignal::new(CopyState::Idle);
    let attempt = RwSignal::new(0_u32);
    let full = address.clone();
    let copy = move |_| {
        if state.get_untracked() == CopyState::Pending {
            return;
        }
        let generation = attempt.get_untracked().wrapping_add(1);
        attempt.set(generation);
        state.set(CopyState::Pending);
        let full = full.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = copy_text(&full).await;
            let _ = state.try_set(if result.is_ok() { CopyState::Copied } else { CopyState::Failed });
            if result.is_ok() {
                set_timeout(
                    move || {
                        if attempt.try_get_untracked() == Some(generation) {
                            let _ = state.try_set(CopyState::Idle);
                        }
                    },
                    std::time::Duration::from_millis(1400),
                );
            }
        });
    };
    view! {
        <li class:copy-failed=move || state.get() == CopyState::Failed>
            <span class="chain-tag">{kind}</span>
            <code>{short_address(&address)}</code>
            <button type="button" class="copy" on:click=copy aria-label=format!("Copy {address}")
                aria-disabled=move || (state.get() == CopyState::Pending).to_string()>
                {move || match state.get() {
                    CopyState::Pending => "Copying…",
                    CopyState::Copied => "Copied",
                    CopyState::Idle | CopyState::Failed => "Copy",
                }}
            </button>
            <span class="visually-hidden" aria-live="polite">{move || match state.get() {
                CopyState::Copied => "Copied to clipboard",
                CopyState::Failed => "Could not copy. Select and copy the address manually.",
                CopyState::Idle | CopyState::Pending => "",
            }}</span>
            <Show when=move || state.get() == CopyState::Failed>
                <span class="wallet-copy-error muted small">
                    "Could not copy. Select and copy the address manually. "<code>{address.clone()}</code>
                </span>
            </Show>
        </li>
    }
}

/// Report success only after the browser accepts the clipboard write.
async fn copy_text(text: &str) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsValue;
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window"))?;
    let clipboard = js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("clipboard"))?;
    let write = js_sys::Reflect::get(&clipboard, &JsValue::from_str("writeText"))?.dyn_into::<js_sys::Function>()?;
    let promise = write.call1(&clipboard, &JsValue::from_str(text))?.dyn_into::<js_sys::Promise>()?;
    wasm_bindgen_futures::JsFuture::from(promise).await?;
    Ok(())
}
