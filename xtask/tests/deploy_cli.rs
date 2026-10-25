// SPDX-License-Identifier: AGPL-3.0-or-later
//! Black-box deployment command checks. Only loopback HTTP is contacted.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

const EXPECTED: &str = "a1234567890abcdef1234567890abcdef1234567";

#[test]
fn command_accepts_a_matching_live_revision_and_only_requests_version() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while stream.read(&mut byte).unwrap() == 1 {
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("GET /version HTTP/1.1\r\n"), "{request}");
        assert!(request.to_ascii_lowercase().contains("cache-control: no-cache\r\n"));
        let body = format!(r#"{{"commit":"{}"}}"#, EXPECTED[..7].to_ascii_uppercase());
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["wait-for-deploy", &format!("http://{address}/ignored?q=ignored"), EXPECTED])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Deployment A123456 is live"));
    worker.join().unwrap();
}

#[test]
fn invalid_arguments_fail_before_network_and_have_no_budget_override() {
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec![], "usage:"),
        (vec!["https://allthing.social"], "usage:"),
        (vec!["https://allthing.social", "dev"], "full 40-digit Git commit"),
        (vec!["file:///tmp/site", EXPECTED], "HTTPS site URL"),
        (vec!["http://example.com", EXPECTED], "HTTPS site URL"),
        (vec!["https://allthing.social", EXPECTED, "--timeout=1"], "usage:"),
    ];
    for (args, reason) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_xtask")).arg("wait-for-deploy").args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(reason),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
}
