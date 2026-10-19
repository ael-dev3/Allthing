// SPDX-License-Identifier: AGPL-3.0-or-later
//! Facts about an account that its profile and its profile card share: when
//! it joined, how it voted, and where its wallets are active, each saying
//! where it comes from.

use allthing_core::api::{vote_path, wallets_path};
use allthing_core::format::full_number;
use allthing_core::model::Joined;
use allthing_core::vote::VotePayload;
use allthing_core::wallet::WalletActivity;
use leptos::prelude::*;

use crate::api;
use crate::util::now;

/// When the account joined: its onchain registration, or for an account
/// from before the ID registry's migration, its first username's date (or
/// "before Nov 2023").
#[component]
pub fn JoinedFact(joined: Joined) -> impl IntoView {
    view! {
        <div class="fact" title=joined.note(now())>
            <dt>"Joined"</dt>
            <dd>{joined.date()}</dd>
        </div>
    }
}

/// What the NS vote is, for the tooltip.
const NS_VOTE_TIP: &str = "NS vote: how this account answered farcaster.xyz's vote after Network School took over Farcaster. Stayed (yes), left (no), or didn't vote.";

/// How the account voted when farcaster.xyz asked everyone to stay (yes) or
/// leave (no), loaded when shown. Left out when the sources can't settle it.
#[component]
pub fn VoteFact(fid: u64) -> impl IntoView {
    let data = LocalResource::new(move || async move { api::get::<VotePayload>(&vote_path(fid)).await });
    let row = |answer: AnyView| {
        view! {
            <div class="fact vote-fact">
                <dt title=NS_VOTE_TIP>
                    <img class="ns-icon" src=crate::util::href("/ns-icon.png") width="14" height="14" alt=""/>
                    "NS vote "
                    <a class="fact-help" href="https://github.com/ael-dev3/Allthing/blob/main/docs/VOTE.md" title="What was the vote? (docs/VOTE.md)">"?"</a>
                </dt>
                <dd>{answer}</dd>
            </div>
        }
        .into_any()
    };
    // Prefetched (see `api::prefetch_card_facts`): shown in the first frame.
    let ready = api::ready::<VotePayload>(&vote_path(fid));
    move || match data.get().or_else(|| ready.clone().map(Ok)) {
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

/// Wallet activity by chain, loaded when shown: it is read from many RPCs,
/// so it has an endpoint of its own, cached for hours.
#[component]
pub fn WalletShares(fid: u64) -> impl IntoView {
    let ready = api::ready::<WalletActivity>(&wallets_path(fid));
    let data = LocalResource::new(move || async move { api::get::<WalletActivity>(&wallets_path(fid)).await });
    // Prefetched (see `api::prefetch_card_facts`): shown in the first frame.
    move || match data.get().or_else(|| ready.clone().map(Ok)) {
        None => view! { <div class="share-bar loading" aria-label="Loading wallet activity"></div> }.into_any(),
        Some(Err(_)) => view! { <p class="muted small">"Wallet activity is unavailable right now."</p> }.into_any(),
        Some(Ok(fetched)) => view! { <WalletBreakdown activity=fetched.data.as_ref().clone()/> }.into_any(),
    }
}

/// Each measured chain's share of the transactions the account's verified
/// wallets sent: a bar and its legend.
#[component]
fn WalletBreakdown(mut activity: WalletActivity) -> impl IntoView {
    // Shares follow this version's rules from the counts, even in a payload
    // the edge cached (for hours) under an earlier version's.
    allthing_core::wallet::apportion(&mut activity.chains);
    // Chains under 1% are left out, so every row has a whole percent. The
    // shares sum to 100, so one chain at 100% is the only row.
    let measured: Vec<_> = activity.chains.iter().filter(|c| c.error.is_none() && c.percent >= 1).cloned().collect();
    // An account's Solana wallets go uncounted when Solana's read fails: say so.
    let solana_unread =
        activity.chains.iter().any(|c| c.error.is_some() && c.kind == allthing_core::wallet::ChainKind::Solana);
    let solana_note = move || {
        solana_unread.then(|| {
            view! { <p class="muted small">"Solana couldn't be read just now, so its wallets aren't counted."</p> }
        })
    };
    let missed = activity.chains.iter().any(|c| c.error.is_some() && c.kind == allthing_core::wallet::ChainKind::Evm);
    let carried_note = |carried: bool| {
        carried.then(|| view! { <p class="muted small">"Some counts are from an earlier read: those chains couldn't be read just now."</p> })
    };
    if measured.is_empty() {
        let any_read = activity.chains.iter().any(|c| c.error.is_none());
        let evm_read =
            activity.chains.iter().any(|c| c.error.is_none() && c.kind == allthing_core::wallet::ChainKind::Evm);
        let none = (missed || evm_read || !solana_unread).then(|| {
            let text = if missed && !any_read {
                "Wallet activity couldn't be read just now."
            } else if missed {
                "No activity on the chains counted."
            } else if solana_unread {
                "No activity on the EVM chains counted."
            } else {
                "No onchain activity from verified wallets."
            };
            view! { <p class="muted small">{text}</p> }
        });
        let carried = activity.chains.iter().any(|c| c.error.is_none() && c.carried);
        return view! {
            {none}
            {(missed && any_read).then(|| view! { <p class="muted small">"Some chains weren't read just now."</p> })}
            {carried_note(carried)}
            {solana_note()}
        }
        .into_any();
    }
    // An EVM chain that couldn't be read just now is left out of the shares:
    // say so, rather than let the shares look complete.
    // A shown chain whose count is from an earlier read: say so.
    let carried = measured.iter().any(|c| c.carried);
    let bar_label = measured.iter().map(|c| format!("{} {}%", c.name, c.percent)).collect::<Vec<_>>().join(", ");
    view! {
        <div class="share-bar" role="img" aria-label=bar_label>
            {measured.iter().filter(|c| c.percent > 0).map(|c| view! {
                <span class=format!("seg chain-{}", c.chain) style=format!("flex-grow: {}", c.percent) title=format!("{}: {}%", c.name, c.percent)></span>
            }).collect_view()}
        </div>
        <ul class="share-list">
            {measured.iter().map(|c| view! {
                <li title=chain_count_note(c)>
                    <span class=format!("swatch chain-{}", c.chain) aria-hidden="true"></span>
                    <span class="chain-name">{c.name.clone()}</span>
                    <strong class="pct">{format!("{}%", c.percent)}</strong>
                </li>
            }).collect_view()}
        </ul>
        {missed.then(|| view! { <p class="muted small">"Some chains weren't read just now, so the shares cover the rest."</p> })}
        {carried_note(carried)}
        {solana_note()}
    }
    .into_any()
}

/// What a chain's count is: transactions sent (EVM nonces), or transactions
/// involving the address (Solana signatures, "1,000+" at the cap).
fn chain_count_note(c: &allthing_core::wallet::ChainActivity) -> String {
    let plus = if c.capped { "+" } else { "" };
    match c.kind {
        allthing_core::wallet::ChainKind::Evm => format!("{}{plus} transactions sent", full_number(c.count)),
        allthing_core::wallet::ChainKind::Solana => format!("{}{plus} transactions", full_number(c.count)),
    }
}
