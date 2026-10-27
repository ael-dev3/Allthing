// SPDX-License-Identifier: AGPL-3.0-or-later
//! Select Canonical's Ubuntu archive before Playwright installs its unchanged
//! dependencies. Only the GitHub Linux runner's regular Ubuntu configs apply.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

use crate::Result;

const MAX_BYTES: usize = 64 * 1024;
const ARCHIVE: &str = "https://archive.ubuntu.com/ubuntu";

#[derive(Clone, Copy)]
enum Format {
    Mirrors,
    Sources,
    Deb822,
}

const CONFIGS: [(&str, Format); 3] = [
    ("/etc/apt/apt-mirrors.txt", Format::Mirrors),
    ("/etc/apt/sources.list", Format::Sources),
    ("/etc/apt/sources.list.d/ubuntu.sources", Format::Deb822),
];

fn guard(args: &[String], os: &str, github_actions: Option<&str>) -> Result<()> {
    if !args.is_empty() {
        return Err("usage: cargo xtask browser-mirror".into());
    }
    if os != "linux" || github_actions != Some("true") {
        return Err("browser-mirror requires Linux GitHub Actions; no APT files were accessed".into());
    }
    Ok(())
}

fn azure(token: &str) -> bool {
    matches!(
        token,
        "http://azure.archive.ubuntu.com/ubuntu"
            | "http://azure.archive.ubuntu.com/ubuntu/"
            | "https://azure.archive.ubuntu.com/ubuntu"
            | "https://azure.archive.ubuntu.com/ubuntu/"
    )
}

fn security_suite(suite: &str) -> bool {
    suite.trim_end_matches('/').ends_with("-security")
}

/// Active ASCII-whitespace tokens, keeping their original byte ranges. A
/// comment starts at its own token; a URL suffix or fragment is never exact.
fn tokens(line: &str, offset: usize) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if bytes[start] == b'#' {
            break;
        }
        found.push((offset + start, offset + at));
    }
    found
}

fn line_ranges(input: &str, format: Format, offset: usize, line: &str, ranges: &mut Vec<(usize, usize)>) {
    let parts = tokens(line, offset);
    let Some(&(first, end)) = parts.first() else { return };
    let uri = match format {
        Format::Mirrors => {
            let security = parts.iter().skip(1).any(|&(a, b)| {
                input[a..b].split_once(':').is_some_and(|(name, suites)| {
                    name.eq_ignore_ascii_case("suite") && suites.split(',').any(security_suite)
                })
            });
            if security { None } else { Some((first, end)) }
        }
        Format::Sources if matches!(&input[first..end], "deb" | "deb-src") => {
            let mut at = 1;
            if parts.get(at).is_some_and(|&(a, _)| input[a..].starts_with('[')) {
                while parts.get(at).is_some_and(|&(_, b)| !input[..b].ends_with(']')) {
                    at += 1;
                }
                at += 1;
            }
            // Security repositories retain their existing mirror, including
            // a source list whose URI happens to be Azure's archive.
            let security = parts.get(at + 1).is_some_and(|&(a, b)| security_suite(&input[a..b]));
            if security { None } else { parts.get(at).copied() }
        }
        _ => None,
    };
    if let Some((a, b)) = uri
        && azure(&input[a..b])
    {
        ranges.push((a, b));
    }
}

fn deb822_ranges(input: &str, offset: usize, stanza: &str, ranges: &mut Vec<(usize, usize)>) {
    let mut field = "";
    let mut security = false;
    let mut disabled = false;
    let mut uris = Vec::new();
    let mut at = offset;
    for line in stanza.split_inclusive('\n') {
        let leading = line.bytes().next().is_some_and(|b| b.is_ascii_whitespace());
        if line.trim_start().starts_with('#') {
            at += line.len();
            continue;
        }
        let (start, value) = if leading {
            (at, line)
        } else if let Some((name, value)) = line.split_once(':') {
            field = name;
            (at + name.len() + 1, value)
        } else {
            field = "";
            at += line.len();
            continue;
        };
        let parts = tokens(value, start);
        if field.eq_ignore_ascii_case("Suites") {
            security |= parts.iter().any(|&(a, b)| security_suite(&input[a..b]));
        } else if field.eq_ignore_ascii_case("Enabled") {
            disabled |= parts.iter().any(|&(a, b)| input[a..b].eq_ignore_ascii_case("no"));
        } else if field.eq_ignore_ascii_case("URIs") {
            uris.extend(parts.into_iter().filter(|&(a, b)| azure(&input[a..b])));
        }
        at += line.len();
    }
    if !security && !disabled {
        ranges.extend(uris);
    }
}

