// SPDX-License-Identifier: AGPL-3.0-or-later
//! Build tasks, run with `cargo xtask <task>`:
//!
//! - `dist`: compile the browser app to WebAssembly, run wasm-bindgen (and
//!   wasm-opt when available), fingerprint the assets and write the static
//!   site into `dist/`, which Workers Static Assets serves.
//! - `labels <spam.jsonl> [version]`: compact the Farcaster app's spam-label
//!   dataset into the per-fid shards under `dist/data/labels/spam/`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use allthing_core::labels::{LabelsMeta, SHARD_SIZE, ShardBuilder};

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("dist") => dist(),
        Some("labels") => match args.get(1) {
            Some(input) => labels(Path::new(input), args.get(2).cloned()),
            None => Err("usage: cargo xtask labels <spam.jsonl> [version]".into()),
        },
        _ => Err("usage: cargo xtask dist | cargo xtask labels <spam.jsonl> [version]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root").to_path_buf()
}

fn run(command: &mut Command) -> Result<()> {
    let shown = format!("{command:?}");
    eprintln!("$ {shown}");
    let status = command.status().map_err(|e| format!("could not run {shown}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{shown} failed with {status}")) }
}

/// A tool from an environment override, else from PATH.
fn tool(env: &str, name: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| name.to_owned())
}

fn available(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok_and(|o| o.status.success())
}

/// FNV-1a, for cache-busting file names.
fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:010x}", hash >> 24)
}

/// Strip comments and needless whitespace; good enough for our own CSS.
fn minify_css(css: &str) -> String {
    let mut without_comments = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        without_comments.push_str(&rest[..start]);
        rest = rest[start + 2..].split_once("*/").map_or("", |(_, after)| after);
    }
    without_comments.push_str(rest);
    let mut out = String::with_capacity(without_comments.len());
    let mut last = ' ';
    for ch in without_comments.split_whitespace().collect::<Vec<_>>().join(" ").chars() {
        if ch == ' ' && "{};,>".contains(last) {
            continue;
        }
        if "{};,>".contains(ch) && last == ' ' {
            out.pop();
        }
        out.push(ch);
        last = ch;
    }
    out
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn dist() -> Result<()> {
    let root = root();
    let dist = root.join("dist");
    let assets = dist.join("assets");
    let bindgen_out = root.join("target/web-bindgen");

    run(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())).current_dir(&root).args([
        "build",
        "--release",
        "--locked",
        "-p",
        "allthing-web",
        "--target",
        "wasm32-unknown-unknown",
    ]))?;
    let _ = fs::remove_dir_all(&bindgen_out);
    run(Command::new(tool("WASM_BINDGEN_BIN", "wasm-bindgen")).current_dir(&root).args([
        "--target",
        "web",
        "--no-typescript",
        "--out-name",
        "allthing",
        "--out-dir",
        "target/web-bindgen",
        "target/wasm32-unknown-unknown/release/allthing_web.wasm",
    ]))?;
    let wasm_path = bindgen_out.join("allthing_bg.wasm");
    let wasm_opt = tool("WASM_OPT_BIN", "wasm-opt");
    if available(&wasm_opt) {
        let optimized = bindgen_out.join("allthing_opt.wasm");
        run(Command::new(&wasm_opt)
            .args(["-Oz", "--strip-debug", "--strip-producers", "-o"])
            .arg(&optimized)
            .arg(&wasm_path))?;
        fs::rename(&optimized, &wasm_path).map_err(|e| e.to_string())?;
    } else {
        eprintln!("xtask: wasm-opt not found; shipping the unoptimized module");
    }

    // Start from a clean assets directory; label shards under dist/data stay.
    let _ = fs::remove_dir_all(&assets);
    for stale in ["index.html", "_headers", "favicon.svg", "robots.txt"] {
        let _ = fs::remove_file(dist.join(stale));
    }

    let wasm = read(&wasm_path)?;
    let wasm_name = format!("allthing-{}.wasm", fingerprint(&wasm));
    write(&assets.join(&wasm_name), &wasm)?;
    let js = read(&bindgen_out.join("allthing.js"))?;
    let js_name = format!("allthing-{}.js", fingerprint(&js));
    write(&assets.join(&js_name), &js)?;

    let css = minify_css(&String::from_utf8(read(&root.join("style/allthing.css"))?).map_err(|e| e.to_string())?);
    let css_name = format!("allthing-{}.css", fingerprint(css.as_bytes()));
    write(&assets.join(&css_name), &css)?;

    let boot_template = String::from_utf8(read(&root.join("web/boot.js"))?).map_err(|e| e.to_string())?;
    let boot = boot_template.replace("{{JS}}", &js_name).replace("{{WASM}}", &wasm_name);
    let boot_name = format!("boot-{}.js", fingerprint(boot.as_bytes()));
    write(&assets.join(&boot_name), &boot)?;

    for font in fs::read_dir(root.join("web/fonts")).map_err(|e| e.to_string())? {
        let font = font.map_err(|e| e.to_string())?.path();
        if let Some(name) = font.file_name() {
            write(&assets.join("fonts").join(name), read(&font)?)?;
        }
    }
    for file in ["favicon.svg", "robots.txt", "_headers"] {
        write(&dist.join(file), read(&root.join("web").join(file))?)?;
    }

    let html = String::from_utf8(read(&root.join("web/index.html"))?).map_err(|e| e.to_string())?;
    let html = html
        .replace("{{CSS}}", &format!("/assets/{css_name}"))
        .replace("{{JS}}", &format!("/assets/{js_name}"))
        .replace("{{WASM}}", &format!("/assets/{wasm_name}"))
        .replace("{{BOOT}}", &format!("/assets/{boot_name}"));
    write(&dist.join("index.html"), html)?;

    let kb = |n: usize| n.div_ceil(1024);
    eprintln!(
        "xtask: dist ready: {wasm_name} {} KB, {js_name} {} KB, {css_name} {} KB",
        kb(wasm.len()),
        kb(js.len()),
        kb(css.len())
    );
    Ok(())
}

