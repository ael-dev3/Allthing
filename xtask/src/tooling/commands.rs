// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native orchestration for existing checks and browser fixture phases.
//! These commands do not alter Playwright's retries, assertions or budgets.

use super::find_wasm_opt;
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, String>;

fn no_args(args: &[String], usage: &str) -> Result<()> {
    if args.is_empty() { Ok(()) } else { Err(format!("usage: cargo xtask {usage}")) }
}

pub fn run(command: &mut Command) -> Result<()> {
    let shown = format!("{command:?}");
    eprintln!("$ {shown}");
    let status = command.status().map_err(|error| format!("could not run {shown}: {error}"))?;
    if status.success() { Ok(()) } else { Err(format!("{shown} failed with {status}")) }
}

/// Functional fixtures first, then calibrated measurements by themselves,
/// even when the first runner fails to start or an assertion fails.
pub fn browser_fixtures(root: &Path, args: &[String]) -> Result<()> {
    no_args(args, "browser-fixtures")?;
    let npx = std::env::var_os("NPX_BIN").unwrap_or_else(|| "npx".into());
    let phases = fixture_phases();
    let failures = run_phases(&phases, |phase| {
        eprintln!("{}", phase.label);
        run(Command::new(&npx).current_dir(root).env("PLAYWRIGHT_HTML_OUTPUT_DIR", phase.report).args(phase.args))
    });
    if failures.is_empty() { Ok(()) } else { Err(failures.join("\n")) }
}

struct Phase {
    label: &'static str,
    report: &'static str,
    args: &'static [&'static str],
}

fn fixture_phases() -> [Phase; 2] {
    [
        Phase {
            label: "Functional fixtures (2 workers)",
            report: "playwright-report/functional",
            args: &[
                "playwright",
                "test",
                r"^(?!.*[/\\]smoothness\.fixture\.spec\.ts$).*\.fixture\.spec\.ts$",
                "--workers=2",
                "--output=test-results/functional",
            ],
        },
        Phase {
            label: "Smoothness fixtures (1 worker, after functional fixtures finish)",
            report: "playwright-report/smoothness",
            args: &[
                "playwright",
                "test",
                r"[/\\]smoothness\.fixture\.spec\.ts$",
                "--workers=1",
                "--output=test-results/smoothness",
            ],
        },
    ]
}

fn run_phases(phases: &[Phase], mut run: impl FnMut(&Phase) -> Result<()>) -> Vec<String> {
    phases.iter().filter_map(|phase| run(phase).err()).collect()
}

/// The exact native/wasm checks formerly listed by scripts/check.sh.
/// Repository version/CSP/changelog checks run before this in the dispatcher.
pub fn check(root: &Path, args: &[String]) -> Result<()> {
    no_args(args, "check")?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    for args in check_steps() {
        run(Command::new(&cargo).current_dir(root).args(args))?;
    }
    if find_wasm_opt::on_path(OsStr::new("cargo-deny"), std::env::var_os("PATH").as_deref()).is_some() {
        run(Command::new(&cargo).current_dir(root).args(["deny", "--log-level", "error", "check", "licenses"]))?;
    } else {
        eprintln!("cargo-deny is not installed; skipping the license audit (cargo install cargo-deny)");
    }
    Ok(())
}

fn check_steps() -> [&'static [&'static str]; 5] {
    [
        &["fmt", "--all", "--", "--check"],
        &["clippy", "--locked", "-p", "allthing-core", "-p", "xtask", "--all-targets", "--", "-D", "warnings"],
        &[
            "clippy",
            "--locked",
            "-p",
            "allthing-data",
            "-p",
            "allthing-web",
            "-p",
            "allthing-worker",
            "--target",
            "wasm32-unknown-unknown",
            "--",
            "-D",
            "warnings",
        ],
        &[
            "clippy",
            "--locked",
            "-p",
            "allthing-web",
            "--features",
            "static-host",
            "--target",
            "wasm32-unknown-unknown",
            "--",
            "-D",
            "warnings",
        ],
        &[
            "test",
            "--locked",
            "-p",
            "allthing-core",
            "-p",
            "allthing-data",
            "-p",
            "allthing-web",
            "-p",
            "allthing-worker",
            "-p",
            "xtask",
        ],
    ]
}

