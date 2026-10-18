// SPDX-License-Identifier: AGPL-3.0-or-later
//! A profile: who the account is, how others rate and label it, where its
//! wallets are active, and its casts. Labels inform; they never hide content.

use std::sync::Arc;

use allthing_core::api::{UserPayload, user_path, vote_path, wallets_path};
use allthing_core::format::{full_number, score as format_score, short_address};
use allthing_core::labels::SpamLabel;
use allthing_core::listing::{replies_only, top_level};
use allthing_core::model::User;
use allthing_core::params::username_from_param;
use allthing_core::ranking::{Sort, sorted};
use allthing_core::time::{duration_since, format_date};
use allthing_core::vote::VotePayload;
use allthing_core::wallet::WalletActivity;
use leptos::prelude::*;
use leptos_router::hooks::{use_params_map, use_query_map};

use crate::Notes;
use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons};
use crate::components::chips::{Avatar, Count};
use crate::components::layout::{Columns, ErrorPanel, NotFound, SortTabs, StaleNotice};
use crate::components::popover::Popover;
use crate::components::text::CastText;
use crate::labels::{LabelState, spam_label};
use crate::util::{now, set_title, user_href, with_query};
use wasm_bindgen::JsCast;

/// FIDs registered before the November 2023 move of the ID registry to OP
/// Mainnet report the migration date as their registration.
const MIGRATION_DAY: std::ops::Range<i64> = 1_699_315_200..1_699_401_600;

#[component]
pub fn UserPage() -> impl IntoView {
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
        if let Some(Some(Ok(fetched))) = data.get() {
            let user = &fetched.data.user;
            set_title(&format!("{} (@{})", user.name(), user.username));
        }
    });

    move || {
        if name.get().is_none() {
            return view! { <NotFound/> }.into_any();
        }
        match data.get() {
            None | Some(None) => view! {
                <div class="profile skeleton" aria-busy="true"><span class="sk sk-avatar"></span><span class="sk sk-title"></span><span class="sk sk-title short"></span></div>
                <CastSkeletons count=3/>
            }
            .into_any(),
            Some(Some(Err(failure))) => view! { <ErrorPanel failure/> }.into_any(),
            Some(Some(Ok(Fetched { data, stale, .. }))) => {
                let payload: Arc<UserPayload> = data;
                let notes: Notes = Arc::new(payload.notes.clone());
                let user = payload.user.clone();
                let path = user_href(&user.username);
                let pool = payload.pool.clone();
                // Switching sort or tab reorders the list, not the page.
                let casts = Signal::derive(move || {
                    if replies_tab.get() { replies_only(&pool) } else { sorted(&top_level(&pool), sort.get()) }
                });
                let side = view! { <WalletPanel user=user.clone()/> }.into_any();
                let casts_link = path.clone();
                let replies_link = with_query(&path, &[("tab", Some("replies".to_owned()))]);
                view! {
                    <Columns side class="profile-columns">
                        <ProfileHeader user=user.clone() score=notes.scores.get(&user.fid).copied()/>
                        <nav class="tabs" aria-label="Profile sections">
                            <a href=casts_link data-noscroll="" class:selected=move || !replies_tab.get() aria-current=move || (!replies_tab.get()).then_some("page")>"Casts"</a>
                            <a href=replies_link data-noscroll="" class:selected=move || replies_tab.get() aria-current=move || replies_tab.get().then_some("page")>"Replies"</a>
                        </nav>
                        {move || (!replies_tab.get()).then(|| view! {
                            <div class="listing-head"><SortTabs path=path.clone() current=sort.get() default_sort=Sort::New/></div>
                        })}
                        <StaleNotice stale/>
                        {move || view! { <CastList casts notes=notes.clone() show_parent=replies_tab.get()/> }}
                    </Columns>
                }
                .into_any()
            }
        }
    }
}

