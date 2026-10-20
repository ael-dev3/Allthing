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
        <div class="fact">
            <dt>"Joined"</dt>
            <dd aria-description=joined.note(now())>{joined.date()}</dd>
        </div>
    }
}

/// An account's vote, read when it is asked for (at once from the client
/// cache when preloaded).
pub type VoteData = LocalResource<Result<api::Fetched<VotePayload>, api::Failure>>;
/// An account's wallet activity, read when it is asked for.
pub type WalletData = LocalResource<Result<api::Fetched<WalletActivity>, api::Failure>>;

pub fn vote_data(fid: u64) -> VoteData {
    LocalResource::new(move || async move { api::get::<VotePayload>(&vote_path(fid)).await })
}

pub fn wallet_data(fid: u64) -> WalletData {
    LocalResource::new(move || async move { api::get::<WalletActivity>(&wallets_path(fid)).await })
}

/// What the NS vote is, read out with the fact's label.
const NS_VOTE_TIP: &str = "NS vote: how this account answered farcaster.xyz's vote after Network School took over Farcaster. Stayed (yes), left (no), or didn't vote.";

/// How the account voted when farcaster.xyz asked everyone to stay (yes) or
/// leave (no), loaded when shown, or from `data` that a profile card reads
/// itself. Left out when the sources can't settle it, unless `always` (a
/// card, which keeps its size): then it says "Unknown".
#[component]
pub fn VoteFact(fid: u64, #[prop(optional)] data: Option<VoteData>, #[prop(optional)] always: bool) -> impl IntoView {
    let data = data.unwrap_or_else(|| vote_data(fid));
    let row = |answer: AnyView| {
        view! {
            <div class="fact vote-fact">
                <dt aria-description=NS_VOTE_TIP>
                    <img class="ns-icon" src=crate::util::href("/ns-icon.png") width="14" height="14" alt=""/>
                    "NS vote "
                    <a class="fact-help" href="https://github.com/ael-dev3/Allthing/blob/main/docs/VOTE.md" aria-label="What was the NS vote? (docs/VOTE.md)">"?"</a>
                </dt>
                <dd>{answer}</dd>
            </div>
        }
        .into_any()
    };
    let unknown = move || {
        always.then(|| {
            row(view! { <span class="vote vote-unknown" aria-description="The vote couldn't be read just now.">"Unknown"</span> }
                .into_any())
        })
    };
    // Prefetched (see `api::prefetch_card_facts`): shown in the first frame.
    let ready = api::ready::<VotePayload>(&vote_path(fid));
    move || match data.get().or_else(|| ready.clone().map(Ok)) {
        None => {
            Some(row(view! { <span class="vote vote-loading" aria-busy="true" aria-label="Loading the vote"></span> }
                .into_any()))
        }
        Some(Ok(fetched)) => match fetched.data.summary() {
            Some((result, detail)) => Some(row(view! {
                <span class=format!("vote {}", result.class()) aria-description=detail>{result.label()}</span>
            }
            .into_any())),
            None => unknown(),
        },
        Some(Err(_)) => unknown(),
    }
}

/// What a wallet chart's shares are of; `docs/WALLETS.md` has how each
/// count is read, and how the shares are rounded.
pub const WALLETS_WHAT: &str = "Share of the verified wallets' transactions, by chain.";

/// "?" beside "Wallet activity": how it is counted, in `docs/WALLETS.md`.
#[component]
pub fn WalletsHelp() -> impl IntoView {
    view! {
        <a class="fact-help" href="https://github.com/ael-dev3/Allthing/blob/main/docs/WALLETS.md" aria-label="How is wallet activity counted? (docs/WALLETS.md)">"?"</a>
    }
}

/// Wallet activity by chain, loaded when shown (or from `data`, as for
/// [`VoteFact`]): it is read from many RPCs, so it has an endpoint of its
/// own, cached for hours. `compact` (a card) keeps one height whatever
/// comes: the bar and one line of legend, or one message, with any note
/// folded into a word beside the section's label.
#[component]
pub fn WalletShares(
    fid: u64,
    #[prop(optional)] data: Option<WalletData>,
    #[prop(optional)] compact: bool,
) -> impl IntoView {
    let ready = api::ready::<WalletActivity>(&wallets_path(fid));
    let data = data.unwrap_or_else(|| wallet_data(fid));
    // Prefetched (see `api::prefetch_card_facts`): shown in the first frame.
    let shares = move || match data.get().or_else(|| ready.clone().map(Ok)) {
        None if compact => view! {
            <div class="share-bar loading" aria-label="Loading wallet activity"></div>
            <div class="share-list-loading" aria-hidden="true"></div>
        }
        .into_any(),
        None => view! { <div class="share-bar loading" aria-label="Loading wallet activity"></div> }.into_any(),
        Some(Err(_)) => {
            view! { <p class="muted small share-message">"Wallet activity is unavailable right now."</p> }.into_any()
        }
        Some(Ok(fetched)) => view! { <WalletBreakdown activity=fetched.data.as_ref().clone() compact/> }.into_any(),
    };
    if compact { view! { <div class="share-body">{shares}</div> }.into_any() } else { shares.into_any() }
}

/// The notes a wallet breakdown may carry: why its shares are partial, or
/// from an earlier read.
const SOLANA_UNREAD: &str = "Solana couldn't be read just now, so its wallets aren't counted.";
const CARRIED: &str = "Some counts are from an earlier read: those chains couldn't be read just now.";
const MISSED_SOME: &str = "Some chains weren't read just now.";
const MISSED_SHARES: &str = "Some chains weren't read just now, so the shares cover the rest.";

