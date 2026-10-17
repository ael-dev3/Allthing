// SPDX-License-Identifier: AGPL-3.0-or-later
//! A profile: who the account is, how others rate and label it, where its
//! wallets are active, and its casts. Labels inform; they never hide content.

use std::collections::BTreeMap;
use std::sync::Arc;

use allthing_core::api::{UserPayload, user_path, wallets_path};
use allthing_core::format::{full_number, score as format_score, short_address};
use allthing_core::labels::SpamLabel;
use allthing_core::listing::{PAGE_SIZE, paginate, parse_page, replies_only, top_level};
use allthing_core::model::User;
use allthing_core::params::username_from_param;
use allthing_core::ranking::{Sort, sorted};
use allthing_core::time::{duration_since, format_date};
use allthing_core::wallet::{ChainKind, WalletActivity};
use leptos::prelude::*;
use leptos_router::hooks::{use_params_map, use_query_map};

use crate::Notes;
use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons};
use crate::components::chips::{Avatar, Count};
use crate::components::layout::{Columns, ErrorPanel, Freshness, NotFound, Pager, SideBox, SortTabs};
use crate::components::text::CastText;
use crate::labels::{LabelState, spam_label};
use crate::pages::side::KeyboardBox;
use crate::util::{now, set_title, user_href, with_query};

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
    let page = Memo::new(move |_| parse_page(query.read().get("page").as_deref()));
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
            Some(Some(Ok(Fetched { data, age, stale }))) => {
                let payload: Arc<UserPayload> = data;
                let notes: Notes = Arc::new(payload.notes.clone());
                let user = payload.user.clone();
                let path = user_href(&user.username);
                let listing = if replies_tab.get() {
                    replies_only(&payload.pool)
                } else {
                    sorted(&top_level(&payload.pool), sort.get())
                };
                let slice = paginate(listing.len(), page.get(), PAGE_SIZE);
                let casts = listing[slice.offset..slice.end].to_vec();
                let side = view! {
                    <AppsBox payload=payload.clone()/>
                    <AddressesBox user=user.clone()/>
                    <KeyboardBox/>
                }
                .into_any();
                let casts_link = path.clone();
                let replies_link = with_query(&path, &[("tab", Some("replies".to_owned()))]);
                view! {
                    <Columns side>
                        <ProfileHeader user=user.clone() score=notes.scores.get(&user.fid).copied()/>
                        <WalletPanel fid=user.fid/>
                        <nav class="tabs" aria-label="Profile sections">
                            <a href=casts_link class:selected=!replies_tab.get() aria-current=(!replies_tab.get()).then_some("page")>"Casts"</a>
                            <a href=replies_link.clone() class:selected=replies_tab.get() aria-current=replies_tab.get().then_some("page")>"Replies"</a>
                        </nav>
                        {(!replies_tab.get()).then(|| view! {
                            <div class="listing-head"><SortTabs path=path.clone() current=sort.get() default_sort=Sort::New/></div>
                        })}
                        <Freshness age stale/>
                        <CastList casts notes show_parent=replies_tab.get()/>
                        <Pager path=if replies_tab.get() { replies_link } else { path.clone() } slice sort=sort.get() default_sort=Sort::New/>
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

/// Wallet activity by chain, loaded separately because it queries many RPCs.
#[component]
fn WalletPanel(fid: u64) -> impl IntoView {
    let data = LocalResource::new(move || async move { api::get::<WalletActivity>(&wallets_path(fid)).await });
    view! {
        <section class="wallets" aria-label="Wallet activity by chain">
            <h2>"Wallet activity by chain"</h2>
            {move || match data.get() {
                None => view! { <p class="muted wallets-loading">"Counting transactions on each chain…"</p> }.into_any(),
                Some(Err(failure)) => view! { <p class="muted">{format!("Unavailable: {}", failure.message())}</p> }.into_any(),
                Some(Ok(fetched)) => view! { <WalletBreakdown activity=fetched.data.as_ref().clone()/> }.into_any(),
            }}
        </section>
    }
}

#[component]
fn WalletBreakdown(activity: WalletActivity) -> impl IntoView {
    if activity.addresses.is_empty() {
        return view! { <p class="muted">"No verified addresses, so there is no wallet activity to show."</p> }
            .into_any();
    }
    let measured: Vec<_> = activity.chains.iter().filter(|c| c.error.is_none() && c.count > 0).cloned().collect();
    let idle: Vec<String> =
        activity.chains.iter().filter(|c| c.error.is_none() && c.count == 0).map(|c| c.name.clone()).collect();
    let failed: Vec<_> = activity.chains.iter().filter(|c| c.error.is_some()).cloned().collect();
    let evm_addresses = activity.addresses.iter().filter(|a| a.kind == ChainKind::Evm).count();
    let bar_label = measured.iter().map(|c| format!("{} {}%", c.name, c.percent)).collect::<Vec<_>>().join(", ");
    view! {
        {if measured.is_empty() {
            view! { <p class="muted">"No transactions sent from the verified addresses on the measured chains."</p> }.into_any()
        } else {
            view! {
                <div class="share-bar" role="img" aria-label=bar_label>
                    {measured.iter().filter(|c| c.percent > 0).map(|c| view! {
                        <span class=format!("seg chain-{}", c.chain) style=format!("flex-grow: {}", c.percent) title=format!("{}: {}%", c.name, c.percent)></span>
                    }).collect_view()}
                </div>
                <ul class="share-list">
                    {measured.iter().map(|c| view! {
                        <li>
                            <span class=format!("swatch chain-{}", c.chain) aria-hidden="true"></span>
                            <span class="chain-name">{c.name.clone()}</span>
                            <strong class="pct">{if c.percent == 0 { "<1%".to_owned() } else { format!("{}%", c.percent) }}</strong>
                            <span class="muted">{format!("{} sent", full_number(c.count))}</span>
                        </li>
                    }).collect_view()}
                </ul>
            }
            .into_any()
        }}
        {(!idle.is_empty()).then(|| view! { <p class="muted small">"No activity on "{idle.join(", ")}"."</p> })}
        {failed.into_iter().map(|c| view! {
            <p class="muted small chain-missing">{format!("{}: {}", c.name, c.error.unwrap_or_default())}</p>
        }).collect_view()}
        <details class="sources">
            <summary>{format!("How this is measured ({evm_addresses} EVM address{})", if evm_addresses == 1 { "" } else { "es" })}</summary>
            <ul>{activity.sources.iter().map(|s| view! { <li>{s.clone()}</li> }).collect_view()}</ul>
            <p class="muted small">"Smart-contract wallets send through bundlers, so their own counts can be low. Cached for six hours."</p>
        </details>
    }
    .into_any()
}

/// Which apps signed this account's recent casts.
#[component]
fn AppsBox(payload: Arc<UserPayload>) -> impl IntoView {
    let notes = &payload.notes;
    let mut counts: BTreeMap<Option<u64>, u32> = BTreeMap::new();
    for cast in &payload.pool {
        if let Some(origin) = notes.origins.get(&cast.hash) {
            *counts.entry(origin.app_fid).or_default() += 1;
        }
    }
    let total: u32 = counts.values().sum();
    if total == 0 {
        return ().into_any();
    }
    let mut rows: Vec<(String, Option<String>, u32)> = counts
        .into_iter()
        .map(|(fid, n)| {
            let app = fid.and_then(|f| notes.apps.get(&f));
            let name = match (app, fid) {
                (Some(app), _) => app.name().to_owned(),
                (None, Some(fid)) => format!("app fid {fid}"),
                (None, None) => "unattributed key".to_owned(),
            };
            (name, app.map(|a| user_href(&a.username)), n)
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.2));
    view! {
        <SideBox title="Posts via">
            <ul class="apps">
                {rows.into_iter().map(|(name, href, n)| view! {
                    <li>
                        {match href {
                            Some(href) => view! { <a href=href>{name}</a> }.into_any(),
                            None => view! { <span>{name}</span> }.into_any(),
                        }}
                        <span class="muted">{format!("{}%", n * 100 / total)}</span>
                    </li>
                }).collect_view()}
            </ul>
            <p class="muted small">{format!("Across {total} recent casts, by the app that registered each signing key.")}</p>
        </SideBox>
    }
    .into_any()
}

#[component]
fn AddressesBox(user: User) -> impl IntoView {
    let eth = user.eth_addresses.clone();
    let sol = user.sol_addresses.clone();
    if eth.is_empty() && sol.is_empty() {
        return ().into_any();
    }
    view! {
        <SideBox title="Verified addresses">
            <ul class="addresses">
                {eth.iter().map(|a| view! { <li><span class="chain-tag">"EVM"</span><code title=a.clone()>{short_address(a)}</code></li> }).collect_view()}
                {sol.iter().map(|a| view! { <li><span class="chain-tag">"SOL"</span><code title=a.clone()>{short_address(a)}</code></li> }).collect_view()}
            </ul>
        </SideBox>
    }
    .into_any()
}