#[component]
fn ProfileHeader(user: User, score: Option<u32>) -> impl IntoView {
    let label = LocalResource::new({
        let fid = user.fid;
        move || spam_label(fid)
    });
    let registered = user.registered_at.map(|ts| {
        if MIGRATION_DAY.contains(&ts) {
            ("before Nov 2023".to_owned(), "Registered before the ID registry moved to OP Mainnet on 7 Nov 2023, which is the earliest date onchain data reports.".to_owned())
        } else {
            (format_date(ts), format!("Registered onchain {} ago", duration_since(ts, now())))
        }
    });
    let (score_text, score_note) = match score {
        Some(0) => ("no score".to_owned(), "Neynar has not scored this account."),
        Some(micros) => (format_score(micros), "Neynar user score, 0–1, read onchain on Base; updated weekly."),
        None => ("unavailable".to_owned(), "The onchain score could not be read just now."),
    };
    view! {
        <section class="profile" aria-label="Profile">
            <div class="profile-top">
                <Avatar url=user.pfp_url.clone() size=88 large=true/>
                <div class="profile-id">
                    <h1 class="profile-name">{user.name().to_owned()}</h1>
                    <p class="handle">"@"{user.username.clone()}<span class="muted">{format!(" · fid {}", user.fid)}</span></p>
                    <p class="counts">
                        <Count n=user.follower_count label="followers"/>
                        <span class="sep" aria-hidden="true">"·"</span>
                        <Count n=user.following_count label="following"/>
                    </p>
                </div>
            </div>
            {if user.bio.trim().is_empty() {
                view! { <p class="bio muted">"No bio."</p> }.into_any()
            } else {
                view! { <div class="bio"><CastText text=user.bio.clone() mentions=Vec::new()/></div> }.into_any()
            }}
            <dl class="facts">
                <div class="fact" title=score_note>
                    <dt>"Neynar score"</dt>
                    <dd class=format!("score-value tier-{}", score.map_or("none", |s| if s == 0 { "none" } else { crate::components::chips::score_tier(s) }))>{score_text}</dd>
                </div>
                {registered.map(|(date, note)| view! {
                    <div class="fact" title=note>
                        <dt>"Registered"</dt>
                        <dd>{date}</dd>
                    </div>
                })}
                <VoteFact fid=user.fid/>
                <div class="fact">
                    <dt>"Farcaster app label"</dt>
                    <dd>{move || match label.get() {
                        None => view! { <span class="muted">"checking…"</span> }.into_any(),
                        Some(state) => view! { <LabelBadge state/> }.into_any(),
                    }}</dd>
                </div>
            </dl>
            {move || match label.get() {
                Some(LabelState::Labeled(l, _)) if l.is_flag() => Some(view! {
                    <p class="label-note" role="note">
                        {format!("The Farcaster app labels this account: “{}” Allthing shows the label for context and never hides anyone's casts.", l.description())}
                    </p>
                }),
                _ => None,
            }}
        </section>
    }
}

#[component]
fn LabelBadge(state: LabelState) -> impl IntoView {
    match state {
        LabelState::Labeled(label, meta) => {
            let class = match label {
                SpamLabel::LikelySpam => "spam-label spam-label-spam",
                SpamLabel::Nerfed => "spam-label spam-label-nerfed",
                SpamLabel::NotSpam => "spam-label spam-label-ok",
            };
            let title = format!(
                "{} From the Farcaster app's public spam dataset (github.com/warpcast/labels), newest label {}.",
                label.description(),
                meta.newest_label.map(format_date).unwrap_or_else(|| "of unknown date".into())
            );
            view! { <span class=class title=title>{label.title()}</span> }.into_any()
        }
        LabelState::Unlabeled(meta) => {
            let title = format!(
                "The Farcaster app's public spam dataset (newest label {}) has no label for this account.",
                meta.newest_label.map(format_date).unwrap_or_else(|| "of unknown date".into())
            );
            view! { <span class="spam-label spam-label-none" title=title>"no label"</span> }.into_any()
        }
        LabelState::Unavailable => view! { <span class="muted" title="The label dataset is not available on this deployment.">"unavailable"</span> }.into_any(),
    }
}

