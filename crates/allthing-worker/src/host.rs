// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Workers runtime as the data builders' host: `fetch` with an abort
//! timeout, the clock, and `waitUntil` for background work.

use std::time::Duration;

use allthing_data::{FetchError, Host, HttpMethod, HttpRequest, HttpResponse};
use futures::future::{Either, FutureExt, LocalBoxFuture, select};
use worker::{AbortController, Context, Date, Delay, Fetch, Headers, Method, Request, RequestInit};

pub struct WorkerHost {
    ctx: Context,
}

impl WorkerHost {
    pub fn new(ctx: Context) -> WorkerHost {
        WorkerHost { ctx }
    }
}

async fn exchange(request: HttpRequest) -> Result<HttpResponse, FetchError> {
    let network = |e: worker::Error| FetchError::Network(e.to_string());
    let headers = Headers::new();
    let _ = headers.set("accept", "application/json");
    let _ = headers.set("user-agent", "allthing/0.2 (+https://allthing.social)");
    let mut init = RequestInit::new();
    let method = match request.method {
        HttpMethod::Get => Method::Get,
        HttpMethod::Post => Method::Post,
    };
    init.with_method(method).with_headers(headers);
    if let Some(body) = request.body {
        let _ = init.headers.set("content-type", "application/json");
        init.with_body(Some(body.into()));
    }
    let outgoing = Request::new_with_init(&request.url, &init).map_err(network)?;

    let controller = AbortController::default();
    let signal = controller.signal();
    let send = async {
        let mut response = Fetch::Request(outgoing).send_with_signal(&signal).await.map_err(network)?;
        let status = response.status_code();
        let retry_after = response.headers().get("retry-after").ok().flatten();
        let body = if (200..300).contains(&status) { Some(response.bytes().await.map_err(network)?) } else { None };
        Ok(HttpResponse { status, retry_after, body })
    };
    let timeout = Delay::from(Duration::from_millis(u64::from(request.timeout_ms)));
    futures::pin_mut!(send);
    match select(send, timeout).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => {
            controller.abort();
            Err(FetchError::Timeout)
        }
    }
}

impl Host for WorkerHost {
    fn now(&self) -> f64 {
        Date::now().as_millis() as f64
    }

    fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
        exchange(request).boxed_local()
    }

    fn spawn(&self, future: LocalBoxFuture<'static, ()>) {
        self.ctx.wait_until(future);
    }
}
