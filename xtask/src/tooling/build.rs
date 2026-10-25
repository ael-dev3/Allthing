// SPDX-License-Identifier: AGPL-3.0-or-later
//! The native part of the Cloudflare build, after the tiny rustup bootstrap.
//! Version selection, optimizer discovery and command ordering have one owner.

use super::{commands, find_wasm_opt};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub type Result<T> = std::result::Result<T, String>;

/// Read Cargo.lock as TOML rather than relying on adjacent lines. A build
/// must not silently choose an arbitrary version if a CLI's crate occurs
/// at more than one version.
fn locked_version(lock: &str, name: &str) -> Result<String> {
    let lock: toml::Value = toml::from_str(lock).map_err(|error| format!("Cargo.lock: {error}"))?;
    let packages = lock.get("package").and_then(toml::Value::as_array).ok_or("Cargo.lock has no packages")?;
    let mut versions: Vec<&str> = packages
        .iter()
        .filter(|package| package.get("name").and_then(toml::Value::as_str) == Some(name))
        .filter_map(|package| package.get("version").and_then(toml::Value::as_str))
        .collect();
    versions.sort_unstable();
    versions.dedup();
    match versions.as_slice() {
        [version]
            if !version.is_empty()
                && version.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".-+".contains(&byte)) =>
        {
            Ok((*version).to_owned())
        }
        [] => Err(format!("Cargo.lock has no version for {name}")),
        _ => Err(format!("Cargo.lock needs one unambiguous version for {name}")),
    }
}

fn version_matches(output: &[u8], version: &str, named: bool) -> bool {
    let output = String::from_utf8_lossy(output);
    let text = output.trim();
    if named { text == format!("wasm-bindgen {version}") } else { text == version }
}

fn matching_tool(name: &str, version: &str, path: &OsStr, named: bool) -> Option<PathBuf> {
    let binary = commands::tool_on_path(name, path).ok()?;
    let output = Command::new(&binary).arg("--version").env("PATH", path).output().ok()?;
    (output.status.success() && version_matches(&output.stdout, version, named)).then_some(binary)
}

fn commit_stamp(ci: Option<&str>, git: Option<&str>) -> Result<String> {
    let value = ci.filter(|value| !value.is_empty()).or_else(|| git.filter(|value| !value.is_empty())).unwrap_or("dev");
    if value == "dev" {
        return Ok(value.to_owned());
    }
    if !(7..=40).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("build commit must be a 7–40 digit Git revision".into());
    }
    Ok(value[..7].to_owned())
}

