// SPDX-License-Identifier: AGPL-3.0-or-later
//! A small anchored panel. Hover popovers open after a short dwell (or on
//! keyboard focus) and stay open while the pointer is inside them; on touch
//! screens, and for click popovers, the trigger toggles them. Escape and a
//! click outside close them. Content renders only while open.

use std::time::Duration;

use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::util::can_hover;

const OPEN_DELAY: Duration = Duration::from_millis(280);
const CLOSE_DELAY: Duration = Duration::from_millis(180);
const CLOSE_ANIMATION: Duration = Duration::from_millis(120);
/// The sticky header covers this much of the top of the viewport.
const HEADER_PX: f64 = 72.0;
/// Panels are at most this tall, and shrink to the room they have.
const MAX_PANEL_PX: f64 = 360.0;
/// Gap between the trigger and the panel, plus a margin from the edge.
const GAP_PX: f64 = 16.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Closed,
    Open,
    Closing,
}

#[component]
pub fn Popover(
    /// Accessible name of the panel.
    #[prop(into)]
    label: String,
    /// Accessible name of the trigger button.
    #[prop(into)]
    trigger_label: String,
    #[prop(into, optional)] trigger_class: String,
    /// Open on hover and focus (desktop); otherwise only on click.
    #[prop(optional)]
    hover: bool,
    /// What a click does on hover-capable screens, instead of toggling.
    #[prop(optional)]
    on_activate: Option<Callback<()>>,
    #[prop(into)] trigger: ViewFn,
    #[prop(into)] content: ViewFn,
) -> impl IntoView {
    let state = RwSignal::new(State::Closed);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let root = NodeRef::<leptos::html::Span>::new();
    let button = NodeRef::<leptos::html::Button>::new();
    // Where the panel opens: above the trigger (default) or below it, and
    // how tall it may be, measured each time it opens.
    let below = RwSignal::new(false);
    let max_height = RwSignal::new(MAX_PANEL_PX);
    let place = move || {
        let Some(button) = button.get_untracked() else { return };
        let rect = button.get_bounding_client_rect();
        let viewport = window().inner_height().ok().and_then(|h| h.as_f64()).unwrap_or(800.0);
        let above_room = rect.top() - HEADER_PX - GAP_PX;
        let below_room = viewport - rect.bottom() - GAP_PX;
        let open_below = above_room < MAX_PANEL_PX && below_room > above_room;
        below.set(open_below);
        max_height.set(if open_below { below_room } else { above_room }.clamp(120.0, MAX_PANEL_PX));
    };

    let cancel = move || {
        if let Some(handle) = timer.get_value() {
            handle.clear();
            timer.set_value(None);
        }
    };
    let schedule = move |delay: Duration, f: Box<dyn FnOnce()>| {
        cancel();
        timer.set_value(set_timeout_with_handle(f, delay).ok());
    };
    let open = move || {
        cancel();
        place();
        state.set(State::Open);
    };
    let close = move || {
        cancel();
        if state.get_untracked() == State::Open {
            state.set(State::Closing);
            schedule(CLOSE_ANIMATION, Box::new(move || state.set(State::Closed)));
        }
    };
    let hovering = move || hover && can_hover();

    let enter = move |_: ev::MouseEvent| {
        if hovering() {
            if state.get_untracked() == State::Open {
                cancel();
            } else {
                schedule(OPEN_DELAY, Box::new(open));
            }
        }
    };
    let leave = move |_: ev::MouseEvent| {
        if hovering() {
            if state.get_untracked() == State::Open {
                schedule(CLOSE_DELAY, Box::new(close));
            } else {
                cancel();
            }
        }
    };
    let focus_in = move |_: ev::FocusEvent| {
        if hover && state.get_untracked() != State::Open {
            open();
        }
    };
    let focus_out = move |ev: ev::FocusEvent| {
        let inside = ev
            .related_target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
            .zip(root.get_untracked())
            .is_some_and(|(next, root)| root.contains(Some(&next)));
        if !inside {
            close();
        }
    };
    let click = move |ev: ev::MouseEvent| {
        ev.stop_propagation();
        match on_activate {
            Some(activate) if hovering() => {
                close();
                activate.run(());
            }
            _ => {
                if state.get_untracked() == State::Open {
                    close();
                } else {
                    open();
                }
            }
        }
    };
    let keydown = move |ev: ev::KeyboardEvent| {
        if ev.key() == "Escape" && state.get_untracked() == State::Open {
            ev.stop_propagation();
            close();
            if let Some(button) = button.get_untracked() {
                let _ = button.focus();
            }
        }
    };
    // While open, a click anywhere else closes it.
    Effect::new(move |previous: Option<Option<WindowListenerHandle>>| {
        if let Some(Some(handle)) = previous {
            handle.remove();
        }
        (state.get() == State::Open).then(|| {
            window_event_listener(ev::click, move |ev| {
                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
                let inside = root.get_untracked().zip(target).is_some_and(|(root, t)| root.contains(Some(&t)));
                if !inside {
                    close();
                }
            })
        })
    });
    on_cleanup(cancel);

    let id = format!("pop-{}", next_id());
    view! {
        <span
            class="popover"
            node_ref=root
            on:mouseenter=enter
            on:mouseleave=leave
            on:focusout=focus_out
            on:keydown=keydown
        >
            <button
                node_ref=button
                type="button"
                class=format!("popover-trigger {trigger_class}")
                aria-label=trigger_label
                aria-expanded=move || (state.get() == State::Open).to_string()
                aria-controls=id.clone()
                on:click=click
                on:focusin=focus_in
            >
                {trigger.run()}
            </button>
            <Show when=move || state.get() != State::Closed>
                <div
                    class="popover-panel"
                    class:closing=move || state.get() == State::Closing
                    class:below=move || below.get()
                    style=move || format!("max-height: {}px", max_height.get().round())
                    id=id.clone()
                    role="dialog"
                    aria-label=label.clone()
                    on:click=|ev: ev::MouseEvent| ev.stop_propagation()
                >
                    {content.run()}
                </div>
            </Show>
        </span>
    }
}

fn next_id() -> u32 {
    thread_local! {
        static NEXT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }
    NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    })
}
