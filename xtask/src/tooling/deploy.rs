// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wait for the intended Cloudflare deployment without warming Home or its API.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use url::{Host, Url};

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy)]
struct Options {
    timeout: Duration,
    poll_interval: Duration,
    request_timeout: Duration,
    git_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(1_800),
            poll_interval: Duration::from_secs(10),
            request_timeout: Duration::from_secs(5),
            git_timeout: Duration::from_secs(60),
        }
    }
}

impl Options {
    fn validate(self) -> Result<()> {
        if [self.timeout, self.poll_interval, self.request_timeout, self.git_timeout].contains(&Duration::ZERO) {
            return Err("Wait durations must be positive and finite".into());
        }
        Ok(())
    }
}

struct Tools {
    curl: OsString,
    git: OsString,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            curl: std::env::var_os("CURL_BIN").unwrap_or_else(|| "curl".into()),
            git: std::env::var_os("GIT_BIN").unwrap_or_else(|| "git".into()),
        }
    }
}

fn valid_sha(value: &str, full: bool) -> bool {
    (if full { value.len() == 40 } else { (7..=40).contains(&value.len()) })
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn revision_matches(actual: &str, expected: &str) -> bool {
    valid_sha(actual, false)
        && valid_sha(expected, true)
        && expected.to_ascii_lowercase().starts_with(&actual.to_ascii_lowercase())
}

/// Plain HTTP is useful only for local fixtures. Production checks never
/// downgrade to an insecure origin or accept credentials in a URL.
fn allowed_url(url: &Url) -> bool {
    let local = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && (url.scheme() == "https" || (url.scheme() == "http" && local))
}

fn version_url(base: &str) -> Result<Url> {
    let base = Url::parse(base).map_err(|_| "Expected an HTTP(S) site URL")?;
    if !allowed_url(&base) {
        return Err("Expected an HTTPS site URL (HTTP is allowed only for loopback tests)".into());
    }
    base.join("/version").map_err(|_| "Expected an HTTP(S) site URL".into())
}

#[derive(Debug, PartialEq, Eq)]
enum ChildError {
    TimedOut,
    TooLarge,
    Failed(String),
}

/// Every child has a group of its own. In particular, killing `git fetch`
/// must also stop its transport helper and close inherited output pipes.
fn stop(child: &mut Child) {
    #[cfg(unix)]
    {
        // SAFETY: process_group(0) below makes this child's PID its group
        // ID. The negative PID targets only the group this command owns.
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn capture(
    command: &mut Command,
    deadline: Instant,
    limit: usize,
) -> std::result::Result<(ExitStatus, Vec<u8>), ChildError> {
    if Instant::now() >= deadline {
        return Err(ChildError::TimedOut);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ChildError::Failed(e.to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| ChildError::Failed("missing child stdout".into()))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| ChildError::Failed(e.to_string()))
            .and(if bytes.len() > limit { Err(ChildError::TooLarge) } else { Ok(bytes) });
        let _ = sender.send(result);
    });
    let mut output = None;
    loop {
        if output.is_none() {
            match receiver.try_recv() {
                Ok(Ok(bytes)) => output = Some(bytes),
                Ok(Err(error)) => {
                    stop(&mut child);
                    return Err(error);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    stop(&mut child);
                    return Err(ChildError::Failed("child output reader stopped".into()));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                stop(&mut child);
                return Err(ChildError::Failed(error.to_string()));
            }
        };
        if let Some(status) = status {
            if let Some(bytes) = output {
                return Ok((status, bytes));
            }
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok(bytes)) => return Ok((status, bytes)),
                Ok(Err(error)) => {
                    stop(&mut child);
                    return Err(error);
                }
                Err(_) => {
                    stop(&mut child);
                    return Err(ChildError::TimedOut);
                }
            }
        }
        if Instant::now() >= deadline {
            stop(&mut child);
            // Also prove inherited pipes closed, without an unbounded join
            // if a broken external tool escaped its process group.
            if output.is_none() && receiver.recv_timeout(Duration::from_millis(200)).is_err() {
                return Err(ChildError::Failed("child output did not close after cancellation".into()));
            }
            return Err(ChildError::TimedOut);
        }
        thread::sleep(Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())));
    }
}

