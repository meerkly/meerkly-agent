//! The agent's configuration: a small TOML file, plus the environment and CLI
//! flags layered over it.
//!
//! Resolution order, strongest first: **CLI flag → environment → config file →
//! SDK defaults**. The environment names are the ones the rest of meerkly
//! already uses (`MEERKLY_PUBLISHER_ID`, `MEERKLY_GATEWAY_ADDRESSES`,
//! `MEERKLY_CA_CERT_PATH`), so a container that already sets them keeps working
//! with no config file at all.
//!
//! **Nothing here is a secret.** A publisher id is a public identifier: it ships
//! inside third-party apps and is handed out as a QR code, and says only which
//! account earns the bandwidth shared through it. So the file is written with
//! ordinary permissions and printed in full — no keychain, no masking.

use anyhow::{Context, Result};
use meerkly_sdk::{CaCert, ClientConfig};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const ENV_PUBLISHER_ID: &str = "MEERKLY_PUBLISHER_ID";
pub const ENV_GATEWAY_ADDRESSES: &str = "MEERKLY_GATEWAY_ADDRESSES";
pub const ENV_CA_CERT_PATH: &str = "MEERKLY_CA_CERT_PATH";

/// The on-disk file. Every field is optional so a half-written config still
/// loads and reports precisely what is missing.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The public `pub_…` id that earns the bandwidth this machine shares.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher_id: Option<String>,
    /// Gateways to try, in rotation. Unset means the production gateway.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_addresses: Option<Vec<String>>,
    /// Pin a self-signed development gateway's CA. Unset — the normal case —
    /// verifies the gateway against the public root CAs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_cert_path: Option<String>,
    /// `tracing` filter directive, e.g. `info` or `meerkly=debug`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

const HEADER: &str = "\
# meerkly agent configuration.
#
# Edit by hand or with `meerkly config set`; either way, restart the service for
# changes to take effect (`meerkly restart`).
#
# The publisher id is a public identifier, not a secret — it only says which
# account earns the bandwidth this machine shares.

";

impl Config {
    /// Read the file, or an empty config if it does not exist yet. A missing
    /// file is the normal state on a fresh install, not an error; unreadable or
    /// malformed content is.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("{} is not valid meerkly configuration", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// Write the file, creating its directory. Written to a temporary file and
    /// renamed, so a crash or a full disk can never leave a half-written config
    /// that stops the service from starting.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let body = toml::to_string_pretty(self).context("cannot serialise the configuration")?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, format!("{HEADER}{body}"))
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(())
    }
}

/// A config resolved from every layer and ready to run.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub publisher_id: String,
    pub gateway_addresses: Vec<String>,
    pub ca_cert_path: Option<String>,
    pub log: Option<String>,
    pub source: PathBuf,
}

/// How the environment layer is read.
///
/// Injectable so the resolution rules can be tested without touching process
/// state — otherwise every test would depend on whether the machine running it
/// happens to have the agent configured, which is exactly the case on a
/// developer's own box.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

impl Resolved {
    /// Apply the process environment over a loaded file.
    pub fn from_file(file: Config, source: PathBuf) -> Result<Self> {
        Self::from_parts(file, source, &|key| {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        })
    }

    /// Apply an environment over a loaded file, then require what the SDK cannot
    /// default.
    pub fn from_parts(file: Config, source: PathBuf, env: EnvLookup<'_>) -> Result<Self> {
        let publisher_id = env(ENV_PUBLISHER_ID)
            .or(file.publisher_id)
            .with_context(|| {
                format!(
                    "no publisher id configured.\n\n  Set one up with:\n    meerkly \
                     init\n\n  Create a publisher id at https://dashboard.meerkly.com — it is a \
                     public identifier, not a secret.\n  Config file: {}",
                    source.display()
                )
            })?;
        let publisher_id = validate_publisher_id(&publisher_id)?;

        let gateway_addresses = env(ENV_GATEWAY_ADDRESSES)
            .map(|raw| split_addresses(&raw))
            .or(file.gateway_addresses)
            .unwrap_or_else(|| {
                meerkly_sdk::DEFAULT_GATEWAY_ADDRESSES
                    .iter()
                    .map(|a| (*a).to_owned())
                    .collect()
            });

        Ok(Self {
            publisher_id,
            gateway_addresses,
            ca_cert_path: env(ENV_CA_CERT_PATH).or(file.ca_cert_path),
            log: file.log,
            source,
        })
    }

    /// Hand the resolved settings to the SDK.
    pub fn to_client_config(&self, identity: &crate::device::DeviceIdentity) -> ClientConfig {
        let mut config = ClientConfig::new(&self.publisher_id);
        config.gateway_addresses = self.gateway_addresses.clone();
        // A path pins that specific CA (a development gateway); no path is the
        // production path — verify against the public roots.
        config.ca_cert = CaCert::from_opts(None, self.ca_cert_path.clone());
        // Who this machine is. OS, architecture and the SDK's own version are
        // the SDK's to report; `sdk` stays unset so it names itself ("rust").
        config.device_id = Some(identity.device_id.clone());
        config.device_name = identity.device_name.clone();
        config.app = Some(identity.app.clone());
        config
    }
}

