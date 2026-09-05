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

use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

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

/// The daemon's own log files: one per day, seven kept. Beside status.json so
/// that everything the daemon writes lives in one place, and so the systemd
/// unit's ReadWritePaths= already covers it.
pub fn log_dir() -> Result<PathBuf> {
    Ok(state_dir()?.join("logs"))
}

/// This machine's persistent device id. State, not config: never hand-edited,
/// never copied to another machine — see `device.rs` for why that matters.
pub fn device_id_file() -> Result<PathBuf> {
    Ok(state_dir()?.join("device_id"))
}

#[cfg(not(windows))]
fn home_dir() -> Result<PathBuf> {
    // Under `sudo`, the human's home — not root's. `sudo meerkly init` is the
    // documented first run on Linux, and the service it installs runs as that
    // human; a config written to /root is one the service never finds.
    if let Some((_, entry)) = sudo_user() {
        return Ok(PathBuf::from(entry.home));
    }
    // Then `$HOME`, so a systemd unit's `Environment=HOME=…` is authoritative:
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

/// The fields of a passwd entry the agent needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub uid: u32,
    pub gid: u32,
    pub home: String,
}

/// One `name:x:uid:gid:gecos:home:shell` line. `None` for anything malformed,
/// including a user with no home directory — there is nowhere to put a config.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_passwd_line(line: &str) -> Option<PasswdEntry> {
    let fields: Vec<&str> = line.trim().split(':').collect();
    if fields.len() < 6 {
        return None;
    }
    let uid = fields[2].parse().ok()?;
    let gid = fields[3].parse().ok()?;
    let home = fields[5];
    if home.is_empty() {
        return None;
    }
    Some(PasswdEntry {
        uid,
        gid,
        home: home.to_owned(),
    })
}

/// Look a user up. `getent` rather than reading /etc/passwd, so LDAP and SSSD
/// users resolve too.
#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn passwd_entry(user: &str) -> Result<PasswdEntry> {
    let out = std::process::Command::new("getent")
        .args(["passwd", user])
        .output()
        .context("cannot run getent")?;
    anyhow::ensure!(out.status.success(), "no such user: {user}");
    parse_passwd_line(&String::from_utf8_lossy(&out.stdout))
        .with_context(|| format!("user {user} has no home directory"))
}

/// The person behind a `sudo`, when the agent is running as root on their
/// behalf. `None` when not under sudo, when sudo's target is not root, or when
/// the invoker is root already. Linux only: on macOS the service is a per-user
/// LaunchAgent and sudo is never the right tool.
#[cfg(target_os = "linux")]
pub fn sudo_user() -> Option<(String, PasswdEntry)> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        return None;
    }
    let user = std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root")?;
    let entry = passwd_entry(&user).ok()?;
    Some((user, entry))
}

#[cfg(not(target_os = "linux"))]
pub fn sudo_user() -> Option<(String, PasswdEntry)> {
    None
}

/// Hand a file the agent has just written as root back to the sudo user, and
/// any directory it had to create on the way — `~/.config` may not have existed
/// yet, and a root-owned `~/.config` is a worse surprise than a root-owned
/// config file. A no-op when not running under sudo.
pub fn adopt_for_sudo_user(path: &Path) -> Result<()> {
    let Some((user, entry)) = sudo_user() else {
        return Ok(());
    };
    give_to(path, &entry, &user)
}

/// Create `dir` (and its parents) so that the whole chain below `entry.home`
/// belongs to that user, even when this process is root. The service unit
/// names directories in `ReadWritePaths=`, and systemd refuses to start a unit
/// whose `ReadWritePaths=` do not exist — so they have to be made before the
/// daemon that would otherwise make them can run.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn create_dir_for(dir: &Path, entry: &PasswdEntry, user: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    give_to(dir, entry, user)
}

/// chown `path` to the user, then walk up towards their home chowning every
/// directory that root left behind. Stops at the home itself, which is theirs
/// already and must not be touched.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn give_to(path: &Path, entry: &PasswdEntry, user: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{chown, MetadataExt};
        let (uid, gid) = (Some(entry.uid), Some(entry.gid));
        chown(path, uid, gid)
            .with_context(|| format!("cannot give {} to {user}", path.display()))?;
        let home = Path::new(&entry.home);
        let mut dir = path.parent();
        while let Some(d) = dir {
            if d == home || !d.starts_with(home) {
                break;
            }
            if std::fs::metadata(d).map(|m| m.uid() == 0).unwrap_or(false) {
                chown(d, uid, gid)
                    .with_context(|| format!("cannot give {} to {user}", d.display()))?;
            }
            dir = d.parent();
        }
    }
    #[cfg(not(unix))]
    let _ = (path, entry, user);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_passwd_line_yields_uid_gid_and_home() {
        let e = parse_passwd_line("rasmus:x:1000:1000:Rasmus,,,:/home/rasmus:/bin/zsh\n").unwrap();
        assert_eq!(
            e,
            PasswdEntry {
                uid: 1000,
                gid: 1000,
                home: "/home/rasmus".into()
            }
        );
    }

    #[test]
    fn a_user_without_a_home_is_not_somewhere_to_put_a_config() {
        assert_eq!(
            parse_passwd_line("nobody:x:65534:65534:nobody::/usr/sbin/nologin"),
            None
        );
    }

    #[test]
    fn junk_is_rejected_rather_than_guessed_at() {
        assert_eq!(parse_passwd_line(""), None);
        assert_eq!(parse_passwd_line("only:two"), None);
        assert_eq!(parse_passwd_line("u:x:notanumber:0::/home/u:/bin/sh"), None);
    }
}
