// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Workers runtime as the data builders' host: `fetch` with an abort
//! timeout, the clock, `waitUntil` for background work, and the index.

use std::rc::Rc;
use std::time::Duration;

use allthing_core::message::NODE;
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
    /// A stand-in for Hypersnap's node on this machine, for the end-to-end
    /// tests: what would go to the node goes there (`crate::node_stand_in`).
    node: Option<String>,
}

impl WorkerHost {
    pub fn new(
        lifetime: Lifetime,
        index: Option<Rc<D1Index>>,
        solana_rpc: Option<String>,
        node: Option<String>,
    ) -> WorkerHost {
        WorkerHost { lifetime, index, solana_rpc, node }
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
    for (name, value) in &request.headers {
        let _ = headers.set(name, value);
    }
    let mut init = RequestInit::new();
    let method = match request.method {
        HttpMethod::Get => Method::Get,
        HttpMethod::Post => Method::Post,
        HttpMethod::Put => Method::Put,
    };
    init.with_method(method).with_headers(headers);
    if let Some(body) = request.body {
        let _ = init.headers.set("content-type", "application/json");
        init.with_body(Some(body.into()));
    } else if let Some(octets) = &request.octets {
        let _ = init.headers.set("content-type", "application/octet-stream");
        init.with_body(Some(worker::js_sys::Uint8Array::from(octets.as_slice()).into()));
    }
    let outgoing = Request::new_with_init(&request.url, &init).map_err(network)?;

    let controller = AbortController::default();
    let signal = controller.signal();
    let send = async {
        let mut response = Fetch::Request(outgoing).send_with_signal(&signal).await.map_err(network)?;
        let status = response.status_code();
        let retry_after = response.headers().get("retry-after").ok().flatten();
        let body = if !(200..300).contains(&status) && !request.any_body {
            None
        } else if let Some(max) = request.max_bytes {
            // Stop reading at the cap (servers may ignore the Range header),
            // or once the request has what it needs.
            let mut stream = response.stream().map_err(network)?;
            let mut body = Vec::new();
            while body.len() < max as usize {
                let seen = body.len();
                match stream.next().await {
                    Some(chunk) => body.extend(chunk.map_err(network)?),
                    None => break,
                }
                if request.stop_at.is_some_and(|enough| enough(&body, seen)) {
                    break;
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

    fn fetch(&self, mut request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
        if let Some(node) = &self.node
            && let Some(rest) = request.url.strip_prefix(NODE)
        {
            request.url = format!("{node}{rest}");
        }
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
