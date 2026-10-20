// SPDX-License-Identifier: AGPL-3.0-or-later
//! The composer's emoji and sticker picker: Allthing's Pepe pack
//! (`allthing_core::emoji`), with a search box, and the reader's recently
//! used emoji first. Each shows still and moves while pointed at or focused,
//! unless the reader asks for less motion.
//!
//! An emoji puts its shortcode in the draft, which Allthing shows as the
//! emoji and other Farcaster apps as the shortcode. A sticker goes with the
//! cast as an image embed of its GIF, which every Farcaster app shows.

use std::cell::RefCell;
use std::collections::HashMap;

use allthing_core::emoji::{self, Emoji, STICKER_HEIGHT, Sticker};
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::util::{SOURCE_URL, can_hover, document, href, prefers_reduced_motion};

/// Where the reader's recently used emoji and stickers are kept between
/// visits.
const RECENT_EMOJI: &str = "allthing.emoji";
const RECENT_STICKERS: &str = "allthing.stickers";

thread_local! {
    /// The recently used emoji and stickers for this page, for when
    /// localStorage is blocked.
    static RECENT: RefCell<HashMap<&'static str, String>> = RefCell::new(HashMap::new());
}

/// An emoji picked, and whether the picker stays open for another: Shift
/// held, or a tap on a touch screen.
pub type Pick = (&'static Emoji, bool);

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The reader's recently used emoji or stickers, as `emoji::remember` and
/// `emoji::remember_sticker` keep them.
fn kept(key: &'static str) -> String {
    match storage().map(|s| s.get_item(key)) {
        Some(Ok(Some(kept))) => kept,
        _ => RECENT.with(|r| r.borrow().get(key).cloned().unwrap_or_default()),
    }
}

fn keep(key: &'static str, kept: String) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(key, &kept);
    }
    RECENT.with(|r| r.borrow_mut().insert(key, kept));
}

/// Note `emoji` as the one the reader used last.
pub fn used(emoji: &Emoji) {
    keep(RECENT_EMOJI, emoji::remember(&kept(RECENT_EMOJI), emoji));
}

/// Note `sticker` as the one the reader used last.
pub fn used_sticker(sticker: &Sticker) {
    keep(RECENT_STICKERS, emoji::remember_sticker(&kept(RECENT_STICKERS), sticker));
}

/// What the picker shows. The composer holds it, so its emoji and sticker
/// buttons say which is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Emoji,
    Stickers,
}

impl Tab {
    fn id(self) -> &'static str {
        match self {
            Tab::Emoji => "emoji-tab-emoji",
            Tab::Stickers => "emoji-tab-stickers",
        }
    }

    fn other(self) -> Tab {
        match self {
            Tab::Emoji => Tab::Stickers,
            Tab::Stickers => Tab::Emoji,
        }
    }
}

/// What a tile is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Emoji(&'static Emoji),
    Sticker(&'static Sticker),
}

impl Item {
    fn file(self) -> String {
        match self {
            Item::Emoji(emoji) => emoji.file(),
            Item::Sticker(sticker) => sticker.file(),
        }
    }

    fn still(self) -> String {
        match self {
            Item::Emoji(emoji) => emoji.still(),
            Item::Sticker(sticker) => sticker.still(),
        }
    }
}

/// The tile an event happened in, and what it is.
fn tile_of(target: Option<web_sys::EventTarget>) -> Option<(Element, Item)> {
    let tile = target?.dyn_into::<Element>().ok()?.closest(".emoji-tile").ok()??;
    let code = tile.get_attribute("data-code")?;
    let item = if tile.has_attribute("data-sticker") {
        Item::Sticker(emoji::sticker(&code)?)
    } else {
        Item::Emoji(emoji::find(&code)?)
    };
    Some((tile, item))
}