/// A card's word for its notes, beside the section's label: what they say
/// is read out.
fn note_word(notes: Vec<&'static str>, partial: bool) -> Option<impl IntoView> {
    (!notes.is_empty()).then(|| {
        let text = notes.join(" ");
        view! {
            <span class="share-note">
                {if partial { "partial" } else { "earlier counts" }}
                <span class="visually-hidden">{format!(": {text}")}</span>
            </span>
        }
    })
}

/// Each measured chain's share of the transactions the account's verified
/// wallets sent: a bar and its legend.
#[component]
fn WalletBreakdown(mut activity: WalletActivity, #[prop(optional)] compact: bool) -> impl IntoView {
    // Shares follow this version's rules from the counts, even in a payload
    // the edge cached (for hours) under an earlier version's.
    allthing_core::wallet::apportion(&mut activity.chains);
    // Chains under 1% are left out, so every row has a whole percent. The
    // shares sum to 100, so one chain at 100% is the only row.
    let measured: Vec<_> = activity.chains.iter().filter(|c| c.error.is_none() && c.percent >= 1).cloned().collect();
    // An account's Solana wallets go uncounted when Solana's read fails: say so.
    let solana_unread =
        activity.chains.iter().any(|c| c.error.is_some() && c.kind == allthing_core::wallet::ChainKind::Solana);
    let solana_note = move || solana_unread.then(|| view! { <p class="muted small">{SOLANA_UNREAD}</p> });
    let missed = activity.chains.iter().any(|c| c.error.is_some() && c.kind == allthing_core::wallet::ChainKind::Evm);
    let carried_note = |carried: bool| carried.then(|| view! { <p class="muted small">{CARRIED}</p> });
    if measured.is_empty() {
        let any_read = activity.chains.iter().any(|c| c.error.is_none());
        let evm_read =
            activity.chains.iter().any(|c| c.error.is_none() && c.kind == allthing_core::wallet::ChainKind::Evm);
        // Only Solana, unread: its note is all there is to say.
        let says_none = missed || evm_read || !solana_unread;
        let carried = activity.chains.iter().any(|c| c.error.is_none() && c.carried);
        if compact {
            // One message; the rest in the note's word.
            let message = if says_none { none_text(missed, any_read, solana_unread) } else { SOLANA_UNREAD };
            let notes = [
                (missed && any_read, MISSED_SOME),
                (carried, CARRIED),
                (solana_unread && message != SOLANA_UNREAD, SOLANA_UNREAD),
            ];
            let notes: Vec<_> = notes.into_iter().filter_map(|(on, text)| on.then_some(text)).collect();
            return view! {
                <p class="muted small share-message">{message}</p>
                {note_word(notes, missed || solana_unread)}
            }
            .into_any();
        }
        let none = says_none.then(|| view! { <p class="muted small">{none_text(missed, any_read, solana_unread)}</p> });
        return view! {
            {none}
            {(missed && any_read).then(|| view! { <p class="muted small">{MISSED_SOME}</p> })}
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
    let bar = view! {
        <div class="share-bar" role="img" aria-label=bar_label>
            {measured.iter().filter(|c| c.percent > 0).map(|c| view! {
                <span class=format!("seg chain-{}", c.chain) style=format!("flex-grow: {}", c.percent)></span>
            }).collect_view()}
        </div>
    };
    if compact {
        // The two largest chains by name, and how many more with their share
        // together, so the numbers shown add up to 100; the bar, and its
        // label, have them all.
        const NAMED: usize = 2;
        let rest = &measured[NAMED.min(measured.len())..];
        let more = (!rest.is_empty()).then(|| {
            let listed = rest.iter().map(|c| format!("{} {}%", c.name, c.percent)).collect::<Vec<_>>().join(", ");
            let together: u32 = rest.iter().map(|c| c.percent).sum();
            view! {
                <li class="share-more" aria-description=listed>
                    // The space keeps "+4" and "35%" apart when read out.
                    <span class="chain-name">{format!("+{} ", rest.len())}</span>
                    <strong class="pct">{format!("{together}%")}</strong>
                </li>
            }
        });
        let notes = [(missed, MISSED_SHARES), (carried, CARRIED), (solana_unread, SOLANA_UNREAD)];
        let notes: Vec<_> = notes.into_iter().filter_map(|(on, text)| on.then_some(text)).collect();
        return view! {
            {bar}
            <ul class="share-list compact">
                {measured.iter().take(NAMED).map(|c| view! {
                    <li aria-description=chain_count_note(c)>
                        <span class=format!("swatch chain-{}", c.chain) aria-hidden="true"></span>
                        <span class="chain-name">{c.name.clone()}</span>
                        <strong class="pct">{format!("{}%", c.percent)}</strong>
                    </li>
                }).collect_view()}
                {more}
            </ul>
            {note_word(notes, missed || solana_unread)}
        }
        .into_any();
    }
    view! {
        {bar}
        <ul class="share-list">
            {measured.iter().map(|c| view! {
                <li aria-description=chain_count_note(c)>
                    <span class=format!("swatch chain-{}", c.chain) aria-hidden="true"></span>
                    <span class="chain-name">{c.name.clone()}</span>
                    <strong class="pct">{format!("{}%", c.percent)}</strong>
                </li>
            }).collect_view()}
        </ul>
        {missed.then(|| view! { <p class="muted small">{MISSED_SHARES}</p> })}
        {carried_note(carried)}
        {solana_note()}
    }
    .into_any()
}

/// What a breakdown with no chain to measure says.
fn none_text(missed: bool, any_read: bool, solana_unread: bool) -> &'static str {
    if missed && !any_read {
        "Wallet activity couldn't be read just now."
    } else if missed {
        "No activity on the chains counted."
    } else if solana_unread {
        "No activity on the EVM chains counted."
    } else {
        "No onchain activity from verified wallets."
    }
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
