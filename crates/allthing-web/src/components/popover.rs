// SPDX-License-Identifier: AGPL-3.0-or-later
//! A small anchored panel. Hover popovers open after a short dwell (or on
//! keyboard focus) and stay open while the pointer is inside them; on touch
//! screens, and for click popovers, the trigger toggles them. Escape and a
//! click outside close them. Content renders only while open.
//!
//! A panel that a tap or a click opened, and that the pointer leaving
//! doesn't close, is dismissed lightly: the first press outside it only
//! closes it. On a phone, a tap beside a likes preview closes the preview
//! and doesn't also open the post under the tap, follow a link or open
//! another panel; the next tap does. A panel the pointer opened by resting
//! on it closes as the pointer leaves, and a click elsewhere does what it
//! does.
//!
//! The trigger is a button, or the caller's own links (`links`), such as a
//! profile's picture and name: a mouse click or the keyboard follows them as
//! usual, a tap on a touch screen opens the panel instead, and keyboard focus
//! that rests on them opens it after the hover dwell. A `lead` before the
//! trigger, such as a like button before its count, is inside the popover:
//! pointing at it opens the panel, but its clicks are its own.

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
/// A modal dialog over the page, and its backdrop.
const MODAL: &str = ".modal-backdrop, [aria-modal='true']";

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
    /// Accessible name of the trigger button (unused with `links`): it may
    /// change, with the count it names.
    #[prop(into, optional)]
    trigger_label: MaybeProp<String>,
    #[prop(into, optional)] trigger_class: String,
    /// Before the trigger, in the popover but not part of it: pointing at
    /// it opens the panel too, but a click or tap on it is its own (a like
    /// button before its count).
    #[prop(into, optional)]
    lead: Option<ViewFn>,
    /// Nothing to show (a count of 0): the trigger is plain text, and
    /// nothing opens until there is.
    #[prop(into, optional)]
    idle: Option<Signal<bool>>,
    /// Open on hover and focus (desktop); otherwise only on click.
    #[prop(optional)]
    hover: bool,
    /// What a click does on hover-capable screens, instead of toggling.
    #[prop(optional)]
    on_activate: Option<Callback<()>>,
    /// Called as the pointer comes onto the trigger, presses it or keyboard
    /// focus reaches it, before the panel opens: to start fetching what the
    /// panel needs (its code, `crate::split`).
    #[prop(optional)]
    on_warm: Option<Callback<()>>,
    /// The trigger is `trigger` itself, links rather than a button: see the
    /// module notes.
    #[prop(optional)]
    links: bool,
    /// Keyboard focus on the trigger does not open the panel: with `links`,
    /// in a dialog that keeps focus to itself, for one; or a button that
    /// Enter and Space open, as a press would.
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
    /// Closes the panel whenever it changes: when what the panel was for is
    /// done (an emoji picked to react with).
    #[prop(optional, into)]
    shut: Option<Signal<u32>>,
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
    // The open panel was opened by a press on its trigger (a tap, a click or
    // a key), not by the pointer resting there or by keyboard focus.
    let pressed_open = StoredValue::new(false);
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
            let mut edge = if open_below { rect.bottom() + GAP_PX / 2.0 } else { rect.top() - GAP_PX / 2.0 };
            let mut left = rect.left();
            if let Some(outer) = outer_panel() {
                // Beside the outer panel: to its right if the width fits on
                // screen, otherwise to its left. The width is the panel's
                // own, set in CSS and nudged in view below.
                let opened_below = outer.class_list().contains("below");
                let outer = outer.get_bounding_client_rect();
                let width = panel_width();
                let side = if outer.right() + GAP_PX / 2.0 + width <= page_width() {
                    Some(outer.right() + GAP_PX / 2.0)
                } else {
                    Some(outer.left() - GAP_PX / 2.0 - width).filter(|x| *x >= 0.0)
                };
                // Level with the outer panel, the way it opened: from its
                // top down, or from its bottom up. Placed by the name it
                // came from, it hung below the list and the card the list
                // belongs to. Without room at either side (a phone), it
                // stays by that name, which it then leaves in view.
                if let Some(side) = side {
                    left = side;
                    edge = if opened_below { outer.top() } else { outer.bottom() };
                    let room = if opened_below {
                        viewport - outer.top() - GAP_PX
                    } else {
                        outer.bottom() - HEADER_PX - GAP_PX
                    };
                    below.set(opened_below);
                    max_height.set(room.clamp(120.0, max_px));
                } else {
                    left = outer.left() - GAP_PX / 2.0 - width;
                }
            }
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
    // Opens the panel; `press` when a press on the trigger opened it.
    let open_by = move |press: bool| {
        cancel();
        place();
        pressed_open.set_value(press);
        state.set(State::Open);
    };
    let open = move || open_by(false);
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
    if let Some(shut) = shut {
        Effect::new(move |seen: Option<u32>| {
            let now = shut.get();
            if seen.is_some_and(|seen| seen != now) {
                close();
            }
            now
        });
    }
    // Another page: one on a page kept while another shows (the front page,
    // kept.rs) closes, as one on a page left goes with it.
    let pathname = leptos_router::hooks::use_location().pathname;
    Effect::new(move |seen: Option<String>| {
        let now = pathname.get();
        if seen.is_some_and(|seen| seen != now) {
            close();
        }
        now
    });
    let idle = move || idle.is_some_and(|idle| idle.get());
    let hovering = move || hover && can_hover() && !idle();
    let open_delay = if links { LINKS_OPEN_DELAY } else { OPEN_DELAY };

    let warm = move || {
        if let Some(warm) = on_warm {
            warm.run(());
        }
    };
    let enter = move |_: ev::MouseEvent| {
        warm();
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
        warm();
        let keyboard = button.get_untracked().is_some_and(|b| b.matches(":focus-visible").unwrap_or(false));
        // Escape returns focus here while the closing animation runs.
        // That focus must not open the panel again.
        if hover && keyboard && !no_focus_open && state.get_untracked() == State::Closed {
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
                    open_by(true);
                }
            }
        }
    };
    let pointer_down = move |ev: ev::PointerEvent| {
        warm();
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
                open_by(true);
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
    //
    // A panel a press opened, which the pointer leaving doesn't close (any
    // on a touch screen, and click popovers), is dismissed lightly: the
    // first press outside it is the panel's. Its listeners are the window's
    // capture listeners, which hear a press before anything in the page: they
    // keep the press and its click from what is under them (a card, which
    // would open its post, a link, the router, another popover's trigger),
    // and the click closes the panel. A press that turns into a scroll makes
    // no click, and leaves it open. A modal dialog over the page (opened from
    // the panel: "See everyone") keeps its presses.
    let outside_click = StoredValue::new(None::<WindowListenerHandle>);
    let escape_key = StoredValue::new(None::<WindowListenerHandle>);
    let pointer_up = StoredValue::new(None::<WindowListenerHandle>);
    // The window's capture listeners: light dismissal's, and a fixed panel's.
    let captures = StoredValue::new_local(Vec::<Capture>::new());
    let remove_listeners = move || {
        for listener in [outside_click, escape_key, pointer_up] {
            listener.update_value(|current| {
                if let Some(handle) = current.take() {
                    handle.remove();
                }
            });
        }
        captures.update_value(Vec::clear);
    };
    // A press on `target` dismisses the panel lightly: it is outside the
    // popover, and not in a modal dialog over it (one the popover is in, a
    // list of who liked a cast, is no exception).
    let dismisses = move |target: &web_sys::Element| {
        let over = target.closest(MODAL).ok().flatten().is_some_and(|layer| {
            !root.try_get_untracked().flatten().is_some_and(|root| layer.contains(Some(root.unchecked_ref())))
        });
        !inside_root(target) && !over
    };
    Effect::new(move |_| {
        remove_listeners();
        if state.get() != State::Open {
            return;
        }
        if pressed_open.get_value() && !untrack(hovering) {
            captures.update_value(|list| {
                list.push(Capture::new("pointerdown", move |ev| {
                    let primary = ev.dyn_ref::<web_sys::MouseEvent>().is_some_and(|ev| ev.button() == 0);
                    if primary && target_element(&ev).is_some_and(|target| dismisses(&target)) {
                        ev.prevent_default();
                        ev.stop_propagation();
                    }
                }));
                list.push(Capture::new("click", move |ev| {
                    let Some(target) = target_element(&ev) else { return };
                    if inside_root(&target) || selected_in_panel() {
                        return;
                    }
                    if dismisses(&target) {
                        ev.prevent_default();
                        ev.stop_propagation();
                    }
                    close();
                }));
            });
        } else {
            outside_click.set_value(Some(window_event_listener(ev::click, move |ev| {
                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
                if !target.is_some_and(|t| inside_root(&t)) && !selected_in_panel() {
                    close();
                }
            })));
        }
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
            captures.update_value(|list| {
                list.push(Capture::new("scroll", move |ev| {
                    let scrolled = ev.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
                    if !scrolled.is_some_and(|node| inside_panel(&node)) {
                        close();
                    }
                }));
            });
        }
    });
    // Measure the open panel and nudge it back inside the page if it hangs
    // over either side.
    let nudge = move |panel: &web_sys::HtmlElement| {
        let (left, top, width, height) = laid_out(panel);
        let mut left = left - shift.get_untracked();
        if fixed {
            // A transformed ancestor holds fixed elements too: move the
            // panel by however far it landed from where it was placed.
            let (want_left, want_edge) = fixed_at.get_untracked();
            let edge = if below.get_untracked() { top } else { top + height };
            let (dx, dy) = (want_left - left, want_edge - edge);
            if dx.abs() > 0.5 || dy.abs() > 0.5 {
                fixed_at.set((want_left + dx, want_edge + dy));
            }
            left = want_left;
        }
        let page = page_width();
        let margin = 8.0;
        let mut dx = 0.0;
        if left + width > page - margin {
            dx = page - margin - width - left;
        }
        if left + dx < margin {
            dx = margin - left;
        }
        shift.set(dx.round());
    };
    // In the same task as the press that opened it, once the rest of this
    // update has put it in the page, so nothing painted or run after it sees
    // it out of place. Placed a frame later, it hung over a phone's edge in
    // the meantime for anything that measured the page, and widened the
    // phone's layout viewport.
    Effect::new(move || {
        if state.get() != State::Open {
            return;
        }
        let in_page = move || panel.try_get_untracked().flatten().filter(|open| open.is_connected());
        queue_microtask(move || match in_page() {
            Some(open) => nudge(&open),
            // Not in the page yet: as soon as it is, before it's painted.
            None => request_animation_frame(move || {
                if let Some(open) = in_page() {
                    nudge(&open);
                }
            }),
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
        let id = id.clone();
        (move || {
            if idle() {
                return view! { <span class=format!("popover-idle {trigger_class}")>{trigger.run()}</span> }.into_any();
            }
            view! {
                <button
                    node_ref=button
                    type="button"
                    class=format!("popover-trigger {trigger_class}")
                    aria-label=move || trigger_label.get()
                    aria-expanded=move || (state.get() == State::Open).to_string()
                    aria-controls=id.clone()
                    on:click=click
                    on:focusin=focus_in
                >
                    {trigger.run()}
                </button>
            }
            .into_any()
        })
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
            {lead.map(|lead| lead.run())}
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

/// A listener on the window for the capture phase, which hears an event
/// before anything in the page; removed when dropped.
struct Capture {
    event: &'static str,
    listener: Closure<dyn Fn(web_sys::Event)>,
}

impl Capture {
    fn new(event: &'static str, listen: impl Fn(web_sys::Event) + 'static) -> Capture {
        let listener = Closure::<dyn Fn(web_sys::Event)>::new(listen);
        let _ = window().add_event_listener_with_callback_and_bool(event, listener.as_ref().unchecked_ref(), true);
        Capture { event, listener }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = window().remove_event_listener_with_callback_and_bool(
            self.event,
            self.listener.as_ref().unchecked_ref(),
            true,
        );
    }
}

/// The element an event is aimed at (a text node's parent).
fn target_element(ev: &web_sys::Event) -> Option<web_sys::Element> {
    let node = ev.target()?.dyn_into::<web_sys::Node>().ok()?;
    match node.dyn_ref::<web_sys::Element>() {
        Some(element) => Some(element.clone()),
        None => node.parent_element(),
    }
}

/// The width the page is laid out at, without a scrollbar. Not
/// `innerWidth`: on a phone, content that hangs over the edge widens that.
/// Nor `screen.width`, which on an iPhone in landscape stays the width of
/// the screen upright.
fn page_width() -> f64 {
    document()
        .document_element()
        .map(|page| f64::from(page.client_width()))
        .filter(|width| *width > 0.0)
        .unwrap_or_else(|| window().inner_width().ok().and_then(|w| w.as_f64()).unwrap_or(1024.0))
}

/// A panel's box in the viewport as laid out (left, top, width, height),
/// without its own transform: as it opens, its opening animation scales it
/// about its middle and lifts it, which put the edges it was nudged by a
/// few pixels out. (`translate`, a fixed panel's nudge and lift, stays in.)
fn laid_out(panel: &web_sys::HtmlElement) -> (f64, f64, f64, f64) {
    let rect = panel.get_bounding_client_rect();
    let (width, height) = (f64::from(panel.offset_width()), f64::from(panel.offset_height()));
    // A scale about the middle leaves the middle where it was; only the
    // transform's own move moves it.
    let (moved_x, moved_y) = window()
        .get_computed_style(panel)
        .ok()
        .flatten()
        .and_then(|style| style.get_property_value("transform").ok())
        .and_then(|transform| web_sys::DomMatrixReadOnly::new_with_str(&transform).ok())
        .map_or((0.0, 0.0), |matrix| (matrix.e(), matrix.f()));
    let middle_x = (rect.left() + rect.right()) / 2.0 - moved_x;
    let middle_y = (rect.top() + rect.bottom()) / 2.0 - moved_y;
    (middle_x - width / 2.0, middle_y - height / 2.0, width, height)
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
