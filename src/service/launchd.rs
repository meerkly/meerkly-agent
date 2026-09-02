//! macOS: a per-user LaunchAgent.
//!
//! A LaunchAgent is already user-scoped, so this is the platform where "runs as
//! the installing user" needs no arranging at all. The trade-off is that it
//! starts **at login rather than at boot**; the alternative, a LaunchDaemon,
//! runs as root and would put the configuration back behind `sudo`.
//!
//! `brew services start meerkly` produces an equivalent agent from the formula's
//! own `service` block; this exists for tarball and `cargo install` users.

use super::{binary_path, run_command, ServiceManager, ServiceState};
use anyhow::{Context, Result};
use std::path::PathBuf;

// `agent`, not `meerkly`: the desktop app takes `com.meerkly.app`, and two
// products from one vendor need distinct reverse-DNS labels — launchd keys its
// whole namespace on this, so a collision would have one silently displace the
// other.
const LABEL: &str = "com.meerkly.agent";

pub struct Manager {
    plist_path: PathBuf,
}

impl Default for Manager {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
        Self {
            plist_path: PathBuf::from(home)
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
        }
    }
}

impl Manager {
    pub fn for_user(_user: &str) -> Result<Self> {
        // A LaunchAgent always belongs to whoever loads it, so there is nobody
        // else to install for. Say so rather than silently ignoring the flag.
        anyhow::bail!(
            "--user does not apply on macOS: a LaunchAgent always runs as the user who installs \
             it. Run `meerkly service install` as that user."
        )
    }

    fn plist_text(exe: &std::path::Path, log_dir: &std::path::Path) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- Managed by meerkly. Regenerate with `meerkly service install`. -->
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>run</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>EnvironmentVariables</key>
  <dict>
    <key>MEERKLY_LOG</key><string>info</string>
  </dict>
  <key>StandardOutPath</key><string>{log}/meerkly.log</string>
  <key>StandardErrorPath</key><string>{log}/meerkly.log</string>
</dict>
</plist>
"#,
            exe = exe.display(),
            log = log_dir.display()
        )
    }
}

impl ServiceManager for Manager {
    fn install(&self) -> Result<()> {
        let exe = binary_path()?;
        let log_dir = crate::paths::state_dir()?;
        std::fs::create_dir_all(&log_dir)?;
        if let Some(dir) = self.plist_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.plist_path, Manager::plist_text(&exe, &log_dir))
            .with_context(|| format!("cannot write {}", self.plist_path.display()))?;

        // Reloading a changed plist requires unloading first; a fresh install has
        // nothing to unload, so that failure is expected and ignored.
        let _ = run_command("launchctl", &["unload", &self.plist_path.to_string_lossy()]);
        run_command(
            "launchctl",
            &["load", "-w", &self.plist_path.to_string_lossy()],
        )?;
        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        let _ = run_command(
            "launchctl",
            &["unload", "-w", &self.plist_path.to_string_lossy()],
        );
        if self.plist_path.exists() {
            std::fs::remove_file(&self.plist_path)
                .with_context(|| format!("cannot remove {}", self.plist_path.display()))?;
        }
        Ok(())
    }

    fn start(&self) -> Result<()> {
        run_command("launchctl", &["start", LABEL]).map(|_| ())
    }

    fn stop(&self) -> Result<()> {
        run_command("launchctl", &["stop", LABEL]).map(|_| ())
    }

    fn state(&self) -> ServiceState {
        if !self.plist_path.exists() {
            return ServiceState::NotInstalled;
        }
        match run_command("launchctl", &["list"]) {
            Ok(list) => list
                .lines()
                .find(|line| line.ends_with(LABEL))
                .map(|line| {
                    // "PID  Status  Label" — a numeric pid means it is up.
                    let running = line
                        .split_whitespace()
                        .next()
                        .is_some_and(|pid| pid.parse::<u32>().is_ok());
                    if running {
                        ServiceState::Running
                    } else {
                        ServiceState::Stopped
                    }
                })
                .unwrap_or(ServiceState::Stopped),
            Err(_) => ServiceState::Unknown,
        }
    }

    fn native_hint(&self) -> String {
        format!("launchctl start|stop {LABEL}    (or: brew services start|stop meerkly)")
    }
}
