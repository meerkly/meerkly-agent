//! Where the agent keeps its files.
//!
//! The service runs **as the user who installed it**, so every path here lives in
//! that user's own home. That is the whole point: `meerkly config set` and the
//! desktop app must be able to write the configuration without asking for
//! elevation, while the service itself still starts at boot.
//!
//! Deliberately **not** `~/.meerkly/` — the installer for the older meerkly
//! browser worker already owns that directory, and both can be installed on one
//! machine.

use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use std::path::PathBuf;

/// Environment override for the configuration file, for tests, containers and
/// anyone running more than one agent on a box.
pub const CONFIG_ENV: &str = "MEERKLY_CONFIG";

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("com", "meerkly", "meerkly").ok_or_else(|| {
        anyhow!("cannot determine this user's home directory; set {CONFIG_ENV} to a file path")
    })
}

/// The configuration file.
///
/// `$MEERKLY_CONFIG` wins if set. Otherwise `~/.config/meerkly/config.toml` on
/// Linux and macOS — macOS gets the XDG path rather than
/// `~/Library/Application Support` because this is a console tool an
/// administrator edits by hand — and `%APPDATA%\meerkly\config.toml` on Windows.
pub fn config_file() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_ENV) {
        return Ok(PathBuf::from(path));
    }
    Ok(config_dir()?.join("config.toml"))
}

pub fn config_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(project_dirs()?.config_dir().to_path_buf())
    }
    #[cfg(not(windows))]
    {
        // ProjectDirs points macOS at ~/Library/Application Support; a CLI's
        // config belongs somewhere a person can find and edit.
        let _ = project_dirs()?;
        let home = home_dir()?;
        Ok(home.join(".config").join("meerkly"))
    }
}

/// Machine-written state: the status file the daemon publishes and `meerkly
/// status` reads back. Never edited by hand.
pub fn state_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(project_dirs()?.data_local_dir().to_path_buf())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = project_dirs()?;
        Ok(home_dir()?
            .join("Library")
            .join("Application Support")
            .join("meerkly"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = project_dirs()?;
        Ok(home_dir()?.join(".local").join("state").join("meerkly"))
    }
}

pub fn status_file() -> Result<PathBuf> {
    Ok(state_dir()?.join("status.json"))
}

#[cfg(not(windows))]
fn home_dir() -> Result<PathBuf> {
    // `$HOME` first so a systemd unit's `Environment=HOME=…` is authoritative:
    // the service runs as the installing user and must resolve to that user's
    // home even though systemd starts it from an empty environment.
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    Ok(directories::UserDirs::new()
        .ok_or_else(|| anyhow!("cannot determine this user's home directory; set $HOME"))?
        .home_dir()
        .to_path_buf())
}
