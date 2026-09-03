//! The status file — the agent's answer to "how is it going?" without an IPC
//! channel.
//!
//! The desktop app drives the agent entirely through its CLI (the Docker split:
//! the daemon and the GUI are separate programs). Rather than open a socket or a
//! port for one read-only question, the running daemon publishes what it knows
//! to a small JSON file and `meerkly status` reads it back. Nothing but
//! `meerkly status --json` is a supported interface — the path and the shape of
//! the file are ours to change.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Idle,
    Connecting,
    Connected,
    Stopped,
}

impl From<meerkly_sdk::State> for ConnectionState {
    fn from(state: meerkly_sdk::State) -> Self {
        match state {
            meerkly_sdk::State::Idle => Self::Idle,
            meerkly_sdk::State::Connecting => Self::Connecting,
            meerkly_sdk::State::Connected { .. } => Self::Connected,
            meerkly_sdk::State::Stopped => Self::Stopped,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub state: ConnectionState,
    pub publisher_id: String,
    /// This machine's persistent id — the row it appears as in the dashboard.
    /// Stable across restarts and reconnects; see `device.rs`.
    pub device_id: String,
    /// The gateway currently serving this node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_id: Option<String>,
    /// The cluster-wide identity the gateway assigned to *this connection*.
    /// **Ephemeral** — every reconnect yields a new one. The persistent identity
    /// is `device_id`; this one is what keeps two concurrent connections from
    /// one machine distinguishable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    /// Seconds the daemon has been running — not seconds connected.
    pub uptime_seconds: u64,
    /// Why the last connection attempt failed, if the agent is not connected.
    /// Survives across retries so a misconfigured id is visible rather than
    /// scrolling past in the log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub pid: u32,
    pub version: String,
}

impl Status {
    /// Replace the status file atomically: a reader either sees the previous
    /// status or the new one, never a truncated file.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(())
    }

    /// Read the last published status, or `None` if the agent has never run.
    ///
    /// A file left behind by a killed daemon is stale by definition, so callers
    /// pair this with a live service-manager query rather than trusting it
    /// alone.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// Remove it on a clean shutdown, so a stopped agent does not report a
    /// months-old "connected".
    pub fn clear(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}
