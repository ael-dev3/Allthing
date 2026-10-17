use allthing_web::app::{App, shell};
use axum::{Json, Router, routing::get};
use leptos::prelude::LeptosOptions;
use leptos_axum::{LeptosRoutes, generate_route_list};
use serde::Serialize;

pub fn app(options: LeptosOptions) -> Router {
    let routes = generate_route_list(App);
    Router::new()
        .route("/healthz", get(health))
        .leptos_routes(&options, routes, {
            let options = options.clone();
            move || shell(options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(options)
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    mode: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        mode: "demo",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_is_explicit_about_demo_mode() {
        let response = app(LeptosOptions::builder().output_name("allthing").build())
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], br#"{"status":"ok","mode":"demo"}"#);
    }
}
