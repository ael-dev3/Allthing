// SPDX-License-Identifier: AGPL-3.0-or-later
//! Black-box contracts for the native commands, with real subprocesses and
//! temporary fake tools. No browser, package registry or build is started.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        for _ in 0..100 {
            let path = std::env::temp_dir().join(format!(
                "allthing-cli-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("scratch directory: {error}"),
            }
        }
        panic!("could not allocate scratch directory")
    }
    fn binary(&self, relative: &str, body: &str, executable: bool) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 })).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn xtask() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
}

fn phases(functional: i32, smoothness: i32, extra: &[&str]) -> (Output, Vec<Vec<String>>) {
    let scratch = Scratch::new();
    let log = scratch.0.join("calls");
    let npx = scratch.binary(
        "tools with spaces/npx",
        r#"#!/bin/sh
printf 'phase\nhtml=%s\nci=%s\n' "$PLAYWRIGHT_HTML_OUTPUT_DIR" "$CI" >> "$PHASE_LOG"
printf 'arg=%s\n' "$@" >> "$PHASE_LOG"
case "$*" in
  *--workers=2*) exit "$FUNCTIONAL_EXIT" ;;
  *--workers=1*) exit "$SMOOTHNESS_EXIT" ;;
  *) exit 90 ;;
esac
"#,
        true,
    );
    let output = xtask()
        .arg("browser-fixtures")
        .args(extra)
        .env("NPX_BIN", &npx)
        .env("CI", "1")
        .env("PHASE_LOG", &log)
        .env("FUNCTIONAL_EXIT", functional.to_string())
        .env("SMOOTHNESS_EXIT", smoothness.to_string())
        .output()
        .unwrap();
    let text = fs::read_to_string(log).unwrap_or_default();
    let calls = text
        .split("phase\n")
        .filter(|part| !part.is_empty())
        .map(|part| part.trim().lines().map(str::to_owned).collect())
        .collect();
    (output, calls)
}

#[test]
fn native_phases_run_functional_then_quiet_with_unchanged_ci_and_separate_reports() {
    let (result, calls) = phases(0, 0, &[]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0][..2], ["html=playwright-report/functional", "ci=1"]);
    assert_eq!(calls[1][..2], ["html=playwright-report/smoothness", "ci=1"]);
    assert_eq!(
        calls[0].iter().filter(|line| line.starts_with("arg=--")).map(String::as_str).collect::<Vec<_>>(),
        ["arg=--workers=2", "arg=--output=test-results/functional"]
    );
    assert_eq!(
        calls[1].iter().filter(|line| line.starts_with("arg=--")).map(String::as_str).collect::<Vec<_>>(),
        ["arg=--workers=1", "arg=--output=test-results/smoothness"]
    );
    assert!(calls[0].contains(&r"arg=^(?!.*[/\\]smoothness\.fixture\.spec\.ts$).*\.fixture\.spec\.ts$".into()));
    assert!(calls[1].contains(&r"arg=[/\\]smoothness\.fixture\.spec\.ts$".into()));
}

#[test]
fn native_phases_still_measure_smoothness_after_an_assertion_or_runner_failure() {
    for status in [1, 127] {
        let (result, calls) = phases(status, 0, &[]);
        assert_eq!(result.status.code(), Some(1));
        assert_eq!(calls.len(), 2);
        assert!(calls[1].contains(&"arg=--workers=1".into()));
    }
}

#[test]
fn native_phases_fail_when_either_or_both_runners_fail() {
    for (functional, smoothness) in [(0, 1), (1, 1)] {
        let (result, calls) = phases(functional, smoothness, &[]);
        assert_eq!(result.status.code(), Some(1));
        assert_eq!(calls.len(), 2);
    }
}