/// The SpacetimeDB module remains outside the application workspace. Its
/// own manifest and lockfile are selected by cwd; the browser's relocation
/// flags must never leak into its Wasm publish build.
pub fn module_check(root: &Path, args: &[String]) -> Result<()> {
    no_args(args, "module-check")?;
    let module = root.join("spacetime/spacetimedb");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    for args in module_steps() {
        run(Command::new(&cargo).current_dir(&module).env("RUSTFLAGS", "").args(args))?;
    }
    if find_wasm_opt::on_path(OsStr::new("cargo-deny"), std::env::var_os("PATH").as_deref()).is_some() {
        run(Command::new(&cargo).current_dir(&module).env("RUSTFLAGS", "").args([
            "deny",
            "--log-level",
            "error",
            "check",
            "licenses",
        ]))?;
    } else {
        eprintln!("cargo-deny is not installed; skipping the license audit");
    }
    Ok(())
}

fn module_steps() -> [&'static [&'static str]; 5] {
    [
        &["fmt", "--", "--check"],
        &["clippy", "--locked", "--all-targets", "--", "-D", "warnings"],
        &["clippy", "--locked", "--target", "wasm32-unknown-unknown", "--", "-D", "warnings"],
        &["test", "--locked"],
        &["build", "--locked", "--release", "--target", "wasm32-unknown-unknown"],
    ]
}

#[derive(Debug, PartialEq)]
enum Audit {
    Available { vulnerabilities: Value, serious: bool },
    Unavailable(String),
}

fn audit_report(bytes: &[u8], stderr: &[u8]) -> Audit {
    let report: Value = match serde_json::from_slice(bytes) {
        Ok(report) => report,
        Err(_) => return Audit::Unavailable(last_line(stderr)),
    };
    if let Some(error) = report.get("error").filter(|error| !error.is_null()) {
        return Audit::Unavailable(
            error.get("summary").and_then(Value::as_str).map_or_else(|| error.to_string(), str::to_owned),
        );
    }
    let Some(found) = report.pointer("/metadata/vulnerabilities").filter(|found| found.is_object()) else {
        return Audit::Unavailable("unrecognized npm audit response".into());
    };
    let (Some(high), Some(critical)) =
        (found.get("high").and_then(Value::as_u64), found.get("critical").and_then(Value::as_u64))
    else {
        return Audit::Unavailable("unrecognized npm audit severity counts".into());
    };
    Audit::Available { vulnerabilities: found.clone(), serious: high > 0 || critical > 0 }
}

fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("no answer").to_owned()
}

fn warning(message: &str) {
    // npm's registry supplies this text: it must remain one workflow command.
    let escaped = message.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A");
    println!("::warning title=npm audit unavailable::{escaped}");
}

struct Captured {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn bounded_read(mut reader: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok((retained, truncated));
        }
        let keep = count.min(limit.saturating_sub(retained.len()));
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
}

fn stop(child: &mut Child) {
    #[cfg(unix)]
    {
        // SAFETY: capture creates a new child process group. Terminating
        // that group also stops helpers retaining its inherited pipes.
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

enum Pipe {
    Out(std::io::Result<(Vec<u8>, bool)>),
    Err(std::io::Result<(Vec<u8>, bool)>),
}

/// Drain both pipes concurrently, but never wait for a pipe outside the
/// deadline. A child may have exited while a helper still owns stdout.
fn capture(command: &mut Command, timeout: Duration) -> Result<Captured> {
    let deadline = Instant::now().checked_add(timeout).ok_or("npm audit timeout is too large")?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let stderr = child.stderr.take().ok_or("missing child stderr")?;
    let (sender, receiver) = mpsc::sync_channel(2);
    let errors = sender.clone();
    thread::spawn(move || {
        let _ = sender.send(Pipe::Out(bounded_read(stdout, 4 * 1024 * 1024)));
    });
    thread::spawn(move || {
        let _ = errors.send(Pipe::Err(bounded_read(stderr, 64 * 1024)));
    });
    let (mut out, mut err) = (None, None);
    let mut exited = false;
    loop {
        if !exited {
            match child.try_wait() {
                Ok(status) => exited = status.is_some(),
                Err(error) => {
                    stop(&mut child);
                    return Err(error.to_string());
                }
            }
        }
        if exited && out.is_some() && err.is_some() {
            return Ok(Captured {
                stdout: out.take().expect("stdout present"),
                stderr: err.take().expect("stderr present"),
            });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            stop(&mut child);
            return Err("npm audit service timed out".into());
        }
        let step = remaining.min(Duration::from_millis(10));
        if out.is_some() && err.is_some() {
            thread::sleep(step);
            continue;
        }
        let message = match receiver.recv_timeout(step) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop(&mut child);
                return Err("npm audit output reader stopped".into());
            }
        };
        let (is_out, result) = match message {
            Pipe::Out(result) => (true, result),
            Pipe::Err(result) => (false, result),
        };
        match result {
            Ok((bytes, false)) => {
                if is_out {
                    out = Some(bytes)
                } else {
                    err = Some(bytes)
                }
            }
            Ok((_, true)) => {
                stop(&mut child);
                return Err("npm audit response exceeded the diagnostic limit".into());
            }
            Err(error) => {
                stop(&mut child);
                return Err(format!("npm audit output failed: {error}"));
            }
        }
    }
}

