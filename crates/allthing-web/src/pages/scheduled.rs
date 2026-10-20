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

#[cfg(not(feature = "static-host"))]
fn list() -> impl IntoView {
    view! { <with_worker::List/> }
}

#[cfg(not(feature = "static-host"))]
mod with_worker {
    use allthing_core::scheduled::{
        Cancel, SCHEDULED_PATH, SavedDraft, ScheduledCast, ScheduledList, State, cancel_path, until,
    };
    use allthing_core::time::iso_utc;
    use leptos::prelude::*;
    use leptos::task::spawn_local;

    use crate::api;
    use crate::components::compose::{Composer, use_composer};
    use crate::components::signin::use_session;
    use crate::components::writer::{Writer, use_writer};
    use crate::util::{cast_href, local_time, now};

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
        move || match list.get() {
            None => view! { <p class="muted scheduled-loading">"Loading your scheduled casts…"</p> }.into_any(),
            Some(None) => view! {
                <div class="scheduled-empty">
                    <p>"Sign in to see your scheduled casts."</p>
                    <button type="button" class="button" on:click=move |_| session.sign_in()>"Sign in"</button>
                </div>
            }
            .into_any(),
            Some(Some(Err(failure))) => {
                view! { <p class="scheduled-empty" role="alert">{failure.message()}</p> }.into_any()
            }
            Some(Some(Ok(fetched))) => {
                let casts = fetched.data.casts.clone();
                let of = |state: State| casts.iter().filter(|c| c.state == state).cloned().collect::<Vec<_>>();
                let (waiting, failed) = (of(State::Waiting), of(State::Failed));
                let mut posted = of(State::Posted);
                posted.reverse();
                if casts.is_empty() {
                    // No clock where drafts go to the Farcaster app to post.
                    let text = if writer.is_some_and(Writer::can_write) {
                        "Nothing scheduled. Write a cast, and the clock beside its GIF button picks when it goes out."
                    } else {
                        "Nothing scheduled."
                    };
                    return view! { <p class="scheduled-empty">{text}</p> }.into_any();
                }
                view! {
                    <Group title="Waiting" casts=waiting composer writer/>
                    <Group title="Didn't go out" casts=failed composer writer/>
                    <Group title="Posted this week" casts=posted composer writer/>
                }
                .into_any()
            }
        }
    }

    #[component]
    fn Group(
        title: &'static str,
        casts: Vec<ScheduledCast>,
        composer: Composer,
        writer: Option<Writer>,
    ) -> impl IntoView {
        (!casts.is_empty()).then(|| {
            view! {
                <section class="scheduled-group">
                    <h2>{title}" "<span class="muted">{casts.len()}</span></h2>
                    <ol class="scheduled-list">
                        {casts.into_iter().map(|cast| view! { <Item cast composer writer/> }).collect_view()}
                    </ol>
                </section>
            }
        })
    }

    #[component]
    fn Item(cast: ScheduledCast, composer: Composer, writer: Option<Writer>) -> impl IntoView {
        let saved = SavedDraft::parse(&cast.draft);
        let (id, state) = (cast.id, cast.state);
        let confirming = RwSignal::new(false);
        let busy = RwSignal::new(false);
        let error = RwSignal::new(None::<String>);
        // Cancel one waiting, or take off one that didn't go out.
        let take_off = move |_| {
            busy.set(true);
            spawn_local(async move {
                match api::post::<_, serde_json::Value>(&cancel_path(), &Cancel { id }).await {
                    Ok(_) => {
                        if let Some(writer) = writer {
                            writer.rescheduled();
                        }
                    }
                    Err(failure) => {
                        error.set(Some(failure.message()));
                        busy.set(false);
                    }
                }
            });
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
        view! {
            <li class="scheduled-item" class:failed=state == State::Failed aria-label=label>
                <p class="scheduled-when">
                    <time datetime=iso_utc(cast.at)>{when}</time>
                    {saved.context().map(|context| view! { <span class="scheduled-context muted">{context}</span> })}
                </p>
                {(!text.is_empty()).then(|| view! { <p class="scheduled-text">{text}</p> })}
                {saved.gif.map(|gif| view! {
                    <img class="scheduled-gif" src=gif.tile alt=gif.title width=gif.width height=gif.height loading="lazy"/>
                })}
                {cast.error.map(|why| view! { <p class="scheduled-error">{why}</p> })}
                {move || error.get().map(|why| view! { <p class="scheduled-error" role="alert">{why}</p> })}
                <div class="scheduled-actions">
                    {match state {
                        State::Posted => view! { <a class="scheduled-view" href=cast_href(&cast.hash)>"View cast"</a> }.into_any(),
                        _ => {
                            let edit = edit.clone();
                            let (cancel, confirm) = if state == State::Waiting {
                                ("Cancel", "Cancel this cast? It won't go out.")
                            } else {
                                ("Remove", "Remove it from the list?")
                            };
                            view! {
                                <button type="button" class="button small" on:click=edit>
                                    {if state == State::Waiting { "Edit" } else { "Edit to try again" }}
                                </button>
                                {move || if confirming.get() {
                                    view! {
                                        <span class="scheduled-confirm" role="group" aria-label=confirm>
                                            <span class="muted small">{confirm}</span>
                                            <button type="button" class="button small danger" disabled=move || busy.get() on:click=take_off>
                                                {if state == State::Waiting { "Yes, cancel it" } else { "Yes, remove it" }}
                                            </button>
                                            <button type="button" class="button small ghost" on:click=move |_| confirming.set(false)>"Keep it"</button>
                                        </span>
                                    }
                                    .into_any()
                                } else {
                                    view! { <button type="button" class="button small ghost" on:click=move |_| confirming.set(true)>{cancel}</button> }.into_any()
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
