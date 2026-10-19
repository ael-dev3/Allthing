// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's browser app: Leptos components compiled to WebAssembly,
//! reading one payload per page from the Worker's `/api/v1`.

mod api;
mod components;
#[cfg(feature = "static-host")]
mod host;
mod nav;
mod pages;
mod preload;
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
use crate::pages::channel::ChannelPage;
use crate::pages::channels::ChannelsPage;
use crate::pages::home::Home;
use crate::pages::post::PostPage;
use crate::pages::search::SearchPage;
use crate::pages::user::UserPage;

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
    if let Some(boot) = util::document().get_element_by_id("boot") {
        boot.remove();
    }
    let _ = util::document().document_element().map(|e| e.class_list().add_1("ready"));
}

#[component]
fn App() -> impl IntoView {
    provide_context(Viewer::new());
    provide_context(WhoList::new());
    #[cfg(not(feature = "static-host"))]
    provide_context(components::signin::Session::new());
    view! {
        <Router base=util::BASE>
            <a class="skip-link" href="#main">"Skip to content"</a>
            <SiteHeader/>
            <main id="main" class="page">
                <Routes fallback=|| view! { <NotFound/> }>
                    <Route path=path!("/") view=Home/>
                    <Route path=path!("/c/:key") view=ChannelPage/>
                    <Route path=path!("/c") view=ChannelPage/>
                    <Route path=path!("/p/:hash") view=PostPage/>
                    <Route path=path!("/p") view=PostPage/>
                    <Route path=path!("/u/:name") view=UserPage/>
                    <Route path=path!("/u") view=UserPage/>
                    <Route path=path!("/channels") view=ChannelsPage/>
                    <Route path=path!("/search") view=SearchPage/>
                </Routes>
            </main>
            <SiteFooter/>
            <Lightbox/>
            <WhoDialog/>
            {sign_in_dialog()}
            <nav::ScrollMemory/>
        </Router>
    }
}

/// The sign-in dialog, in front of the Worker.
#[cfg(not(feature = "static-host"))]
fn sign_in_dialog() -> impl IntoView {
    view! { <components::signin::SignInDialog/> }
}

/// The static copy has no Worker to sign in with.
#[cfg(feature = "static-host")]
fn sign_in_dialog() -> impl IntoView {}
