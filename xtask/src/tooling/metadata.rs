// SPDX-License-Identifier: AGPL-3.0-or-later
//! Repository metadata checks and the opt-in, read-only asset credit audit.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

type Result<T> = std::result::Result<T, String>;

pub fn check_repo(root: &Path) -> Result<()> {
    let read = |name: &str| fs::read_to_string(root.join(name)).map_err(|e| format!("{name}: {e}"));
    check_versions(&read("Cargo.toml")?, &read("package.json")?)?;
    check_inline_scripts(&read("web/index.html")?, &read("web/_headers")?)?;
    check_changelog(&read("CHANGELOG.md")?)?;
    println!("Repository versions, inline-script CSP hashes and release links agree.");
    Ok(())
}

fn check_versions(cargo: &str, package: &str) -> Result<()> {
    // Value::from_str parses a standalone TOML value in toml 1.x; this is
    // a complete document, including its workspace tables and comments.
    let cargo: toml::Value = toml::from_str(cargo).map_err(|e| format!("Cargo.toml: {e}"))?;
    let cargo = cargo
        .get("workspace")
        .and_then(|v| v.get("package"))
        .and_then(|v| v.get("version"))
        .and_then(toml::Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or("Cargo.toml lacks workspace.package.version")?;
    let package: Value = serde_json::from_str(package).map_err(|e| format!("package.json: {e}"))?;
    let package = package
        .get("version")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or("package.json lacks a string version")?;
    if cargo != package {
        return Err(format!("version mismatch: Cargo.toml {cargo}, package.json {package}"));
    }
    Ok(())
}

/// Only the effective global script directive can authorize inline scripts;
/// a hash in a comment, another header or a narrower path is no evidence.
fn script_hashes(headers: &str) -> Result<BTreeSet<String>> {
    let mut route = "";
    let mut policies = Vec::new();
    for line in headers.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            route = line.trim();
            continue;
        }
        if route == "/*"
            && let Some((name, value)) = line.trim().split_once(':')
            && name.eq_ignore_ascii_case("Content-Security-Policy")
        {
            policies.push(value.trim());
        }
    }
    if policies.len() != 1 {
        return Err("web/_headers must have one global Content-Security-Policy".into());
    }
    let mut directives = BTreeMap::new();
    for directive in policies[0].split(';') {
        let mut words = directive.split_whitespace();
        if let Some(name) = words.next()
            && directives.insert(name.to_ascii_lowercase(), words.collect::<Vec<_>>()).is_some()
        {
            return Err(format!("web/_headers repeats CSP directive {name}"));
        }
    }
    let words = ["script-src-elem", "script-src", "default-src"]
        .into_iter()
        .find_map(|name| directives.get(name))
        .ok_or("web/_headers has no script policy")?;
    Ok(words
        .iter()
        .filter_map(|word| word.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .filter(|word| word.starts_with("sha256-"))
        .map(str::to_owned)
        .collect())
}

fn tag_end(html: &str, start: usize) -> Result<usize> {
    let mut quote = None;
    for (i, byte) in html.as_bytes().iter().enumerate().skip(start) {
        match (quote, byte) {
            (Some(q), b) if q == *b => quote = None,
            (None, b'\'' | b'"') => quote = Some(*byte),
            (None, b'>') => return Ok(i),
            _ => {}
        }
    }
    Err("web/index.html contains an unterminated tag".into())
}

fn attributes(tag: &str) -> Result<BTreeSet<String>> {
    let bytes = tag.as_bytes();
    let mut i = 0;
    let mut names = BTreeSet::new();
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        if i == bytes.len() {
            break;
        }
        let first = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !b"=/".contains(&bytes[i]) {
            i += 1;
        }
        if i == first {
            return Err("web/index.html contains a malformed script attribute".into());
        }
        let name = tag[first..i].to_ascii_lowercase();
        if !names.insert(name.clone()) {
            return Err(format!("web/index.html repeats script attribute {name}"));
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if let Some(quote @ (b'\'' | b'"')) = bytes.get(i).copied() {
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            if i == bytes.len() {
                return Err("web/index.html contains an unterminated script attribute".into());
            }
            i += 1;
        } else {
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
        }
    }
    Ok(names)
}

