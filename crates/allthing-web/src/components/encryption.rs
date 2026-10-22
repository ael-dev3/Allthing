// SPDX-License-Identifier: AGPL-3.0-or-later
//! How an encrypted feature is encrypted, told where it's used: a lock
//! whose tooltip states the algorithms, where the keys live, and what the
//! database stores against what stays in the browser
//! (`allthing_core::e2e::Explainer`), with a Copy button for the full
//! technical explanation, written to be pasted into an AI agent. Every
//! encrypted feature shows one: messages now; private notes and group
//! chats as they come.

use allthing_core::e2e::Explainer;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::popover::Popover;

/// What the Copy button did.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Copied {
    Not,
    Done,
    Failed,
}

#[component]
pub fn EncryptionNote(explainer: Explainer) -> impl IntoView {
    let Explainer { feature, points, full } = explainer;
    let full = StoredValue::new(full);
    let points = StoredValue::new(points);
    let copied = RwSignal::new(Copied::Not);
    let copy = move |_| {
        let text = full.get_value();
        leptos::task::spawn_local(async move {
            copied.set(if write_clipboard(&text).await { Copied::Done } else { Copied::Failed });
            set_timeout(move || copied.set(Copied::Not), std::time::Duration::from_secs(3));
        });
    };
    view! {
        <Popover
            label=format!("How {} are encrypted", feature.to_lowercase())
            trigger_label="End-to-end encrypted: how"
            trigger_class="e2e-lock"
            class="e2e-note"
            panel_class="e2e-panel"
            hover=true
            max_px=520.0
            trigger=move || view! {
                <svg class="e2e-icon" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
                    <path fill="currentColor" d="M8 1a3.5 3.5 0 0 0-3.5 3.5V6H4a1.5 1.5 0 0 0-1.5 1.5v6A1.5 1.5 0 0 0 4 15h8a1.5 1.5 0 0 0 1.5-1.5v-6A1.5 1.5 0 0 0 12 6h-.5V4.5A3.5 3.5 0 0 0 8 1Zm2 5H6V4.5a2 2 0 1 1 4 0V6Z"/>
                </svg>
                "End-to-end encrypted"
            }
            content=move || view! {
                <p class="e2e-title">{feature}": how they're encrypted"</p>
                <dl class="e2e-points">
                    {points.get_value().into_iter().map(|(label, line)| view! {
                        <dt>{label}</dt>
                        <dd>{line}</dd>
                    }).collect_view()}
                </dl>
                <div class="e2e-copy-row">
                    <button type="button" class="button e2e-copy" on:click=copy>
                        {move || match copied.get() {
                            Copied::Not => "Copy technical explanation",
                            Copied::Done => "Copied",
                            Copied::Failed => "Couldn't copy",
                        }}
                    </button>
                    <span class="e2e-agent muted">"Paste this into your agent to have it walked through and verified."</span>
                </div>
                <span class="visually-hidden" role="status">
                    {move || match copied.get() {
                        Copied::Done => "Technical explanation copied",
                        Copied::Failed => "Couldn't copy the technical explanation",
                        Copied::Not => "",
                    }}
                </span>
            }
        />
    }
}

/// Put `text` on the clipboard: whether it went.
async fn write_clipboard(text: &str) -> bool {
    let Some(navigator) = web_sys::window().map(|w| w.navigator()) else { return false };
    let Ok(clipboard) = js_sys::Reflect::get(&navigator, &"clipboard".into()) else { return false };
    let Ok(write) =
        js_sys::Reflect::get(&clipboard, &"writeText".into()).and_then(|f| f.dyn_into::<js_sys::Function>())
    else {
        return false;
    };
    match write.call1(&clipboard, &text.into()).and_then(|p| p.dyn_into::<js_sys::Promise>()) {
        Ok(promise) => wasm_bindgen_futures::JsFuture::from(promise).await.is_ok(),
        Err(_) => false,
    }
}
