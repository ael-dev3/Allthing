// SPDX-License-Identifier: AGPL-3.0-or-later
//! The composer's emoji picker: Allthing's Pepe pack (`allthing_core::emoji`),
//! with a search box and the reader's recently used emoji first. Each shows
//! still and moves while pointed at or focused, unless the reader asks for
//! less motion. A pick puts its shortcode in the draft, which Allthing
//! shows as the emoji and other Farcaster apps as the shortcode.

use std::cell::RefCell;

use allthing_core::emoji::{self, Emoji};
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::util::{SOURCE_URL, can_hover, document, href, prefers_reduced_motion};

/// Where the reader's recently used emoji are kept between visits.
const RECENT_KEY: &str = "allthing.emoji";

thread_local! {
    /// The recently used emoji for this page, for when localStorage is blocked.
    static RECENT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// An emoji picked, and whether the picker stays open for another: Shift
/// held, or a tap on a touch screen.
pub type Pick = (&'static Emoji, bool);

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The reader's recently used emoji, as `emoji::remember` keeps them.
fn kept() -> String {
    match storage().map(|s| s.get_item(RECENT_KEY)) {
        Some(Ok(Some(kept))) => kept,
        _ => RECENT.with(|r| r.borrow().clone()),
    }
}

/// Note `emoji` as the one the reader used last.
pub fn used(emoji: &Emoji) {
    let kept = emoji::remember(&kept(), emoji);
    if let Some(storage) = storage() {
        let _ = storage.set_item(RECENT_KEY, &kept);
    }
    RECENT.with(|r| *r.borrow_mut() = kept);
}

/// The tile an event happened in, and its emoji.
fn tile_of(target: Option<web_sys::EventTarget>) -> Option<(Element, &'static Emoji)> {
    let tile = target?.dyn_into::<Element>().ok()?.closest(".emoji-tile").ok()??;
    let emoji = emoji::find(&tile.get_attribute("data-code")?)?;
    Some((tile, emoji))
}

/// Show a tile's emoji moving, or still.
fn animate(tile: &Element, emoji: &Emoji, moving: bool) {
    if let Some(img) = tile.first_element_child() {
        let _ = img.set_attribute("src", &href(&if moving { emoji.file() } else { emoji.still() }));
    }
}

/// One emoji, still until pointed at or focused (the picker's listeners do
/// that, for all of them).
fn tile(emoji: &'static Emoji) -> impl IntoView {
    view! {
        <li>
            <button type="button" class="emoji-tile" data-code=emoji.code aria-label=emoji.label()>
                // Decoded at once: a search draws its results anew.
                <img src=href(&emoji.still()) alt="" width=emoji.width height=emoji.height loading="lazy"/>
            </button>
        </li>
    }
}

/// The picker. Enter in its search box picks the first match.
#[component]
pub fn EmojiPicker(picked: Callback<Pick>) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let found = Memo::new(move |_| query.with(|q| emoji::search(q)));
    let searching = Memo::new(move |_| query.with(|q| !q.trim().is_empty()));
    let recent = emoji::recent(&kept());
    // The emoji pointed at or focused, shown larger in the picker's foot.
    let shown = RwSignal::new(None::<&'static Emoji>);
    let motion = !prefers_reduced_motion();
    // The last press was a touch: a tap leaves the picker open.
    let touch = StoredValue::new(false);
    let input = NodeRef::<leptos::html::Input>::new();
    let section = NodeRef::<leptos::html::Section>::new();
    Effect::new(move |_| {
        // All of it in view, where the dialog scrolls.
        if let Some(section) = section.get() {
            let options = web_sys::ScrollIntoViewOptions::new();
            options.set_block(web_sys::ScrollLogicalPosition::Nearest);
            section.scroll_into_view_with_scroll_into_view_options(&options);
        }
        // Into the search box where there's a keyboard to type with; on a
        // phone, the keyboard would cover the emoji.
        if let Some(input) = input.get().filter(|_| can_hover()) {
            let _ = input.focus();
        }
    });
    let on_keydown = move |ev: ev::KeyboardEvent| {
        if ev.key() == "Enter" {
            ev.prevent_default();
            if let Some(first) = found.with_untracked(|f| f.first().copied()) {
                picked.run((first, ev.shift_key()));
            }
        }
    };
    // The pack's heading, or what a search found, which is read out.
    let heading = move || {
        if !searching.get() {
            return "Pepe".to_owned();
        }
        let q = query.with(|q| q.trim().to_owned());
        match found.with(Vec::len) {
            0 => format!("No emoji for “{q}”"),
            1 => format!("1 emoji for “{q}”"),
            n => format!("{n} emoji for “{q}”"),
        }
    };
    view! {
        <section class="emoji-picker" id="emoji-picker" aria-label="Pepe emoji" node_ref=section>
            <input
                class="emoji-search"
                type="search"
                node_ref=input
                placeholder="Search Pepe emoji"
                aria-label="Search emoji"
                autocomplete="off"
                enterkeyhint="done"
                on:input=move |ev| {
                    // What was pointed at may be gone from the results.
                    shown.set(None);
                    query.set(event_target_value(&ev));
                }
                on:keydown=on_keydown
            />
            // The tiles' listeners, once for all of them.
            <div
                class="emoji-results"
                on:pointerleave=move |_| shown.set(None)
                on:pointerover=move |ev: ev::PointerEvent| {
                    if let Some((tile, emoji)) = tile_of(ev.target()) {
                        animate(&tile, emoji, motion);
                        shown.set(Some(emoji));
                    }
                }
                on:pointerout=move |ev: ev::PointerEvent| {
                    if let Some((tile, emoji)) = tile_of(ev.target()) {
                        let inside = ev.related_target().and_then(|to| to.dyn_into::<web_sys::Node>().ok());
                        let focused = document().active_element().is_some_and(|el| el == tile);
                        if !tile.contains(inside.as_ref()) && !focused {
                            animate(&tile, emoji, false);
                        }
                    }
                }
                on:focusin=move |ev: ev::FocusEvent| {
                    if let Some((tile, emoji)) = tile_of(ev.target()) {
                        animate(&tile, emoji, motion);
                        shown.set(Some(emoji));
                    }
                }
                on:focusout=move |ev: ev::FocusEvent| {
                    if let Some((tile, emoji)) = tile_of(ev.target()) {
                        animate(&tile, emoji, motion && tile.matches(":hover").unwrap_or(false));
                        shown.set(None);
                    }
                }
                on:pointerdown=move |ev: ev::PointerEvent| touch.set_value(ev.pointer_type() == "touch")
                // A tap leaves it open; a click or Enter closes it unless Shift is held.
                on:click=move |ev: ev::MouseEvent| {
                    if let Some((_, emoji)) = tile_of(ev.target()) {
                        picked.run((emoji, ev.shift_key() || (ev.detail() > 0 && touch.get_value())));
                    }
                }
            >
                {(!recent.is_empty()).then(|| view! {
                    <div class="emoji-recent" hidden=move || searching.get()>
                        <h3 class="emoji-section">"Recently used"</h3>
                        <ul class="emoji-grid">{recent.into_iter().map(tile).collect_view()}</ul>
                    </div>
                })}
                <h3 class="emoji-section" aria-live="polite">{heading}</h3>
                // Drawn anew for each search: lighter than a keyed list.
                <ul class="emoji-grid">{move || found.get().into_iter().map(tile).collect_view()}</ul>
            </div>
            <footer class="emoji-foot">
                {move || match shown.get() {
                    Some(emoji) => view! {
                        <span class="emoji-preview" aria-hidden="true">
                            <img src=href(&if motion { emoji.file() } else { emoji.still() }) alt="" width=emoji.width height=emoji.height/>
                            <code>":"{emoji.code}":"</code>
                        </span>
                    }
                    .into_any(),
                    None => view! { <span class="emoji-hint">"Other apps show the "<code>":shortcode:"</code></span> }.into_any(),
                }}
                <a class="emoji-credits" href=format!("{SOURCE_URL}/blob/main/web/emoji/CREDITS.md") target="_blank" rel="noopener noreferrer">
                    "Credits"
                </a>
            </footer>
        </section>
    }
}
