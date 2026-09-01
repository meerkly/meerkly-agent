//! The command surface.
//!
//! Two audiences, one set of commands: a person at a terminal, and the desktop
//! app, which drives the agent entirely through this CLI. `systemctl` remains
//! the documented way to start and stop on Linux — `meerkly start`/`stop` exist
//! so the desktop app has one command shape that works on all three platforms.

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "meerkly",
    version,
    about = "Share this machine's connection as a meerkly proxy exit node.",
    long_about = "The meerkly agent runs in the background and shares this machine's connection \
                  as a proxy exit node, earning per GB shared.\n\nFirst run:\n  meerkly config \
                  set publisher-id pub_…\n  sudo meerkly service install\n\nGet a publisher id at \
                  https://dashboard.meerkly.com"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run in the foreground. This is what the service executes.
    Run {
        /// Use this publisher id instead of the configured one.
        #[arg(long, value_name = "pub_…")]
        publisher_id: Option<String>,
        /// Gateways to try, comma-separated `host:port`. Defaults to production.
        #[arg(long, value_name = "HOST:PORT")]
        gateway: Option<String>,
    },

    /// Install, remove or inspect the background service.
    #[command(subcommand)]
    Service(ServiceCommand),

    /// Start the background service.
    Start,
    /// Stop the background service.
    Stop,
    /// Restart the background service — how configuration changes take effect.
    Restart,

    /// Show whether the agent is running and connected.
    Status {
        /// Machine-readable output. This is the desktop app's interface.
        #[arg(long)]
        json: bool,
    },

    /// Read and write the configuration file.
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Subcommand, Debug)]
pub enum ServiceCommand {
    /// Register the service and start it. Needs `sudo` on Linux.
    Install {
        /// Run the service as this user instead of the one invoking sudo.
        #[arg(long, value_name = "NAME")]
        user: Option<String>,
    },
    /// Stop and deregister the service. Leaves the configuration in place.
    Uninstall,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Set a value: `publisher-id`, `gateway-addresses`, `ca-cert-path`, `log`.
    Set {
        key: String,
        value: String,
        /// Restart the service so the change takes effect immediately.
        #[arg(long)]
        restart: bool,
    },
    /// Print one value.
    Get { key: String },
    /// Print the whole configuration file.
    Show,
    /// Print the path to the configuration file.
    Path,
    /// Remove a value, falling back to its default.
    Unset {
        key: String,
        #[arg(long)]
        restart: bool,
    },
}
