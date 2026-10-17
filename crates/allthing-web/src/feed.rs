use allthing_core::Post;
use leptos::prelude::*;

/// Read-only seed transport. Replace its provider when live adapters are ready.
/// This runs on the native server; Leptos serializes its result for hydration.
#[server(GetSeedFeed, "/api")]
pub async fn get_seed_feed() -> Result<Vec<Post>, ServerFnError> {
    Ok(allthing_core::demo::posts())
}
