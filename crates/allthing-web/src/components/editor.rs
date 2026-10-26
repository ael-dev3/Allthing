// SPDX-License-Identifier: AGPL-3.0-or-later
//! The composer's text box, where the pack's emoji show as they are as soon
//! as their shortcode is typed: ":pepe_smug:" becomes the smug Pepe, while
//! the draft keeps the shortcode, which is what posts. It is a
//! `contenteditable` element whose text nodes hold what was typed, with
//! line breaks as newlines (`white-space: pre-wrap`), and each emoji an
//! image whose `data-text` is its shortcode as typed
//! (`allthing_core::text::written_emoji`). An offset into the draft counts
//! UTF-16 units, as a text box's selection does, and an emoji counts as its
//! shortcode. What the browser puts in on its own (a line as a `<div>`, a
//! `<br>`) is read as newlines, and the box is drawn again as it draws it.

use allthing_core::compose::Edits;
use allthing_core::text::{between, splice, written_emoji};
use leptos::html::Div;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, Node};

use crate::util::{document, href, prefers_reduced_motion};

/// The box, for the draft `text`. `caret` keeps where the selection was when
/// the box lost focus (a tap elsewhere can move it), and focus brings it
/// back. Typing changes the draft as it goes, and a shortcode just completed
/// shows its emoji; Enter is a newline; a paste is its plain text, unless
/// `files` takes what is pasted as files (a screenshot); a copy or a cut is
/// the draft's text, shortcodes and all; undo and redo go by the draft (the
/// box is drawn again, which a browser's own undo can't follow).
/// `described_by` names what a screen reader hears with it; Cmd or Ctrl
/// and Enter runs `submit`.
#[component]
pub fn ComposeText(
    node_ref: NodeRef<Div>,
    text: RwSignal<String>,
    caret: StoredValue<Option<(usize, usize)>>,
    label: &'static str,
    #[prop(optional)] files: Option<Callback<web_sys::DataTransfer, bool>>,
    #[prop(optional)] described_by: Option<&'static str>,
    #[prop(optional)] submit: Option<Callback<()>>,
) -> impl IntoView {
    let edits = StoredValue::new(Edits::default());
    // Where the caret was after the last change: where undoing it goes.
    let last = StoredValue::new((0, 0));
    let composing = StoredValue::new(false);
    // The draft drawn, as it opens and when it changes from outside: taken
    // up again, emptied once posted, an emoji picked.
    Effect::new(move |_| {
        let value = text.get();
        if let Some(root) = node_ref.get()
            && text_of(&root) != value
        {
            show(&root, &value);
        }
    });
    // A change the box made (typing, `typed`) or that was made in it.
    let changed = move |next: String, at: (usize, usize), typed: bool| {
        let before = text.get_untracked();
        if before != next {
            edits.update_value(|e| e.changed((before, last.get_value()), typed, js_sys::Date::now()));
            text.set(next);
        }
        last.set_value(at);
    };
    // What the browser did: read, and drawn again if it isn't as the box
    // draws it.
    let read = move || {
        let Some(root) = node_ref.get_untracked() else { return };
        let draft = text_of(&root);
        let at = caret_in(&root);
        // Emptied, it holds nothing, so its placeholder shows.
        if draft.is_empty() {
            root.set_text_content(None);
        } else if !drawn(&root, &draft) {
            show(&root, &draft);
            if let Some((from, to)) = at {
                place(&root, from, to);
            }
        }
        changed(draft, at.unwrap_or_default(), true);
    };
    // Put `put` in place of the selection, the caret after it.
    let put = move |put: &str| {
        let Some(root) = node_ref.get_untracked() else { return };
        let draft = text_of(&root);
        let end = draft.encode_utf16().count();
        let (from, to) = caret_in(&root).unwrap_or((end, end));
        let (next, at) = splice(&draft, from, to, put);
        show(&root, &next);
        place(&root, at, at);
        changed(next, (at, at), false);
    };
    let go_to = move |(draft, (from, to)): (String, (usize, usize))| {
        let Some(root) = node_ref.get_untracked() else { return };
        show(&root, &draft);
        place(&root, from, to);
        last.set_value((from, to));
        text.set(draft);
    };
    let now = move || {
        let at = node_ref.get_untracked().and_then(|root| caret_in(&root)).unwrap_or(last.get_value());
        (text.get_untracked(), at)
    };
    let undo = move || {
        if let Some(back) = edits.try_update_value(|e| e.undo(now())).flatten() {
            go_to(back);
        }
    };
    let redo = move || {
        if let Some(next) = edits.try_update_value(|e| e.redo(now())).flatten() {
            go_to(next);
        }
    };
    let copy = move |ev: web_sys::ClipboardEvent, cut: bool| {
        let Some(root) = node_ref.get_untracked() else { return };
        let Some((from, to)) = caret_in(&root).filter(|(from, to)| from != to) else { return };
        let Some(data) = ev.clipboard_data() else { return };
        ev.prevent_default();
        let _ = data.set_data("text/plain", &between(&text_of(&root), from, to));
        if cut {
            put("");
        }
    };
    view! {
        <div
            class="compose-text"
            node_ref=node_ref
            contenteditable="true"
            role="textbox"
            aria-multiline="true"
            aria-label=label
            aria-describedby=described_by
            data-placeholder=label
            spellcheck="true"
            on:beforeinput=move |ev: web_sys::InputEvent| match ev.input_type().as_str() {
                "insertParagraph" | "insertLineBreak" => {
                    ev.prevent_default();
                    put("\n");
                }
                "historyUndo" => {
                    ev.prevent_default();
                    undo();
                }
                "historyRedo" => {
                    ev.prevent_default();
                    redo();
                }
                // No bold or italics: a cast is plain text.
                kind if kind.starts_with("format") => ev.prevent_default(),
                _ => {}
            }
            // The keys themselves too: a browser has nothing of its own to
            // undo once the box is drawn again, and then sends no undo.
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if !(ev.ctrl_key() || ev.meta_key()) || ev.alt_key() {
                    return;
                }
                match (ev.key().to_ascii_lowercase().as_str(), ev.shift_key()) {
                    // Posts, as in the Farcaster app: no newline. Not while
                    // an input method is composing a word.
                    ("enter", _) if !ev.is_composing() => {
                        ev.prevent_default();
                        if let Some(submit) = submit {
                            submit.run(());
                        }
                    }
                    ("z", false) => {
                        ev.prevent_default();
                        undo();
                    }
                    ("z", true) | ("y", false) => {
                        ev.prevent_default();
                        redo();
                    }
                    _ => {}
                }
            }
            on:input=move |_| {
                if !composing.get_value() {
                    read();
                }
            }
            on:compositionstart=move |_| composing.set_value(true)
            on:compositionend=move |_| {
                composing.set_value(false);
                read();
            }
            on:paste=move |ev: web_sys::ClipboardEvent| {
                ev.prevent_default();
                if let (Some(files), Some(data)) = (files, ev.clipboard_data())
                    && files.run(data)
                {
                    return;
                }
                let pasted = ev.clipboard_data().and_then(|data| data.get_data("text/plain").ok()).unwrap_or_default();
                put(&pasted.replace("\r\n", "\n").replace('\r', "\n"));
            }
            on:copy=move |ev| copy(ev, false)
            on:cut=move |ev| copy(ev, true)
            // Dropped text would bring its markup along. Dropped files go on
            // to the composer around the box, which attaches them.
            on:drop=move |ev: web_sys::DragEvent| ev.prevent_default()
            // Replacing a draft or closing its dialog can move focus while
            // the old editor's DOM remains, after its reactive owner retired.
            // Those final focus/blur events must not read disposed handles.
            on:focus=move |_| {
                if let Some(root) = node_ref.try_get_untracked().flatten()
                    && caret_in(&root).is_none()
                    && let Some(end) = text.try_with_untracked(|t| t.encode_utf16().count())
                    && let Some(caret) = caret.try_get_value()
                {
                    let (from, to) = caret.unwrap_or((end, end));
                    place(&root, from, to);
                }
            }
            on:blur=move |_| {
                if let Some(root) = node_ref.try_get_untracked().flatten() {
                    let _ = caret.try_update_value(|at| *at = caret_in(&root));
                }
            }
        ></div>
    }
}