fn check_inline_scripts(html: &str, headers: &str) -> Result<()> {
    let hashes = script_hashes(headers)?;
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find('<') {
        let start = cursor + relative;
        if lower[start..].starts_with("<!--") {
            cursor =
                start + 4 + lower[start + 4..].find("-->").ok_or("web/index.html contains an unclosed comment")? + 3;
            continue;
        }
        let name_start = start + 1;
        let name_end = lower[name_start..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .map_or(lower.len(), |n| name_start + n);
        let name = &lower[name_start..name_end];
        let open_end = tag_end(html, name_end)?;
        cursor = open_end + 1;
        // These are raw-text/RCDATA elements in the source shell: their
        // literal '<script>' text is not another executable element.
        if !matches!(
            name,
            "script" | "style" | "textarea" | "title" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript"
        ) {
            continue;
        }
        let closing = format!("</{name}");
        let mut search = cursor;
        let close_start = loop {
            let found = search
                + lower[search..]
                    .find(&closing)
                    .ok_or_else(|| format!("web/index.html contains an unclosed {name}"))?;
            let next = lower.as_bytes().get(found + closing.len());
            if next.is_some_and(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'>')) {
                break found;
            }
            search = found + closing.len();
        };
        if name == "script" && !attributes(&html[name_end..open_end])?.contains("src") {
            // HTML normalizes line endings before creating script text.
            let body = html[cursor..close_start].replace("\r\n", "\n").replace('\r', "\n");
            if !body.is_empty() {
                let hash = format!(
                    "sha256-{}",
                    base64::engine::general_purpose::STANDARD.encode(Sha256::digest(body.as_bytes()))
                );
                if !hashes.contains(&hash) {
                    return Err(format!(
                        "web/_headers lacks the CSP hash of an inline script in web/index.html ({hash})"
                    ));
                }
            }
        }
        cursor = tag_end(html, close_start + closing.len())? + 1;
    }
    Ok(())
}

fn release_version(version: &str) -> bool {
    let pieces: Vec<_> = version.split('.').collect();
    pieces.len() == 3
        && pieces
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && (p.len() == 1 || !p.starts_with('0')))
}

fn check_changelog(changelog: &str) -> Result<()> {
    let mut releases = BTreeSet::new();
    let mut references = BTreeMap::new();
    for line in changelog.lines() {
        if let Some(rest) = line.strip_prefix("## [") {
            let Some((version, _)) = rest.split_once(']') else { continue };
            if version.bytes().all(|b| b.is_ascii_digit() || b == b'.') && !release_version(version) {
                return Err(format!("CHANGELOG.md has an invalid release version {version}"));
            }
            if release_version(version) && !releases.insert(version) {
                return Err(format!("CHANGELOG.md repeats release {version}"));
            }
        }
        if let Some((version, url)) = line.strip_prefix('[').and_then(|s| s.split_once("]: "))
            && release_version(version)
            && references.insert(version, url).is_some()
        {
            return Err(format!("CHANGELOG.md repeats link [{version}]"));
        }
    }
    let prefix = "https://github.com/ael-dev3/Allthing/";
    for version in releases {
        let valid = references.get(version).is_some_and(|url| {
            *url == format!("{prefix}releases/tag/v{version}")
                || url
                    .strip_prefix(&format!("{prefix}compare/v"))
                    .and_then(|s| s.split_once("...v"))
                    .is_some_and(|(before, after)| release_version(before) && after == version)
        });
        if !valid {
            return Err(format!("CHANGELOG.md: [{version}] should link to the v{version} tag"));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    SevenTv(String),
    Ffz(String),
    EmojiGg(String),
}

impl Source {
    fn parse(url: &str) -> Result<Self> {
        if let Some(id) = url.strip_prefix("https://7tv.app/emotes/")
            && !id.is_empty()
            && id.len() <= 64
            && id.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return Ok(Self::SevenTv(id.into()));
        }
        if let Some(slug) = url.strip_prefix("https://www.frankerfacez.com/emoticon/")
            && let Some((id, rest)) = slug.split_once('-')
            && !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_digit())
            && safe_slug(rest)
        {
            return Ok(Self::Ffz(id.into()));
        }
        if let Some(slug) = url.strip_prefix("https://emoji.gg/emoji/")
            && safe_slug(slug)
        {
            return Ok(Self::EmojiGg(url.into()));
        }
        Err(format!("unsupported or malformed credit source {url}"))
    }

    fn url(&self) -> String {
        match self {
            Self::SevenTv(id) => format!("https://7tv.io/v3/emotes/{id}"),
            Self::Ffz(id) => format!("https://api.frankerfacez.com/v1/emote/{id}"),
            Self::EmojiGg(url) => url.clone(),
        }
    }

    fn terms(&self) -> &'static str {
        match self {
            Self::SevenTv(_) => "7TV public",
            Self::Ffz(_) => "FFZ public",
            Self::EmojiGg(_) => "emoji.gg free",
        }
    }
}

