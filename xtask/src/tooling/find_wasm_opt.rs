// SPDX-License-Identifier: AGPL-3.0-or-later
//! One optimizer lookup for Worker builds, static builds and the command line.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

pub fn executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else { return false };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn on_path(program: &OsStr, path: Option<&OsStr>) -> Option<PathBuf> {
    let name = Path::new(program);
    if name.components().count() > 1 {
        return executable(name).then(|| name.to_owned());
    }
    std::env::split_paths(path?).map(|dir| dir.join(name)).find(|candidate| executable(candidate))
}

/// Binaryen's platform names as worker-build caches them. An unsupported
/// machine may still supply an explicit binary or have one on PATH.
pub fn platform(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos" | "Darwin", "arm64" | "aarch64") => Some("arm64-macos"),
        ("macos" | "Darwin", "x86_64") => Some("x86_64-macos"),
        ("linux" | "Linux", "arm64" | "aarch64") => Some("aarch64-linux"),
        ("linux" | "Linux", "x86_64") => Some("x86_64-linux"),
        _ => None,
    }
}

pub fn default_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut roots = Vec::new();
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME").filter(|value| !value.is_empty()) {
        roots.push(cache.into());
    } else if let Some(home) = &home {
        roots.push(home.join(".cache"));
    }
    if let Some(home) = home {
        roots.push(home.join("Library/Caches"));
    }
    roots.push(
        std::env::var_os("TMPDIR").filter(|value| !value.is_empty()).map_or_else(std::env::temp_dir, PathBuf::from),
    );
    roots
}

/// Override, installed tool, then each supplied cache root in order. Cache
/// names are sorted like the old shell glob; a different architecture or a
/// non-executable file can never be picked from a shared cache.
pub fn find(
    override_path: Option<OsString>,
    path: Option<OsString>,
    roots: &[PathBuf],
    target: Option<&str>,
) -> Result<Option<PathBuf>> {
    if let Some(path) = override_path.filter(|value| !value.is_empty()) {
        return Ok(Some(path.into()));
    }
    if let Some(installed) = on_path(OsStr::new("wasm-opt"), path.as_deref()) {
        return Ok(Some(installed));
    }
    let Some(target) = target else { return Ok(None) };
    let prefix = format!("wasm-opt-{target}-");
    for root in roots {
        let mut candidates: Vec<PathBuf> = match fs::read_dir(root.join("worker-build")) {
            Ok(entries) => entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect(),
            Err(error)
                if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied) =>
            {
                continue;
            }
            Err(error) => return Err(format!("{}: {error}", root.display())),
        };
        candidates.sort();
        for candidate in candidates {
            if candidate.file_name().and_then(OsStr::to_str).is_some_and(|name| name.starts_with(&prefix)) {
                let binary = candidate.join("bin/wasm-opt");
                if executable(&binary) {
                    return Ok(Some(binary));
                }
            }
        }
    }
    Ok(None)
}

pub fn current(roots: &[PathBuf]) -> Result<Option<PathBuf>> {
    find(
        std::env::var_os("WASM_OPT_BIN"),
        std::env::var_os("PATH"),
        roots,
        platform(std::env::consts::OS, std::env::consts::ARCH),
    )
}

pub fn run(args: &[String]) -> Result<()> {
    let roots = if args.is_empty() { default_roots() } else { args.iter().map(PathBuf::from).collect() };
    if let Some(path) = current(&roots)? {
        println!("{}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "allthing-optimizer-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn binary(&self, relative: &str, executable: bool) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 })).unwrap();
            }
            path
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicit_path_with_spaces_is_not_reinterpreted_or_required_to_exist() {
        let path = OsString::from("/chosen optimizer/wasm-opt");
        assert_eq!(find(Some(path.clone()), None, &[], None).unwrap(), Some(PathBuf::from(path)));
    }

    #[test]
    fn installed_path_precedes_cache_and_directories_are_not_executables() {
        let scratch = Scratch::new();
        let installed = scratch.binary("tools with spaces/wasm-opt", true);
        scratch.binary("worker-build/wasm-opt-arm64-macos-132/bin/wasm-opt", true);
        let search = std::env::join_paths([installed.parent().unwrap()]).unwrap();
        assert_eq!(
            find(None, Some(search), std::slice::from_ref(&scratch.0), Some("arm64-macos")).unwrap(),
            Some(installed)
        );
        assert!(!executable(&scratch.0));
    }

    #[test]
    fn platform_cache_skips_wrong_architecture_and_non_executable_after_empty_root() {
        let scratch = Scratch::new();
        scratch.binary("cache with spaces/worker-build/wasm-opt-x86_64-linux-132/bin/wasm-opt", true);
        scratch.binary("cache with spaces/worker-build/wasm-opt-arm64-macos-131/bin/wasm-opt", false);
        let wanted = scratch.binary("cache with spaces/worker-build/wasm-opt-arm64-macos-132/bin/wasm-opt", true);
        let roots = [scratch.0.join("empty-xdg"), scratch.0.join("cache with spaces")];
        assert_eq!(find(None, Some(OsString::new()), &roots, Some("arm64-macos")).unwrap(), Some(wanted));
    }

    #[test]
    fn absent_optimizer_and_unknown_architecture_remain_optional() {
        let scratch = Scratch::new();
        assert_eq!(find(None, None, std::slice::from_ref(&scratch.0), Some("arm64-macos")).unwrap(), None);
        assert_eq!(platform("windows", "x86_64"), None);
        assert_eq!(platform("Darwin", "aarch64"), Some("arm64-macos"));
        assert_eq!(find(None, None, &[scratch.0.join("absent")], None).unwrap(), None);
    }
}
