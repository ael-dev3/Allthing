// SPDX-License-Identifier: AGPL-3.0-or-later
//! The composer's emoji and sticker picker: Allthing's Pepe pack
//! (`allthing_core::emoji`), with a search box, and the reader's recently
//! used emoji first. The pack shows group by group, under headings, with a
//! bar of the groups that brings one to the top and marks the one in view.
//! Each emoji shows still and moves while pointed at or focused, unless the
//! reader asks for less motion. Stickers, as on Telegram, move while in view
//! (unless the reader asks for less motion, or to save data), in a taller
//! panel, and a sticker held down shows large until let go.
//!
//! An emoji puts its shortcode in the draft, which Allthing shows as the
//! emoji and other Farcaster apps as the shortcode. A sticker goes with the
//! cast as an image embed of its GIF, which every Farcaster app shows.

use std::cell::RefCell;
use std::collections::HashMap;

use allthing_core::emoji::{self, Emoji, Group, STICKER_HEIGHT, Sticker};
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{Element, IntersectionObserver, IntersectionObserverEntry, IntersectionObserverInit};

use crate::util::{SOURCE_URL, can_hover, document, href, prefers_reduced_motion, saves_data};

/// Where the reader's recently used emoji and stickers are kept between
/// visits.
const RECENT_EMOJI: &str = "allthing.emoji";
const RECENT_STICKERS: &str = "allthing.stickers";

/// The tiles a picker draws as it opens, about the rows in view; the other
/// groups follow, one a frame, once that frame is painted. Drawing all 219
/// at once, and loading all their pictures, held the click that opened it
/// for 280 to 344 ms on a computer (docs/PERFORMANCE.md).
const FIRST_TILES: usize = 48;

/// How many of the groups, in order, the first `FIRST_TILES` tiles are in.
fn first_groups() -> usize {
    let mut tiles = 0;
    Group::ALL
        .iter()
        .take_while(|group| {
            let before = tiles;
            tiles += group.emoji().count();
            before < FIRST_TILES
        })
        .count()
}

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

/// How long a sticker is held down before it shows large.
const HOLD_MS: u64 = 350;

/// What a tile is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Emoji(&'static Emoji),
    Sticker(&'static Sticker),
}

impl Item {
    /// It moving: an emoji's file, a sticker's tile (its GIF is for casts).
    fn file(self) -> String {
        match self {
            Item::Emoji(emoji) => emoji.file(),
            Item::Sticker(sticker) => sticker.tile(),
        }
    }

    fn still(self) -> String {
        match self {
            Item::Emoji(emoji) => emoji.still(),
            Item::Sticker(sticker) => sticker.still(),
        }
    }

