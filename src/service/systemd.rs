//! Linux: a systemd **system** unit that runs as the installing user.
//!
//! A system unit rather than `systemctl --user` because a user unit stops when
//! the user logs out unless lingering is enabled — not what an always-on exit
//! node should do. `User=` and `Environment=HOME=` give us the other half: the
//! service still resolves the installing user's `~/.config/meerkly/config.toml`,
//! so editing it never needs sudo even though installing did.

use super::{binary_path, run_command, ServiceManager, ServiceState};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const UNIT: &str = "meerkly.service";
const UNIT_DIR: &str = "/etc/systemd/system";

pub struct Manager {
    unit_path: PathBuf,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            unit_path: Path::new(UNIT_DIR).join(UNIT),
        }
    }
}

impl Manager {
    /// Who the service should run as: the human behind a `sudo`, not root.
    ///
    /// `$SUDO_USER` is the whole question — installing as root and leaving
    /// `User=root` would put the config in `/root` where neither the desktop app
    /// nor the person who installed it can reach it.
    fn target_user() -> Result<(String, crate::paths::PasswdEntry)> {
        let user = std::env::var("SUDO_USER")
            .ok()
            .filter(|u| !u.is_empty() && u != "root")
            .or_else(|| std::env::var("USER").ok())
            .filter(|u| !u.is_empty())
            .context("cannot tell which user the service should run as; pass `--user <name>`")?;
        let entry = crate::paths::passwd_entry(&user)?;
        Ok((user, entry))
    }

    pub fn for_user(user: &str) -> Result<Self> {
        // Validated eagerly so a typo fails before we write a unit file.
        home_of(user)?;
        Ok(Self::default())
    }

    fn unit_text(user: &str, home: &str, exe: &Path) -> String {
        format!(
            "\
# Managed by meerkly. Regenerate with `sudo meerkly service install`.
[Unit]
Description=meerkly agent — shares this machine's connection as a proxy exit node
Documentation=https://meerkly.com
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User={user}
# The agent resolves its configuration under $HOME; systemd starts units with an
# empty environment, so this is what points it at the installing user's file
# rather than root's.
Environment=HOME={home}
Environment=MEERKLY_LOG=info
ExecStart={exe} run
Restart=always
RestartSec=5

# The agent needs outbound network and its own config; nothing else.
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths={home}/.config/meerkly {home}/.local/state/meerkly
ProtectControlGroups=yes
ProtectKernelTunables=yes
RestrictSUIDSGID=yes

[Install]
WantedBy=multi-user.target
",
            exe = exe.display()
        )
    }
}

fn home_of(user: &str) -> Result<String> {
    crate::paths::passwd_entry(user).map(|e| e.home)
}

impl ServiceManager for Manager {
    fn install(&self) -> Result<()> {
        let (user, entry) = Manager::target_user()?;
        let home = entry.home.clone();

        // The unit below lists these in ReadWritePaths=, and systemd will not
        // start a unit whose ReadWritePaths= do not exist — it fails namespace
        // setup with status 226 before ExecStart is ever reached. The state
        // directory is normally created by the daemon on first run, which is
        // exactly the run that cannot happen. So make both here, as the user's.
        for rel in [".config/meerkly", ".local/state/meerkly"] {
            crate::paths::create_dir_for(&Path::new(&home).join(rel), &entry, &user)?;
        }

        let exe = binary_path()?;
        let text = Manager::unit_text(&user, &home, &exe);

        std::fs::write(&self.unit_path, text).with_context(|| {
            format!(
                "cannot write {} — installing the service needs root, try `sudo meerkly service \
                 install`",
                self.unit_path.display()
            )
        })?;

        run_command("systemctl", &["daemon-reload"])?;
        run_command("systemctl", &["enable", "--now", UNIT])?;
        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        // Best-effort: a half-installed service must still be removable.
        let _ = run_command("systemctl", &["disable", "--now", UNIT]);
        if self.unit_path.exists() {
            std::fs::remove_file(&self.unit_path).with_context(|| {
                format!(
                    "cannot remove {} — try `sudo meerkly service uninstall`",
                    self.unit_path.display()
                )
            })?;
        }
        let _ = run_command("systemctl", &["daemon-reload"]);
        Ok(())
    }

    fn start(&self) -> Result<()> {
        run_command("systemctl", &["start", UNIT]).map(|_| ())
    }

    fn stop(&self) -> Result<()> {
        run_command("systemctl", &["stop", UNIT]).map(|_| ())
    }

    fn state(&self) -> ServiceState {
        if !self.unit_path.exists() {
            return ServiceState::NotInstalled;
        }
        // `is-active` exits non-zero when inactive, so the error case is a real
        // answer here rather than a failure.
        match run_command("systemctl", &["is-active", UNIT]) {
            Ok(out) if out.trim() == "active" => ServiceState::Running,
            _ => ServiceState::Stopped,
        }
    }

    fn native_hint(&self) -> String {
        format!("systemctl start|stop|status meerkly    (logs: journalctl -u {UNIT} -f)")
    }
}