fn safe_slug(slug: &str) -> bool {
    !slug.is_empty() && slug.len() <= 256 && slug.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

#[derive(Debug)]
struct Credit {
    source: Source,
    by: String,
    codes: Vec<String>,
}

fn parse_credits(files: &[(&str, &str, bool)]) -> Result<BTreeMap<String, Credit>> {
    let mut found: BTreeMap<String, Credit> = BTreeMap::new();
    let mut codes: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (file, text, has_terms) in files {
        let mut local = BTreeSet::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if !line.starts_with("| `") {
                continue;
            }
            let fail = |why: &str| format!("{file}:{}: {why}", index + 1);
            let cells: Vec<_> = line.trim_matches('|').split('|').map(str::trim).collect();
            if cells.len() != if *has_terms { 6 } else { 5 } {
                return Err(fail("malformed credit row"));
            }
            let code = cells[0]
                .strip_prefix('`')
                .and_then(|s| s.strip_suffix('`'))
                .ok_or_else(|| fail("invalid shortcode"))?
                .trim_matches(':');
            if !code.starts_with("pepe_")
                || !code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(fail("invalid shortcode"));
            }
            if !local.insert(code.to_owned()) {
                return Err(fail("duplicate shortcode"));
            }
            let by = cells[3];
            if by.is_empty() {
                return Err(fail("missing maker"));
            }
            let url = cells[4];
            let source = Source::parse(url).map_err(|e| fail(&e))?;
            if (*has_terms && cells[5] != source.terms()) || (!has_terms && !matches!(source, Source::SevenTv(_))) {
                return Err(fail("credit terms are not allowed for this source"));
            }
            if let Some((before_url, before_by)) = codes.insert(code.into(), (url.into(), by.into()))
                && (before_url != url || before_by != by)
            {
                return Err(fail("shortcode has conflicting credits across packs"));
            }
            if let Some(seen) = found.get_mut(url) {
                if seen.by != by {
                    return Err(fail("source has conflicting makers"));
                }
                seen.codes.push(code.into());
            } else {
                found.insert(url.into(), Credit { source, by: by.into(), codes: vec![code.into()] });
            }
        }
        if local.is_empty() {
            return Err(format!("{file}: no credit rows found"));
        }
    }
    Ok(found)
}

fn same_name(left: &str, right: &str) -> bool {
    left.nfkc().collect::<String>().to_lowercase() == right.nfkc().collect::<String>().to_lowercase()
}