pub fn npm_audit(root: &Path, args: &[String]) -> Result<()> {
    no_args(args, "npm-audit")?;
    let npm = std::env::var_os("NPM_BIN").unwrap_or_else(|| "npm".into());
    let captured = match capture(
        Command::new(&npm).current_dir(root).args(["audit", "--json", "--audit-level=high"]),
        Duration::from_secs(180),
    ) {
        Ok(captured) => captured,
        Err(error) => {
            warning(&error);
            return Ok(());
        }
    };
    match audit_report(&captured.stdout, &captured.stderr) {
        Audit::Unavailable(why) => {
            warning(&why);
            Ok(())
        }
        Audit::Available { vulnerabilities, serious } => {
            println!("npm audit: {vulnerabilities}");
            if serious {
                // A second readable report helps people fix the advisories;
                // its status cannot turn the already-known failure green.
                match capture(
                    Command::new(&npm).current_dir(root).args(["audit", "--audit-level=high"]),
                    Duration::from_secs(180),
                ) {
                    Ok(report) => {
                        let readable =
                            [String::from_utf8_lossy(&report.stdout), String::from_utf8_lossy(&report.stderr)];
                        for line in readable.iter().flat_map(|text| text.lines()) {
                            // Registry-supplied text must not become an Actions command.
                            println!("npm audit: {line}");
                        }
                    }
                    Err(why) => eprintln!("npm audit: readable advisory report unavailable ({why})"),
                }
                Err("npm audit found high or critical advisories".into())
            } else {
                Ok(())
            }
        }
    }
}

/// PATH for the post-bootstrap build; no global environment mutation is
/// necessary (or safe in a multi-threaded Rust process).
pub fn build_path(root: &Path, inherited: Option<&OsStr>) -> Result<OsString> {
    let mut paths = vec![root.join("target/tools/bin")];
    paths.extend(inherited.into_iter().flat_map(std::env::split_paths));
    std::env::join_paths(paths).map_err(|error| error.to_string())
}