#[derive(Debug)]
enum FetchError {
    TimedOut,
    Failed,
}

fn fetch_version(url: &Url, curl: &OsString, deadline: Instant) -> std::result::Result<(u16, String), FetchError> {
    let mut url = url.clone();
    for _ in 0..=20 {
        if !allowed_url(&url) {
            return Err(FetchError::Failed);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(FetchError::TimedOut);
        }
        let seconds = remaining.as_secs_f64().to_string();
        // Redirects are followed explicitly, so each target is checked
        // before a request. All hops share the same request deadline.
        let result = capture(
            Command::new(curl).args([
                "--silent",
                "--show-error",
                "--proto",
                "=http,https",
                "--max-time",
                &seconds,
                "--connect-timeout",
                &seconds,
                "--max-filesize",
                "65536",
                "--noproxy",
                "localhost,127.0.0.1,::1",
                "--header",
                "accept: application/json",
                "--header",
                "cache-control: no-cache",
                "--write-out",
                "\n%{http_code}\n%{redirect_url}",
                "--url",
                url.as_str(),
            ]),
            deadline,
            72 * 1024,
        );
        let (status, bytes) = match result {
            Ok(value) => value,
            Err(ChildError::TimedOut) => return Err(FetchError::TimedOut),
            Err(_) => return Err(FetchError::Failed),
        };
        if !status.success() {
            return Err(if status.code() == Some(28) { FetchError::TimedOut } else { FetchError::Failed });
        }
        let output = String::from_utf8(bytes).map_err(|_| FetchError::Failed)?;
        let mut pieces = output.rsplitn(3, '\n');
        let redirect = pieces.next().ok_or(FetchError::Failed)?;
        let status = pieces.next().ok_or(FetchError::Failed)?.parse::<u16>().map_err(|_| FetchError::Failed)?;
        let body = pieces.next().ok_or(FetchError::Failed)?;
        if body.len() > 64 * 1024 {
            return Err(FetchError::Failed);
        }
        if matches!(status, 301 | 302 | 303 | 307 | 308) && !redirect.is_empty() {
            url = Url::parse(redirect).map_err(|_| FetchError::Failed)?;
            continue;
        }
        return Ok((status, body.into()));
    }
    Err(FetchError::Failed)
}

struct History<'a> {
    root: &'a Path,
    git: OsString,
    timeout: Duration,
    settled: BTreeMap<(String, String), bool>,
}

impl<'a> History<'a> {
    fn new(root: &'a Path, git: &OsString, timeout: Duration) -> Self {
        Self { root, git: git.clone(), timeout, settled: BTreeMap::new() }
    }

    fn git(&self, args: &[&str], deadline: Instant) -> Option<i32> {
        capture(
            Command::new(&self.git).current_dir(self.root).args(args),
            deadline.min(Instant::now() + self.timeout),
            64 * 1024,
        )
        .ok()
        .and_then(|(status, _)| status.code())
    }

    fn includes(&mut self, deployed: &str, expected: &str, deadline: Instant) -> bool {
        if !valid_sha(deployed, false) || !valid_sha(expected, true) {
            return false;
        }
        let key = (deployed.to_ascii_lowercase(), expected.to_ascii_lowercase());
        if let Some(answer) = self.settled.get(&key) {
            return *answer;
        }
        let object = format!("{}^{{commit}}", key.0);
        if self.git(&["cat-file", "-e", &object], deadline) != Some(0)
            && !(self.git(&["fetch", "--quiet", "origin", "main"], deadline) == Some(0)
                && self.git(&["cat-file", "-e", &object], deadline) == Some(0))
        {
            return false;
        }
        match self.git(&["merge-base", "--is-ancestor", &key.1, &key.0], deadline) {
            Some(code @ (0 | 1)) => {
                let answer = code == 0;
                self.settled.insert(key, answer);
                answer
            }
            _ => false,
        }
    }
}

