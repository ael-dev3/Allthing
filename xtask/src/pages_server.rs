// SPDX-License-Identifier: AGPL-3.0-or-later
//! The local static-site test server, with GitHub Pages' sub-path and 404 behavior.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

type Result<T> = std::result::Result<T, String>;

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    if args.len() > 3 {
        return Err("usage: cargo xtask serve-pages [port] [base] [root]".into());
    }
    let port = args.first().map_or(Ok(4173), |p| p.parse::<u16>()).map_err(|e| format!("invalid port: {e}"))?;
    let base = args.get(1).map_or("/Allthing", String::as_str).trim_end_matches('/').to_owned();
    validate_base(&base)?;
    let directory = args.get(2).map_or_else(|| root.join("dist-pages"), PathBuf::from);
    let directory = fs::canonicalize(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let server = Arc::new(Server::http(("127.0.0.1", port)).map_err(|e| e.to_string())?);
    println!("dist-pages on http://{}{:}/", server.server_addr(), base);
    // Fetching a large Wasm file must not hold up the parallel CSS/module reads.
    // Eight workers bound concurrency; the server only listens on loopback.
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let server = Arc::clone(&server);
            let directory = &directory;
            let base = &base;
            scope.spawn(move || {
                for request in server.incoming_requests() {
                    respond(request, directory, base);
                }
            });
        }
    });
    Ok(())
}

fn validate_base(base: &str) -> Result<()> {
    if (!base.is_empty() && !base.starts_with('/'))
        || base.starts_with("//")
        || base.chars().any(|ch| ch.is_whitespace() || matches!(ch, '?' | '#' | '\\'))
        || Path::new(base).components().any(|part| part == Component::ParentDir)
    {
        return Err("static base must be an absolute URL path without query, fragment or traversal".into());
    }
    Ok(())
}

struct Answer {
    status: u16,
    content_type: &'static str,
    location: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    fn text(status: u16, text: &str) -> Self {
        Self { status, content_type: "text/plain; charset=utf-8", location: None, body: text.as_bytes().to_vec() }
    }
}

fn respond(request: Request, directory: &Path, base: &str) {
    let mut answer = if matches!(request.method(), Method::Get | Method::Head) {
        answer(directory, base, request.url())
    } else {
        Answer::text(405, "Method not allowed")
    };
    let length = answer.body.len();
    if request.method() == &Method::Head {
        answer.body.clear();
    }
    let mut response = Response::from_data(answer.body)
        .with_status_code(StatusCode(answer.status))
        .with_header(Header::from_bytes("Content-Type", answer.content_type).expect("constant header"))
        .with_header(Header::from_bytes("Cache-Control", "no-cache").expect("constant header"));
    if request.method() == &Method::Head {
        response.add_header(Header::from_bytes("Content-Length", length.to_string()).expect("numeric length"));
    }
    if let Some(location) = answer.location {
        response.add_header(Header::from_bytes("Location", location).expect("validated URL path"));
    }
    if answer.status == 405 {
        response.add_header(Header::from_bytes("Allow", "GET, HEAD").expect("constant header"));
    }
    // A browser cancelling a read is ordinary and must not stop the server.
    let _ = request.respond(response);
}

fn answer(directory: &Path, base: &str, target: &str) -> Answer {
    let origin = url::Url::parse("http://localhost/").expect("constant origin");
    let Ok(url) = origin.join(target) else { return Answer::text(400, "Bad request") };
    if url.path() == base {
        return Answer {
            status: 301,
            content_type: "text/plain; charset=utf-8",
            location: Some(format!("{base}/{}", url.query().map_or(String::new(), |q| format!("?{q}")))),
            body: Vec::new(),
        };
    }
    let relative = url.path().strip_prefix(&format!("{base}/"));
    let decoded = match relative.map(decode_path).transpose() {
        Ok(path) => path,
        Err(()) => return Answer::text(400, "Bad request"),
    };
    let found = decoded.as_deref().and_then(|path| file(directory, path));
    let status = if found.is_some() { 200 } else { 404 };
    match found.or_else(|| file(directory, "404.html")) {
        Some((path, body)) => Answer { status, content_type: content_type(&path), location: None, body },
        None => Answer::text(404, "Not found"),
    }
}

fn decode_path(path: &str) -> std::result::Result<String, ()> {
    let bytes = path.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && !(bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit))
        {
            return Err(());
        }
    }
    percent_encoding::percent_decode_str(path).decode_utf8().map(|s| s.into_owned()).map_err(|_| ())
}