/// The draft the box holds.
pub fn text_of(root: &Node) -> String {
    let mut read = Read::default();
    read.children(root);
    read.out
}

/// Where the selection is in the draft, as (from, to), if it is in `root`.
pub fn caret_in(root: &Node) -> Option<(usize, usize)> {
    let selection = web_sys::window()?.get_selection().ok()??;
    let (anchor, focus) = (selection.anchor_node()?, selection.focus_node()?);
    if !root.contains(Some(&anchor)) || !root.contains(Some(&focus)) {
        return None;
    }
    let at = |node: &Node, offset: u32| {
        let mut read = Read { stop: Some((node.clone(), offset)), ..Read::default() };
        read.children(root);
        read.out.encode_utf16().count()
    };
    let (anchor, focus) = (at(&anchor, selection.anchor_offset()), at(&focus, selection.focus_offset()));
    Some((anchor.min(focus), anchor.max(focus)))
}

/// Draw `text` in the box: its runs of text, and its emoji as images. A
/// newline at the end has a line break after it, for the empty last line
/// to show and hold the caret.
pub fn show(root: &Node, text: &str) {
    root.set_text_content(None);
    let document = document();
    let still = prefers_reduced_motion();
    for (emoji, written) in written_emoji(text) {
        let node: Node = match emoji {
            None => document.create_text_node(written).into(),
            Some(emoji) => {
                let Ok(image) = document.create_element("img") else { continue };
                let file = if still { emoji.still() } else { emoji.file() };
                for (name, value) in [
                    ("class", "emoji"),
                    ("src", &href(&file)),
                    ("alt", written),
                    ("data-text", written),
                    ("aria-description", &emoji.label()),
                    ("draggable", "false"),
                ] {
                    let _ = image.set_attribute(name, value);
                }
                image.into()
            }
        };
        let _ = root.append_child(&node);
    }
    if text.ends_with('\n')
        && let Ok(end) = document.create_element("br")
    {
        let _ = root.append_child(&end);
    }
}