fn wait_for_deploy(
    url: &Url,
    expected: &str,
    options: Options,
    tools: &Tools,
    mut includes: impl FnMut(&str, &str, Instant) -> bool,
    mut log: impl FnMut(String),
) -> Result<String> {
    if !valid_sha(expected, true) {
        return Err("Expected a full 40-digit Git commit".into());
    }
    options.validate()?;
    let deadline = Instant::now().checked_add(options.timeout).ok_or("Wait duration is too large")?;
    let mut last = "no response".to_owned();
    while Instant::now() < deadline {
        let request_deadline = deadline.min(Instant::now() + options.request_timeout);
        match fetch_version(url, &tools.curl, request_deadline) {
            Ok((status, _)) if !(200..300).contains(&status) => last = format!("HTTP {status}"),
            Ok((_, body)) => match serde_json::from_str::<Value>(&body) {
                Ok(body) => {
                    let actual = body.get("commit").and_then(Value::as_str);
                    if let Some(actual) = actual.filter(|actual| valid_sha(actual, false)) {
                        if Instant::now() < deadline && revision_matches(actual, expected) {
                            log(format!("Deployment {actual} is live"));
                            return Ok(actual.into());
                        }
                        if includes(actual, expected, deadline) && Instant::now() < deadline {
                            log(format!("Deployment {actual} is live, and includes {}", &expected[..7]));
                            return Ok(actual.into());
                        }
                        last = format!("commit {actual}");
                    } else {
                        last = "unrecognized version response".into();
                    }
                }
                Err(_) => last = "version request failed".into(),
            },
            Err(FetchError::TimedOut) => last = "version request timed out".into(),
            Err(FetchError::Failed) => last = "version request failed".into(),
        }
        log(format!("Waiting for deployment {}: {last}", &expected[..7]));
        thread::sleep(options.poll_interval.min(deadline.saturating_duration_since(Instant::now())));
    }
    Err(format!("Deployment {} was not live within {}s ({last})", &expected[..7], options.timeout.as_secs_f64()))
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    if args.len() != 2 {
        return Err("usage: cargo xtask wait-for-deploy <site-url> <full-commit>".into());
    }
    if !valid_sha(&args[1], true) {
        return Err("Expected a full 40-digit Git commit".into());
    }
    let url = version_url(&args[0])?;
    let options = Options::default();
    let tools = Tools::default();
    let mut history = History::new(root, &tools.git, options.git_timeout);
    wait_for_deploy(
        &url,
        &args[1],
        options,
        &tools,
        |deployed, expected, deadline| history.includes(deployed, expected, deadline),
        |message| println!("{message}"),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };

    const EXPECTED: &str = "a1234567890abcdef1234567890abcdef1234567";
    fn fast() -> Options {
        Options {
            timeout: Duration::from_secs(2),
            request_timeout: Duration::from_millis(120),
            poll_interval: Duration::from_millis(10),
            git_timeout: Duration::from_millis(120),
        }
    }
    fn response(status: u16, body: &str) -> Vec<u8> {
        format!("HTTP/1.1 {status} test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            .into_bytes()
    }

    struct Http {
        url: Url,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Http {
        fn new(handler: impl Fn(usize) -> Vec<u8> + Send + Sync + 'static, stall: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = version_url(&format!("http://{}/ignored?q=ignored", listener.local_addr().unwrap())).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let handler = Arc::new(handler);
            let (asked, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
            let worker = thread::spawn(move || {
                let mut children = Vec::new();
                while !stopped.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let (asked, stopped, handler) =
                                (Arc::clone(&asked), Arc::clone(&stopped), Arc::clone(&handler));
                            children.push(thread::spawn(move || {
                                stream.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
                                stream.set_write_timeout(Some(Duration::from_millis(200))).unwrap();
                                let mut request = Vec::new();
                                let mut byte = [0];
                                while request.len() < 8192 && stream.read(&mut byte).unwrap_or(0) == 1 {
                                    request.push(byte[0]);
                                    if request.ends_with(b"\r\n\r\n") {
                                        break;
                                    }
                                }
                                let number = {
                                    let mut asked = asked.lock().unwrap();
                                    asked.push(String::from_utf8(request).unwrap());
                                    asked.len()
                                };
                                let _ = stream.write_all(&handler(number));
                                while stall && !stopped.load(Ordering::Relaxed) {
                                    thread::sleep(Duration::from_millis(5));
                                }
                            }));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2))
                        }
                        Err(error) => panic!("local server: {error}"),
                    }
                }
                for child in children {
                    child.join().unwrap();
                }
            });
            Self { url, requests, stop, worker: Some(worker) }
        }
    }
    impl Drop for Http {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }

    #[test]
    fn only_full_expected_and_valid_matching_stamps_are_accepted() {
        for actual in [&EXPECTED[..7], EXPECTED, &EXPECTED.to_ascii_uppercase()] {
            assert!(revision_matches(actual, EXPECTED));
        }
        for actual in ["dev", "", "a12345", "g123456", "b123456", &format!("{EXPECTED}a")] {
            assert!(!revision_matches(actual, EXPECTED));
        }
        assert!(!revision_matches("a123456", "dev"));
        assert!(version_url("file:///tmp/site").is_err());
        assert!(version_url("http://example.com").is_err());
        assert!(version_url("https://secret:password@example.com").is_err());
        assert!(Options { timeout: Duration::ZERO, ..fast() }.validate().is_err());
        assert_eq!(Options::default().timeout, Duration::from_secs(1800));
        assert_eq!(Options::default().poll_interval, Duration::from_secs(10));
        assert_eq!(Options::default().request_timeout, Duration::from_secs(5));
    }

    #[test]
    fn polls_only_uncached_version_through_old_errors_malformed_and_unstamped_releases() {
        let server = Http::new(
            |n| match n {
                1 => response(200, r#"{"commit":"b123456"}"#),
                2 => response(503, "not ready"),
                3 => response(200, "not JSON"),
                4 => response(200, r#"{"commit":"dev"}"#),
                5 => response(200, r#"{"commit":42}"#),
                _ => response(200, &format!(r#"{{"commit":"{EXPECTED}"}}"#)),
            },
            false,
        );
        let actual =
            wait_for_deploy(&server.url, EXPECTED, fast(), &Tools::default(), |_, _, _| false, |_| {}).unwrap();
        assert_eq!(actual, EXPECTED);
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 6);
        for request in requests.iter() {
            assert!(request.starts_with("GET /version HTTP/1.1\r\n"), "{request}");
            assert!(request.to_ascii_lowercase().contains("cache-control: no-cache\r\n"));
            assert!(request.to_ascii_lowercase().contains("accept: application/json\r\n"));
        }
    }

    #[test]
    fn later_including_commits_are_accepted_but_invalid_stamps_never_reach_git() {
        let server = Http::new(
            |n| {
                response(
                    200,
                    match n {
                        1 => r#"{"commit":"b123456"}"#,
                        2 => r#"{"commit":"dev"}"#,
                        _ => r#"{"commit":"c123456"}"#,
                    },
                )
            },
            false,
        );
        let mut asked = Vec::new();
        let actual = wait_for_deploy(
            &server.url,
            EXPECTED,
            fast(),
            &Tools::default(),
            |actual, expected, _| {
                asked.push((actual.to_owned(), expected.to_owned()));
                actual == "c123456"
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(actual, "c123456");
        assert_eq!(asked, [("b123456".into(), EXPECTED.into()), ("c123456".into(), EXPECTED.into())]);
    }

    #[test]
    fn wrong_release_and_stalled_headers_or_body_respect_monotonic_deadlines() {
        for (stall, bytes) in [
            (false, response(200, r#"{"commit":"b123456"}"#)),
            (true, Vec::new()),
            (true, b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"commit\":".to_vec()),
        ] {
            let server = Http::new(move |_| bytes.clone(), stall);
            let start = Instant::now();
            let options = Options { timeout: Duration::from_millis(600), ..fast() };
            let result = wait_for_deploy(&server.url, EXPECTED, options, &Tools::default(), |_, _, _| false, |_| {});
            assert!(result.unwrap_err().contains("was not live within 0.6s"));
            assert!(start.elapsed() < Duration::from_millis(1500));
            assert!(server.requests.lock().unwrap().len() >= 2);
        }
    }

    #[test]
    fn redirects_cannot_read_files_or_downgrade_to_public_http_and_bodies_are_bounded() {
        let destination = Http::new(|_| response(200, &format!(r#"{{"commit":"{EXPECTED}"}}"#)), false);
        let location = destination.url.to_string();
        let redirect = Http::new(
            move |_| format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n").into_bytes(),
            false,
        );
        let (status, body) =
            fetch_version(&redirect.url, &Tools::default().curl, Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(status, 200);
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["commit"], EXPECTED);
        assert_eq!(destination.requests.lock().unwrap().len(), 1);
        for location in ["file:///etc/passwd", "http://example.com/version", "https://user:secret@example.com/version"]
        {
            let response = format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n");
            let server = Http::new(move |_| response.as_bytes().to_vec(), false);
            assert!(
                fetch_version(&server.url, &Tools::default().curl, Instant::now() + Duration::from_secs(1)).is_err()
            );
            assert_eq!(server.requests.lock().unwrap().len(), 1);
        }
        let server = Http::new(|_| response(200, &"x".repeat(70 * 1024)), false);
        assert!(fetch_version(&server.url, &Tools::default().curl, Instant::now() + Duration::from_secs(1)).is_err());
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            for _ in 0..100 {
                let path = std::env::temp_dir().join(format!(
                    "allthing-deploy-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                if fs::create_dir(&path).is_ok() {
                    return Self(path);
                }
            }
            panic!("could not create fixture directory");
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn git(root: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(root)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
    fn commit(root: &Path, name: &str) -> String {
        fs::write(root.join(name), name).unwrap();
        git(root, &["add", name]);
        git(root, &["commit", "-q", "-m", name]);
        git(root, &["rev-parse", "HEAD"])
    }

    #[test]
    fn ancestry_fetches_unknown_main_commits_and_caches_settled_answers() {
        let scratch = Scratch::new();
        let work = scratch.0.join("work");
        let clone = scratch.0.join("clone");
        git(&scratch.0, &["init", "-q", work.to_str().unwrap()]);
        let old = commit(&work, "a");
        let expected = commit(&work, "b");
        git(&scratch.0, &["clone", "-q", "--no-local", work.to_str().unwrap(), clone.to_str().unwrap()]);
        let later = commit(&work, "c");
        git(&work, &["checkout", "-q", "-b", "side", &old]);
        let side = commit(&work, "d");
        git(&work, &["checkout", "-q", "main"]);
        let tools = Tools::default();
        let mut history = History::new(&clone, &tools.git, Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(10);
        assert!(history.includes(&later[..7], &expected, deadline));
        assert!(history.includes(&expected[..7], &expected, deadline));
        assert!(!history.includes(&old[..7], &expected, deadline));
        assert!(!history.includes(&side[..7], &expected, deadline));
        assert!(!history.includes("0000000", &expected, deadline));
        history.git = "missing-git-fixture".into();
        assert!(history.includes(&later[..7], &expected, deadline));
        assert!(!history.includes(&old[..7], &expected, deadline));
        assert_eq!(history.settled.len(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn deadlines_kill_helpers_that_inherit_the_output_pipe() {
        // A shell stands in for git with a transport helper. Killing only
        // the parent leaves sleep holding stdout open for thirty seconds.
        let start = Instant::now();
        let error = capture(
            Command::new("sh").args(["-c", "sleep 30 & wait"]),
            Instant::now() + Duration::from_millis(100),
            1024,
        )
        .unwrap_err();
        assert_eq!(error, ChildError::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