fn file(directory: &Path, relative: &str) -> Option<(PathBuf, Vec<u8>)> {
    if relative.contains(['\\', '\0'])
        || Path::new(relative).components().any(|p| !matches!(p, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let mut full = directory.join(relative);
    if full.is_dir() {
        full.push("index.html");
    }
    let canonical = fs::canonicalize(full).ok()?;
    if !canonical.starts_with(directory) || !canonical.is_file() {
        return None;
    }
    Some((canonical.clone(), fs::read(canonical).ok()?))
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        Some("json") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "allthing-pages-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("site/assets")).unwrap();
            fs::create_dir_all(path.join("site-private")).unwrap();
            fs::write(path.join("site/index.html"), "home").unwrap();
            fs::write(path.join("site/404.html"), "fallback").unwrap();
            fs::write(path.join("site/assets/app.wasm"), [0, 97, 115, 109]).unwrap();
            fs::write(path.join("site-private/proof.txt"), "outside-root-fixture").unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
        fn root(&self) -> PathBuf {
            self.0.join("site")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn github_pages_paths_queries_and_deep_links() {
        let fixture = Fixture::new();
        assert_eq!(answer(&fixture.root(), "/Allthing", "/Allthing/").body, b"home");
        let redirect = answer(&fixture.root(), "/Allthing", "/Allthing?sort=new");
        assert_eq!(redirect.status, 301);
        assert_eq!(redirect.location.as_deref(), Some("/Allthing/?sort=new"));
        let deep = answer(&fixture.root(), "/Allthing", "/Allthing/u/reader");
        assert_eq!((deep.status, deep.body.as_slice()), (404, b"fallback".as_slice()));
        assert_eq!(answer(&fixture.root(), "/Allthing", "/Allthing-sibling/").status, 404);
        let wasm = answer(&fixture.root(), "/Allthing", "/Allthing/assets/app.wasm");
        assert_eq!(wasm.content_type, "application/wasm");
        assert_eq!(wasm.body, [0, 97, 115, 109]);
    }

    #[test]
    fn root_base_is_valid_after_normalization_and_serves_root_static_builds() {
        let fixture = Fixture::new();
        let base = "/".trim_end_matches('/');
        assert!(validate_base(base).is_ok());
        assert_eq!(answer(&fixture.root(), base, "/").body, b"home");
        let deep = answer(&fixture.root(), base, "/u/reader");
        assert_eq!((deep.status, deep.body.as_slice()), (404, b"fallback".as_slice()));
        let module = answer(&fixture.root(), base, "/assets/app.wasm");
        assert_eq!(module.status, 200);
        assert_eq!(module.content_type, "application/wasm");
    }

    #[test]
    fn malformed_paths_do_not_poison_valid_requests_or_queries() {
        let fixture = Fixture::new();
        for path in ["/Allthing/%", "/Allthing/%E0%A4%A", "/Allthing/%FF"] {
            assert_eq!(answer(&fixture.root(), "/Allthing", path).status, 400, "{path}");
        }
        assert_eq!(answer(&fixture.root(), "/Allthing", "/Allthing/?q=%").status, 200);
    }

    #[test]
    fn encoded_traversal_and_sibling_prefix_are_not_files() {
        let fixture = Fixture::new();
        for path in [
            "/Allthing/%2e%2e%2fsite-private/proof.txt",
            "/Allthing/../site-private/proof.txt",
            "/Allthing/%2fetc/passwd",
            "/Allthing/%00",
            "/Allthing/%5c../site-private/proof.txt",
        ] {
            let response = answer(&fixture.root(), "/Allthing", path);
            assert_eq!((response.status, response.body.as_slice()), (404, b"fallback".as_slice()), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_the_site_root() {
        let fixture = Fixture::new();
        std::os::unix::fs::symlink(fixture.0.join("site-private"), fixture.root().join("escape")).unwrap();
        assert_eq!(answer(&fixture.root(), "/Allthing", "/Allthing/escape/proof.txt").status, 404);
    }

    #[test]
    fn real_http_preserves_headers_head_and_read_only_behavior() {
        let fixture = Fixture::new();
        let server = Server::http(("127.0.0.1", 0)).unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let root = fixture.root();
        let worker = std::thread::spawn(move || {
            for _ in 0..3 {
                respond(server.recv().unwrap(), &root, "/Allthing");
            }
        });
        for (method, target, status, expected_body) in [
            ("GET", "/Allthing/", "200", "home"),
            ("HEAD", "/Allthing/assets/app.wasm", "200", ""),
            ("POST", "/Allthing/", "405", "Method not allowed"),
        ] {
            let mut socket = TcpStream::connect(address).unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            write!(
                socket,
                "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {status}")), "{response}");
            let (headers, body) = response.split_once("\r\n\r\n").unwrap();
            assert!(headers.to_ascii_lowercase().contains("cache-control: no-cache"));
            assert_eq!(body, expected_body);
            if method == "HEAD" {
                assert!(headers.to_ascii_lowercase().contains("content-length: 4"));
            }
        }
        worker.join().unwrap();
    }
}
