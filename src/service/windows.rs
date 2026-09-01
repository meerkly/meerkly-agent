//! Windows: a Scheduled Task that runs at logon, as the logged-on user.
//!
//! Deliberately **not** a Windows service. A real service runs as a service
//! account or as a named user whose password Windows must store — either would
//! move the configuration out of the user's own `%APPDATA%` and back behind
//! elevation, which is exactly what this design avoids. A logon task needs
//! neither a password nor administrator rights.
//!
//! The cost is that it **starts at logon rather than at boot**. For a desktop
//! machine sharing spare bandwidth that is the honest scope; a headless Windows
//! server that needs boot-start wants a `--system` LocalService variant, which
//! does not exist yet.

use super::{binary_path, run_command, ServiceManager, ServiceState};
use anyhow::Result;

const TASK: &str = "meerkly";

#[derive(Default)]
pub struct Manager;

impl Manager {
    pub fn for_user(_user: &str) -> Result<Self> {
        anyhow::bail!(
            "--user does not apply on Windows: the logon task always runs as the user who \
             installs it. Run `meerkly service install` as that user."
        )
    }
}

impl ServiceManager for Manager {
    fn install(&self) -> Result<()> {
        let exe = binary_path()?;
        let command = format!("\"{}\" run", exe.display());
        run_command(
            "schtasks",
            &[
                "/Create", "/F", "/TN", TASK, "/SC", "ONLOGON", "/TR", &command, "/RL", "LIMITED",
            ],
        )?;
        run_command("schtasks", &["/Run", "/TN", TASK])?;
        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        let _ = run_command("schtasks", &["/End", "/TN", TASK]);
        run_command("schtasks", &["/Delete", "/F", "/TN", TASK]).map(|_| ())
    }

    fn start(&self) -> Result<()> {
        run_command("schtasks", &["/Run", "/TN", TASK]).map(|_| ())
    }

    fn stop(&self) -> Result<()> {
        run_command("schtasks", &["/End", "/TN", TASK]).map(|_| ())
    }

    fn state(&self) -> ServiceState {
        match run_command("schtasks", &["/Query", "/TN", TASK, "/FO", "LIST"]) {
            // "Status: Running" while the task's program is up; "Ready" when it
            // is registered but not currently executing.
            Ok(out) if out.contains("Running") => ServiceState::Running,
            Ok(_) => ServiceState::Stopped,
            Err(_) => ServiceState::NotInstalled,
        }
    }

    fn native_hint(&self) -> String {
        format!("schtasks /Run /TN {TASK}    |    schtasks /End /TN {TASK}")
    }
}
