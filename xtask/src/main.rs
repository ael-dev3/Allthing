// SPDX-License-Identifier: AGPL-3.0-or-later
//! Build tasks, run with `cargo xtask <task>`:
//!
//! - `dist`: compile the browser app to WebAssembly, cut the parts it loads
//!   when used into modules of their own (wasm-split), run wasm-bindgen (and
//!   wasm-opt when available), fingerprint the assets and write the static
//!   site into `dist/`, which Workers Static Assets serves.
//! - `pages [base]`: the same site for static hosting under `base` (default
//!   `/Allthing`, for GitHub Pages) in `dist-pages/`, built with the
//!   `static-host` feature so the browser builds payloads itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("dist") => site(&Site::workers()),
        Some("pages") => site(&Site::pages(args.get(1).map_or("/Allthing", String::as_str))),
        _ => Err("usage: cargo xtask dist | cargo xtask pages [base]".into()),
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

/// Write `input` compressed at brotli's highest level to `output`, with
/// Node's zlib (wrangler needs Node anyway). Without Node no copy is written,
/// and the Worker serves the module as it is.
fn brotli(input: &Path, output: &Path) {
    const SCRIPT: &str = "const z = require('zlib'), fs = require('fs'); \
        const bytes = fs.readFileSync(process.argv[1]); \
        fs.writeFileSync(process.argv[2], z.brotliCompressSync(bytes, { params: { \
          [z.constants.BROTLI_PARAM_QUALITY]: 11, [z.constants.BROTLI_PARAM_LGWIN]: 24, \
          [z.constants.BROTLI_PARAM_SIZE_HINT]: bytes.length } }));";
    let _ = fs::remove_file(output);
    match Command::new(tool("NODE_BIN", "node")).args(["-e", SCRIPT]).arg(input).arg(output).status() {
        Ok(status) if status.success() => {}
        _ => eprintln!("xtask: could not compress {} with Node; the Worker will serve it as is", input.display()),
    }
}

