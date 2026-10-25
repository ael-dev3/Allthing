// SPDX-License-Identifier: AGPL-3.0-or-later
//! Generate the existing Ed25519 attestation seed for a pipe to Wrangler.
//! Only its public key is written to the repository or diagnostics.

use ed25519_dalek::SigningKey;
use std::fs::{self, OpenOptions};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zeroize::{Zeroize, Zeroizing};

pub type Result<T> = std::result::Result<T, String>;
const FILE: &str = "spacetime/spacetimedb/src/attest_key.rs";
const DECLARATION: &str = "pub const ATTEST_PUBLIC_KEY: [u8; 32] = [";
static NEXT: AtomicU64 = AtomicU64::new(0);

fn allowed(args: &[String], terminal: bool) -> Result<()> {
    if !args.is_empty() {
        return Err("usage: cargo xtask attest-key | npx wrangler secret put ATTEST_KEY".into());
    }
    if terminal {
        return Err(
            "Not printing the private key to a terminal: pipe this into `npx wrangler secret put ATTEST_KEY`.".into()
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 15)] as char);
    }
    out
}

fn replace_public(source: &str, public: &[u8; 32]) -> Result<String> {
    let mut positions = source.match_indices(DECLARATION);
    let start =
        positions.next().map(|(start, _)| start).ok_or("no ATTEST_PUBLIC_KEY declaration in attestation module")?;
    if positions.next().is_some() {
        return Err("more than one ATTEST_PUBLIC_KEY declaration in attestation module".into());
    }
    let body = start + DECLARATION.len();
    let end = body + source[body..].find("];").ok_or("unterminated ATTEST_PUBLIC_KEY declaration")? + 2;
    let existing = &source[body..end - 2];
    // Refuse to replace expressions or comments; the pinned key is a plain
    // array, so a changed template cannot accidentally overwrite other code.
    let literals: Vec<&str> = existing.split(',').map(str::trim).filter(|value| !value.is_empty()).collect();
    if literals.len() != 32
        || !literals.iter().all(|value| {
            value
                .strip_prefix("0x")
                .is_some_and(|digits| digits.len() == 2 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        })
    {
        return Err("ATTEST_PUBLIC_KEY must contain exactly 32 hexadecimal byte literals".into());
    }
    let mut next = String::with_capacity(source.len());
    next.push_str(&source[..body]);
    next.push('\n');
    for bytes in public.chunks(8) {
        next.push_str("    ");
        for byte in bytes {
            next.push_str(&format!("0x{byte:02x}, "));
        }
        next.pop();
        next.push('\n');
    }
    next.push_str("];");
    next.push_str(&source[end..]);
    Ok(next)
}

struct Lock(PathBuf);
impl Lock {
    fn acquire(path: &Path) -> Result<Self> {
        let path = path.with_extension("rs.lock");
        OpenOptions::new().create_new(true).write(true).open(&path).map_err(|error| {
            format!("could not lock attestation key source (another generator may be running): {error}")
        })?;
        Ok(Self(path))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct Pending(PathBuf);
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn write_public(path: &Path, original: &str, next: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("attestation key source must be a regular file".into());
    }
    let pending_path =
        path.with_extension(format!("rs.{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let mut file =
        OpenOptions::new().create_new(true).write(true).open(&pending_path).map_err(|error| error.to_string())?;
    let pending = Pending(pending_path);
    file.write_all(next.as_bytes()).map_err(|error| error.to_string())?;
    file.set_permissions(metadata.permissions()).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    if fs::read_to_string(path).map_err(|error| error.to_string())? != original {
        return Err("attestation key source changed during generation; no private key was printed".into());
    }
    fs::rename(&pending.0, path).map_err(|error| error.to_string())
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    // This guard precedes reading, locking, generating or changing anything.
    allowed(args, std::io::stdout().is_terminal())?;
    let path = root.join(FILE);
    let _lock = Lock::acquire(&path)?;
    let original = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    // Validate the template before requesting any entropy.
    replace_public(&original, &[0; 32])?;
    let mut seed = Zeroizing::new([0; 32]);
    getrandom::fill(seed.as_mut()).map_err(|_| "could not read operating-system randomness")?;
    let key = SigningKey::from_bytes(&seed);
    let public = key.verifying_key().to_bytes();
    let next = replace_public(&original, &public)?;
    write_public(&path, &original, &next)?;
    eprintln!(
        "ATTEST_KEY public half: {}\nwritten to {FILE}; run cargo fmt in spacetime/spacetimedb, commit it and republish the module.",
        hex(&public)
    );
    let mut secret = Zeroizing::new(hex(seed.as_ref()));
    let output = std::io::stdout().lock().write_all(secret.as_bytes());
    seed.zeroize();
    secret.zeroize();
    output.map_err(|_| {
        "public key was updated but the private key could not be delivered to the pipe; generate again".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> String {
        format!("// before\n{DECLARATION}{}];\n// after\n", "0x00, ".repeat(32))
    }

    #[test]
    fn terminal_and_extra_arguments_are_refused_before_any_work() {
        assert!(allowed(&[], true).unwrap_err().contains("Not printing"));
        assert!(allowed(&["--show-secret".into()], false).is_err());
        assert!(allowed(&[], false).is_ok());
    }

    #[test]
    fn ed25519_seed_to_public_key_matches_rfc8032_test_vector_one() {
        let seed = [
            0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4, 0x44, 0x49,
            0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
        ];
        let key = SigningKey::from_bytes(&seed);
        assert_eq!(
            hex(&key.verifying_key().to_bytes()),
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        );
    }

    #[test]
    fn only_public_array_changes_and_repeated_public_pin_is_idempotent() {
        let original = template();
        let next = replace_public(&original, &[0x7f; 32]).unwrap();
        assert!(next.starts_with("// before\n"));
        assert!(next.ends_with("// after\n"));
        assert_eq!(next.matches("0x7f").count(), 32);
        assert_eq!(replace_public(&next, &[0x7f; 32]).unwrap(), next);
        assert!(!next.contains("PRIVATE"));
    }

    #[test]
    fn unexpected_templates_are_not_overwritten() {
        for source in [
            "nothing".into(),
            format!("{DECLARATION}0x00];"),
            format!("{}{}", template(), template()),
            format!("{DECLARATION}make_key()];"),
        ] {
            assert!(replace_public(&source, &[0; 32]).is_err());
        }
    }

    #[test]
    fn public_source_update_is_atomic_and_detects_concurrent_changes() {
        let root = std::env::temp_dir().join(format!(
            "allthing-attest-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("attest_key.rs");
        let original = template();
        fs::write(&path, &original).unwrap();
        let lock = Lock::acquire(&path).unwrap();
        assert!(Lock::acquire(&path).is_err());
        let next = replace_public(&original, &[7; 32]).unwrap();
        write_public(&path, &original, &next).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), next);
        assert!(write_public(&path, &original, "must not overwrite").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), next);
        drop(lock);
        assert!(!path.with_extension("rs.lock").exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
