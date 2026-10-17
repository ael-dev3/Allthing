use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let configuration = leptos::prelude::get_configuration(None)?;
    let address = configuration.leptos_options.site_addr;
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("Allthing preview listening on http://{address}");
    axum::serve(listener, allthing_server::app(configuration.leptos_options))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("Could not listen for shutdown: {error}");
    }
}