/// Whether the box is drawn as [`show`] draws `text`: if not, it is drawn
/// again (an emoji whose shortcode was just completed, a line the browser
/// put in a block). A line break the browser left at the end, which shows
/// nothing, may stay.
pub fn drawn(root: &Node, text: &str) -> bool {
    let parts = written_emoji(text);
    let children = root.child_nodes();
    let trailing = root.last_child().is_some_and(|node| node.node_name() == "BR");
    if text.ends_with('\n') && !trailing {
        return false;
    }
    if children.length() as usize != parts.len() + usize::from(trailing) {
        return false;
    }
    parts.iter().enumerate().all(|(i, (emoji, written))| {
        let Some(node) = children.item(i as u32) else { return false };
        match emoji {
            None => node.node_type() == Node::TEXT_NODE && node.node_value().as_deref() == Some(*written),
            Some(_) => node
                .dyn_ref::<Element>()
                .is_some_and(|el| el.tag_name() == "IMG" && el.get_attribute("data-text").as_deref() == Some(*written)),
        }
    })
}

/// Select from `from` to `to` in the draft, in a box drawn by [`show`].
pub fn place(root: &Node, from: usize, to: usize) {
    let (Some(start), Some(end)) = (position(root, from), position(root, to)) else { return };
    let Some(selection) = web_sys::window().and_then(|w| w.get_selection().ok().flatten()) else { return };
    let Ok(range) = document().create_range() else { return };
    if range.set_start(&start.0, start.1).is_ok() && range.set_end(&end.0, end.1).is_ok() {
        let _ = selection.remove_all_ranges();
        let _ = selection.add_range(&range);
    }
}