fn labels(input: &Path, version: Option<String>) -> Result<()> {
    let file = fs::File::open(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut builder = ShardBuilder::default();
    let (mut lines, mut used) = (0u64, 0u64);
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        lines += 1;
        if builder.add_line(&line) {
            used += 1;
        }
    }
    if used == 0 {
        return Err(format!("no spam labels found in {} ({lines} lines)", input.display()));
    }
    let shards: BTreeMap<u64, String> = builder.shards();
    let out = root().join("dist/data/labels/spam");
    let _ = fs::remove_dir_all(&out);
    for (shard, contents) in &shards {
        write(&out.join(format!("{shard:04}.txt")), contents)?;
    }
    let meta = LabelsMeta {
        source: "https://github.com/warpcast/labels (spam.jsonl), published by the Farcaster app".into(),
        version,
        newest_label: builder.newest(),
        built_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64),
        shard_size: SHARD_SIZE,
        shards: shards.len() as u64,
        counts: builder.counts(),
    };
    write(&out.join("meta.json"), serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?)?;
    eprintln!("xtask: {used} of {lines} lines used; {} shards; counts {:?}", shards.len(), meta.counts);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minifies_css() {
        assert_eq!(minify_css("/* c */ a  >  b {\n  color: red ;\n}\n"), "a>b{color: red;}");
        assert_eq!(minify_css("a, b { x: 1 }"), "a,b{x: 1}");
    }

    #[test]
    fn fingerprints_are_stable() {
        assert_eq!(fingerprint(b"abc"), fingerprint(b"abc"));
        assert_ne!(fingerprint(b"abc"), fingerprint(b"abd"));
        assert_eq!(fingerprint(b"").len(), 10);
    }
}