fn rewrite(input: &str, format: Format) -> Result<(String, usize)> {
    if input.len() > MAX_BYTES || input.contains('\0') {
        return Err("APT configuration exceeds the bound or contains a NUL byte".into());
    }
    let mut ranges = Vec::new();
    let mut at = 0;
    let mut stanza = 0;
    for line in input.split_inclusive('\n') {
        if matches!(format, Format::Deb822) {
            if line.trim().is_empty() {
                deb822_ranges(input, stanza, &input[stanza..at], &mut ranges);
                stanza = at + line.len();
            }
        } else {
            line_ranges(input, format, at, line, &mut ranges);
        }
        at += line.len();
    }
    if matches!(format, Format::Deb822) {
        deb822_ranges(input, stanza, &input[stanza..], &mut ranges);
    }
    let mut output = String::with_capacity(input.len());
    let mut previous = 0;
    for &(a, b) in &ranges {
        output.push_str(&input[previous..a]);
        output.push_str(ARCHIVE);
        previous = b;
    }
    output.push_str(&input[previous..]);
    Ok((output, ranges.len()))
}

fn read_regular(path: &str) -> Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(format!("could not inspect Ubuntu APT configuration {path}")),
    };
    if !metadata.file_type().is_file() {
        return Err(format!("Ubuntu APT configuration is not a regular file: {path}"));
    }
    let file = File::open(path).map_err(|_| format!("could not read Ubuntu APT configuration {path}"))?;
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("could not read Ubuntu APT configuration {path}"))?;
    if bytes.len() > MAX_BYTES {
        return Err(format!("Ubuntu APT configuration exceeds the size bound: {path}"));
    }
    String::from_utf8(bytes).map(Some).map_err(|_| format!("Ubuntu APT configuration is not UTF-8: {path}"))
}

fn write_regular(path: &str, body: &str) -> Result<()> {
    let mut child = Command::new("/usr/bin/sudo")
        .args(["-n", "/usr/bin/tee", "--", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| format!("could not write Ubuntu APT configuration {path}"))?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| format!("could not open APT writer for {path}"))
        .and_then(|mut stdin| stdin.write_all(body.as_bytes()).map_err(|_| format!("could not write {path}")));
    let status = child.wait().map_err(|_| format!("could not finish APT writer for {path}"))?;
    written?;
    if status.success() { Ok(()) } else { Err(format!("Ubuntu APT writer failed for {path}")) }
}