fn stamp(root: &Path) -> Result<String> {
    let ci = std::env::var("WORKERS_CI_COMMIT_SHA").ok();
    let git = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
    commit_stamp(ci.as_deref(), git.as_deref())
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new(tools: &Path) -> Result<Self> {
        for _ in 0..100 {
            let path =
                tools.join(format!(".wasm-bindgen-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
        Err("could not allocate a build-tool download directory".into())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Use the same upstream Linux-musl release as before. Only the named
/// binary is streamed out of the tar, never arbitrary archive paths. Both
/// download and redirection stay HTTPS; failure falls back to cargo install.
fn prebuilt_bindgen(tools: &Path, version: &str, path: &OsStr) -> Result<()> {
    let scratch = Scratch::new(tools)?;
    let release = format!("wasm-bindgen-{version}-x86_64-unknown-linux-musl");
    let archive = scratch.0.join("release.tar.gz");
    let binary = scratch.0.join("wasm-bindgen");
    let url = format!("https://github.com/wasm-bindgen/wasm-bindgen/releases/download/{version}/{release}.tar.gz");
    commands::run(
        Command::new("curl")
            .env("PATH", path)
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "--max-time",
                "120",
                "--max-filesize",
                "33554432",
                "--output",
            ])
            .arg(&archive)
            .arg(url),
    )?;
    let output = fs::File::create(&binary).map_err(|error| error.to_string())?;
    commands::run(
        Command::new("tar")
            .env("PATH", path)
            .args(["-xzOf"])
            .arg(&archive)
            .arg(format!("{release}/wasm-bindgen"))
            .stdout(Stdio::from(output)),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).map_err(|error| error.to_string())?;
    }
    let checked = Command::new(&binary).arg("--version").output().map_err(|error| error.to_string())?;
    if !checked.status.success() || !version_matches(&checked.stdout, version, true) {
        return Err("downloaded wasm-bindgen does not report the locked version".into());
    }
    fs::rename(&binary, tools.join("bin/wasm-bindgen")).map_err(|error| error.to_string())
}

fn install(root: &Path, path: &OsStr, tools: &Path, name: &str, version: &str) -> Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    commands::run(
        Command::new(cargo)
            .current_dir(root)
            .env("PATH", path)
            .args(["install", name, "--version", version, "--locked", "--root"])
            .arg(tools),
    )
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err("usage: cargo xtask build".into());
    }
    let lock = fs::read_to_string(root.join("Cargo.lock")).map_err(|error| error.to_string())?;
    let bindgen_version = locked_version(&lock, "wasm-bindgen")?;
    let worker_version = locked_version(&lock, "worker")?;
    let tools = root.join("target/tools");
    fs::create_dir_all(tools.join("bin")).map_err(|error| error.to_string())?;
    let path = commands::build_path(root, std::env::var_os("PATH").as_deref())?;

    if matching_tool("wasm-bindgen", &bindgen_version, &path, true).is_none() {
        eprintln!("Installing wasm-bindgen-cli {bindgen_version}");
        let prebuilt = if std::env::consts::OS == "linux" && std::env::consts::ARCH == "x86_64" {
            prebuilt_bindgen(&tools, &bindgen_version, &path)
        } else {
            Err("no prebuilt binary for this platform".into())
        };
        if let Err(why) = prebuilt {
            eprintln!("xtask: {why}; compiling wasm-bindgen-cli");
            install(root, &path, &tools, "wasm-bindgen-cli", &bindgen_version)?;
        }
    }
    let bindgen = matching_tool("wasm-bindgen", &bindgen_version, &path, true)
        .ok_or("wasm-bindgen is not at the locked version after installation")?;
    if matching_tool("worker-build", &worker_version, &path, false).is_none() {
        eprintln!("Installing worker-build {worker_version}");
        install(root, &path, &tools, "worker-build", &worker_version)?;
    }
    let worker = matching_tool("worker-build", &worker_version, &path, false)
        .ok_or("worker-build is not at the locked version after installation")?;
    let commit = stamp(root)?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let configured = |command: &mut Command| {
        command.env("PATH", &path).env("ALLTHING_COMMIT", &commit).env("WASM_BINDGEN_BIN", &bindgen);
    };
    eprintln!("Building the Worker");
    let mut build_worker = Command::new(worker);
    build_worker.current_dir(root.join("crates/allthing-worker")).arg("--release");
    configured(&mut build_worker);
    commands::run(&mut build_worker)?;
    // Invoke this already-built executable, not a recursive cargo run.
    let mut strip = Command::new(&executable);
    strip.current_dir(root).arg("strip-worker");
    configured(&mut strip);
    commands::run(&mut strip)?;
    let optimizer = find_wasm_opt::find(
        std::env::var_os("WASM_OPT_BIN"),
        Some(path.clone()),
        &find_wasm_opt::default_roots(),
        find_wasm_opt::platform(std::env::consts::OS, std::env::consts::ARCH),
    )?;
    eprintln!("Building the browser app");
    let mut browser = Command::new(executable);
    browser
        .current_dir(root)
        .arg("dist")
        .env("WASM_OPT_BIN", optimizer.map_or_else(OsString::new, PathBuf::into_os_string));
    configured(&mut browser);
    commands::run(&mut browser)?;
    eprintln!("Built dist/ and crates/allthing-worker/build/");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_version_is_structural_and_missing_or_ambiguous_versions_fail() {
        let lock = "version = 4\n[[package]]\nversion = '0.2.108'\nname = 'wasm-bindgen'\n[[package]]\nname = 'worker'\nversion = '0.8.5'\n";
        assert_eq!(locked_version(lock, "wasm-bindgen").unwrap(), "0.2.108");
        assert_eq!(locked_version(lock, "worker").unwrap(), "0.8.5");
        assert!(locked_version(lock, "missing").is_err());
        assert!(locked_version(&format!("{lock}[[package]]\nname = 'worker'\nversion = '0.9.0'\n"), "worker").is_err());
        assert!(locked_version("not toml = ", "worker").is_err());
    }

    #[test]
    fn version_checks_reject_stale_or_prefixed_tool_output() {
        assert!(version_matches(b"wasm-bindgen 0.2.108\n", "0.2.108", true));
        assert!(!version_matches(b"wasm-bindgen 0.2.1089\n", "0.2.108", true));
        assert!(version_matches(b"0.8.5\n", "0.8.5", false));
        assert!(!version_matches(b"worker-build 0.8.5\n", "0.8.5", false));
    }

    #[test]
    fn commit_stamp_uses_ci_before_git_and_rejects_untrusted_non_revisions() {
        assert_eq!(commit_stamp(Some("a1234567890abcdef1234567890abcdef1234567"), Some("b123456")).unwrap(), "a123456");
        assert_eq!(commit_stamp(Some(""), Some("b123456")).unwrap(), "b123456");
        assert_eq!(commit_stamp(None, None).unwrap(), "dev");
        assert!(commit_stamp(Some("$(secret)"), None).is_err());
        assert!(commit_stamp(Some("abc"), None).is_err());
    }
}
