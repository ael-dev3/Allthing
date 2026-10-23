// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whether the reader sees mini app casts: casts that embed a mini app or a
//! frame, which Allthing doesn't run. Payloads keep them, marked
//! (`Cast::mini_app`), and lists leave them out unless the reader chose to
//! see them, then show each with a link to open it in Farcaster. Spam stays
//! hidden either way (`Cast::hidden_for`). The choice is kept in this
//! browser's localStorage (`allthing.show-mini-apps`), read by web/paint.js
//! too, and the reader's other tabs follow it (the `storage` event). The
//! switch lives in the header's Settings panel (`Settings`), not in the feed.

use allthing_core::model::Cast;
use leptos::prelude::*;

const KEY: &str = "allthing.show-mini-apps";

fn read() -> bool {
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    storage.and_then(|s| s.get_item(KEY).ok().flatten()).is_some_and(|v| v == "1")
}

thread_local! {
    /// Reference-counted, not owned by whichever view first asked: that
    /// one's going would dispose of it.
    static SHOWN: ArcRwSignal<bool> = {
        let shown = ArcRwSignal::new(read());
        let follow = shown.clone();
        // For as long as the app runs: another tab changed it.
        let _ = window_event_listener(leptos::ev::storage, move |ev| {
            if ev.key().is_none_or(|key| key == KEY) {
                follow.set(read());
            }
        });
        shown
    };
}

/// Whether the reader shows mini app casts (tracked).
pub fn shown() -> bool {
    SHOWN.with(|s| s.get())
}

/// The same, untracked: for drawing one cast.
pub fn shown_untracked() -> bool {
    SHOWN.with(|s| s.get_untracked())
}

/// Show or hide mini app casts, here and in the reader's other tabs.
pub fn set_shown(on: bool) {
    SHOWN.with(|s| s.set(on));
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = if on { storage.set_item(KEY, "1") } else { storage.remove_item(KEY) };
    }
}

/// Whether `cast` is left out, or emptied, for this reader.
pub fn hides(cast: &Cast) -> bool {
    cast.hidden_for(shown_untracked())
}

/// The casts of `pool` this reader sees, as they are now: for what counts
/// them (the apps bar over a feed).
pub fn as_shown(pool: &[Cast]) -> Vec<Cast> {
    let show = shown_untracked();
    pool.iter().filter(|c| !c.hidden_for(show)).cloned().collect()
}

/// The switch: "Show mini app casts", a checkbox-like button.
#[component]
pub fn MiniAppsSwitch(#[prop(optional)] class: &'static str) -> impl IntoView {
    let on = Memo::new(move |_| shown());
    view! {
        <button
            type="button"
            role="switch"
            class=format!("mini-apps-switch {class}")
            aria-checked=move || on.get().to_string()
            title="Casts with mini apps or frames. Allthing doesn't run them: each opens in Farcaster. Kept in this browser."
            on:click=move |_| set_shown(!on.get_untracked())
        >
            <span class="switch-track" aria-hidden="true"><span class="switch-thumb"></span></span>
            "Show mini app casts"
        </button>
    }
}

/// The header's Settings: a button opening a panel with the reader's
/// choices, kept in this browser: mini app visibility and reading-history reset.
#[component]
pub fn Settings() -> impl IntoView {
    view! {
        <crate::components::popover::Popover
            label="Settings"
            trigger_label="Settings"
            trigger_class="settings-button"
            class="settings"
            panel_class="settings-panel"
            trigger=|| view! {
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true">
                    <circle cx="8" cy="8" r="2.2"/>
                    <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4"/>
                </svg>
            }
            content=|| view! {
                <p class="settings-title">"Settings"</p>
                <MiniAppsSwitch class="account-link"/>
                <crate::seen::ResetSeen/>
            }
        />
    }
}

/// On a mini app cast that is shown: what it is, and where to open it. It
/// links to the cast in Farcaster, which runs it; Allthing doesn't.
#[component]
pub fn MiniAppLink(cast: Cast) -> impl IntoView {
    cast.mini_app.then(|| {
        let url = allthing_core::compose::cast_url(&cast.author.username, &cast.hash);
        view! {
            <p class="mini-app-link">
                <span class="mini-app-badge">"Mini app"</span>
                <a href=url target="_blank" rel="noopener" on:click=|e| e.stop_propagation()>
                    "Open in Farcaster"
                    <span class="visually-hidden">" (new tab)"</span>
                </a>
            </p>
        }
    })
}
