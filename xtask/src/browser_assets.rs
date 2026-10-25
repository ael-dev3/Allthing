// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native compression and browser SDK build orchestration.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use super::{Result, read, run};

pub fn build_bridge(root: &Path, out: &Path) -> Result<Vec<u8>> {
    let manifest = root.join("node_modules/spacetimedb/package.json");
    let package: serde_json::Value =
        serde_json::from_slice(&read(&manifest)?).map_err(|e| format!("{}: {e}", manifest.display()))?;
    // The CSP adapter is verified against this SDK's actual BSATN factories.
    // An upgrade must not silently reintroduce its runtime code generation.
    if package["version"].as_str() != Some("2.13.0") {
        return Err(format!("review web/stdb/csp-codec.ts before upgrading SpacetimeDB {}", package["version"]));
    }
    // npm may replace esbuild's launcher with its native executable during
    // postinstall. Execute it directly; passing that binary to Node disables
    // Messages by trying to parse machine code as JavaScript.
    run(Command::new(root.join("node_modules/esbuild/bin/esbuild"))
        .current_dir(root)
        .args([
            "web/stdb/bridge.ts",
            "--bundle",
            "--format=esm",
            "--platform=browser",
            "--target=es2022",
            "--minify",
            "--legal-comments=eof",
            "--log-level=warning",
        ])
        .arg(format!("--outfile={}", out.display())))?;
    read(out)
}

fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    let params =
        brotli::enc::BrotliEncoderParams { quality: 11, lgwin: 24, size_hint: bytes.len(), ..Default::default() };
    let mut compressed = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::with_params(&mut compressed, 64 * 1024, &params);
        writer.write_all(bytes).map_err(|e| e.to_string())?;
    }
    Ok(compressed)
}

/// Keep the Worker's uncompressed fallback if an optional compressed copy
/// cannot be written. Compression itself no longer needs Node or its zlib.
pub fn brotli(input: &Path, output: &Path) {
    let _ = fs::remove_file(output);
    let result = read(input).and_then(|bytes| compress(&bytes)).and_then(|bytes| super::write(output, bytes));
    if let Err(why) = result {
        let _ = fs::remove_file(output);
        eprintln!("xtask: could not compress {}: {why}; the Worker will serve it as is", input.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn native_brotli_roundtrips_binary_wasm_and_empty_files() {
        for bytes in [Vec::new(), [0, 97, 115, 109, 1, 0, 0, 0].to_vec(), (0..=255).cycle().take(128 * 1024).collect()]
        {
            let compressed = compress(&bytes).unwrap();
            let mut decoded = Vec::new();
            brotli::Decompressor::new(compressed.as_slice(), 64 * 1024).read_to_end(&mut decoded).unwrap();
            assert_eq!(decoded, bytes);
        }
    }
}
