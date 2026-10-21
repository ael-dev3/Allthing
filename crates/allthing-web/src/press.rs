// SPDX-License-Identifier: AGPL-3.0-or-later
//! Press feedback. A control that a finger, the mouse or a key presses shows
//! it in the frame after the press (`data-press="down"`: a little smaller
//! and dimmer, `style/allthing.css`), and eases back when it's let go
//! (`"up"`, 90 ms). One passive listener on the document sets it, for the
//! whole app, rather than `:active`: a touch screen shows `:active` late or
//! not at all (Chrome on Android only once a touch has lasted a moment, iOS
//! Safari only on a page that listens to touches), and a press style that
//! eases in shows nothing in its first frame.

use std::cell::RefCell;
use std::time::Duration;

use leptos::prelude::set_timeout;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::components::cast::after_paint;
use crate::util::document;

/// What shows a press: controls, and links. A button-like link (a tab, a
/// pill, a card) shrinks a little; a link in a line of text, which can't,
/// dims.
const PRESSABLE: &str = "button, [role=button], [role=tab], select, summary, a[href]";

/// How long a control keeps `data-press="up"` once it's let go: long enough
/// for its 90 ms release to play, and no longer, since one shown again later
/// (a list drawn anew) would play it again.
const RELEASE: Duration = Duration::from_millis(150);

thread_local! {
    /// The control being pressed, if any.
    static PRESSED: RefCell<Option<web_sys::Element>> = const { RefCell::new(None) };
}

/// Listen for presses, once, as the app starts.
pub fn install() {
    let options = web_sys::AddEventListenerOptions::new();
    options.set_capture(true);
    options.set_passive(true);
    let listen = |target: &web_sys::EventTarget, name: &str, handler: Closure<dyn Fn(web_sys::Event)>| {
        let _ = target.add_event_listener_with_callback_and_add_event_listener_options(
            name,
            handler.as_ref().unchecked_ref(),
            &options,
        );
        handler.forget();
    };
    let doc: web_sys::EventTarget = document().into();
    // The main button of the primary pointer: a finger, the mouse's left
    // button or a pen's tip.
    listen(
        &doc,
        "pointerdown",
        Closure::new(|ev: web_sys::Event| {
            let Some(ev) = ev.dyn_ref::<web_sys::PointerEvent>() else { return };
            if ev.is_primary() && ev.button() == 0 {
                press(ev.target());
            }
        }),
    );
    // Let go, a touch that turned into a scroll, a long press's menu, a drag.
    for name in ["pointerup", "pointercancel", "contextmenu", "dragstart"] {
        listen(&doc, name, Closure::new(|_: web_sys::Event| release()));
    }
    // Space or Enter on a control with keyboard focus.
    listen(
        &doc,
        "keydown",
        Closure::new(|ev: web_sys::Event| {
            let Some(ev) = ev.dyn_ref::<web_sys::KeyboardEvent>() else { return };
            if !ev.repeat() && matches!(ev.key().as_str(), " " | "Enter") {
                press(ev.target());
            }
        }),
    );
    listen(&doc, "keyup", Closure::new(|_: web_sys::Event| release()));
    listen(&web_sys::window().expect("window").into(), "blur", Closure::new(|_: web_sys::Event| release()));
    // iOS Safari shows `:active` only on a page that listens to touches.
    listen(&doc, "touchstart", Closure::new(|_: web_sys::Event| {}));
}

/// Show a press on the control `target` is in, if it's one that isn't
/// disabled.
fn press(target: Option<web_sys::EventTarget>) {
    release();
    let Some(control) = target
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|element| element.closest(PRESSABLE).ok().flatten())
    else {
        return;
    };
    if control.matches(":disabled").unwrap_or(false)
        || control.get_attribute("aria-disabled").as_deref() == Some("true")
    {
        return;
    }
    let _ = control.set_attribute("data-press", "down");
    PRESSED.with(|pressed| *pressed.borrow_mut() = Some(control));
}

/// Let go of the control being pressed: it eases back once the frame that
/// answers the press (its click's) has been painted, so that frame has no
/// more to draw than the answer, a control the answer took off the page (a
/// tab drawn anew, a page left) isn't animated, and one it marked otherwise
/// keeps that. A frame later than the release is nothing to see.
fn release() {
    let Some(control) = PRESSED.with(|pressed| pressed.borrow_mut().take()) else { return };
    after_paint(move || {
        let pressed_again = PRESSED.with(|pressed| pressed.borrow().as_ref() == Some(&control));
        if pressed_again || control.get_attribute("data-press").as_deref() != Some("down") {
            return;
        }
        if !control.is_connected() {
            let _ = control.remove_attribute("data-press");
            return;
        }
        let _ = control.set_attribute("data-press", "up");
        set_timeout(
            move || {
                if control.get_attribute("data-press").as_deref() == Some("up") {
                    let _ = control.remove_attribute("data-press");
                }
            },
            RELEASE,
        );
    });
}
