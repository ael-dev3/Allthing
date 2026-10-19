// SPDX-License-Identifier: AGPL-3.0-or-later
//! A small anchored panel. Hover popovers open after a short dwell (or on
//! keyboard focus) and stay open while the pointer is inside them; on touch
//! screens, and for click popovers, the trigger toggles them. Escape and a
//! click outside close them. Content renders only while open.
//!
//! The trigger is a button, or the caller's own links (`links`), such as a
//! profile's picture and name: a mouse click or the keyboard follows them as
//! usual, a tap on a touch screen opens the panel instead, and keyboard focus
//! that rests on them opens it after the hover dwell.

use std::time::Duration;

use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::util::can_hover;

const OPEN_DELAY: Duration = Duration::from_millis(280);
/// Profile cards (on links) open almost at once: their data is preloaded,
/// and 100 ms still skips a pointer just passing over a name.
const LINKS_OPEN_DELAY: Duration = Duration::from_millis(100);
const CLOSE_DELAY: Duration = Duration::from_millis(180);
const CLOSE_ANIMATION: Duration = Duration::from_millis(120);
/// The sticky header covers this much of the top of the viewport.
const HEADER_PX: f64 = 72.0;
/// Panels are at most this tall, unless told otherwise, and shrink to the
/// room they have.
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
    /// Accessible name of the trigger button (unused with `links`).
    #[prop(into, optional)]
    trigger_label: String,
    #[prop(into, optional)] trigger_class: String,
    /// Open on hover and focus (desktop); otherwise only on click.
    #[prop(optional)]
    hover: bool,
    /// What a click does on hover-capable screens, instead of toggling.
    #[prop(optional)]
    on_activate: Option<Callback<()>>,
    /// The trigger is `trigger` itself, links rather than a button: see the
    /// module notes.
    #[prop(optional)]
    links: bool,
    /// With `links`: keyboard focus on them does not open the panel (in a
    /// dialog that keeps focus to itself, for one).
    #[prop(optional)]
    no_focus_open: bool,
    /// Place the panel against the viewport rather than inside the page's
    /// flow, so a container around the trigger that scrolls or clips (a
    /// dialog's list, say) does not cut it off. Anything scrolling closes it.
    #[prop(optional)]
    fixed: bool,
    /// With `fixed`, inside another popover's panel: the panel opens beside
    /// that panel rather than over the trigger, so it covers the list it
    /// came from.
    #[prop(optional)]
    beside: bool,
    /// How tall the panel may be, in pixels, room permitting.
    #[prop(optional)]
    max_px: Option<f64>,
    /// Classes for the root element.
    #[prop(into, optional)]
    class: String,
    /// Classes for the panel.
    #[prop(into, optional)]
    panel_class: String,
    #[prop(into)] trigger: ViewFn,
    #[prop(into)] content: ViewFn,
) -> impl IntoView {
    let state = RwSignal::new(State::Closed);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let root = NodeRef::<leptos::html::Span>::new();
    let button = NodeRef::<leptos::html::Button>::new();
    let max_px = max_px.unwrap_or(MAX_PANEL_PX);
    // Where the panel opens: above the trigger (default) or below it, and
    // how tall it may be, measured each time it opens.
    let below = RwSignal::new(false);
    let max_height = RwSignal::new(max_px);
    // Sideways nudge that keeps the open panel on screen on narrow viewports.
    let shift = RwSignal::new(0.0_f64);
    // Viewport position of a fixed panel: its left edge, and the edge that
    // faces the trigger (its top when below, its bottom when above).
    let fixed_at = RwSignal::new((0.0_f64, 0.0_f64));
    let panel = NodeRef::<leptos::html::Div>::new();
    // How the last press began ("mouse", "touch", "pen"), and the trigger
    // link that keyboard focus last rested on.
    let pointer = StoredValue::new(String::new());
    let focused_link = StoredValue::new_local(None::<web_sys::HtmlElement>);
    // A mouse button went down in the panel, perhaps to select its text, and
    // is still down: the pointer may leave the panel without closing it.
    let pressing = StoredValue::new(false);
    let anchor = move || -> Option<web_sys::Element> {
        if links {
            root.get_untracked().map(|r| r.unchecked_into())
        } else {
            button.get_untracked().map(|b| b.unchecked_into())
        }
    };
    // The popover this one sits in, when it's `beside` that one's panel.
    let outer_panel = move || {
        if !beside {
            return None;
        }
        let root = root.get_untracked()?;
        root.closest(".popover-panel").ok().flatten()?.dyn_into::<web_sys::HtmlElement>().ok()
    };
    // The width the panel will take, which CSS sets: measured from the
    // element once it exists, else the CSS default for profile cards.
    let panel_width = move || {
        panel.get_untracked().map_or(300.0, |p| {
            let width = p.get_bounding_client_rect().width();
            if width > 0.0 { width } else { 300.0 }
        })
    };
    let place = move || {
        let Some(anchor) = anchor() else { return };
        let rect = anchor.get_bounding_client_rect();
        let viewport = window().inner_height().ok().and_then(|h| h.as_f64()).unwrap_or(800.0);
        let above_room = rect.top() - HEADER_PX - GAP_PX;
        let below_room = viewport - rect.bottom() - GAP_PX;
        let open_below = above_room < max_px && below_room > above_room;
        below.set(open_below);
        max_height.set(if open_below { below_room } else { above_room }.clamp(120.0, max_px));
        shift.set(0.0);
        if fixed {
            let edge = if open_below { rect.bottom() + GAP_PX / 2.0 } else { rect.top() - GAP_PX / 2.0 };
            let left = match outer_panel() {
                Some(outer) => {
                    // Beside the outer panel: to its right if the width fits
                    // on screen, otherwise to its left. The width is the
                    // panel's own, set in CSS and nudged in view below.
                    let outer = outer.get_bounding_client_rect();
                    let width = panel_width();
                    let inner = window().inner_width().ok().and_then(|w| w.as_f64()).unwrap_or(1024.0);
                    if outer.right() + GAP_PX / 2.0 + width <= inner {
                        outer.right() + GAP_PX / 2.0
                    } else {
                        outer.left() - GAP_PX / 2.0 - width
                    }
                }
                None => rect.left(),
            };
            fixed_at.set((left, edge));
        }
    };
    let inside_panel =
        move |node: &web_sys::Node| panel.try_get_untracked().flatten().is_some_and(|p| p.contains(Some(node)));
    // A panel placed `beside` is still this popover's: its pointer, clicks and
    // focus count as inside.
    let inside_root = move |node: &web_sys::Node| {
        root.try_get_untracked().flatten().is_some_and(|r| r.contains(Some(node))) || inside_panel(node)
    };
    // Text selected in the panel: the reader is copying from it.
    let selected_in_panel = move || {
        window()
            .get_selection()
            .ok()
            .flatten()
            .filter(|s| !s.is_collapsed())
            .and_then(|s| s.anchor_node())
            .is_some_and(|node| inside_panel(&node))
    };

    // A pending open or close (`timer`), which the pointer and focus cancel
    // or replace. The end of the closing animation has a timer of its own,
    // so nothing leaves a panel invisible but still in the page.
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
        pressing.set_value(false);
        if state.get_untracked() == State::Open {
            state.set(State::Closing);
            set_timeout(
                move || {
                    if state.try_get_untracked() == Some(State::Closing) {
                        let _ = state.try_set(State::Closed);
                    }
                },
                CLOSE_ANIMATION,
            );
        }
    };
    let hovering = move || hover && can_hover();
    let open_delay = if links { LINKS_OPEN_DELAY } else { OPEN_DELAY };

    let enter = move |_: ev::MouseEvent| {
        if hovering() {
            if state.get_untracked() == State::Open {
                cancel();
            } else {
                schedule(open_delay, Box::new(open));
            }
        }
    };
    let leave = move |_: ev::MouseEvent| {
        if hovering() {
            if state.get_untracked() != State::Open {
                cancel();
            } else if !pressing.get_value() {
                schedule(CLOSE_DELAY, Box::new(close));
            }
        }
    };
    // Keyboard focus opens hover popovers. A tap or click also focuses the
    // button, but then the click itself decides; opening here as well would
    // have the click toggle the popover straight back closed.
    let focus_in = move |_: ev::FocusEvent| {
        let keyboard = button.get_untracked().is_some_and(|b| b.matches(":focus-visible").unwrap_or(false));
        // Escape returns focus here while the closing animation runs.
        // That focus must not open the panel again.
        if hover && keyboard && state.get_untracked() == State::Closed {
            open();
        }
    };
    // Links: keyboard focus that rests on one opens the panel after the
    // hover dwell; a mouse click focuses links without `:focus-visible`.
    let link_focus = move |ev: ev::FocusEvent| {
        let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) else { return };
        if !links || inside_panel(&target) {
            return;
        }
        let keyboard = target.matches(":focus-visible").unwrap_or(false);
        focused_link.set_value(Some(target));
        if hover && keyboard && !no_focus_open && state.get_untracked() == State::Closed {
            schedule(open_delay, Box::new(open));
        }
    };
    let focus_out = move |ev: ev::FocusEvent| {
        let inside =
            ev.related_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()).is_some_and(|next| inside_root(&next));
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
    let pointer_down = move |ev: ev::PointerEvent| {
        let in_panel = ev.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()).is_some_and(|n| inside_panel(&n));
        pressing.set_value(in_panel && ev.pointer_type() == "mouse");
        pointer.set_value(ev.pointer_type());
    };
    // Links: a tap opens or closes the panel instead of following the link
    // (the panel has its own links); a mouse click or the keyboard follows
    // it. A link in the panel goes where it points, and the panel closes.
    let link_click = move |ev: ev::MouseEvent| {
        let mut pressed = String::new();
        pointer.update_value(|kind| pressed = std::mem::take(kind));
        if !links {
            return;
        }
        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
        let Some(link) = target.and_then(|t| t.closest("a[href]").ok().flatten()) else { return };
        if inside_panel(&link) {
            close();
            return;
        }
        // Clicks are pointer events in current browsers, with no pointer type
        // when a key made them; elsewhere, the press that began the click says.
        let kind = js_sys::Reflect::get(&ev, &wasm_bindgen::JsValue::from_str("pointerType"))
            .ok()
            .and_then(|t| t.as_string())
            .filter(|kind| !kind.is_empty())
            .unwrap_or(pressed);
        let tap = kind == "touch" || (kind == "pen" && !can_hover());
        if tap {
            ev.prevent_default();
            if state.get_untracked() == State::Open {
                close();
            } else {
                open();
            }
        } else {
            close();
        }
    };
    let keydown = move |ev: ev::KeyboardEvent| {
        // A key press, not a finger, will follow (a tap that became a
        // scroll or a long press leaves its press behind).
        pointer.set_value(String::new());
        if ev.key() == "Escape" && state.get_untracked() == State::Open {
            ev.stop_propagation();
            close();
            if links {
                // Back to the link focus rested on, or else the first one.
                let back = focused_link.get_value().filter(|link| link.is_connected()).or_else(|| {
                    let first = root.get_untracked()?.query_selector("a[href]:not([tabindex='-1'])").ok()??;
                    first.dyn_into::<web_sys::HtmlElement>().ok()
                });
                if let Some(link) = back {
                    let _ = link.focus();
                }
            } else if let Some(button) = button.get_untracked() {
                let _ = button.focus();
            }
        }
    };
    // While open, a click anywhere else closes it, and so does Escape when
    // focus is elsewhere (a panel the pointer opened). A selection that began
    // in the panel keeps it open: the click that ends the selection, outside
    // it, doesn't count, and the pointer leaves it only once the button is up.
    let outside_click = StoredValue::new(None::<WindowListenerHandle>);
    let escape_key = StoredValue::new(None::<WindowListenerHandle>);
    let pointer_up = StoredValue::new(None::<WindowListenerHandle>);
    let any_scroll = StoredValue::new_local(None::<Closure<dyn Fn(web_sys::Event)>>);
    let remove_listeners = move || {
        for listener in [outside_click, escape_key, pointer_up] {
            listener.update_value(|current| {
                if let Some(handle) = current.take() {
                    handle.remove();
                }
            });
        }
        any_scroll.update_value(|current| {
            if let Some(closure) = current.take() {
                let _ = window().remove_event_listener_with_callback_and_bool(
                    "scroll",
                    closure.as_ref().unchecked_ref(),
                    true,
                );
            }
        });
    };
    Effect::new(move |_| {
        remove_listeners();
        if state.get() != State::Open {
            return;
        }
        outside_click.set_value(Some(window_event_listener(ev::click, move |ev| {
            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            if !target.is_some_and(|t| inside_root(&t)) && !selected_in_panel() {
                close();
            }
        })));
        escape_key.set_value(Some(window_event_listener(ev::keydown, move |ev| {
            if ev.key() == "Escape" {
                close();
            }
        })));
        pointer_up.set_value(Some(window_event_listener(ev::pointerup, move |ev| {
            if !pressing.get_value() {
                return;
            }
            pressing.set_value(false);
            let over = ev.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()).is_some_and(|t| inside_root(&t));
            if !over && hovering() && !selected_in_panel() {
                schedule(CLOSE_DELAY, Box::new(close));
            }
        })));
        if fixed {
            // A fixed panel would stay put while its trigger scrolls away.
            let closure = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
                let scrolled = ev.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
                if !scrolled.is_some_and(|node| inside_panel(&node)) {
                    close();
                }
            });
            let _ =
                window().add_event_listener_with_callback_and_bool("scroll", closure.as_ref().unchecked_ref(), true);
            any_scroll.set_value(Some(closure));
        }
    });
    // Once the panel is laid out, measure it and nudge it back inside the
    // viewport if it hangs over either side.
    Effect::new(move || {
        if state.get() != State::Open {
            return;
        }
        request_animation_frame(move || {
            let Some(panel) = panel.try_get_untracked().flatten() else { return };
            let rect = panel.get_bounding_client_rect();
            let mut left = rect.left() - shift.get_untracked();
            if fixed {
                // A transformed ancestor holds fixed elements too: move the
                // panel by however far it landed from where it was placed.
                let (want_left, want_edge) = fixed_at.get_untracked();
                let edge = if below.get_untracked() { rect.top() } else { rect.bottom() };
                let (dx, dy) = (want_left - left, want_edge - edge);
                if dx.abs() > 0.5 || dy.abs() > 0.5 {
                    fixed_at.set((want_left + dx, want_edge + dy));
                }
                left = want_left;
            }
            // On phones an overflowing panel widens the layout viewport
            // (innerWidth), so bound it by the screen as well.
            let inner = window().inner_width().ok().and_then(|w| w.as_f64()).unwrap_or(1024.0);
            let screen = window().screen().ok().and_then(|s| s.width().ok()).map_or(inner, f64::from);
            let viewport = inner.min(screen);
            let margin = 8.0;
            let mut dx = 0.0;
            if left + rect.width() > viewport - margin {
                dx = viewport - margin - rect.width() - left;
            }
            if left + dx < margin {
                dx = margin - left;
            }
            shift.set(dx.round());
        });
    });
    on_cleanup(move || {
        cancel();
        remove_listeners();
    });

    let id = format!("pop-{}", next_id());
    let panel_style = move || {
        let mut style = format!("max-height: {}px; ", max_height.get().round());
        if fixed {
            let (left, edge) = fixed_at.get();
            let lift = if below.get() { "0" } else { "-100%" };
            style.push_str(&format!(
                "position: fixed; left: {left:.0}px; top: {edge:.0}px; bottom: auto; translate: {}px {lift}",
                shift.get()
            ));
        } else if shift.get() != 0.0 {
            // Margins, not a transform: a transform, even `translate: 0`,
            // would hold the fixed panels inside this one (a profile card
            // opened from a who-list) and clip them to its scroll box. The
            // pair moves a panel pinned by its left edge or by its right.
            let s = shift.get();
            style.push_str(&format!("margin-left: {s}px; margin-right: {}px", -s));
        }
        style
    };
    let trigger = if links {
        trigger.run()
    } else {
        view! {
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
        }
        .into_any()
    };
    view! {
        <span
            class=format!("popover {class}")
            node_ref=root
            on:mouseenter=enter
            on:mouseleave=leave
            on:focusin=link_focus
            on:focusout=focus_out
            on:keydown=keydown
            on:pointerdown=pointer_down
            on:click=link_click
        >
            {trigger}
            <Show when=move || state.get() != State::Closed>
                <div
                    class=format!("popover-panel {panel_class}")
                    class:beside=beside
                    class:closing=move || state.get() == State::Closing
                    class:below=move || below.get()
                    class:fixed=fixed
                    node_ref=panel
                    style=panel_style
                    id=id.clone()
                    role="dialog"
                    aria-label=label.clone()
                    // A link in the panel goes where it points, and the panel
                    // closes. A tap that opened a name's own card instead (its
                    // link not followed) leaves it open.
                    on:click=move |ev: ev::MouseEvent| {
                        let in_link = ev
                            .target()
                            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                            .is_some_and(|e| e.closest("a[href]").ok().flatten().is_some());
                        if in_link && !ev.default_prevented() {
                            close();
                        }
                    }
                    // A click on the panel's text keeps focus inside, so a
                    // trigger link losing it doesn't close the panel.
                    tabindex=links.then_some("-1")
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