/// Where draft offset `at` is in a box drawn by [`show`]: in a text node
/// where one holds it, else between nodes. An emoji isn't split: an offset
/// inside its shortcode is after it.
fn position(root: &Node, at: usize) -> Option<(Node, u32)> {
    let children = root.child_nodes();
    let mut count = 0;
    for i in 0..children.length() {
        let child = children.item(i)?;
        if child.node_type() == Node::TEXT_NODE {
            let len = child.node_value().unwrap_or_default().encode_utf16().count();
            if at <= count + len {
                return Some((child, (at - count) as u32));
            }
            count += len;
            continue;
        }
        let len = child
            .dyn_ref::<Element>()
            .and_then(|el| el.get_attribute("data-text"))
            .map_or(0, |t| t.encode_utf16().count());
        if len == 0 {
            continue;
        }
        if at <= count {
            return Some((root.clone(), i));
        }
        count += len;
        if at < count {
            return Some((root.clone(), i + 1));
        }
    }
    // The end: before the last line break, which holds an empty last line.
    let end = children.length() - u32::from(root.last_child().is_some_and(|node| node.node_name() == "BR"));
    Some((root.clone(), end))
}

/// Reads a box as the draft, up to a point (`stop`) or all of it.
#[derive(Default)]
struct Read {
    out: String,
    /// Before child `offset` of an element, or `offset` UTF-16 units into
    /// a text node.
    stop: Option<(Node, u32)>,
    stopped: bool,
}

impl Read {
    fn push(&mut self, text: &str) {
        // A browser may put a no-break space where a space was typed.
        self.out.extend(text.chars().map(|c| if c == '\u{a0}' { ' ' } else { c }));
    }

    fn stops_at(&self, node: &Node) -> Option<u32> {
        self.stop.as_ref().filter(|(stop, _)| stop.is_same_node(Some(node))).map(|(_, offset)| *offset)
    }

    fn children(&mut self, node: &Node) {
        let children = node.child_nodes();
        let count = children.length();
        for i in 0..count {
            if self.stops_at(node) == Some(i) {
                self.stopped = true;
                return;
            }
            let Some(child) = children.item(i) else { continue };
            self.child(&child, i + 1 == count);
            if self.stopped {
                return;
            }
        }
        if self.stops_at(node) == Some(count) {
            self.stopped = true;
        }
    }

    fn child(&mut self, child: &Node, last: bool) {
        let stop = self.stops_at(child);
        match child.node_type() {
            Node::TEXT_NODE => {
                let text = child.node_value().unwrap_or_default();
                match stop {
                    Some(offset) => {
                        let units: Vec<u16> = text.encode_utf16().take(offset as usize).collect();
                        self.push(&String::from_utf16_lossy(&units));
                        self.stopped = true;
                    }
                    None => self.push(&text),
                }
            }
            Node::ELEMENT_NODE => {
                let element: &Element = child.unchecked_ref();
                match element.tag_name().as_str() {
                    "IMG" => {
                        if stop != Some(0) {
                            let text = element.get_attribute("data-text").or_else(|| element.get_attribute("alt"));
                            self.push(&text.unwrap_or_default());
                        }
                        self.stopped |= stop.is_some();
                    }
                    // A line break, but the one that holds an empty last
                    // line (of the box, or of a line the browser put in a
                    // block) open.
                    "BR" => {
                        if stop.is_some() {
                            self.stopped = true;
                        } else if !last {
                            self.out.push('\n');
                        }
                    }
                    "DIV" | "P" => {
                        if !self.out.is_empty() && !self.out.ends_with('\n') {
                            self.out.push('\n');
                        }
                        self.children(child);
                    }
                    _ => self.children(child),
                }
            }
            _ => {}
        }
    }
}
