// SPDX-License-Identifier: AGPL-3.0-or-later
//! The HTML shell belongs to the same deployment as its asset manifest.
//! Real assets stay on the free asset-first path; missing files never turn
//! into immutable HTML under a JavaScript or WebAssembly URL.

use worker::{Env, Headers, Method, Request, Response, ResponseBuilder, Result, Url};

const POLICY: &str = include_str!("../../../web/_headers");

/// Header rules on Worker-generated responses are not supplied by Static
/// Assets. Reuse the global policy, including the checked inline CSP hashes.
fn security_policy() -> Vec<(&'static str, &'static str)> {
    let mut global = false;
    let mut headers = Vec::new();
    for line in POLICY.lines() {
        let trimmed = line.trim();
        if trimmed == "/*" {
            global = true;
        } else if global && trimmed.starts_with('/') {
            break;
        } else if global
            && line.starts_with(char::is_whitespace)
            && let Some((name, value)) = trimmed.split_once(':')
        {
            headers.push((name.trim(), value.trim()));
        }
    }
    headers
}

fn secure(headers: &Headers) -> Result<()> {
    for (name, value) in security_policy() {
        headers.set(name, value)?;
    }
    Ok(())
}

fn no_store(headers: &Headers) -> Result<()> {
    for name in ["cache-control", "cdn-cache-control", "cloudflare-cdn-cache-control"] {
        headers.set(name, "no-store")?;
    }
    headers.set("x-allthing-commit", super::COMMIT)
}

pub fn not_found() -> Result<Response> {
    let response = super::error(404, "Not found", None)?;
    secure(response.headers())?;
    no_store(response.headers())?;
    Ok(response)
}

fn document(path: &str, accept: &str, mode: &str) -> bool {
    // A browser navigating directly to a missing asset must not make that
    // file an HTML response either. These prefixes are never app routes.
    if ["/assets", "/api", "/wasm", "/m"]
        .iter()
        .any(|prefix| path.strip_prefix(prefix).is_some_and(|rest| rest.is_empty() || rest.starts_with('/')))
    {
        return false;
    }
    if matches!(path, "/" | "/index.html") {
        return true;
    }
    let first = path.trim_start_matches('/').split('/').next().unwrap_or_default();
    if matches!(
        first,
        "c" | "p" | "u" | "channels" | "dau" | "scheduled" | "messages" | "notifications" | "library" | "search"
    ) {
        return true;
    }
    // Preserve the app's NotFound page for unknown document navigations.
    // Non-navigation requests for missing scripts/images remain real 404s.
    mode == "navigate" || accept.split(',').any(|part| part.trim().split(';').next() == Some("text/html"))
}

pub async fn serve(req: &Request, env: &Env, url: &Url) -> Result<Response> {
    if !matches!(req.method(), Method::Get | Method::Head)
        || !document(
            url.path(),
            &req.headers().get("accept")?.unwrap_or_default(),
            &req.headers().get("sec-fetch-mode")?.unwrap_or_default(),
        )
    {
        return not_found();
    }
    // Fetch only this version's shell. No visitor cookies, Authorization,
    // Range or conditional headers are forwarded to the static binding.
    let shell = env.assets("ASSETS")?.fetch(format!("{}/index.html", url.origin().ascii_serialization()), None).await?;
    let html = shell.headers().get("content-type")?.is_some_and(|value| value.starts_with("text/html"));
    if shell.status_code() != 200 || !html {
        let response = super::error(503, "The app is temporarily unavailable", Some(1))?;
        secure(response.headers())?;
        no_store(response.headers())?;
        return Ok(response);
    }
    let headers: Headers = shell.headers().entries().collect();
    secure(&headers)?;
    no_store(&headers)?;
    headers.delete("etag")?;
    headers.delete("last-modified")?;
    let response = ResponseBuilder::new().with_headers(headers).with_encode_body(*shell.encode_body());
    Ok(if req.method() == Method::Head { response.empty() } else { response.body(shell.body().clone()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_app_routes_and_unknown_documents_keep_the_shell_but_asset_urls_never_do() {
        for path in [
            "/",
            "/index.html",
            "/p/0x123",
            "/u/reader.eth/followers",
            "/c/dev",
            "/notifications/",
            "/library",
            "/search",
        ] {
            assert!(document(path, "*/*", "cors"), "{path}");
        }
        assert!(document("/unknown", "text/html,application/xhtml+xml", ""));
        assert!(document("/unknown", "*/*", "navigate"));
        for path in [
            "/assets/old.js",
            "/assets/old/split_post.wasm",
            "/assets/paint-missing.js",
            "/wasm/old.wasm",
            "/api/unknown",
            "/m/missing.jpg",
        ] {
            assert!(!document(path, "text/html", "navigate"), "{path}");
        }
        assert!(!document("/missing.js", "*/*", "cors"));
        assert!(!document("/missing.png", "image/*", "no-cors"));
    }

    #[test]
    fn worker_shell_reuses_global_security_policy_without_asset_cache_rules() {
        let headers = security_policy();
        assert_eq!(
            headers.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            ["X-Content-Type-Options", "Referrer-Policy", "Permissions-Policy", "Content-Security-Policy"]
        );
        let csp = headers.iter().find(|(name, _)| *name == "Content-Security-Policy").unwrap().1;
        assert!(csp.contains("script-src 'self'"));
        assert!(csp.contains("'sha256-"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(
            !headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("cache-control") || value.contains("immutable"))
        );
    }
}
