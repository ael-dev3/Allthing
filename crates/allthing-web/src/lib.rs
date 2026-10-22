// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's browser app: Leptos components compiled to WebAssembly,
//! reading one payload per page from the Worker's `/api/v1`.

mod api;
mod boot;
mod components;
#[cfg(feature = "static-host")]
mod host;
mod kept;
#[cfg(not(feature = "static-host"))]
mod keys;
mod leanings;
mod nav;
mod pages;
mod preload;
mod press;
mod split;
mod util;
mod video;

use std::sync::Arc;

use allthing_core::api::Annotations;
use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;
use wasm_bindgen::prelude::*;

use crate::components::layout::{NotFound, SiteFooter, SiteHeader};
use crate::components::media::{Lightbox, Viewer};
use crate::components::who::{WhoDialog, WhoList};
use crate::pages::channel::channel_page;
use crate::pages::channels::channels_page;
use crate::pages::dau::DauPage;
use crate::pages::messages::MessagesPage;
use crate::pages::post::post_page;
use crate::pages::scheduled::ScheduledPage;
use crate::pages::search::search_page;
use crate::pages::user::user_page;

/// This build's version (the workspace version) and short commit.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = match option_env!("ALLTHING_COMMIT") {
    Some(commit) => commit,
    None => "dev",
};

/// A page's annotations, shared by all its casts.
pub type Notes = Arc<Annotations>;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let Some(root) = util::element_by_id::<web_sys::HtmlElement>("app") else { return };
    leptos::mount::mount_to(root, App).forget();
    press::install();
    split::preload_pages();
    boot::mounted();
    let _ = util::document().document_element().map(|e| e.class_list().add_1("ready"));
}

#[component]
fn App() -> impl IntoView {
    provide_context(Viewer::new());
    provide_context(WhoList::new());
    provide_context(leanings::Leaning::new());
    #[cfg(not(feature = "static-host"))]
    provide_context(components::signin::Session::new());
    #[cfg(not(feature = "static-host"))]
    provide_context(components::dc_key::DcKey::new());
    #[cfg(not(feature = "static-host"))]
    provide_context(components::notifications::Notifications::new());
    #[cfg(not(feature = "static-host"))]
    provide_context(components::compose::Composer::new());
    #[cfg(not(feature = "static-host"))]
    provide_context(components::handoff::HandOff::new());
    #[cfg(not(feature = "static-host"))]
    provide_context(components::writer::Writer::new());
    view! {
        <Router base=util::BASE>
            <a class="skip-link" href="#main">"Skip to content"</a>
            <SiteHeader/>
            <main id="main" class="page">
                // The front page, kept while another page shows (`kept`).
                <kept::KeptFront/>
                <Routes fallback=|| view! { <NotFound/> }>
                    <Route path=path!("/") view=|| ()/>
                    // Pages whose code is fetched on first use (`split`).
                    <Route path=path!("/c/:key") view=|| split::page(channel_page())/>
                    <Route path=path!("/c") view=|| split::page(channel_page())/>
                    <Route path=path!("/p/:hash") view=|| split::page(post_page())/>
                    <Route path=path!("/p") view=|| split::page(post_page())/>
                    <Route path=path!("/u/:name") view=|| split::page(user_page())/>
                    <Route path=path!("/u") view=|| split::page(user_page())/>
                    <Route path=path!("/channels") view=|| split::page(channels_page())/>
                    <Route path=path!("/dau") view=DauPage/>
                    <Route path=path!("/scheduled") view=ScheduledPage/>
                    <Route path=path!("/messages") view=MessagesPage/>
                    <Route path=path!("/notifications") view=pages::notifications::page/>
                    <Route path=path!("/search") view=|| split::page(search_page())/>
                </Routes>
            </main>
            <SiteFooter/>
            <Lightbox/>
            <WhoDialog/>
            {sign_in_dialog()}
            <nav::ScrollMemory/>
            <nav::InstantLinks/>
        </Router>
    }
}

/// The composer, the sign-in dialog over it (whose approve form asks for
/// Allthing's key as a first cast goes), and Cast's button on phones, in
/// front of the Worker; the hand-off to the Farcaster app over them.
#[cfg(not(feature = "static-host"))]
fn sign_in_dialog() -> impl IntoView {
    view! {
        <components::compose::ComposerDialog/>
        <components::signin::SignInDialog/>
        <components::writer::WriterDialog/>
        <components::handoff::HandOffDialog/>
        <components::compose::CastFab/>
    }
}

/// The static copy has no Worker to sign in with.
#[cfg(feature = "static-host")]
fn sign_in_dialog() -> impl IntoView {}