fn check_source(source: &Source, by: &str, status: u16, body: &str) -> Result<()> {
    let provider = match source {
        Source::SevenTv(_) => "7TV",
        Source::Ffz(_) => "FrankerFaceZ",
        Source::EmojiGg(_) => "emoji.gg",
    };
    if !(200..300).contains(&status) {
        return Err(format!("{provider} answers {status}"));
    }
    if matches!(source, Source::EmojiGg(_)) {
        return if body.contains("Do whatever you want with this emoji, have fun!") {
            Ok(())
        } else {
            Err("no longer under emoji.gg's free license".into())
        };
    }
    let body: Value = serde_json::from_str(body).map_err(|e| format!("invalid {provider} JSON: {e}"))?;
    let (owner, names) = match source {
        Source::SevenTv(_) => {
            if body.get("listed").and_then(Value::as_bool) != Some(true) {
                return Err("no longer listed on 7TV".into());
            }
            if body.get("flags").and_then(Value::as_u64) != Some(0) {
                return Err("flagged or missing flags on 7TV".into());
            }
            let owner = body.get("owner");
            if owner.is_none_or(Value::is_null) && by == "*DeletedUser" {
                return Ok(());
            }
            (owner, ["username", "display_name"])
        }
        Source::Ffz(_) => {
            let emote = body.get("emote").ok_or("FrankerFaceZ has no emote")?;
            if emote.get("public").and_then(Value::as_bool) != Some(true)
                || emote.get("hidden").and_then(Value::as_bool) != Some(false)
                || emote.get("status").and_then(Value::as_u64) != Some(1)
            {
                return Err("no longer public and approved on FrankerFaceZ".into());
            }
            (emote.get("owner"), ["name", "display_name"])
        }
        Source::EmojiGg(_) => unreachable!(),
    };
    if names
        .iter()
        .any(|name| owner.and_then(|o| o.get(name)).and_then(Value::as_str).is_some_and(|name| same_name(name, by)))
    {
        Ok(())
    } else {
        Err(format!("{provider} no longer names {by} as its owner"))
    }
}

