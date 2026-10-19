// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Workers runtime as the data builders' host: `fetch` with an abort
//! timeout, the clock, `waitUntil` for background work, and the index.

use std::rc::Rc;
use std::time::Duration;

use allthing_data::indexer::Index;
use allthing_data::{FetchError, Host, HttpMethod, HttpRequest, HttpResponse};
use futures::StreamExt;
use futures::future::{Either, FutureExt, LocalBoxFuture, select};
use worker::{AbortController, Context, Date, Delay, Fetch, Headers, Method, Request, RequestInit, ScheduleContext};

use crate::index::D1Index;

/// What keeps background work alive: a request's or a cron run's context.
pub enum Lifetime {
    Request(Context),
    Schedule(ScheduleContext),
}

pub struct WorkerHost {
    lifetime: Lifetime,
    index: Option<Rc<D1Index>>,
    solana_rpc: Option<String>,
}

impl WorkerHost {
    pub fn new(lifetime: Lifetime, index: Option<Rc<D1Index>>, solana_rpc: Option<String>) -> WorkerHost {
        WorkerHost { lifetime, index, solana_rpc }
    }
}

async fn exchange(request: HttpRequest) -> Result<HttpResponse, FetchError> {
    let network = |e: worker::Error| FetchError::Network(e.to_string());
    let headers = Headers::new();
    let _ = headers.set("accept", request.accept);
    if let Some(max) = request.max_bytes {
        let _ = headers.set("range", &format!("bytes=0-{}", max.saturating_sub(1)));
    }
    let _ = headers.set("user-agent", &format!("allthing/{} (+https://allthing.social)", crate::VERSION));
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
        let body = if !(200..300).contains(&status) {
            None
        } else if let Some(max) = request.max_bytes {
            // Stop reading at the cap: servers may ignore the Range header.
            let mut stream = response.stream().map_err(network)?;
            let mut body = Vec::new();
            while body.len() < max as usize {
                match stream.next().await {
                    Some(chunk) => body.extend(chunk.map_err(network)?),
                    None => break,
                }
            }
            body.truncate(max as usize);
            Some(body)
        } else {
            Some(response.bytes().await.map_err(network)?)
        };
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

    fn solana_rpc(&self) -> Option<String> {
        self.solana_rpc.clone()
    }

    fn spawn(&self, future: LocalBoxFuture<'static, ()>) {
        match &self.lifetime {
            Lifetime::Request(ctx) => ctx.wait_until(future),
            Lifetime::Schedule(ctx) => ctx.wait_until(future),
        }
    }

    fn index(&self) -> Option<Rc<dyn Index>> {
        self.index.clone().map(|index| index as Rc<dyn Index>)
    }
}
