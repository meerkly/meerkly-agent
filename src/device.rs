//! This machine's persistent identity.
//!
//! Deliberately not in `config.toml`: that file is hand-edited and copied between
//! machines, and a device id that travels with a config stops identifying a
//! device. It lives beside `status.json` in the state directory instead —
//! machine-written, never edited, and per-user, so two users running agents on
//! one box are two devices, which matches the service running as the user who
//! installed it.
//!
//! The id is public and cosmetic. It lets the dashboard show one row per machine
//! and remember the name the user gave it; it decides nothing about earnings.

use anyhow::{Context, Result};
use std::path::Path;

const PREFIX: &str = "dev_";

/// Read this machine's id, minting and persisting one if there is none yet.
///
/// Anything that is not a well-formed id — an empty file, a hand-edit gone
/// wrong, a partial write from a crash — is replaced rather than reported. The
/// id is cosmetic, and a daemon that will not start over it is a worse outcome
/// than a device that reappears under a new name.
pub fn load_or_create(path: &Path) -> Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let existing = existing.trim();
        if is_well_formed(existing) {
            return Ok(existing.to_owned());
        }
    }

    let id = format!("{PREFIX}{}", uuid::Uuid::new_v4().simple());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    // Temp file + rename, like the config: a crash mid-write must not leave a
    // half id that the next start then dutifully replaces with yet another.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format!("{id}\n"))
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("cannot move {} into place", tmp.display()))?;
    Ok(id)
}

/// `dev_` followed by at least one character, none of them whitespace.
fn is_well_formed(id: &str) -> bool {
    id.strip_prefix(PREFIX)
        .is_some_and(|rest| !rest.is_empty() && !rest.chars().any(char::is_whitespace))
}

/// Everything the agent tells the network about this machine. Assembled once
/// at startup and handed to the SDK; the SDK adds OS, architecture and its own
/// version itself.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub device_name: Option<String>,
    pub app: String,
}

impl DeviceIdentity {
    /// Load — or on first run, mint — this machine's identity.
    pub fn load(id_path: &Path) -> Result<Self> {
        Ok(Self {
            device_id: load_or_create(id_path)?,
            device_name: hostname(),
            app: app_identity(),
        })
    }
}

/// The machine's hostname, which seeds the dashboard label. `None` when the OS
/// will not say — the device is then shown by its id until the user names it.
pub fn hostname() -> Option<String> {
    gethostname::gethostname()
        .into_string()
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
}

/// What this host calls itself on the wire: "meerkly-agent/<version>". Distinct
/// from the SDK version, which the SDK reports on its own.
pub fn app_identity() -> String {
    concat!("meerkly-agent/", env!("CARGO_PKG_VERSION")).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_fresh_state_dir_mints_a_dev_prefixed_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device_id");

        let id = load_or_create(&path).unwrap();

        assert!(id.starts_with("dev_"), "got {id}");
        assert!(id.len() > "dev_".len() + 20, "too short to be a uuid: {id}");
        assert_eq!(
            fs::read_to_string(&path).unwrap().trim(),
            id,
            "must be persisted"
        );
    }

    #[test]
    fn the_id_is_stable_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device_id");

        let first = load_or_create(&path).unwrap();
        let second = load_or_create(&path).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn two_state_dirs_are_two_devices() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();

        let ida = load_or_create(&a.path().join("device_id")).unwrap();
        let idb = load_or_create(&b.path().join("device_id")).unwrap();

        assert_ne!(ida, idb);
    }

    /// A corrupt or empty file must be replaced, not fatal: a daemon that refuses
    /// to start over a cosmetic id is a worse failure than a renamed device.
    #[test]
    fn a_corrupt_file_is_replaced_rather_than_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device_id");

        for junk in ["", "   \n", "not-a-device-id", "dev_", "dev_has spaces"] {
            fs::write(&path, junk).unwrap();
            let id = load_or_create(&path).unwrap();
            assert!(id.starts_with("dev_"), "junk {junk:?} yielded {id}");
            assert!(!id.contains(' '));
            assert_eq!(fs::read_to_string(&path).unwrap().trim(), id);
        }
    }

    #[test]
    fn a_missing_parent_directory_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deeper").join("device_id");

        let id = load_or_create(&path).unwrap();

        assert!(path.exists());
        assert!(id.starts_with("dev_"));
    }

    /// Surrounding whitespace from a hand-edit or a stray newline must not become
    /// part of the identity.
    #[test]
    fn a_valid_id_with_surrounding_whitespace_is_kept_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device_id");
        fs::write(&path, "  dev_0123456789abcdef0123456789abcdef  \n").unwrap();

        let id = load_or_create(&path).unwrap();

        assert_eq!(id, "dev_0123456789abcdef0123456789abcdef");
    }
}