/// Keep this opt-in audit bounded even when a credit host stalls or lies
/// about its response length. URLs come only from Source's HTTPS allowlist.
fn fetch(source: &Source) -> Result<(u16, String)> {
    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    let mut child = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--location",
            "--max-redirs",
            "3",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            "15",
            "--max-filesize",
            "2097152",
            "--user-agent",
            "allthing-credits-check (+https://github.com/ael-dev3/Allthing)",
            "--header",
            if matches!(source, Source::EmojiGg(_)) { "accept: text/html" } else { "accept: application/json" },
            "--write-out",
            "\n%{http_code}",
            "--url",
            &source.url(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;
    let mut output = Vec::new();
    let read = child.stdout.take().ok_or("curl has no output")?.take(MAX_BYTES + 5).read_to_end(&mut output);
    if read.is_err() || output.len() as u64 > MAX_BYTES + 4 {
        let _ = child.kill();
        let _ = child.wait();
        return Err("credit response is unreadable or exceeds 2 MiB".into());
    }
    let status = child.wait().map_err(|e| format!("could not wait for curl: {e}"))?;
    if !status.success() {
        return Err(format!("credit request failed ({status})"));
    }
    let output = String::from_utf8(output).map_err(|_| "credit response is not UTF-8")?;
    let (body, status) = output.rsplit_once('\n').ok_or("credit response has no HTTP status")?;
    let status = status.parse().map_err(|_| "credit response has an invalid HTTP status")?;
    Ok((status, body.into()))
}

pub fn check_emoji_sources(root: &Path) -> Result<()> {
    let names = ["web/emoji/CREDITS.md", "web/stickers/CREDITS.md"];
    let text = names.map(|name| fs::read_to_string(root.join(name)).map_err(|e| format!("{name}: {e}")));
    let [emoji, stickers] = text;
    let (emoji, stickers) = (emoji?, stickers?);
    let all = parse_credits(&[(names[0], &emoji, true), (names[1], &stickers, false)])?;
    let mut wrong = Vec::new();
    for (url, credit) in &all {
        let checked =
            fetch(&credit.source).and_then(|(status, body)| check_source(&credit.source, &credit.by, status, &body));
        if let Err(why) = checked {
            wrong.push(format!("{}: {why} ({url})", credit.codes.join(", ")));
        }
    }
    println!("{} of {} sources are as credited.", all.len() - wrong.len(), all.len());
    if wrong.is_empty() { Ok(()) } else { Err(wrong.join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(hash: &str) -> String {
        format!("/*\n  Content-Security-Policy: default-src 'none'; script-src 'self' '{hash}'\n")
    }
    fn hash(body: &str) -> String {
        format!("sha256-{}", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(body)))
    }

    #[test]
    fn versions_parse_actual_fields_and_reject_missing_or_mismatched_versions() {
        assert!(check_versions("[workspace.package]\nversion='0.5.1'", "{\"version\":\"0.5.1\"}").is_ok());
        for cargo in [
            "# version='0.5.1'",
            "[package]\nversion='0.5.1'",
            "[workspace.package]\nversion=5",
            "[workspace.package]\nversion='0.5.2'",
        ] {
            assert!(check_versions(cargo, "{\"version\":\"0.5.1\"}").is_err());
        }
        assert!(
            check_versions("[workspace.package]\nversion='0.5.1'", "{\"nested\":{\"version\":\"0.5.1\"}}").is_err()
        );
    }

    #[test]
    fn csp_hashes_multiline_scripts_and_does_not_stop_at_less_than_in_javascript() {
        let body = "\nif (1 < 2) console.log('ok');\n";
        assert!(
            check_inline_scripts(
                "<SCRIPT id='x'>\r\nif (1 < 2) console.log('ok');\r\n</SCRIPT>",
                &headers(&hash(body))
            )
            .is_ok()
        );
        assert!(check_inline_scripts("<script>changed()</script>", &headers(&hash(body))).is_err());
        assert_eq!(hash("alert(1)"), "sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI=");
    }

    #[test]
    fn csp_only_accepts_the_effective_script_policy() {
        let good = hash("run()");
        let html = "<script>run()</script>";
        for policy in [
            format!("# '{good}'\n/*\n  Content-Security-Policy: script-src 'self'"),
            format!("/*\n  Content-Security-Policy: script-src 'self'; img-src '{good}'"),
            format!("/assets/*\n  Content-Security-Policy: script-src '{good}'"),
            format!("/*\n  Content-Security-Policy: script-src '{good}'; script-src-elem 'none'"),
        ] {
            assert!(check_inline_scripts(html, &policy).is_err());
        }
    }

    #[test]
    fn csp_distinguishes_external_scripts_attributes_comments_and_raw_text() {
        let policy = headers(&hash("real()"));
        assert!(check_inline_scripts("<!-- <script>inert()</script> --><textarea><script>inert()</script></textarea><script SRC='file.js'>ignored()</script><script>real()</script>", &policy).is_ok());
        assert!(check_inline_scripts("<script data-src='file.js'>unhashed()</script>", &policy).is_err());
        assert!(check_inline_scripts("<script src='file.js' SRC='other.js'></script>", &policy).is_err());
        assert!(check_inline_scripts("<script>never closed", &policy).is_err());
    }

    #[test]
    fn changelog_requires_unambiguous_release_tag_links() {
        let good = "## [0.5.1] - today\n[0.5.1]: https://github.com/ael-dev3/Allthing/compare/v0.5.0...v0.5.1";
        assert!(check_changelog(good).is_ok());
        for url in [
            "https://github.com/ael-dev3/Allthing/commit/abc",
            "https://github.com/ael-dev3/Allthing/releases/tag/v0.5.2",
            "https://github.com/ael-dev3/Allthing/compare/v0..5...v0.5.1",
            "https://github.com/ael-dev3/Allthing/releases/tag/v0.5.1#different",
        ] {
            assert!(check_changelog(&format!("## [0.5.1]\n[0.5.1]: {url}")).is_err());
        }
        assert!(check_changelog(&format!("{good}\n[0.5.1]: https://example.com")).is_err());
        assert!(check_changelog(&format!("{good}\n## [0.5.1]")).is_err());
        assert!(
            check_changelog("## [00.5.1]\n[00.5.1]: https://github.com/ael-dev3/Allthing/releases/tag/v00.5.1")
                .is_err()
        );
    }

    fn row(code: &str, by: &str, url: &str, terms: &str) -> String {
        format!("| `:{code}:` | does a thing | original | {by} | {url} | {terms} |")
    }

    #[test]
    fn credit_sources_cannot_escape_the_https_provider_allowlist() {
        for url in [
            "http://7tv.app/emotes/ABC",
            "https://7tv.app.evil/emotes/ABC",
            "https://7tv.app@evil/emotes/ABC",
            "https://7tv.app/emotes/../../private",
            "https://emoji.gg/emoji/%2e%2e%2fsecret",
            "https://emoji.gg/emoji/foo?url=http://localhost",
            "file:///etc/passwd",
        ] {
            assert!(Source::parse(url).is_err(), "{url}");
        }
    }

    #[test]
    fn credit_terms_duplicates_and_cross_pack_disagreements_fail_closed() {
        let good = row("pepe_one", "Maker", "https://7tv.app/emotes/ABC", "7TV public");
        assert!(parse_credits(&[("emoji", &good, true)]).is_ok());
        assert!(parse_credits(&[("emoji", &format!("{good}\n{good}"), true)]).is_err());
        let conflict = row("pepe_two", "Other", "https://7tv.app/emotes/ABC", "7TV public");
        assert!(parse_credits(&[("emoji", &format!("{good}\n{conflict}"), true)]).is_err());
        assert!(parse_credits(&[("emoji", &good.replace("7TV public", "emoji.gg free"), true)]).is_err());
        assert!(parse_credits(&[("emoji", &good.replace("pepe_one", "../../secret"), true)]).is_err());
        assert!(
            parse_credits(&[
                ("emoji", &good, true),
                ("stickers", "| `pepe_one` | thing | original | Maker | https://7tv.app/emotes/XYZ |", false)
            ])
            .is_err()
        );
    }

    #[test]
    fn public_flags_owner_identity_and_unicode_normalization_are_preserved() {
        let source = Source::SevenTv("ABC".into());
        let good = r#"{"listed":true,"flags":0,"owner":{"username":"ｍＡＫＥＲ"}}"#;
        assert!(check_source(&source, "maker", 200, good).is_ok());
        for body in [
            good.replace("true", "false"),
            good.replace("\"flags\":0", "\"flags\":1"),
            good.replace("\"flags\":0", "\"flags\":null"),
            good.replace("ｍＡＫＥＲ", "other"),
        ] {
            assert!(check_source(&source, "maker", 200, &body).is_err());
        }
        assert!(check_source(&source, "maker", 503, good).is_err());
        assert!(check_source(&source, "*DeletedUser", 200, r#"{"listed":true,"flags":0,"owner":null}"#).is_ok());
        assert!(check_source(&source, "maker", 200, r#"{"listed":true,"flags":0,"owner":null}"#).is_err());
        assert!(same_name("e\u{301}", "É"));
    }

    #[test]
    fn ffz_and_emoji_license_checks_do_not_accept_restricted_or_missing_data() {
        let source = Source::Ffz("123".into());
        let good = r#"{"emote":{"public":true,"hidden":false,"status":1,"owner":{"name":"Maker"}}}"#;
        assert!(check_source(&source, "maker", 200, good).is_ok());
        for body in [
            "{}".into(),
            good.replace("\"hidden\":false", "\"hidden\":true"),
            good.replace("\"status\":1", "\"status\":0"),
        ] {
            assert!(check_source(&source, "maker", 200, &body).is_err());
        }
        let source = Source::EmojiGg("https://emoji.gg/emoji/example".into());
        assert!(check_source(&source, "unused", 200, "Do whatever you want with this emoji, have fun!").is_ok());
        assert!(check_source(&source, "unused", 200, "Only on platforms that don't charge").is_err());
    }

    #[test]
    fn current_repository_metadata_and_credit_tables_parse_without_network() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        check_repo(root).unwrap();
        let emoji = fs::read_to_string(root.join("web/emoji/CREDITS.md")).unwrap();
        let stickers = fs::read_to_string(root.join("web/stickers/CREDITS.md")).unwrap();
        let credits = parse_credits(&[("emoji", &emoji, true), ("stickers", &stickers, false)]).unwrap();
        assert_eq!(credits.len(), 219);
        assert_eq!(credits.values().map(|c| c.codes.len()).sum::<usize>(), 251);
    }
}
