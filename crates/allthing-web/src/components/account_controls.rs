// SPDX-License-Identifier: AGPL-3.0-or-later
//! Private account hiding, with the same controls on profiles and hover cards.
//! Preferences belong to the signed-in reader, never to a Farcaster write key.

use leptos::prelude::*;

use crate::components::popover::Popover;
use crate::moderation::{SyncStatus, use_moderation};

const MUTE_WHAT: &str = "Hide posts and notifications involving this account in Allthing, including identifiable mentions, quotes, replies and account links. Private; synced across signed-in sessions.";
const BLOCK_WHAT: &str = "Hide the same content as Mute and stop new Allthing direct messages in both directions. Private; synced across signed-in sessions. Other Farcaster clients are unaffected.";

/// More than a native title: mouse hover, keyboard focus and an explicit
/// touch press all open this explanation. A separate help control never
/// changes the preference merely to reveal what the preference does.
#[component]
fn AccountHelp() -> impl IntoView {
    view! {
        <Popover
            label="About mute and block"
            trigger_label="About mute and block"
            trigger_class="account-controls-help"
            class="account-controls-help-wrap"
            panel_class="account-controls-explanation"
            hover=true
            fixed=true
            beside=true
            max_px=440.0
            trigger=|| view! { <span aria-hidden="true">"?"</span> }
            content=|| view! {
                <p><strong>"Mute"</strong>" hides posts, quotes, recasts, replies and notifications involving the account, including identifiable mentions and account links."</p>
                <p><strong>"Block"</strong>" hides the same content and also stops new Allthing direct messages in both directions. Mute and Block are independent; turning one off leaves the other in place."</p>
                <p>"These are private Allthing preferences, synced through SpacetimeDB across your signed-in sessions. Changes made offline wait to sync."</p>
                <p>"They do not delete public posts or block anyone in other Farcaster clients. Recognition uses known account IDs, @handles and supported account links; it cannot identify every plain-text reference or text in images."</p>
                <p>"Undo either choice in Settings → Muted and blocked accounts."</p>
            }
        />
    }
}

/// Controls are absent for signed-out readers and the reader's own account.
/// A pending operation is visible, and failures retain the retry path.
#[component]
pub fn AccountControls(fid: u64, username: String) -> impl IntoView {
    let moderation = use_moderation();
    let session = crate::components::signin::use_session();
    let available =
        Memo::new(move |_| session.user.with(|user| user.as_ref().is_some_and(|user| user.fid != fid && fid > 0)));
    let preference = Memo::new(move |_| moderation.preference(fid));
    let mute_username = username.clone();
    view! {
        <Show when=move || available.get()>
            <div class="account-controls" aria-label="Private account controls">
                <div class="account-controls-buttons">
                    <button
                        type="button"
                        class="account-control"
                        class:active=move || preference.get().muted
                        aria-pressed=move || preference.get().muted.to_string()
                        aria-description=MUTE_WHAT
                        disabled=move || preference.get().pending || moderation.status() == SyncStatus::Connecting
                        on:click={let username = mute_username.clone(); move |event| {
                            event.stop_propagation();
                            moderation.set_muted(fid, &username, !preference.get_untracked().muted);
                        }}
                    >{move || if preference.get().muted { "Unmute" } else { "Mute" }}</button>
                    <button
                        type="button"
                        class="account-control"
                        class:active=move || preference.get().blocked
                        aria-pressed=move || preference.get().blocked.to_string()
                        aria-description=BLOCK_WHAT
                        disabled=move || preference.get().pending || moderation.status() == SyncStatus::Connecting
                        on:click={let username = username.clone(); move |event| {
                            event.stop_propagation();
                            moderation.set_blocked(fid, &username, !preference.get_untracked().blocked);
                        }}
                    >{move || if preference.get().blocked { "Unblock" } else { "Block" }}</button>
                    <AccountHelp/>
                </div>
                <AccountSyncStatus/>
            </div>
        </Show>
    }
}