/// The canonical publisher-id rule, matching the dashboard's own pairing-code
/// parser: trim surrounding whitespace, require the `pub_` prefix, reject
/// anything else rather than guessing.
///
/// A **prefix** check, not a length check — ids are `pub_` plus 24 alphanumeric
/// characters today, but nothing promises that stays fixed.
pub fn validate_publisher_id(raw: &str) -> Result<String> {
    let candidate = raw.trim();
    let valid = candidate
        .strip_prefix("pub_")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()));
    anyhow::ensure!(
        valid,
        "{candidate:?} is not a publisher id — expected a `pub_` prefix followed by letters and \
         digits, as shown on https://dashboard.meerkly.com"
    );
    Ok(candidate.to_owned())
}

pub fn split_addresses(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity has to make it from the state file to the wire; this is the
    /// one seam between them. `sdk` is deliberately left unset — the SDK knows
    /// it is being driven from Rust and says so itself.
    fn test_identity() -> crate::device::DeviceIdentity {
        crate::device::DeviceIdentity {
            device_id: "dev_test".into(),
            device_name: None,
            app: "meerkly-agent/test".into(),
        }
    }

    #[test]
    fn the_client_config_carries_the_device_identity() {
        let resolved = Resolved {
            publisher_id: "pub_abc".into(),
            gateway_addresses: vec!["127.0.0.1:4443".into()],
            ca_cert_path: None,
            log: None,
            source: PathBuf::from("/nowhere/config.toml"),
        };
        let identity = crate::device::DeviceIdentity {
            device_id: "dev_1".into(),
            device_name: Some("prod-fra-01".into()),
            app: "meerkly-agent/9.9.9".into(),
        };

        let config = resolved.to_client_config(&identity);

        assert_eq!(config.device_id.as_deref(), Some("dev_1"));
        assert_eq!(config.device_name.as_deref(), Some("prod-fra-01"));
        assert_eq!(config.app.as_deref(), Some("meerkly-agent/9.9.9"));
        assert!(config.sdk.is_none());
        assert_eq!(config.publisher_id, "pub_abc");
    }

    /// The canonical rule, mirroring the dashboard's pairing-code parser.
    #[test]
    fn publisher_ids_are_trimmed_and_prefix_checked() {
        assert_eq!(
            validate_publisher_id("  pub_ABC123  ").unwrap(),
            "pub_ABC123"
        );
        for bad in [
            "", "pub_", "nope", "PUB_ABC", "pub-ABC", "pub_ABC!", "pub_AB C",
        ] {
            assert!(
                validate_publisher_id(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    /// A prefix check, not a length check — ids are 28 characters today, but
    /// nothing promises that stays fixed.
    #[test]
    fn publisher_ids_are_not_length_checked() {
        assert!(validate_publisher_id("pub_a").is_ok());
        assert!(validate_publisher_id(&format!("pub_{}", "a".repeat(64))).is_ok());
    }

    #[test]
    fn addresses_are_split_trimmed_and_compacted() {
        assert_eq!(
            split_addresses(" a:1 , b:2 ,, "),
            vec!["a:1".to_owned(), "b:2".to_owned()]
        );
        assert!(split_addresses("  ,  ").is_empty());
    }

    #[test]
    fn a_missing_file_is_an_empty_config_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    #[test]
    fn a_malformed_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "publisher_id = [1, 2]").unwrap();
        assert!(Config::load(&path).is_err());
    }

    /// An unknown key is a typo the user should hear about, not something to
    /// silently drop on the next save.
    #[test]
    fn unknown_keys_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "publsher_id = \"pub_a\"").unwrap();
        assert!(Config::load(&path).is_err());
    }

    #[test]
    fn a_saved_config_round_trips_and_keeps_its_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let config = Config {
            publisher_id: Some("pub_ABC123".to_owned()),
            gateway_addresses: Some(vec!["a:1".to_owned()]),
            ca_cert_path: None,
            log: Some("debug".to_owned()),
        };
        config.save(&path).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("# meerkly"));
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    /// Saving must not leave a `.tmp` behind — the rename is what makes the
    /// write atomic.
    #[test]
    fn saving_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::default().save(&path).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }

    #[test]
    fn an_unset_gateway_falls_back_to_the_production_default() {
        let resolved = Resolved::from_parts(
            Config {
                publisher_id: Some("pub_ABC123".to_owned()),
                ..Config::default()
            },
            "/tmp/config.toml".into(),
            &|_| None,
        )
        .unwrap();
        assert_eq!(resolved.gateway_addresses, vec!["gw.meerkly.com:4443"]);
        // No pinned CA means the production path: verify against public roots.
        assert!(resolved
            .to_client_config(&test_identity())
            .ca_cert
            .is_public_roots());
    }

    #[test]
    fn a_missing_publisher_id_explains_how_to_set_one() {
        let error = Resolved::from_parts(Config::default(), "/tmp/config.toml".into(), &|_| None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("meerkly init"), "{error}");
    }
    /// The environment beats the file; the file beats the default.
    #[test]
    fn the_environment_overrides_the_file() {
        let file = Config {
            publisher_id: Some("pub_FROMFILE".to_owned()),
            gateway_addresses: Some(vec!["file:1".to_owned()]),
            ..Config::default()
        };
        let resolved = Resolved::from_parts(file, "/tmp/config.toml".into(), &|key| match key {
            ENV_PUBLISHER_ID => Some("pub_FROMENV".to_owned()),
            ENV_GATEWAY_ADDRESSES => Some("env:2, env:3".to_owned()),
            _ => None,
        })
        .unwrap();
        assert_eq!(resolved.publisher_id, "pub_FROMENV");
        assert_eq!(resolved.gateway_addresses, vec!["env:2", "env:3"]);
    }
}
