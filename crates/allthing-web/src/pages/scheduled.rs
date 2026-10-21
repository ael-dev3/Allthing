// SPDX-License-Identifier: AGPL-3.0-or-later
//! The reader's scheduled casts (`allthing_core::scheduled`): those waiting
//! to go out, soonest first, to change or cancel; those that didn't go out,
//! and why; and those that went out this past week. One request,
//! `/api/v1/scheduled`, with the reader's session. Not in the static copy,
//! which has no Worker to keep them.

use leptos::prelude::*;

use crate::util::set_title;

#[component]
pub fn ScheduledPage() -> impl IntoView {
    set_title("Scheduled casts");
    view! {
        <section class="scheduled">
            <header class="scheduled-head">
                <h1 class="page-title">"Scheduled casts"</h1>
                <p class="scheduled-lede muted">
                    "Casts you've written for later. Your browser signs each one with your key when you schedule it, and Allthing's server posts it at its time."
                </p>
            </header>
            {list()}
        </section>
    }
}

#[cfg(feature = "static-host")]
fn list() -> impl IntoView {
    view! {
        <p class="scheduled-empty">
            "Scheduled casts are kept by Allthing's server, which this copy of the site doesn't have. They're at "
            <a href="https://allthing.social/scheduled">"allthing.social/scheduled"</a>"."
        </p>
    }
}

/// The list: code of its own, fetched when the page opens (`crate::split`).
#[cfg(not(feature = "static-host"))]
fn list() -> impl IntoView {
    crate::split::lazy_page(
        || view! { <p class="muted scheduled-loading">"Loading your scheduled casts…"</p> },
        with_worker::list(),
    )
}

#[cfg(not(feature = "static-host"))]
mod with_worker {
    use std::time::Duration;

    use allthing_core::scheduled::{
        Cancel, Cancelled, Known, SCHEDULED_PATH, SavedDraft, ScheduledCast, ScheduledList, State, cancel_path, until,
    };
    use allthing_core::time::iso_utc;
    use leptos::ev::AnimationEvent;
    use leptos::prelude::*;
    use leptos::task::spawn_local;
    use wasm_bindgen::JsCast;

    use crate::api;
    use crate::components::compose::{Composer, use_composer};
    use crate::components::signin::{Session, use_session};
    use crate::components::text::ExcerptText;
    use crate::components::writer::{Writer, note_scheduled, use_writer};
    use crate::util::{cast_href, document, local_time, now, prefers_reduced_motion};

    /// A cast taken off the list folds away in 180 ms (the CSS
    /// `scheduled-leave`); by this, it's gone, even where the end of its
    /// animation isn't heard.
    const LEAVE_MS: u64 = 400;
    const WENT_OUT: &str = "It had already gone out, so it wasn't cancelled.";
    const GOING_OUT: &str = "It was going out as you cancelled it, so it wasn't cancelled.";

    #[leptos::wasm_split::wasm_split(scheduled, wasm_split_path = ::leptos::wasm_split, fallible)]
    pub fn list() -> Result<AnyView, leptos::wasm_split::SplitLoaderError> {
        Ok(view! { <List/> }.into_any())
    }

    /// Where the list stands.
    #[derive(Clone, PartialEq)]
    enum Phase {
        Loading,
        SignedOut,
        Unreadable(String),
        Read,
    }