pub fn run(args: &[String]) -> Result<()> {
    guard(args, std::env::consts::OS, std::env::var("GITHUB_ACTIONS").ok().as_deref())?;
    // Validate every bounded, fixed-path input before any privileged write.
    let mut changes = Vec::new();
    for (path, format) in CONFIGS {
        if let Some(input) = read_regular(path)? {
            let (body, count) = rewrite(&input, format)?;
            if count > 0 {
                changes.push((path, body, count));
            }
        }
    }
    for (path, body, count) in &changes {
        write_regular(path, body)?;
        println!("browser-mirror: updated {count} Azure Ubuntu URI token(s) in {path}");
    }
    if changes.is_empty() {
        println!("browser-mirror: no Azure Ubuntu archive URI changes needed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_list_preserves_comments_priority_whitespace_and_other_uris() {
        let input = "# http://azure.archive.ubuntu.com/ubuntu stays a comment\r\n\t http://azure.archive.ubuntu.com/ubuntu/\tpriority:1 # keep\r\nhttps://security.ubuntu.com/ubuntu priority:2\nhttps://archive.ubuntu.com/ubuntu\nhttp://azure.archive.ubuntu.com/ubuntu\tsuite:noble,noble-security/\n";
        let expected = "# http://azure.archive.ubuntu.com/ubuntu stays a comment\r\n\t https://archive.ubuntu.com/ubuntu\tpriority:1 # keep\r\nhttps://security.ubuntu.com/ubuntu priority:2\nhttps://archive.ubuntu.com/ubuntu\nhttp://azure.archive.ubuntu.com/ubuntu\tsuite:noble,noble-security/\n";
        assert_eq!(rewrite(input, Format::Mirrors).unwrap(), (expected.into(), 1));
        assert_eq!(rewrite(expected, Format::Mirrors).unwrap(), (expected.into(), 0));
    }

    #[test]
    fn exact_uri_tokens_only_accept_all_four_azure_spellings() {
        for uri in [
            "http://azure.archive.ubuntu.com/ubuntu",
            "http://azure.archive.ubuntu.com/ubuntu/",
            "https://azure.archive.ubuntu.com/ubuntu",
            "https://azure.archive.ubuntu.com/ubuntu/",
        ] {
            assert_eq!(rewrite(uri, Format::Mirrors).unwrap(), (ARCHIVE.into(), 1));
        }
        for suffix in ["//", "/pool", "?query=1", "#fragment", ".evil", ":80", "\u{a0}priority:1"] {
            let input = format!("http://azure.archive.ubuntu.com/ubuntu{suffix}");
            assert_eq!(rewrite(&input, Format::Mirrors).unwrap(), (input, 0));
        }
        for input in ["http://azure.archive.ubuntu.com.evil/ubuntu", "https://user@azure.archive.ubuntu.com/ubuntu"] {
            assert_eq!(rewrite(input, Format::Mirrors).unwrap(), (input.into(), 0));
        }
    }

    #[test]
    fn legacy_sources_preserve_signing_suites_components_and_security() {
        let input = "deb [arch=amd64 signed-by=/usr/share/keyrings/ubuntu.gpg] http://azure.archive.ubuntu.com/ubuntu noble main universe # Ubuntu\ndeb-src https://azure.archive.ubuntu.com/ubuntu/ noble-updates main\ndeb http://azure.archive.ubuntu.com/ubuntu noble-security main\ndeb http://azure.archive.ubuntu.com/ubuntu noble-security/\n# deb http://azure.archive.ubuntu.com/ubuntu noble main\n";
        let expected = "deb [arch=amd64 signed-by=/usr/share/keyrings/ubuntu.gpg] https://archive.ubuntu.com/ubuntu noble main universe # Ubuntu\ndeb-src https://archive.ubuntu.com/ubuntu noble-updates main\ndeb http://azure.archive.ubuntu.com/ubuntu noble-security main\ndeb http://azure.archive.ubuntu.com/ubuntu noble-security/\n# deb http://azure.archive.ubuntu.com/ubuntu noble main\n";
        assert_eq!(rewrite(input, Format::Sources).unwrap(), (expected.into(), 2));
    }

    #[test]
    fn deb822_rewrites_only_uri_fields_and_continuations_of_regular_enabled_stanzas() {
        let input = "Types: deb deb-src\r\nURIs: http://azure.archive.ubuntu.com/ubuntu/\r\n  https://azure.archive.ubuntu.com/ubuntu https://other.example/ubuntu\r\nSuites: noble noble-updates noble-backports\r\nComponents: main restricted universe multiverse\r\nSigned-By: http://azure.archive.ubuntu.com/ubuntu\r\n\r\nTypes: deb\nURIs: http://azure.archive.ubuntu.com/ubuntu\nSuites: noble\n noble-security/\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n\nTypes: deb\nURIs: http://azure.archive.ubuntu.com/ubuntu\nSuites: noble\nEnabled: no\n";
        let expected = input
            .replacen("URIs: http://azure.archive.ubuntu.com/ubuntu/", &format!("URIs: {ARCHIVE}"), 1)
            .replacen("  https://azure.archive.ubuntu.com/ubuntu ", &format!("  {ARCHIVE} "), 1);
        assert_eq!(rewrite(input, Format::Deb822).unwrap(), (expected.clone(), 2));
        assert_eq!(rewrite(&expected, Format::Deb822).unwrap(), (expected, 0));
    }

    #[test]
    fn configuration_bounds_and_nul_are_rejected_without_rewriting() {
        assert!(rewrite(&"x".repeat(MAX_BYTES + 1), Format::Mirrors).is_err());
        assert!(rewrite("http://azure.archive.ubuntu.com/ubuntu\0", Format::Mirrors).is_err());
        let exact = "#".repeat(MAX_BYTES);
        assert_eq!(rewrite(&exact, Format::Deb822).unwrap(), (exact, 0));
    }

    #[test]
    fn platform_and_environment_guard_is_exact_and_precedes_all_io() {
        assert!(guard(&[], "linux", Some("true")).is_ok());
        for (os, github) in [
            ("macos", Some("true")),
            ("windows", Some("true")),
            ("linux", None),
            ("linux", Some("TRUE")),
            ("linux", Some("1")),
        ] {
            assert!(guard(&[], os, github).unwrap_err().contains("no APT files were accessed"));
        }
        assert_eq!(
            CONFIGS.map(|(path, _)| path),
            ["/etc/apt/apt-mirrors.txt", "/etc/apt/sources.list", "/etc/apt/sources.list.d/ubuntu.sources"]
        );
    }

    #[test]
    fn command_rejects_all_arguments_before_platform_or_file_access() {
        for args in [vec!["--help".into()], vec!["/tmp/config".into()], vec!["--path".into(), "/etc/apt/other".into()]]
        {
            assert_eq!(run(&args).unwrap_err(), "usage: cargo xtask browser-mirror");
        }
    }
}