/// Wallet activity by chain, loaded separately because it queries many RPCs,
/// with the account's verified addresses one click away.
#[component]
fn WalletPanel(user: User) -> impl IntoView {
    let fid = user.fid;
    let data = LocalResource::new(move || async move { api::get::<WalletActivity>(&wallets_path(fid)).await });
    let addresses: Vec<(&'static str, String)> = user
        .eth_addresses
        .iter()
        .map(|a| ("EVM", a.clone()))
        .chain(user.sol_addresses.iter().map(|a| ("SOL", a.clone())))
        .collect();
    view! {
        <section class="wallets side-box" aria-label="Wallet activity by chain">
            <div class="wallets-head">
                <h2>"Wallet activity"</h2>
                {(!addresses.is_empty()).then(|| view! { <WalletsButton addresses=addresses.clone()/> })}
            </div>
            {move || match data.get() {
                None => view! { <div class="share-bar loading" aria-label="Loading wallet activity"></div> }.into_any(),
                Some(Err(_)) => view! { <p class="muted small">"Wallet activity is unavailable right now."</p> }.into_any(),
                Some(Ok(fetched)) => view! { <WalletBreakdown activity=fetched.data.as_ref().clone()/> }.into_any(),
            }}
        </section>
    }
}

#[component]
fn WalletBreakdown(activity: WalletActivity) -> impl IntoView {
    let measured: Vec<_> = activity.chains.iter().filter(|c| c.error.is_none() && c.count > 0).cloned().collect();
    if measured.is_empty() {
        return view! { <p class="muted small">"No onchain activity from verified wallets."</p> }.into_any();
    }
    let bar_label = measured.iter().map(|c| format!("{} {}%", c.name, c.percent)).collect::<Vec<_>>().join(", ");
    view! {
        <div class="share-bar" role="img" aria-label=bar_label>
            {measured.iter().filter(|c| c.percent > 0).map(|c| view! {
                <span class=format!("seg chain-{}", c.chain) style=format!("flex-grow: {}", c.percent) title=format!("{}: {}%", c.name, c.percent)></span>
            }).collect_view()}
        </div>
        <ul class="share-list">
            {measured.iter().map(|c| view! {
                <li title=format!("{} transactions sent", full_number(c.count))>
                    <span class=format!("swatch chain-{}", c.chain) aria-hidden="true"></span>
                    <span class="chain-name">{c.name.clone()}</span>
                    <strong class="pct">{if c.percent == 0 { "<1%".to_owned() } else { format!("{}%", c.percent) }}</strong>
                </li>
            }).collect_view()}
        </ul>
    }
    .into_any()
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
            <code title=address.clone()>{short_address(&address)}</code>
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

/// How the account voted when farcaster.xyz asked everyone to stay (yes) or
/// leave (no). Left out when the sources can't settle it.
#[component]
fn VoteFact(fid: u64) -> impl IntoView {
    let data = LocalResource::new(move || async move { api::get::<VotePayload>(&vote_path(fid)).await });
    let row = |answer: AnyView| {
        view! {
            <div class="fact vote-fact">
                <dt>
                    "Farcaster vote "
                    <a class="fact-help" href="https://github.com/ael-dev3/Allthing/blob/main/docs/VOTE.md" title="What was the vote? (docs/VOTE.md)">"?"</a>
                </dt>
                <dd>{answer}</dd>
            </div>
        }
        .into_any()
    };
    move || match data.get() {
        None => {
            Some(row(view! { <span class="vote vote-loading" aria-busy="true" aria-label="Loading the vote"></span> }
                .into_any()))
        }
        Some(Ok(fetched)) => fetched.data.summary().map(|(result, detail)| {
            row(view! { <span class=format!("vote {}", result.class()) title=detail>{result.label()}</span> }
                .into_any())
        }),
        Some(Err(_)) => None,
    }
}
