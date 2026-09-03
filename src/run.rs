//! The daemon. This is what the service unit executes.
//!
//! Structurally the same as the monorepo's `meerkly-server-node` harness — build
//! a config, `start()`, wait for a signal, `stop()` — with the three things a
//! packaged product needs on top: configuration from a file rather than only the
//! environment, a status file for `meerkly status`, and a first-connect retry so
//! the service survives starting before the network is up.

use crate::config::Resolved;
use crate::status::{ConnectionState, Status};
use anyhow::Result;
use meerkly_sdk::ProxyClient;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// How often the status file is refreshed while connected.
const STATUS_INTERVAL: Duration = Duration::from_secs(2);
/// First-connect retry bounds. `start()` gives up after its own timeout; a
/// machine that boots before its network is up must keep trying rather than let
/// the service exit and burn its restart budget.
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

pub async fn run(config: Resolved, status_path: PathBuf) -> Result<()> {
    let started = Instant::now();
    // Registered before the first connect attempt, so a stop during startup lands.
    let mut shutdown = Shutdown::new()?;
    // Minted on first run and stable after that. Loaded before the client
    // exists so a failure here is a clear startup error, not a silent
    // anonymous connection.
    let identity = crate::device::DeviceIdentity::load(&crate::paths::device_id_file()?)?;
    let client = ProxyClient::new(
        config.to_client_config(&identity),
        tokio::runtime::Handle::current(),
    )?;

    info!(
        publisher_id = %config.publisher_id,
        device_id = %identity.device_id,
        device_name = ?identity.device_name,
        gateways = %config.gateway_addresses.join(", "),
        config = %config.source.display(),
        "starting the meerkly agent"
    );

    let mut state = Publisher {
        path: status_path,
        publisher_id: config.publisher_id.clone(),
        device_id: identity.device_id.clone(),
        started,
        last_error: None,
    };
    state.publish(&client, ConnectionState::Connecting);

    // Connect, retrying with backoff. The shutdown listener races every wait so a
    // stop during a retry is immediate rather than delayed by up to a minute.
    let mut backoff = RETRY_MIN;
    loop {
        tokio::select! {
            result = client.start() => match result {
                Ok(()) => {
                    state.last_error = None;
                    info!(
                        gateway_id = ?client.gateway_id(),
                        client_key = ?client.client_key(),
                        "exit node online"
                    );
                    break;
                }
                Err(e) => {
                    // The supervisor keeps retrying underneath; this arm is the
                    // outer "we never got a first connection" case.
                    warn!(error = %e, retry_in = ?backoff, "could not reach a gateway");
                    state.last_error = Some(e.to_string());
                    state.publish(&client, ConnectionState::Connecting);
                }
            },
            _ = shutdown.recv() => {
                info!("stopped before connecting");
                Status::clear(&state.path);
                return Ok(());
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.recv() => {
                info!("stopped before connecting");
                Status::clear(&state.path);
                return Ok(());
            }
        }
        backoff = (backoff * 2).min(RETRY_MAX);
    }

    // Connected. Keep the status file current until we are told to stop; the SDK
    // reconnects on its own underneath, so a drop shows up here as a state change
    // rather than as an error to handle.
    let mut ticker = tokio::time::interval(STATUS_INTERVAL);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let current = ConnectionState::from(client.state());
                state.publish(&client, current);
            }
            _ = shutdown.recv() => break,
        }
    }

    info!("shutting down");
    client.stop().await?;
    Status::clear(&state.path);
    Ok(())
}

/// Owns the status file so the run loop does not have to reassemble a `Status`
/// at each of the four points it changes.
struct Publisher {
    path: PathBuf,
    publisher_id: String,
    device_id: String,
    started: Instant,
    last_error: Option<String>,
}

impl Publisher {
    fn publish(&mut self, client: &ProxyClient, state: ConnectionState) {
        let status = Status {
            state,
            publisher_id: self.publisher_id.clone(),
            device_id: self.device_id.clone(),
            gateway_id: client.gateway_id(),
            client_key: client.client_key(),
            uptime_seconds: self.started.elapsed().as_secs(),
            last_error: self.last_error.clone(),
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        // A status file we cannot write is a diagnostic problem, never a reason
        // to stop sharing bandwidth.
        if let Err(e) = status.write(&self.path) {
            warn!(error = %e, "cannot update the status file");
        }
    }
}

/// SIGINT/SIGTERM — the signals systemd, launchd and `docker stop` all send.
///
/// Held for the lifetime of the run loop rather than re-created per wait: the
/// listeners have to be registered *before* a signal arrives, so building one
/// per `select!` would drop anything that lands between two of them.
struct Shutdown {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

impl Shutdown {
    fn new() -> Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    /// Cancel-safe: nothing is lost when a `select!` drops this future.
    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = self.interrupt.recv() => {}
                _ = self.terminate.recv() => {}
            }
        }
        // Windows runs as a logon Scheduled Task, which asks the process to end
        // the same way a console Ctrl-C does.
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}