/// Show a tile moving, or still.
fn animate(tile: &Element, item: Item, moving: bool) {
    if let Some(img) = tile.first_element_child() {
        let _ = img.set_attribute("src", &href(&if moving { item.file() } else { item.still() }));
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

/// One sticker, as `tile`.
fn sticker_tile(sticker: &'static Sticker) -> impl IntoView {
    view! {
        <li>
            <button
                type="button"
                class="emoji-tile sticker-tile"
                data-code=sticker.code
                data-sticker=""
                aria-label=format!("Sticker: {}", sticker.label())
            >
                <img src=href(&sticker.still()) alt="" width=sticker.width height=STICKER_HEIGHT loading="lazy"/>
            </button>
        </li>
    }
}

/// The picker, of emoji or of stickers (`tab`). Enter in its search box
/// picks the first match. Without `stuck`, emoji only (a reaction is one).
#[component]
pub fn EmojiPicker(
    tab: RwSignal<Tab>,
    picked: Callback<Pick>,
    #[prop(optional)] stuck: Option<Callback<&'static Sticker>>,
) -> impl IntoView {
    let stickers_too = stuck.is_some();
    if !stickers_too {
        tab.set(Tab::Emoji);
    }
    let query = RwSignal::new(String::new());
    let found = Memo::new(move |_| query.with(|q| emoji::search(q)));
    let stickers = Memo::new(move |_| query.with(|q| emoji::search_stickers(q)));
    let searching = Memo::new(move |_| query.with(|q| !q.trim().is_empty()));
    let recent = StoredValue::new(emoji::recent(&kept(RECENT_EMOJI)));
    let recent_stickers = StoredValue::new(emoji::recent_stickers(&kept(RECENT_STICKERS)));
    // What is pointed at or focused, shown in the picker's foot.
    let shown = RwSignal::new(None::<Item>);
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
    let pick = move |item: Item, again: bool| match item {
        Item::Emoji(emoji) => picked.run((emoji, again)),
        Item::Sticker(sticker) => {
            if let Some(stuck) = stuck {
                stuck.run(sticker);
            }
        }
    };
    let on_keydown = move |ev: ev::KeyboardEvent| {
        if ev.key() == "Enter" {
            ev.prevent_default();
            let first = match tab.get_untracked() {
                Tab::Emoji => found.with_untracked(|f| f.first().copied().map(Item::Emoji)),
                Tab::Stickers => stickers.with_untracked(|s| s.first().copied().map(Item::Sticker)),
            };
            if let Some(first) = first {
                pick(first, ev.shift_key());
            }
        }
    };
    // The tab's heading, or what a search found, which is read out.
    let heading = move || {
        let (all, one, many, count) = match tab.get() {
            Tab::Emoji => ("Pepe", "emoji", "emoji", found.with(Vec::len)),
            Tab::Stickers => ("Stickers", "sticker", "stickers", stickers.with(Vec::len)),
        };
        if !searching.get() {
            return all.to_owned();
        }
        let q = query.with(|q| q.trim().to_owned());
        match count {
            0 => format!("No {many} for “{q}”"),
            1 => format!("1 {one} for “{q}”"),
            n => format!("{n} {many} for “{q}”"),
        }
    };
    let show = move |this: Tab| {
        shown.set(None);
        tab.set(this);
    };
    // Two tabs: an arrow key moves to the other.
    let tab_button = move |this: Tab, label: &'static str| {
        let selected = move || tab.get() == this;
        view! {
            <button
                type="button"
                role="tab"
                class="emoji-tab"
                id=this.id()
                aria-selected=move || selected().to_string()
                aria-controls="emoji-panel"
                tabindex=move || if selected() { "0" } else { "-1" }
                on:click=move |_| show(this)
                on:keydown=move |ev: ev::KeyboardEvent| {
                    if matches!(ev.key().as_str(), "ArrowLeft" | "ArrowRight") {
                        ev.prevent_default();
                        show(this.other());
                        if let Some(button) = document().get_element_by_id(this.other().id()) {
                            let _ = button.unchecked_into::<web_sys::HtmlElement>().focus();
                        }
                    }
                }
            >
                {label}
            </button>
        }
    };
    view! {
        <section
            class="emoji-picker"
            id="emoji-picker"
            aria-label=if stickers_too { "Pepe emoji and stickers" } else { "Pepe emoji" }
            node_ref=section
        >
            {stickers_too.then(|| view! {
                <div class="emoji-tabs" role="tablist" aria-label="Emoji or stickers">
                    {tab_button(Tab::Emoji, "Emoji")}
                    {tab_button(Tab::Stickers, "Stickers")}
                </div>
            })}
            <input
                class="emoji-search"
                type="search"
                node_ref=input
                placeholder=move || if tab.get() == Tab::Emoji { "Search Pepe emoji" } else { "Search Pepe stickers" }
                aria-label=move || if tab.get() == Tab::Emoji { "Search emoji" } else { "Search stickers" }
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
                id="emoji-panel"
                role=stickers_too.then_some("tabpanel")
                aria-labelledby=move || stickers_too.then(|| tab.get().id())
                on:pointerleave=move |_| shown.set(None)
                on:pointerover=move |ev: ev::PointerEvent| {
                    if let Some((tile, item)) = tile_of(ev.target()) {
                        animate(&tile, item, motion);
                        shown.set(Some(item));
                    }
                }
                on:pointerout=move |ev: ev::PointerEvent| {
                    if let Some((tile, item)) = tile_of(ev.target()) {
                        let inside = ev.related_target().and_then(|to| to.dyn_into::<web_sys::Node>().ok());
                        let focused = document().active_element().is_some_and(|el| el == tile);
                        if !tile.contains(inside.as_ref()) && !focused {
                            animate(&tile, item, false);
                        }
                    }
                }
                on:focusin=move |ev: ev::FocusEvent| {
                    if let Some((tile, item)) = tile_of(ev.target()) {
                        animate(&tile, item, motion);
                        shown.set(Some(item));
                    }
                }
                on:focusout=move |ev: ev::FocusEvent| {
                    if let Some((tile, item)) = tile_of(ev.target()) {
                        animate(&tile, item, motion && tile.matches(":hover").unwrap_or(false));
                        shown.set(None);
                    }
                }
                on:pointerdown=move |ev: ev::PointerEvent| touch.set_value(ev.pointer_type() == "touch")
                // A tap leaves it open; a click or Enter closes it unless
                // Shift is held. A sticker always closes it.
                on:click=move |ev: ev::MouseEvent| {
                    if let Some((_, item)) = tile_of(ev.target()) {
                        pick(item, ev.shift_key() || (ev.detail() > 0 && touch.get_value()));
                    }
                }
            >
                {move || match tab.get() {
                    Tab::Emoji => view! {
                        {recent.with_value(|recent| (!recent.is_empty()).then(|| {
                            let recent = recent.clone();
                            view! {
                                <div class="emoji-recent" hidden=move || searching.get()>
                                    <h3 class="emoji-section">"Recently used"</h3>
                                    <ul class="emoji-grid">{recent.into_iter().map(tile).collect_view()}</ul>
                                </div>
                            }
                        }))}
                        <h3 class="emoji-section" aria-live="polite">{heading}</h3>
                        // Drawn anew for each search: lighter than a keyed list.
                        <ul class="emoji-grid">{move || found.get().into_iter().map(tile).collect_view()}</ul>
                    }
                    .into_any(),
                    Tab::Stickers => view! {
                        {recent_stickers.with_value(|recent| (!recent.is_empty()).then(|| {
                            let recent = recent.clone();
                            view! {
                                <div class="emoji-recent" hidden=move || searching.get()>
                                    <h3 class="emoji-section">"Recently used"</h3>
                                    <ul class="emoji-grid sticker-grid">{recent.into_iter().map(sticker_tile).collect_view()}</ul>
                                </div>
                            }
                        }))}
                        <h3 class="emoji-section" aria-live="polite">{heading}</h3>
                        <ul class="emoji-grid sticker-grid">{move || stickers.get().into_iter().map(sticker_tile).collect_view()}</ul>
                    }
                    .into_any(),
                }}
            </div>
            <footer class="emoji-foot">
                {move || match shown.get() {
                    Some(item) => view! {
                        <span class="emoji-preview" aria-hidden="true">
                            <img src=href(&if motion { item.file() } else { item.still() }) alt=""/>
                            {match item {
                                Item::Emoji(emoji) => view! { <code>":"{emoji.code}":"</code> }.into_any(),
                                Item::Sticker(sticker) => view! { <span>{sticker.label()}</span> }.into_any(),
                            }}
                        </span>
                    }
                    .into_any(),
                    None if tab.get() == Tab::Stickers => {
                        view! { <span class="emoji-hint">"Stickers show in every Farcaster app"</span> }.into_any()
                    }
                    None => view! { <span class="emoji-hint">"Other apps show the "<code>":shortcode:"</code></span> }.into_any(),
                }}
                <a
                    class="emoji-credits"
                    href=move || {
                        let pack = if tab.get() == Tab::Emoji { "emoji" } else { "stickers" };
                        format!("{SOURCE_URL}/blob/main/web/{pack}/CREDITS.md")
                    }
                    target="_blank"
                    rel="noopener noreferrer"
                >
                    "Credits"
                </a>
            </footer>
        </section>
    }
}