/// Sync status is explicit rather than reporting a successful empty list
/// when the private account preferences have not loaded.
#[component]
fn AccountSyncStatus() -> impl IntoView {
    let moderation = use_moderation();
    move || {
        match moderation.status() {
        SyncStatus::SignedOut => ().into_any(),
        SyncStatus::Connecting => view! { <p class="account-controls-status" role="status">"Loading private preferences…"</p> }.into_any(),
        SyncStatus::Saving => view! { <p class="account-controls-status" role="status">"Saving on this device…"</p> }.into_any(),
        SyncStatus::Synced => ().into_any(),
        SyncStatus::Pending => view! { <p class="account-controls-status" role="status">"Saved here; waiting to sync across sessions…"</p> }.into_any(),
        SyncStatus::Failed(_) => view! {
            <div class="account-controls-status account-controls-error" role="status">
                <p>"Preferences are not synced across sessions. Retry to reconnect."</p>
                <button type="button" class="account-controls-retry" on:click=move |_| moderation.retry()>"Retry sync"</button>
            </div>
        }.into_any(),
    }
    }
}

/// An intentionally explicit place to see and undo private account
/// choices. Hidden accounts remain named here so a reader can unhide them.
#[component]
pub fn AccountManagement() -> impl IntoView {
    let moderation = use_moderation();
    let query = RwSignal::new(String::new());
    let filtered = Memo::new(move |_| {
        let query = query.get().trim().trim_start_matches('@').to_lowercase();
        moderation
            .preferences()
            .into_iter()
            .filter(|preference| {
                (preference.muted || preference.blocked || preference.pending)
                    && (query.is_empty()
                        || preference.username.to_lowercase().contains(&query)
                        || preference.target_fid.to_string().contains(&query))
            })
            .collect::<Vec<_>>()
    });
    view! {
        <Show when=move || moderation.signed_in()>
            <details class="account-management">
                <summary>"Muted and blocked accounts"</summary>
                <div class="account-management-body">
                    <p class="account-management-intro">"Private choices across your signed-in Allthing sessions. Unmute and Unblock undo each choice separately."</p>
                    <AccountHelp/>
                    <label class="account-management-search">
                        <span>"Find an account"</span>
                        <input
                            type="search"
                            placeholder="@handle or account ID"
                            autocomplete="off"
                            prop:value=move || query.get()
                            on:input=move |event| query.set(event_target_value(&event))
                        />
                    </label>
                    <AccountSyncStatus/>
                    <ul class="account-management-list" aria-label="Muted and blocked accounts">
                        <For
                            each=move || filtered.get()
                            key=|preference| preference.target_fid
                            children=move |preference| {
                                let fid = preference.target_fid;
                                let current = Memo::new(move |_| moderation.preference(fid));
                                let username = preference.username.clone();
                                let muted_username = username.clone();
                                let label = if username.is_empty() { format!("Account {fid}") } else { format!("@{username}") };
                                let mute_label = format!("Unmute {label}");
                                let block_label = format!("Unblock {label}");
                                view! {
                                    <li class="account-management-row">
                                        <div class="account-management-id"><strong>{label}</strong><span>{format!("Account ID {fid}")}</span></div>
                                        <div class="account-management-actions" aria-busy=move || current.get().pending.then_some("true")>
                                            <Show when=move || current.get().muted>
                                                <button type="button" class="account-control active" aria-label=mute_label.clone() aria-description="Undo Mute; Block, if enabled, stays on" disabled=move || current.get().pending on:click={let username = muted_username.clone(); move |_| moderation.set_muted(fid, &username, false)}>"Unmute"</button>
                                            </Show>
                                            <Show when=move || current.get().blocked>
                                                <button type="button" class="account-control active" aria-label=block_label.clone() aria-description="Undo Block; Mute, if enabled, stays on" disabled=move || current.get().pending on:click={let username = username.clone(); move |_| moderation.set_blocked(fid, &username, false)}>"Unblock"</button>
                                            </Show>
                                            {move || current.get().pending.then(|| view! { <span class="account-management-saving">"Sync pending…"</span> })}
                                        </div>
                                    </li>
                                }
                            }
                        />
                    </ul>
                    {move || (filtered.get().is_empty() && moderation.status() == SyncStatus::Synced).then(|| view! {
                        <p class="account-management-empty">{if query.get().trim().is_empty() { "No muted or blocked accounts." } else { "No matching accounts." }}</p>
                    })}
                </div>
            </details>
        </Show>
    }
}