/// The Pepe pack as web/paint.js reads it: `{"code": [width, height,
/// animated], …}`. Shortcodes are lowercase words (a unit test of the pack's
/// says so), so they need no escaping.
fn emoji_table() -> String {
    let entries: Vec<String> = allthing_core::emoji::PACK
        .iter()
        .map(|e| format!("\"{}\":[{},{},{}]", e.code, e.width, e.height, u8::from(e.animated)))
        .collect();
    format!("{{{}}}", entries.join(","))
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

/// Which static site to build.
struct Site {
    /// Output directory, relative to the workspace root.
    out: &'static str,
    /// Path prefix the site is served under ("" at a domain's root).
    base: String,
    /// Build payloads in the browser (no Worker behind the site).
    static_host: bool,
}

impl Site {
    fn workers() -> Site {
        Site { out: "dist", base: String::new(), static_host: false }
    }

    fn pages(base: &str) -> Site {
        Site { out: "dist-pages", base: base.trim_end_matches('/').to_owned(), static_host: true }
    }
}

fn site(site: &Site) -> Result<()> {
    let root = root();
    let dist = root.join(site.out);
    let assets = dist.join("assets");
    let bindgen_out = root.join("target/web-bindgen");
    let base = site.base.as_str();

    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.current_dir(&root).env("ALLTHING_BASE_PATH", base).args([
        "build",
        "--release",
        "--locked",
        "-p",
        "allthing-web",
        "--target",
        "wasm32-unknown-unknown",
    ]);
    if site.static_host {
        cargo.args(["--features", "static-host"]);
    }
    run(&mut cargo)?;
    let split_out = root.join("target/web-split");
    let parts = split(&root.join("target/wasm32-unknown-unknown/release/allthing_web.wasm"), &split_out)?;
    let _ = fs::remove_dir_all(&bindgen_out);
    run(Command::new(tool("WASM_BINDGEN_BIN", "wasm-bindgen")).current_dir(&root).args([
        "--target",
        "web",
        "--no-typescript",
        // The split parts reach the module's table by its export.
        "--keep-lld-exports",
        "--out-name",
        "allthing",
        "--out-dir",
        "target/web-bindgen",
        "target/web-split/main.wasm",
    ]))?;
    let wasm_path = bindgen_out.join("allthing_bg.wasm");
    let wasm_opt = tool("WASM_OPT_BIN", "wasm-opt");
    if available(&wasm_opt) {
        for module in std::iter::once(&wasm_path).chain(&parts) {
            optimize(&wasm_opt, module)?;
        }
    } else {
        eprintln!("xtask: wasm-opt not found; shipping the unoptimized modules");
    }

    // Everything is rebuilt.
    let _ = fs::remove_dir_all(&dist);

    // The module, its JavaScript, the split parts and their loader only work
    // with one another (the parts share the module's memory and table): one
    // fingerprint names them all. The module sits in assets/ for the
    // Worker's /wasm/; the rest, which name each other, in a folder of
    // their own.
    let wasm = read(&wasm_path)?;
    let js = read(&bindgen_out.join("allthing.js"))?;
    let loader = read(&split_out.join(SPLIT_LOADER))?;
    let mut together = [wasm.as_slice(), &js, &loader].concat();
    let mut part_files = Vec::new();
    for part in &parts {
        let name = part.file_name().and_then(|n| n.to_str()).ok_or("a split part without a name")?.to_owned();
        let bytes = read(part)?;
        together.extend_from_slice(name.as_bytes());
        together.extend_from_slice(&bytes);
        part_files.push((name, bytes));
    }
    let build = format!("allthing-{}", fingerprint(&together));
    let wasm_name = format!("{build}.wasm");
    write(&assets.join(&wasm_name), &wasm)?;
    // The Worker serves the module at /wasm/ from a copy compressed at
    // brotli's highest level; the static copy has no Worker.
    let wasm_url = if site.static_host {
        format!("{base}/assets/{wasm_name}")
    } else {
        brotli(&assets.join(&wasm_name), &assets.join(format!("{wasm_name}.br")));
        format!("/wasm/{wasm_name}")
    };
    let js_name = format!("{build}/allthing.js");
    write(&assets.join(&js_name), &js)?;
    let loader_name = format!("{build}/{SPLIT_LOADER}");
    write(&assets.join(&loader_name), &loader)?;
    for (name, bytes) in &part_files {
        write(&assets.join(&build).join(name), bytes)?;
    }

    let css = minify_css(&String::from_utf8(read(&root.join("style/allthing.css"))?).map_err(|e| e.to_string())?);
    let css_name = format!("allthing-{}.css", fingerprint(css.as_bytes()));
    write(&assets.join(&css_name), &css)?;

    let boot_template = String::from_utf8(read(&root.join("web/boot.js"))?).map_err(|e| e.to_string())?;
    let boot = boot_template
        .replace("{{JS}}", &js_name)
        .replace("{{WASM_URL}}", &wasm_url)
        .replace("{{WASM}}", &wasm_name)
        .replace("{{BASE}}", base);
    let boot_name = format!("boot-{}.js", fingerprint(boot.as_bytes()));
    write(&assets.join(&boot_name), &boot)?;

    // The front page's first cards before the app has come, with the Pepe
    // pack as the app has it. The static copy has no early fetch to draw
    // them from.
    let paint_tag = if site.static_host {
        String::new()
    } else {
        let template = String::from_utf8(read(&root.join("web/paint.js"))?).map_err(|e| e.to_string())?;
        let paint = template.replace("{{EMOJI}}", &emoji_table()).replace("{{BASE}}", base);
        let name = format!("paint-{}.js", fingerprint(paint.as_bytes()));
        write(&assets.join(&name), &paint)?;
        format!("<script type=\"module\" src=\"{base}/assets/{name}\"></script>")
    };

    // The emoji and stickers, each version's folder as it is (their
    // CREDITS.md say whose they are).
    for pack in ["emoji", "stickers"] {
        for version in fs::read_dir(root.join("web").join(pack)).map_err(|e| e.to_string())? {
            let version = version.map_err(|e| e.to_string())?.path();
            let Some(name) = version.file_name().filter(|_| version.is_dir()) else { continue };
            for file in fs::read_dir(&version).map_err(|e| e.to_string())? {
                let file = file.map_err(|e| e.to_string())?.path();
                if let Some(file_name) = file.file_name() {
                    write(&assets.join(pack).join(name).join(file_name), read(&file)?)?;
                }
            }
        }
    }
    for font in fs::read_dir(root.join("web/fonts")).map_err(|e| e.to_string())? {
        let font = font.map_err(|e| e.to_string())?.path();
        if let Some(name) = font.file_name() {
            write(&assets.join("fonts").join(name), read(&font)?)?;
        }
    }
    // pfp.png and the pictures under promo/ are only for Allthing's
    // Farcaster account, whose picture and casts use allthing.social's
    // copies; nothing in the static copy uses them.
    let files: &[&str] = if site.static_host {
        &["favicon.svg", "robots.txt", "ns-icon.png"]
    } else {
        &["favicon.svg", "robots.txt", "ns-icon.png", "pfp.png", "promo/launch-v6.png", "_headers"]
    };
    for file in files {
        write(&dist.join(file), read(&root.join("web").join(file))?)?;
    }

    let page_parts = page_parts(&String::from_utf8_lossy(&loader), &format!("{base}/assets/{build}"));
    let html = String::from_utf8(read(&root.join("web/index.html"))?).map_err(|e| e.to_string())?;
    let html = html
        .replace("{{CSS}}", &format!("{base}/assets/{css_name}"))
        .replace("{{JS}}", &format!("{base}/assets/{js_name}"))
        .replace("{{SPLIT_JS}}", &format!("{base}/assets/{loader_name}"))
        .replace("{{PAGE_PARTS}}", &page_parts.replace('"', "&quot;"))
        .replace("{{WASM}}", &wasm_url)
        .replace("{{BOOT}}", &format!("{base}/assets/{boot_name}"))
        .replace("{{PAINT}}", &paint_tag)
        .replace("{{BASE}}", base);
    write(&dist.join("index.html"), &html)?;
    if site.static_host {
        // Static hosts answer unknown paths with 404.html: let the app route them.
        write(&dist.join("404.html"), &html)?;
        write(&dist.join(".nojekyll"), "")?;
    }

    let kb = |n: usize| n.div_ceil(1024);
    let parts_shown: Vec<String> =
        part_files.iter().map(|(name, bytes)| format!("{name} {} KB", kb(bytes.len()))).collect();
    eprintln!(
        "xtask: {} ready: {wasm_name} {} KB, {js_name} {} KB, {css_name} {} KB; loaded when used: {}",
        site.out,
        kb(wasm.len()),
        kb(js.len()),
        kb(css.len()),
        parts_shown.join(", ")
    );
    Ok(())
}

/// The loader the split parts are fetched with, as the module's JavaScript
/// imports it.
const SPLIT_LOADER: &str = "allthing-split.js";

/// The parts that are pages of their own, which `index.html` starts
/// fetching with the app when a visit starts on one (its `early-parts`
/// script, which names them by the same words).
const PAGE_PARTS: [&str; 5] = ["post", "user", "channel", "search", "channels"];

/// Each page part's files, under `folder`, as JSON: its own module and the
/// code it shares with other parts, which the loader fetches alongside it.
/// The loader names the parts that share a chunk in a comment above it
/// (`/* post, user */`).
fn page_parts(loader: &str, folder: &str) -> String {
    let mut chunks: Vec<(Vec<&str>, &str)> = Vec::new();
    let mut sharing: Option<Vec<&str>> = None;
    for line in loader.lines().map(str::trim) {
        if let Some(names) = line.strip_prefix("/*").and_then(|l| l.strip_suffix("*/")) {
            sharing = Some(names.split(',').map(str::trim).collect());
            continue;
        }
        if let Some(rest) = line.strip_prefix("const __chunk_")
            && let (Some(parts), Some(number)) = (sharing.take(), rest.split_whitespace().next())
        {
            chunks.push((parts, number));
        }
        sharing = None;
    }
    let entries: Vec<String> = PAGE_PARTS
        .iter()
        .map(|part| {
            let mut files = vec![format!("\"{folder}/split_{part}.wasm\"")];
            files.extend(
                chunks
                    .iter()
                    .filter(|(parts, _)| parts.contains(part))
                    .map(|(_, number)| format!("\"{folder}/chunk_{number}.wasm\"")),
            );
            format!("\"{part}\":[{}]", files.join(","))
        })
        .collect();
    format!("{{{}}}", entries.join(","))
}

/// Cut the parts of the app marked `#[wasm_split]` (allthing-web's
/// `split` module) out of the module `raw` into modules of their own, in
/// `out` with the main module (`main.wasm`, for wasm-bindgen) and the
/// JavaScript that fetches each part the first time it's called. The parts
/// share the main module's memory and table, which wasm-split finds from
/// the relocations the linker keeps (`--emit-relocs` in .cargo/config.toml).
fn split(raw: &Path, out: &Path) -> Result<Vec<PathBuf>> {
    let input = read(raw)?;
    let _ = fs::remove_dir_all(out);
    let main = out.join("main.wasm");
    let mut options = wasm_split_cli_support::Options::new(&input);
    options.output_dir = out;
    options.main_out_path = &main;
    let link_name = format!("./{SPLIT_LOADER}");
    options.link_name = &link_name;
    options.main_module = "./allthing.js";
    let split = wasm_split_cli_support::transform(options).map_err(|e| format!("wasm-split: {e:#}"))?;
    let mut parts = split.split_modules;
    parts.sort();
    Ok(parts)
}

/// Optimize a module for size in place with wasm-opt.
fn optimize(wasm_opt: &str, module: &Path) -> Result<()> {
    let optimized = module.with_extension("opt.wasm");
    run(Command::new(wasm_opt).args(["-Oz", "--strip-debug", "--strip-producers", "-o"]).arg(&optimized).arg(module))?;
    fs::rename(&optimized, module).map_err(|e| e.to_string())
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
    fn lists_each_page_part_with_the_chunks_it_shares() {
        let loader = "/* post, user */\nconst __chunk_3 = makeLoad(() => {\n/* composer, react */\nconst __chunk_4 = makeLoad(() => {\n";
        let parts = page_parts(loader, "/assets/b");
        assert!(parts.starts_with(r#"{"post":["/assets/b/split_post.wasm","/assets/b/chunk_3.wasm"],"user":["/assets/b/split_user.wasm","/assets/b/chunk_3.wasm"],"channel":["/assets/b/split_channel.wasm"]"#));
        assert!(!parts.contains("chunk_4"));
    }

    #[test]
    fn fingerprints_are_stable() {
        assert_eq!(fingerprint(b"abc"), fingerprint(b"abc"));
        assert_ne!(fingerprint(b"abc"), fingerprint(b"abd"));
        assert_eq!(fingerprint(b"").len(), 10);
    }
}