    /// What the reader did to the list since it was read, kept while it is
    /// read again: casts taken off at once while the Worker is asked
    /// (folding away, then gone); those the Worker said had gone out, or were
    /// going, as they were cancelled (their hash, and what the row says); why
    /// a cancel didn't go through; and where focus goes once what it goes to
    /// is drawn. By id, in short lists: a reader has a few dozen casts here
    /// at most, and a hash table would cost the page's code more.
    #[derive(Clone, Copy)]
    struct Changes {
        session: Session,
        leaving: RwSignal<Vec<i64>>,
        gone: RwSignal<Vec<i64>>,
        too_late: RwSignal<Vec<(i64, String, &'static str)>>,
        errors: RwSignal<Vec<(i64, String)>>,
        focus: RwSignal<Option<Focus>>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Focus {
        /// A cast's Cancel, or Remove: its cancel didn't go through.
        Cancel(i64),
        /// A cast's View cast: it had gone out.
        View(i64),
        /// The empty list's button: the last cast waiting was taken off.
        Empty,
    }

    impl Changes {
        fn new(session: Session) -> Changes {
            Changes {
                session,
                leaving: RwSignal::new(Vec::new()),
                gone: RwSignal::new(Vec::new()),
                too_late: RwSignal::new(Vec::new()),
                errors: RwSignal::new(Vec::new()),
                focus: RwSignal::new(None),
            }
        }

        /// The casts read that are in `state` now.
        fn shown(self, casts: &[ScheduledCast], state: State) -> Vec<ScheduledCast> {
            self.gone.with(|gone| {
                self.too_late.with(|too_late| {
                    casts
                        .iter()
                        .filter(|cast| !gone.contains(&cast.id))
                        .filter_map(|cast| {
                            let mut cast = cast.clone();
                            if let Some((_, hash, _)) = too_late.iter().find(|(id, ..)| *id == cast.id) {
                                cast.state = State::Posted;
                                cast.hash = hash.clone();
                            }
                            (cast.state == state).then_some(cast)
                        })
                        .collect()
                })
            })
        }

        /// Take `id` off at once: it folds away, or with reduced motion just
        /// goes.
        fn leave(self, id: i64) {
            self.errors.update(|errors| errors.retain(|(at, _)| *at != id));
            if prefers_reduced_motion() {
                self.gone.update(|gone| gone.push(id));
            } else {
                self.leaving.update(|leaving| leaving.push(id));
                set_timeout(move || self.left(id), Duration::from_millis(LEAVE_MS));
            }
        }

        /// Folded away: gone from the page.
        fn left(self, id: i64) {
            if self.leaving.with_untracked(|leaving| leaving.contains(&id)) {
                self.leaving.update(|leaving| leaving.retain(|at| *at != id));
                self.gone.update(|gone| gone.push(id));
            }
        }

        /// Off the list, as the Worker says: so this browser knows, for the
        /// account menu's count.
        fn off(self, id: i64) {
            if let Some(fid) = self.session.user.with_untracked(|u| u.as_ref().map(|u| u.fid)) {
                note_scheduled(|known| known.gone(fid, id));
            }
        }

        /// Back where it was.
        fn back(self, id: i64) {
            self.leaving.update(|leaving| leaving.retain(|at| *at != id));
            self.gone.update(|gone| gone.retain(|at| *at != id));
        }
    }

    #[component]
    pub fn List() -> impl IntoView {
        let session = use_session();
        let writer = use_writer();
        let composer = use_composer();
        let list = LocalResource::new(move || {
            let changed = writer.map(Writer::scheduled_version);
            let signed_in = session.user.with(Option::is_some);
            async move {
                let _ = changed;
                if signed_in { Some(api::get::<ScheduledList>(SCHEDULED_PATH).await) } else { None }
            }
        });
        let phase = Memo::new(move |_| match list.get() {
            None => Phase::Loading,
            Some(None) => Phase::SignedOut,
            Some(Some(Err(failure))) => Phase::Unreadable(failure.message()),
            Some(Some(Ok(_))) => Phase::Read,
        });
        // Read again, the list keeps the casts it shows as they are.
        let casts = Memo::new(move |_| match list.get() {
            Some(Some(Ok(fetched))) => fetched.data.casts.clone(),
            _ => Vec::new(),
        });
        let changes = Changes::new(session);
        // Why a cancel didn't go through was about the list as it was: read
        // again, it says what stands now.
        Effect::new(move |_| {
            casts.track();
            changes.errors.set(Vec::new());
        });
        // What the account menu counts, as the list says.
        Effect::new(move |_| {
            if let Some(Some(Ok(fetched))) = list.get()
                && let Some(fid) = session.user.with_untracked(|u| u.as_ref().map(|u| u.fid))
            {
                note_scheduled(|known| *known = Known::listed(fid, &fetched.data.casts));
            }
        });
        move || match phase.get() {
            Phase::Loading => {
                view! { <p class="muted scheduled-loading">"Loading your scheduled casts…"</p> }.into_any()
            }
            Phase::SignedOut => view! {
                <div class="scheduled-empty">
                    <p>"Sign in to see your scheduled casts."</p>
                    <button type="button" class="button" on:click=move |_| session.sign_in()>"Sign in"</button>
                </div>
            }
            .into_any(),
            Phase::Unreadable(why) => view! { <p class="scheduled-empty" role="alert">{why}</p> }.into_any(),
            Phase::Read => view! { <Lists casts changes composer writer/> }.into_any(),
        }
    }

    #[component]
    fn Lists(
        casts: Memo<Vec<ScheduledCast>>,
        changes: Changes,
        composer: Composer,
        writer: Option<Writer>,
    ) -> impl IntoView {
        let waiting = Memo::new(move |_| casts.with(|casts| changes.shown(casts, State::Waiting)));
        let failed = Memo::new(move |_| casts.with(|casts| changes.shown(casts, State::Failed)));
        let posted = Memo::new(move |_| {
            let mut posted = casts.with(|casts| changes.shown(casts, State::Posted));
            posted.reverse();
            posted
        });
        // Once the last cast waiting has gone, the list says where to
        // schedule one.
        let none_waiting = Memo::new(move |_| waiting.with(Vec::is_empty));
        view! {
            {move || none_waiting.get().then(|| view! { <Empty composer writer changes/> })}
            <Group title="Waiting" casts=waiting changes composer writer/>
            <Group title="Didn't go out" casts=failed changes composer writer/>
            <Group title="Posted this week" casts=posted changes composer writer/>
        }
    }

    /// Nothing waiting: where to schedule a cast, and a button that opens
    /// the composer with its clock on.
    #[component]
    fn Empty(composer: Composer, writer: Option<Writer>, changes: Changes) -> impl IntoView {
        let button = NodeRef::<leptos::html::Button>::new();
        Effect::new(move |_| {
            let Some(button) = button.get() else { return };
            if changes.focus.get() == Some(Focus::Empty) {
                let _ = button.focus();
                changes.focus.set(None);
            }
        });
        move || {
            // No clock where drafts go to the Farcaster app to post.
            let can = writer.is_some_and(Writer::can_write);
            view! {
                <div class="scheduled-empty">
                    <p id="scheduled-none">"Nothing scheduled."</p>
                    {can.then(|| view! {
                        <p id="scheduled-how" class="muted">
                            "Write a cast, and the clock beside its GIF button picks when it goes out."
                        </p>
                        <button
                            type="button"
                            class="button"
                            node_ref=button
                            aria-describedby="scheduled-none scheduled-how"
                            on:click=move |_| composer.start_scheduled()
                        >
                            "Schedule a cast"
                        </button>
                    })}
                </div>
            }
        }
    }

    #[component]
    fn Group(
        title: &'static str,
        casts: Memo<Vec<ScheduledCast>>,
        changes: Changes,
        composer: Composer,
        writer: Option<Writer>,
    ) -> impl IntoView {
        let shown = Memo::new(move |_| casts.with(|casts| !casts.is_empty()));
        // A cast folding away is off the count already.
        let count = Memo::new(move |_| {
            changes
                .leaving
                .with(|leaving| casts.with(|casts| casts.iter().filter(|c| !leaving.contains(&c.id)).count()))
        });
        move || {
            shown.get().then(|| {
                view! {
                    <section class="scheduled-group">
                        <h2>{title}" "<span class="muted">{count}</span></h2>
                        <ol class="scheduled-list">
                            <For
                                each=move || casts.get()
                                key=|cast| format!("{} {} {}", cast.id, cast.state.as_str(), cast.hash)
                                children=move |cast| view! { <Item cast changes composer writer/> }
                            />
                        </ol>
                    </section>
                }
            })
        }
    }

    /// The cast after `item` on the page, or else the one before, of those
    /// not on their way off too.
    fn neighbour(item: &web_sys::Element) -> Option<web_sys::HtmlElement> {
        let all = document().query_selector_all(".scheduled-item:not(.leaving)").ok()?;
        let items: Vec<web_sys::HtmlElement> =
            (0..all.length()).filter_map(|i| all.get(i)).filter_map(|node| node.dyn_into().ok()).collect();
        let item: &wasm_bindgen::JsValue = item.as_ref();
        let at = items.iter().position(|el| AsRef::<wasm_bindgen::JsValue>::as_ref(el) == item)?;
        items.get(at + 1).or_else(|| at.checked_sub(1).and_then(|before| items.get(before))).cloned()
    }

    #[component]
    fn Item(cast: ScheduledCast, changes: Changes, composer: Composer, writer: Option<Writer>) -> impl IntoView {
        let saved = SavedDraft::parse(&cast.draft);
        let (id, state) = (cast.id, cast.state);
        let confirming = RwSignal::new(false);
        // Keep it (or Escape) closed the question: focus goes back to Cancel.
        let kept = StoredValue::new(false);
        // Its hash as the list has it, for an answer that names none.
        let listed = StoredValue::new(cast.hash.clone());
        let item = NodeRef::<leptos::html::Li>::new();
        let cancel = NodeRef::<leptos::html::Button>::new();
        let keep = NodeRef::<leptos::html::Button>::new();
        let view_link = NodeRef::<leptos::html::A>::new();
        let leaving = Memo::new(move |_| changes.leaving.with(|leaving| leaving.contains(&id)));
        let error = Memo::new(move |_| {
            changes.errors.with(|errors| errors.iter().find(|(at, _)| *at == id).map(|(_, why)| why.clone()))
        });
        let error_id = format!("scheduled-error-{id}");
        // Why a cast that went out is here: its cancel came too late.
        let note = changes
            .too_late
            .with_untracked(|too_late| too_late.iter().find(|(at, ..)| *at == id).map(|(.., note)| *note))
            .filter(|_| state == State::Posted);
        // The question opens on its answer that changes nothing.
        Effect::new(move |_| {
            if let Some(keep) = keep.get() {
                let _ = keep.focus();
            }
        });
        Effect::new(move |_| {
            let Some(cancel) = cancel.get() else { return };
            let asked = changes.focus.get() == Some(Focus::Cancel(id));
            if asked || kept.get_value() {
                kept.set_value(false);
                let _ = cancel.focus();
                if asked {
                    changes.focus.set(None);
                }
            }
        });
        Effect::new(move |_| {
            let Some(link) = view_link.get() else { return };
            if changes.focus.get() == Some(Focus::View(id)) {
                let _ = link.focus();
                changes.focus.set(None);
            }
        });
        // Cancel one waiting, or take off one that didn't go out: off the
        // list at once, while the Worker is asked; back where it was, with
        // why, if it can't; a posted one, if it had gone out.
        let take_off = move |_| {
            let Some(li) = item.get_untracked() else { return };
            confirming.set(false);
            // It folds from its own height.
            let _ = web_sys::HtmlElement::style(&li).set_property("--h", &format!("{}px", li.offset_height()));
            // Focus goes on to the cast after it, or the one before; with
            // none left, to the empty list's button, once it's there.
            match neighbour(&li) {
                Some(next) => {
                    let _ = next.focus();
                }
                None => changes.focus.set(Some(Focus::Empty)),
            }
            changes.leave(id);
            let hash = listed.get_value();
            spawn_local(async move {
                let answer = api::post::<_, serde_json::Value>(&cancel_path(), &Cancel { id }).await;
                match answer.map(|answer| Cancelled::read(&answer, &hash)) {
                    Ok(Cancelled::Cancelled) => {
                        changes.off(id);
                        if let Some(writer) = writer {
                            let said = if state == State::Waiting {
                                "Scheduled cast cancelled"
                            } else {
                                "Removed from the list"
                            };
                            writer.say(said.to_owned());
                        }
                    }
                    Ok(Cancelled::Posted { hash }) => {
                        changes.off(id);
                        changes.too_late.update(|too_late| too_late.push((id, hash, WENT_OUT)));
                        changes.back(id);
                        changes.focus.set(Some(Focus::View(id)));
                    }
                    Ok(Cancelled::Sending { hash }) => {
                        changes.off(id);
                        changes.too_late.update(|too_late| too_late.push((id, hash, GOING_OUT)));
                        changes.back(id);
                        changes.focus.set(Some(Focus::View(id)));
                    }
                    Err(failure) => {
                        changes.errors.update(|errors| errors.push((id, failure.message())));
                        changes.back(id);
                        changes.focus.set(Some(Focus::Cancel(id)));
                    }
                }
            });
        };
        let close = move || {
            kept.set_value(true);
            confirming.set(false);
        };
        let edit = {
            let cast = cast.clone();
            move |_| composer.edit(&cast)
        };
        let when = match state {
            State::Waiting => format!("{} · {}", local_time(cast.at), until(cast.at - now())),
            _ => local_time(cast.at),
        };
        let text = saved.text.trim().to_owned();
        let label =
            format!("{} cast, {}", if state == State::Posted { "Posted" } else { "Scheduled" }, local_time(cast.at));
        let described = {
            let error_id = error_id.clone();
            move || error.with(Option::is_some).then(|| error_id.clone())
        };
        view! {
            <li
                class="scheduled-item"
                class:failed=state == State::Failed
                class:leaving=move || leaving.get()
                tabindex="-1"
                aria-label=label
                inert=move || leaving.get()
                node_ref=item
                on:animationend=move |ev: AnimationEvent| {
                    if ev.animation_name() == "scheduled-leave" {
                        changes.left(id);
                    }
                }
            >
                <p class="scheduled-when">
                    <time datetime=iso_utc(cast.at)>{when}</time>
                    {saved.context().map(|context| view! { <span class="scheduled-context muted">{context}</span> })}
                </p>
                {(!text.is_empty()).then(|| view! { <p class="scheduled-text"><ExcerptText text/></p> })}
                {(!saved.media.is_empty()).then(|| view! {
                    <div class="scheduled-media">
                        {saved.media.iter().map(|media| if media.media_type.starts_with("video/") {
                            view! { <video src=format!("{}#t=0.1", media.url) muted=true prop:muted=true playsinline=true preload="metadata" aria-label="Video"></video> }.into_any()
                        } else {
                            view! { <img src=media.url.clone() alt="Image" loading="lazy"/> }.into_any()
                        }).collect_view()}
                    </div>
                })}
                {saved.gif.map(|gif| view! {
                    <img class="scheduled-gif" src=gif.tile alt=gif.title width=gif.width height=gif.height loading="lazy"/>
                })}
                {cast.error.map(|why| view! { <p class="scheduled-error">{why}</p> })}
                {note.map(|note| view! { <p class="scheduled-note" id=format!("scheduled-note-{id}")>{note}</p> })}
                {move || error.get().map(|why| view! { <p class="scheduled-error" id=error_id.clone() role="alert">{why}</p> })}
                <div class="scheduled-actions">
                    {match state {
                        State::Posted => view! {
                            <a
                                class="scheduled-view"
                                href=cast_href(&cast.hash)
                                node_ref=view_link
                                aria-describedby=note.map(|_| format!("scheduled-note-{id}"))
                            >
                                "View cast"
                            </a>
                        }
                        .into_any(),
                        _ => {
                            let (name, question, yes) = if state == State::Waiting {
                                ("Cancel", "Cancel this cast? It won't go out.", "Yes, cancel it")
                            } else {
                                ("Remove", "Remove it from the list?", "Yes, remove it")
                            };
                            let described = described.clone();
                            view! {
                                <button type="button" class="button small" on:click=edit>
                                    {if state == State::Waiting { "Edit" } else { "Edit to try again" }}
                                </button>
                                {move || if confirming.get() {
                                    view! {
                                        <span
                                            class="scheduled-confirm"
                                            role="group"
                                            aria-label=question
                                            on:keydown=move |ev| {
                                                if ev.key() == "Escape" {
                                                    ev.prevent_default();
                                                    close();
                                                }
                                            }
                                        >
                                            <span class="muted small">{question}</span>
                                            <button type="button" class="button small danger" on:click=take_off>{yes}</button>
                                            <button type="button" class="button small ghost" node_ref=keep on:click=move |_| close()>
                                                "Keep it"
                                            </button>
                                        </span>
                                    }
                                    .into_any()
                                } else {
                                    view! {
                                        <button
                                            type="button"
                                            class="button small ghost"
                                            node_ref=cancel
                                            aria-describedby=described.clone()
                                            on:click=move |_| confirming.set(true)
                                        >
                                            {name}
                                        </button>
                                    }
                                    .into_any()
                                }}
                            }
                            .into_any()
                        }
                    }}
                </div>
            </li>
        }
    }
}