pub fn tool_on_path(program: &str, path: &OsStr) -> Result<PathBuf> {
    find_wasm_opt::on_path(OsStr::new(program), Some(path))
        .ok_or_else(|| format!("{program} is not executable on PATH"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_fixture_phases_always_run_and_either_failure_fails_the_aggregate() {
        for failed in [vec![true, false], vec![false, true], vec![true, true], vec![false, false]] {
            let mut order = Vec::new();
            let errors = run_phases(&fixture_phases(), |phase| {
                let index = order.len();
                order.push(phase.report);
                if failed[index] { Err("runner failed or could not start".into()) } else { Ok(()) }
            });
            assert_eq!(order, ["playwright-report/functional", "playwright-report/smoothness"]);
            assert_eq!(errors.len(), failed.iter().filter(|value| **value).count());
        }
    }

    #[test]
    fn fixture_arguments_preserve_worker_counts_filters_outputs_and_default_retry_policy() {
        let phases = fixture_phases();
        assert_eq!(
            phases[0].args,
            [
                "playwright",
                "test",
                r"^(?!.*[/\\]smoothness\.fixture\.spec\.ts$).*\.fixture\.spec\.ts$",
                "--workers=2",
                "--output=test-results/functional"
            ]
        );
        assert_eq!(
            phases[1].args,
            [
                "playwright",
                "test",
                r"[/\\]smoothness\.fixture\.spec\.ts$",
                "--workers=1",
                "--output=test-results/smoothness"
            ]
        );
        assert!(browser_fixtures(Path::new("."), &["--workers=8".into()]).is_err());
    }

    #[test]
    fn native_and_both_wasm_lint_targets_and_tests_are_retained_locked() {
        let steps = check_steps();
        assert_eq!(steps.len(), 5);
        assert_eq!(steps[0], ["fmt", "--all", "--", "--check"]);
        assert!(steps[1].contains(&"--all-targets"));
        assert!(steps[2].contains(&"allthing-worker"));
        assert!(steps[2].contains(&"wasm32-unknown-unknown"));
        assert!(steps[3].windows(2).any(|args| args == ["--features", "static-host"]));
        for step in &steps[1..] {
            assert!(step.contains(&"--locked"));
        }
        for name in ["allthing-core", "allthing-data", "allthing-web", "allthing-worker", "xtask"] {
            assert!(steps[4].contains(&name));
        }
    }

    #[test]
    fn registry_outages_and_malformed_schema_are_unavailable_not_advisory_passes() {
        assert_eq!(
            audit_report(b"not json", b"first\n503 registry unavailable\n"),
            Audit::Unavailable("503 registry unavailable".into())
        );
        assert_eq!(
            audit_report(br#"{"error":{"summary":"registry 400"}}"#, b""),
            Audit::Unavailable("registry 400".into())
        );
        assert!(matches!(audit_report(br#"{"advisories":[]}"#, b""), Audit::Unavailable(_)));
        assert!(matches!(
            audit_report(br#"{"metadata":{"vulnerabilities":{"high":"0","critical":0}}}"#, b""),
            Audit::Unavailable(_)
        ));
    }

    #[test]
    fn only_high_or_critical_advisories_fail_regardless_of_other_severities() {
        for (high, critical, serious) in [(0, 0, false), (1, 0, true), (0, 1, true), (u64::MAX, u64::MAX, true)] {
            let report = serde_json::json!({"metadata":{"vulnerabilities":{"high":high,"critical":critical,"moderate":99,"low":99}}});
            assert!(
                matches!(audit_report(&serde_json::to_vec(&report).unwrap(), b""), Audit::Available { serious: found, .. } if found == serious)
            );
        }
    }

    #[test]
    fn bounded_diagnostics_are_drained_without_retaining_the_excess() {
        let (bytes, truncated) = bounded_read(std::io::Cursor::new(vec![7; 1024]), 33).unwrap();
        assert_eq!(bytes, vec![7; 33]);
        assert!(truncated);
        assert_eq!(bounded_read(std::io::Cursor::new(b"fine"), 33).unwrap(), (b"fine".to_vec(), false));
    }

    #[cfg(unix)]
    #[test]
    fn npm_capture_deadline_stops_helpers_that_keep_pipes_after_parent_exit() {
        let scratch = std::env::temp_dir().join(format!("allthing-npm-descendant-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let marker = scratch.join("must-not-be-written");
        let started = Instant::now();
        let result = capture(
            Command::new("/bin/sh")
                .args(["-c", "(sleep 0.4; printf leaked > \"$1\") & exit 0", "fixture"])
                .arg(&marker),
            Duration::from_millis(80),
        );
        assert!(result.err().unwrap().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
        thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists(), "the owned descendant process group must be terminated");
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn module_validation_retains_isolated_native_wasm_release_and_license_steps() {
        let steps = module_steps();
        assert_eq!(steps[0], ["fmt", "--", "--check"]);
        assert_eq!(steps[1], ["clippy", "--locked", "--all-targets", "--", "-D", "warnings"]);
        assert_eq!(steps[2], ["clippy", "--locked", "--target", "wasm32-unknown-unknown", "--", "-D", "warnings"]);
        assert_eq!(steps[3], ["test", "--locked"]);
        assert_eq!(steps[4], ["build", "--locked", "--release", "--target", "wasm32-unknown-unknown"]);
    }
}