    /// Its shortcode: a sticker's is its emoji's, by the same maker.
    fn code(self) -> &'static str {
        match self {
            Item::Emoji(emoji) => emoji.code,
            Item::Sticker(sticker) => sticker.code,
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

/// Show a tile moving, or still. A sticker that moves while in view keeps
/// moving.
fn animate(tile: &Element, item: Item, moving: bool) {
    if tile.has_attribute("data-moving") {
        return;
    }
    if let Some(img) = tile.first_element_child() {
        // Shown now, it isn't one for `load_in_view` to load any more.
        let _ = img.remove_attribute("data-src");
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

/// One of a group's emoji, as `tile`, its picture loaded as it comes into
/// view (`load_in_view`): all 219 loading as the picker opened took 150 to
/// 400 ms of the page's time on a slow phone, most for emoji never seen.
fn group_tile(emoji: &'static Emoji) -> impl IntoView {
    view! {
        <li>
            <button type="button" class="emoji-tile" data-code=emoji.code aria-label=emoji.label()>
                <img data-src=href(&emoji.still()) alt="" width=emoji.width height=emoji.height/>
            </button>
        </li>
    }
}

/// Draws the groups not drawn yet, one a frame, and has the pictures of
/// those drawn load as they come into view (`watch`).
fn draw_rest(drawn: RwSignal<usize>, watch: impl Fn() + Copy + 'static) {
    crate::components::cast::after_paint(move || {
        watch();
        let Some(now) = drawn.try_get_untracked() else { return };
        if now < Group::ALL.len() {
            let _ = drawn.try_set(now + 1);
            draw_rest(drawn, watch);
        }
    });
}

/// The observer that loads the pictures of a group's emoji and of moving
/// stickers as they come into view in `results` (`data-src` becomes
/// `src`), about a row ahead, not all on opening, as `loading="lazy"` would
/// in a scroll box this short.
type Seen = (IntersectionObserver, Closure<dyn FnMut(js_sys::Array, IntersectionObserver)>);

fn load_in_view(results: &Element) -> Option<Seen> {
    let callback = Closure::<dyn FnMut(js_sys::Array, IntersectionObserver)>::new(
        move |entries: js_sys::Array, observer: IntersectionObserver| {
            let entries = entries.iter().filter_map(|e| e.dyn_into::<IntersectionObserverEntry>().ok());
            for entry in entries.filter(IntersectionObserverEntry::is_intersecting) {
                let img = entry.target();
                if let Some(src) = img.get_attribute("data-src") {
                    let _ = img.set_attribute("src", &src);
                    let _ = img.remove_attribute("data-src");
                }
                observer.unobserve(&img);
            }
        },
    );
    let options = IntersectionObserverInit::new();
    options.set_root(Some(results));
    options.set_root_margin("64px 0px");
    let observer = IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options).ok()?;
    Some((observer, callback))
}

/// One sticker, moving once in view, as on Telegram, or still as `tile`.
fn sticker_tile(sticker: &'static Sticker, moving: bool) -> impl IntoView {
    view! {
        <li>
            <button
                type="button"
                class="emoji-tile sticker-tile"
                data-code=sticker.code
                data-sticker=""
                data-moving=moving.then_some("")
                aria-label=format!("Sticker: {}", sticker.label())
            >
                <img
                    src=(!moving).then(|| href(&sticker.still()))
                    data-src=moving.then(|| href(&sticker.tile()))
                    alt=""
                    width=sticker.width
                    height=STICKER_HEIGHT
                    loading="lazy"
                    draggable="false"
                />
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
    // How many groups are drawn, while they're shown (`draw_rest`).
    let drawing = StoredValue::new(None::<RwSignal<usize>>);
    let found = Memo::new(move |_| query.with(|q| emoji::search(q)));
    let stickers = Memo::new(move |_| query.with(|q| emoji::search_stickers(q)));
    let searching = Memo::new(move |_| query.with(|q| !q.trim().is_empty()));
    let recent = StoredValue::new(emoji::recent(&kept(RECENT_EMOJI)));
    let recent_stickers = StoredValue::new(emoji::recent_stickers(&kept(RECENT_STICKERS)));
    // What is pointed at or focused, shown in the picker's foot.
    let shown = RwSignal::new(None::<Item>);
    let motion = !prefers_reduced_motion();
    // Stickers move while in view, unless the reader saves data.
    let moving = motion && !saves_data();
    // The last press was a touch: a tap leaves the picker open.
    let touch = StoredValue::new(false);
    // A sticker held down shows large; the press that showed it picks
    // nothing. `hold` moves on with each press, so a let-go one stops.
    let preview = RwSignal::new(None::<&'static Sticker>);
    let hold = StoredValue::new(0u64);
    let held = StoredValue::new(false);
    let let_go = move || {
        hold.update_value(|run| *run += 1);
        preview.set(None);
    };
    let input = NodeRef::<leptos::html::Input>::new();
    let section = NodeRef::<leptos::html::Section>::new();
    let results = NodeRef::<leptos::html::Div>::new();
    // Pictures that wait to come into view load then (`load_in_view`): the
    // groups' emoji, as each group is drawn (`draw_rest`), and moving
    // stickers, whenever those shown change.
    let seen = StoredValue::new_local(None::<Seen>);
    let watch = move || {
        let Some(root) = results.get_untracked() else { return };
        if seen.with_value(Option::is_none) {
            seen.set_value(load_in_view(&root));
        }
        seen.with_value(|seen| {
            let Some((observer, _)) = seen else { return };
            let Ok(waiting) = root.query_selector_all("img[data-src]") else { return };
            for img in (0..waiting.length()).filter_map(|i| waiting.item(i)?.dyn_into::<Element>().ok()) {
                observer.observe(&img);
            }
        });
    };
    Effect::new(move |_| {
        let _ = (tab.get(), stickers.with(Vec::len), results.get());
        request_animation_frame(watch);
    });
    on_cleanup(move || {
        seen.with_value(|seen| {
            if let Some((observer, _)) = seen {
                observer.disconnect();
            }
        })
    });
    // The group in view, marked in the bar; and where a jump to a group left
    // the grid, which keeps that group marked even if it can't reach the top
    // (the last ones), until the grid is scrolled from there.
    let current = RwSignal::new(0usize);
    let jumped = StoredValue::new(None::<i32>);
    let headings = move || {
        let found = results.get_untracked().and_then(|r| r.query_selector_all(".emoji-group-heading").ok());
        found.map_or_else(Vec::new, |list| {
            (0..list.length()).filter_map(|i| list.item(i)?.dyn_into::<web_sys::HtmlElement>().ok()).collect()
        })
    };
    // The last group whose heading is at or above the grid's top. Where the
    // scroll is the jump's own, its group stays marked; a scroll by hand in
    // the same frame comes as the same event, and is marked from where the
    // grid ended up.
    let mark = move || {
        let Some(top) = results.get_untracked().map(|r| r.scroll_top()) else { return };
        if jumped.get_value() == Some(top) {
            return;
        }
        jumped.set_value(None);
        let at = headings().iter().rposition(|h| h.offset_top() <= top + 8).unwrap_or(0);
        if current.get_untracked() != at {
            current.set(at);
        }
    };
    let jump = move |i: usize| {
        let go = move || {
            if let (Some(results), Some(heading)) = (results.get_untracked(), headings().get(i)) {
                results.set_scroll_top(heading.offset_top());
                jumped.set_value(Some(results.scroll_top()));
                current.set(i);
            }
        };
        // An already drawn group can still be at the temporary bottom, with
        // too little trailing content to reach the top. Draw the rest before
        // jumping when the heading is missing or would be clamped there.
        let can_align = match (results.get_untracked(), headings().get(i)) {
            (Some(results), Some(heading)) => heading.offset_top() <= results.scroll_height() - results.client_height(),
            _ => false,
        };
        match drawing.get_value() {
            Some(drawn) if drawn.try_get_untracked().is_some_and(|groups| groups < Group::ALL.len()) && !can_align => {
                let _ = drawn.try_set(Group::ALL.len());
                request_animation_frame(go);
            }
            _ => go(),
        }
    };
    // A search shows no groups; cleared, they come back from the first.
    Effect::new(move |was: Option<bool>| {
        let now = searching.get();
        if was == Some(true) && !now {
            current.set(0);
        }
        now
    });
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
            class:stickers=move || tab.get() == Tab::Stickers
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
            // The groups, each with its first emoji for a face.
            <div
                class="emoji-groups"
                role="group"
                aria-label="Groups"
                hidden=move || tab.get() != Tab::Emoji || searching.get()
            >
                {Group::ALL
                    .iter()
                    .enumerate()
                    .map(|(i, group)| {
                        let face = group.emoji().next().map(|e| href(&e.still())).unwrap_or_default();
                        view! {
                            <button
                                type="button"
                                class="emoji-group"
                                aria-label=group.label()
                                aria-current=move || (current.get() == i).then_some("true")
                                on:click=move |_| jump(i)
                            >
                                <img src=face alt=""/>
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
            // The tiles' listeners, once for all of them.
            <div
                class="emoji-results"
                id="emoji-panel"
                role=stickers_too.then_some("tabpanel")
                aria-labelledby=move || stickers_too.then(|| tab.get().id())
                node_ref=results
                on:scroll=move |_| mark()
                on:pointerleave=move |_| {
                    shown.set(None);
                    let_go();
                }
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
                on:pointerdown=move |ev: ev::PointerEvent| {
                    touch.set_value(ev.pointer_type() == "touch");
                    held.set_value(false);
                    if let Some((_, Item::Sticker(sticker))) = tile_of(ev.target()) {
                        hold.update_value(|run| *run += 1);
                        let run = hold.get_value();
                        set_timeout(
                            move || {
                                if hold.get_value() == run {
                                    held.set_value(true);
                                    preview.set(Some(sticker));
                                }
                            },
                            std::time::Duration::from_millis(HOLD_MS),
                        );
                    }
                }
                on:pointerup=move |_| let_go()
                on:pointercancel=move |_| let_go()
                // A phone's own menu for a held image would cover the preview.
                on:contextmenu=move |ev: ev::MouseEvent| {
                    if matches!(tile_of(ev.target()), Some((_, Item::Sticker(_)))) {
                        ev.prevent_default();
                    }
                }
                // A tap leaves it open; a click or Enter closes it unless
                // Shift is held. A sticker always closes it, unless it was
                // held down to see it large.
                on:click=move |ev: ev::MouseEvent| {
                    if held.get_value() {
                        held.set_value(false);
                        return;
                    }
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
                        // What a search found, read out; out of sight between searches,
                        // when the groups have headings of their own.
                        <h3 class="emoji-section" class:visually-hidden=move || !searching.get() aria-live="polite">
                            {heading}
                        </h3>
                        {move || {
                            if searching.get() {
                                // Drawn anew for each search: lighter than a keyed list.
                                view! { <ul class="emoji-grid">{move || found.get().into_iter().map(tile).collect_view()}</ul> }
                                    .into_any()
                            } else {
                                // The groups in view as they show, then one
                                // a frame.
                                let drawn = RwSignal::new(first_groups());
                                drawing.set_value(Some(drawn));
                                draw_rest(drawn, watch);
                                Group::ALL
                                    .iter()
                                    .enumerate()
                                    .map(|(i, &group)| {
                                        let shown = Memo::new(move |_| i < drawn.get());
                                        move || {
                                            shown.get().then(|| view! {
                                                <h4 class="emoji-section emoji-group-heading">{group.label()}</h4>
                                                <ul class="emoji-grid">{group.emoji().map(group_tile).collect_view()}</ul>
                                            })
                                        }
                                    })
                                    .collect_view()
                                    .into_any()
                            }
                        }}
                    }
                    .into_any(),
                    Tab::Stickers => view! {
                        {recent_stickers.with_value(|recent| (!recent.is_empty()).then(|| {
                            let recent = recent.clone();
                            view! {
                                <div class="emoji-recent" hidden=move || searching.get()>
                                    <h3 class="emoji-section">"Recently used"</h3>
                                    <ul class="emoji-grid sticker-grid">
                                        {recent.into_iter().map(|s| sticker_tile(s, moving)).collect_view()}
                                    </ul>
                                </div>
                            }
                        }))}
                        <h3 class="emoji-section" aria-live="polite">{heading}</h3>
                        <ul class="emoji-grid sticker-grid">
                            {move || stickers.get().into_iter().map(|s| sticker_tile(s, moving)).collect_view()}
                        </ul>
                    }
                    .into_any(),
                }}
            </div>
            {move || preview.get().map(|sticker| view! {
                <div class="sticker-preview" aria-hidden="true">
                    <img
                        src=href(&if motion { sticker.tile() } else { sticker.still() })
                        alt=""
                        width=sticker.width
                        height=STICKER_HEIGHT
                    />
                    <span>{sticker.label()}</span>
                </div>
            })}
            <footer class="emoji-foot">
                {move || match shown.get() {
                    Some(item) => view! {
                        <span class="emoji-preview" aria-hidden="true">
                            <img src=href(&if motion { item.file() } else { item.still() }) alt=""/>
                            {match item {
                                Item::Emoji(emoji) => view! { <code>":"{emoji.code}":"</code> }.into_any(),
                                Item::Sticker(sticker) => view! { <span>{sticker.label()}</span> }.into_any(),
                            }}
                            // Whose it is, as its credit says; Credits has
                            // where it came from.
                            {emoji::maker(item.code()).map(|by| view! { <span class="emoji-maker">"by "{by}</span> })}
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
