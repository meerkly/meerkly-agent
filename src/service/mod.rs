//! Installing and controlling the background service.
//!
//! The model, identical on every platform: **the service runs as the user who
//! installed it**, so its configuration is that user's own file and needs no
//! elevation to edit. What differs is the machinery each OS offers, and the two
//! differences worth knowing are documented on the implementations:
//!
//! | | mechanism | starts |
//! |---|---|---|
//! | Linux | systemd system unit, `User=` the installing user | at boot |
//! | macOS | per-user LaunchAgent | at login |
//! | Windows | Scheduled Task at logon | at logon |
//!
//! macOS and Windows start later than Linux on purpose. Making either start at
//! boot means running as root or as a service account with a stored password —
//! which would put the configuration back behind elevation, the exact thing this
//! design avoids.

use anyhow::Result;

#[cfg(target_os = "macos")]
mod launchd;
#[cfg(target_os = "linux")]
mod systemd;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use launchd::Manager;
#[cfg(target_os = "linux")]
pub use systemd::Manager;
#[cfg(windows)]
pub use windows::Manager;

/// Whether the service manager currently has the agent running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// Installed and running.
    Running,
    /// Installed but not running.
    Stopped,
    /// No unit, agent or task registered.
    NotInstalled,
    /// This platform has no service manager we drive, or it could not be
    /// queried. `meerkly run` still works.
    Unknown,
}

impl ServiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::NotInstalled => "not-installed",
            Self::Unknown => "unknown",
        }
    }
}

/// What every platform backend implements. Kept deliberately small: the desktop
/// app and a person at a terminal both drive the agent through these five verbs
/// plus `meerkly status`.
pub trait ServiceManager {
    /// Register the service and start it.
    fn install(&self) -> Result<()>;
    /// Stop it and remove the registration. Leaves the configuration alone.
    fn uninstall(&self) -> Result<()>;
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn state(&self) -> ServiceState;
    /// How a person would do the same thing without this CLI — printed after an
    /// install so `systemctl` stays the primary interface it advertises itself
    /// to be.
    fn native_hint(&self) -> String;
}

/// The path to install into a service definition.
///
/// `current_exe` rather than a hardcoded `/usr/bin/meerkly`, so a unit written
/// from a Homebrew, cargo-install or tarball build points at the binary that
/// actually wrote it.
pub fn binary_path() -> Result<std::path::PathBuf> {
    std::env::current_exe().map_err(Into::into)
}

/// Run a command and turn a non-zero exit into an error carrying its stderr —
/// `systemctl`'s own diagnostics are far better than anything we could invent.
pub fn run_command(program: &str, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run {program}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        anyhow::bail!("{program} {} failed: {}", args.join(" "), detail.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
