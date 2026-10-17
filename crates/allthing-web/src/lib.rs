pub mod app;
pub mod feed;

#[cfg(all(feature = "hydrate", feature = "ssr"))]
compile_error!("Build SSR and browser hydration separately; do not combine these features.");

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(app::App);
}