#[test]
fn native_phases_reject_extra_arguments_before_starting_any_runner() {
    let (result, calls) = phases(0, 0, &["--workers=8"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(calls.is_empty());
}

fn find(roots: &[&Path], override_path: Option<&Path>, search_path: Option<&Path>) -> String {
    let mut command = xtask();
    command.arg("find-wasm-opt").args(roots).env(
        "PATH",
        search_path.map_or_else(
            || "/usr/bin:/bin".into(),
            |dir| std::env::join_paths([dir, Path::new("/usr/bin"), Path::new("/bin")]).unwrap(),
        ),
    );
    if let Some(path) = override_path {
        command.env("WASM_OPT_BIN", path);
    } else {
        command.env_remove("WASM_OPT_BIN");
    }
    let result = command.output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

#[test]
fn native_finder_honors_explicit_paths_including_spaces() {
    let scratch = Scratch::new();
    let explicit = scratch.0.join("chosen optimizer/wasm-opt");
    assert_eq!(find(&[], Some(&explicit), None), explicit.to_str().unwrap());
}

#[test]
fn native_finder_prefers_installed_tool_over_cached_tool() {
    let scratch = Scratch::new();
    let installed = scratch.binary("installed/wasm-opt", "#!/bin/sh\nexit 0\n", true);
    scratch.binary("worker-build/wasm-opt-arm64-macos-132/bin/wasm-opt", "#!/bin/sh\nexit 0\n", true);
    assert_eq!(find(&[&scratch.0], None, installed.parent()), installed.to_str().unwrap());
}

#[test]
fn native_finder_uses_platform_cache_after_empty_xdg_and_rejects_non_executable_or_wrong_arch() {
    let scratch = Scratch::new();
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "arm64-macos",
        ("macos", "x86_64") => "x86_64-macos",
        ("linux", "aarch64") => "aarch64-linux",
        ("linux", "x86_64") => "x86_64-linux",
        other => panic!("unsupported test platform: {other:?}"),
    };
    scratch.binary(
        "Library/Caches with spaces/worker-build/wasm-opt-wrong-platform-132/bin/wasm-opt",
        "#!/bin/sh\nexit 0\n",
        true,
    );
    scratch.binary(
        &format!("Library/Caches with spaces/worker-build/wasm-opt-{target}-131/bin/wasm-opt"),
        "#!/bin/sh\nexit 0\n",
        false,
    );
    let wanted = scratch.binary(
        &format!("Library/Caches with spaces/worker-build/wasm-opt-{target}-132/bin/wasm-opt"),
        "#!/bin/sh\nexit 0\n",
        true,
    );
    assert_eq!(
        find(&[&scratch.0.join("empty-xdg"), &scratch.0.join("Library/Caches with spaces")], None, None),
        wanted.to_str().unwrap()
    );
}

#[test]
fn native_finder_keeps_an_absent_optimizer_optional() {
    let scratch = Scratch::new();
    assert_eq!(find(&[&scratch.0.join("no-cache")], None, None), "");
}

fn audit(stdout: &str, stderr: &str, status: i32) -> Output {
    let scratch = Scratch::new();
    let npm = scratch.binary(
        "fake npm",
        r#"#!/bin/sh
if [ "$*" = 'audit --json --audit-level=high' ]; then
  printf '%s' "$AUDIT_STDOUT"
  printf '%s' "$AUDIT_STDERR" >&2
  exit "$AUDIT_EXIT"
fi
printf '%s\n' 'human advisory details'
exit 0
"#,
        true,
    );
    xtask()
        .arg("npm-audit")
        .env("NPM_BIN", npm)
        .env("AUDIT_STDOUT", stdout)
        .env("AUDIT_STDERR", stderr)
        .env("AUDIT_EXIT", status.to_string())
        .output()
        .unwrap()
}

#[test]
fn native_npm_audit_preserves_advisory_failure_even_when_the_human_report_succeeds() {
    for severity in ["high", "critical"] {
        let (high, critical) = if severity == "high" { (1, 0) } else { (0, 1) };
        let report = format!(r#"{{"metadata":{{"vulnerabilities":{{"high":{high},"critical":{critical}}}}}}}"#);
        let result = audit(&report, "", 1);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains("high or critical"));
    }
    let low_only = audit(r#"{"metadata":{"vulnerabilities":{"high":0,"critical":0,"moderate":8}}}"#, "", 0);
    assert!(low_only.status.success());
}

#[test]
fn native_npm_audit_outages_warn_without_registry_text_injecting_workflow_commands() {
    let outage = audit(r#"{"error":{"summary":"registry 503\n::error::untrusted%text"}}"#, "", 1);
    assert!(outage.status.success());
    let stdout = String::from_utf8(outage.stdout).unwrap();
    assert!(stdout.contains("::warning title=npm audit unavailable::registry 503%0A::error::untrusted%25text"));
    assert_eq!(stdout.lines().count(), 1);
    let malformed = audit("not json", "last registry error", 1);
    assert!(malformed.status.success());
    assert!(String::from_utf8_lossy(&malformed.stdout).contains("npm audit unavailable"));
}

fn module_check(fail_native_clippy: bool, license: bool) -> (Output, Vec<Vec<String>>) {
    let scratch = Scratch::new();
    let log = scratch.0.join("calls");
    let cargo = scratch.binary(
        "bin/cargo",
        r#"#!/bin/sh
printf 'call\ncwd=%s\nflags=%s\n' "$PWD" "$RUSTFLAGS" >> "$MODULE_LOG"
printf 'arg=%s\n' "$@" >> "$MODULE_LOG"
if [ "$FAIL_NATIVE_CLIPPY" = yes ] && [ "$*" = 'clippy --locked --all-targets -- -D warnings' ]; then
  exit 7
fi
exit 0
"#,
        true,
    );
    if license {
        scratch.binary("bin/cargo-deny", "#!/bin/sh\nexit 0\n", true);
    }
    let output = xtask()
        .arg("module-check")
        .env("CARGO", &cargo)
        .env(
            "PATH",
            std::env::join_paths([scratch.0.join("bin"), PathBuf::from("/usr/bin"), PathBuf::from("/bin")]).unwrap(),
        )
        .env("RUSTFLAGS", "browser-only-relocations")
        .env("MODULE_LOG", &log)
        .env("FAIL_NATIVE_CLIPPY", if fail_native_clippy { "yes" } else { "no" })
        .output()
        .unwrap();
    let text = fs::read_to_string(log).unwrap();
    let calls = text
        .split("call\n")
        .filter(|part| !part.is_empty())
        .map(|part| part.trim().lines().map(str::to_owned).collect())
        .collect();
    (output, calls)
}

#[test]
fn native_module_check_uses_its_own_directory_and_clears_only_child_rustflags() {
    let (result, calls) = module_check(false, true);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(calls.len(), 6);
    for call in &calls {
        assert!(call[0].ends_with("/spacetime/spacetimedb"), "{:?}", call);
        assert_eq!(call[1], "flags=");
    }
    let arguments = calls
        .iter()
        .map(|call| call[2..].iter().map(|line| line.strip_prefix("arg=").unwrap()).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        [
            vec!["fmt", "--", "--check"],
            vec!["clippy", "--locked", "--all-targets", "--", "-D", "warnings"],
            vec!["clippy", "--locked", "--target", "wasm32-unknown-unknown", "--", "-D", "warnings"],
            vec!["test", "--locked"],
            vec!["build", "--locked", "--release", "--target", "wasm32-unknown-unknown"],
            vec!["deny", "--log-level", "error", "check", "licenses"],
        ]
    );
}

#[test]
fn native_module_check_stops_after_a_required_failure_and_keeps_optional_license_policy() {
    let (failed, calls) = module_check(true, true);
    assert_eq!(failed.status.code(), Some(1));
    assert_eq!(calls.len(), 2);
    let (passed, calls) = module_check(false, false);
    assert!(passed.status.success());
    assert_eq!(calls.len(), 5);
    assert!(String::from_utf8_lossy(&passed.stderr).contains("skipping the license audit"));
}
