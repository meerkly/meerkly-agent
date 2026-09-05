//! The meerkly agent.
//!
//! A console-only background service that shares this machine's connection as a
//! proxy exit node. It is the SDK ([`meerkly_sdk`]) packaged as a product: a
//! configuration file that survives restarts, a service definition per platform,
//! and a command surface the desktop app drives — the same split Docker has,
//! where the daemon and the GUI are separate programs.

mod cli;
mod config;
mod device;
mod paths;
mod run;
mod service;
mod status;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Command, ConfigCommand, ServiceCommand};
use config::{Config, Resolved};
use service::{ServiceManager, ServiceState};
use status::Status;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { id, no_service } => cmd_init(id, no_service),
        Command::Login { id } => cmd_login(id),
        Command::Run {
            publisher_id,
            gateway,
        } => cmd_run(publisher_id, gateway),
        Command::Service(ServiceCommand::Install { user }) => cmd_service_install(user),
        Command::Service(ServiceCommand::Uninstall) => cmd_service_uninstall(),
        Command::Start => manager(None)?.start().context("cannot start the service"),
        Command::Stop => manager(None)?.stop().context("cannot stop the service"),
        Command::Restart => {
            let m = manager(None)?;
            let _ = m.stop();
            m.start().context("cannot start the service")
        }
        Command::Status { json } => cmd_status(json),
        Command::Logs { lines } => cmd_logs(lines),
        Command::Config(c) => cmd_config(c),
    }
}

// ---- first run -------------------------------------------------------------

/// Everything a new machine needs, in one command.
fn cmd_init(id: Option<String>, no_service: bool) -> Result<()> {
    let path = save_publisher_id(id)?;
    if no_service {
        println!();
        println!("Install the service when you are ready:  meerkly service install");
        return Ok(());
    }

    println!();
    let m = manager(None)?;
    m.install().with_context(|| {
        format!(
            "stored your publisher id in {}, but could not install the service",
            path.display()
        )
    })?;
    println!("meerkly is running.");
    println!("  {}", m.native_hint());
    println!();
    println!("Check on it any time with:  meerkly status");
    Ok(())
}

fn cmd_login(id: Option<String>) -> Result<()> {
    save_publisher_id(id)?;
    // A running service read its config at startup, so it is still using the old
    // id until it restarts.
    after_write(false)?;
    Ok(())
}

/// Resolve a publisher id — from the argument, or by asking — and store it.
fn save_publisher_id(id: Option<String>) -> Result<std::path::PathBuf> {
    let path = paths::config_file()?;
    let mut config = Config::load(&path)?;

    let id = match id {
        Some(given) => config::validate_publisher_id(&given)?,
        None => prompt_for_publisher_id(config.publisher_id.as_deref())?,
    };

    config.publisher_id = Some(id.clone());
    config.save(&path)?;
    // Under `sudo meerkly init` the file was just written by root into the
    // human's home. Give it back, or they can never edit it again.
    paths::adopt_for_sudo_user(&path)?;
    println!("publisher id {id} stored in {}", path.display());
    Ok(path)
}

/// What one line typed at the prompt means.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    /// Nothing typed and there is a current id: keep it, as an empty answer at
    /// a `[default]` prompt means everywhere else.
    Keep,
    Use(String),
    /// Ask again, saying why.
    Retry(String),
}

fn interpret_answer(line: &str, current: Option<&str>) -> Answer {
    if line.trim().is_empty() {
        return match current {
            Some(_) => Answer::Keep,
            None => {
                Answer::Retry("nothing entered — paste your publisher id, or Ctrl-C to stop".into())
            }
        };
    }
    match config::validate_publisher_id(line) {
        Ok(id) => Answer::Use(id),
        Err(e) => Answer::Retry(e.to_string()),
    }
}

/// Ask for the id, re-asking on a typo rather than making the user start over.
///
/// Refuses when there is no terminal: a non-interactive caller (a script, the
/// desktop app, a Dockerfile) must pass the id, and hanging on a read that can
/// never be answered would be worse than failing.
fn prompt_for_publisher_id(current: Option<&str>) -> Result<String> {
    use std::io::{IsTerminal, Write};

    anyhow::ensure!(
        std::io::stdin().is_terminal(),
        "no publisher id given, and there is no terminal to ask on.\n\n  Pass it \
         directly:\n    meerkly login pub_…\n\n  Get one at https://dashboard.meerkly.com"
    );

    println!("Find your publisher id at https://dashboard.meerkly.com");

    loop {
        // The current id is the default, shown in brackets the way every other
        // prompt does it; Enter keeps it.
        match current {
            Some(c) => print!("publisher id [{c}]: "),
            None => print!("publisher id (pub_…): "),
        }
        std::io::stdout().flush()?;

        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            anyhow::bail!("cancelled — nothing was changed");
        }
        match interpret_answer(&line, current) {
            Answer::Keep => {
                let c = current.expect("Keep only occurs with a current id");
                println!("keeping {c}");
                return Ok(c.to_owned());
            }
            Answer::Use(id) => return Ok(id),
            Answer::Retry(why) => eprintln!("  {why}\n"),
        }
    }
}

// ---- run -------------------------------------------------------------------

fn cmd_run(publisher_id: Option<String>, gateway: Option<String>) -> Result<()> {
    let path = paths::config_file()?;
    let mut file = Config::load(&path)?;

    // CLI flags are the strongest layer, so they are applied to the loaded file
    // before the environment is resolved over it.
    if let Some(id) = publisher_id {
        file.publisher_id = Some(config::validate_publisher_id(&id)?);
    }
    if let Some(addresses) = gateway {
        file.gateway_addresses = Some(config::split_addresses(&addresses));
    }

    let resolved = Resolved::from_file(file, path)?;
    init_logging(resolved.log.as_deref());

    let status_path = paths::status_file()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run::run(resolved, status_path))
}

/// `MEERKLY_LOG` — the filter variable the rest of meerkly already uses — then
/// the config file's `log`, then `info`.
///
/// Output goes two places: stderr, which is what systemd's journal and launchd's
/// StandardErrorPath capture, and the daemon's own daily-rotated files under
/// `paths::log_dir()`, which is what `meerkly logs` reads — the same on every
/// platform, and with no journal permissions needed. If the log directory cannot
/// be made the daemon still runs, on stderr alone; a proxy exit that refuses to
/// start over a log file is the wrong trade.
fn init_logging(configured: Option<&str>) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("MEERKLY_LOG")
        .or_else(|_| EnvFilter::try_new(configured.unwrap_or("info")))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    match log_file_appender() {
        Ok(files) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::io::stderr.and(files))
            .init(),
        Err(e) => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::io::stderr)
                .init();
            tracing::warn!("not writing log files: {e:#}");
        }
    }
}

/// One file per day under the log directory, `meerkly.YYYY-MM-DD.log`, the
/// newest seven kept. The date is in the name so `meerkly logs` can order them
/// with a plain sort.
fn log_file_appender() -> Result<tracing_appender::rolling::RollingFileAppender> {
    let dir = paths::log_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("meerkly")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)
        .with_context(|| format!("cannot open a log file in {}", dir.display()))
}

// ---- logs ------------------------------------------------------------------

/// The last `lines` lines the daemon wrote, oldest first; 0 means all of them.
fn cmd_logs(lines: usize) -> Result<()> {
    let dir = paths::log_dir()?;
    let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("meerkly.") && n.ends_with(".log"))
            })
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    if files.is_empty() {
        println!("No log yet — the agent has not run on this machine.");
        println!("  {}", manager(None)?.native_hint());
        println!("  Start it with:  meerkly start");
        return Ok(());
    }
    // Dates in the names sort chronologically; newest first for the tail.
    files.sort();
    files.reverse();

    let newest_first = files.iter().filter_map(|p| std::fs::read_to_string(p).ok());
    for line in tail_across(newest_first, lines) {
        println!("{line}");
    }
    Ok(())
}

/// The last `n` lines across files given newest-first, returned oldest-first.
/// Stops reading as soon as it has enough, so `-n 20` does not load a week.
/// `n == 0` means every line.
fn tail_across(newest_first: impl Iterator<Item = String>, n: usize) -> Vec<String> {
    let mut picked: Vec<String> = Vec::new();
    for file in newest_first {
        let mut lines: Vec<String> = file.lines().map(str::to_owned).collect();
        lines.extend(picked);
        picked = lines;
        if n != 0 && picked.len() >= n {
            let cut = picked.len() - n;
            picked.drain(..cut);
            break;
        }
    }
    picked
}

// ---- service ---------------------------------------------------------------

fn manager(user: Option<&str>) -> Result<service::Manager> {
    match user {
        Some(name) => service::Manager::for_user(name),
        None => Ok(service::Manager::default()),
    }
}

fn cmd_service_install(user: Option<String>) -> Result<()> {
    // Refuse to install a service that cannot start. Discovering a missing
    // publisher id from `systemctl status` minutes later is a bad first run.
    let path = paths::config_file()?;
    Resolved::from_file(Config::load(&path)?, path.clone())
        .context("cannot install the service yet")?;

    let m = manager(user.as_deref())?;
    m.install()?;
    println!("meerkly is installed and running.");
    println!("  configuration: {}", path.display());
    println!("  {}", m.native_hint());
    Ok(())
}

fn cmd_service_uninstall() -> Result<()> {
    manager(None)?.uninstall()?;
    let path = paths::config_file()?;
    println!("meerkly is no longer installed.");
    println!("  configuration left in place: {}", path.display());
    Ok(())
}

// ---- status ----------------------------------------------------------------

fn cmd_status(json: bool) -> Result<()> {
    let service_state = manager(None)
        .map(|m| m.state())
        .unwrap_or(ServiceState::Unknown);
    // The status file is written by the daemon and outlives a crash, so it is
    // only trustworthy alongside a live answer from the service manager.
    let published = Status::read(&paths::status_file()?)?;

    if json {
        let mut body = serde_json::json!({
            "service": service_state.as_str(),
            "config": paths::config_file()?.to_string_lossy(),
        });
        body["connection"] = match &published {
            Some(s) if service_state != ServiceState::Stopped => serde_json::to_value(s)?,
            _ => serde_json::Value::Null,
        };
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    println!("service:  {}", service_state.as_str());
    match published {
        Some(s) if service_state != ServiceState::Stopped => {
            println!(
                "state:    {}",
                serde_json::to_value(&s.state)?.as_str().unwrap_or("?")
            );
            println!("publisher: {}", s.publisher_id);
            println!("device:   {}", s.device_id);
            if let Some(gateway) = &s.gateway_id {
                println!("gateway:  {gateway}");
            }
            if let Some(key) = &s.client_key {
                println!("client:   {key}");
            }
            println!("uptime:   {}s", s.uptime_seconds);
            if let Some(error) = &s.last_error {
                println!("error:    {error}");
            }
        }
        _ => println!("state:    not running"),
    }
    println!("config:   {}", paths::config_file()?.display());
    Ok(())
}

// ---- config ----------------------------------------------------------------

fn cmd_config(command: ConfigCommand) -> Result<()> {
    let path = paths::config_file()?;
    match command {
        ConfigCommand::Path => {
            println!("{}", path.display());
            Ok(())
        }
        ConfigCommand::Show => {
            match std::fs::read_to_string(&path) {
                Ok(text) => print!("{text}"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!("# no configuration yet: {}", path.display());
                    println!("# create one with: meerkly init");
                }
                Err(e) => return Err(e).context(format!("cannot read {}", path.display())),
            }
            Ok(())
        }
        ConfigCommand::Get { key } => {
            let config = Config::load(&path)?;
            match get_key(&config, &key)? {
                Some(value) => println!("{value}"),
                None => std::process::exit(1),
            }
            Ok(())
        }
        ConfigCommand::Set {
            key,
            value,
            restart,
        } => {
            let mut config = Config::load(&path)?;
            set_key(&mut config, &key, Some(&value))?;
            config.save(&path)?;
            // Echo what was *stored*, not what was typed — values are trimmed and
            // normalised on the way in, and the difference matters when someone
            // pastes an id with stray whitespace.
            let stored = get_key(&config, &key)?.unwrap_or_default();
            println!("{key} = {stored}");
            println!("  {}", path.display());
            after_write(restart)
        }
        ConfigCommand::Unset { key, restart } => {
            let mut config = Config::load(&path)?;
            set_key(&mut config, &key, None)?;
            config.save(&path)?;
            println!("{key} unset");
            after_write(restart)
        }
    }
}

/// A configuration change only reaches a running daemon through a restart — the
/// agent reads its file once at startup. Say so rather than let a change look
/// like it took effect.
fn after_write(restart: bool) -> Result<()> {
    let m = manager(None)?;
    match m.state() {
        ServiceState::Running if restart => {
            let _ = m.stop();
            m.start()?;
            println!("  service restarted");
        }
        ServiceState::Running => {
            println!("  restart to apply: meerkly restart");
        }
        _ => {}
    }
    Ok(())
}

fn get_key(config: &Config, key: &str) -> Result<Option<String>> {
    Ok(match normalise(key).as_str() {
        "publisher_id" => config.publisher_id.clone(),
        "gateway_addresses" => config.gateway_addresses.as_ref().map(|a| a.join(",")),
        "ca_cert_path" => config.ca_cert_path.clone(),
        "log" => config.log.clone(),
        other => anyhow::bail!("{}", unknown_key(other)),
    })
}

fn set_key(config: &mut Config, key: &str, value: Option<&str>) -> Result<()> {
    match normalise(key).as_str() {
        "publisher_id" => {
            config.publisher_id = value.map(config::validate_publisher_id).transpose()?
        }
        "gateway_addresses" => {
            config.gateway_addresses = value.map(config::split_addresses).filter(|a| !a.is_empty())
        }
        "ca_cert_path" => config.ca_cert_path = value.map(str::to_owned),
        "log" => config.log = value.map(str::to_owned),
        other => anyhow::bail!("{}", unknown_key(other)),
    }
    Ok(())
}

/// `publisher-id` and `publisher_id` are the same key — the CLI reads better
/// with dashes, TOML with underscores, and nobody should have to remember which
/// context they are in.
fn normalise(key: &str) -> String {
    key.trim().replace('-', "_").to_ascii_lowercase()
}

fn unknown_key(key: &str) -> String {
    format!(
        "unknown configuration key {key:?} — expected one of: publisher-id, gateway-addresses, \
         ca-cert-path, log"
    )
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    #[test]
    fn enter_keeps_the_current_id() {
        assert_eq!(interpret_answer("\n", Some("pub_abc")), Answer::Keep);
        assert_eq!(interpret_answer("   ", Some("pub_abc")), Answer::Keep);
    }

    #[test]
    fn enter_with_nothing_configured_asks_again_and_says_so() {
        match interpret_answer("\n", None) {
            Answer::Retry(why) => assert!(why.contains("nothing entered"), "{why}"),
            other => panic!("expected a retry, got {other:?}"),
        }
    }

    #[test]
    fn a_typed_id_replaces_the_current_one() {
        assert_eq!(
            interpret_answer(" pub_new1 \n", Some("pub_old")),
            Answer::Use("pub_new1".into())
        );
    }

    #[test]
    fn a_typo_asks_again_with_the_validation_message() {
        match interpret_answer("pob_x\n", Some("pub_old")) {
            Answer::Retry(why) => assert!(why.contains("pub_"), "{why}"),
            other => panic!("expected a retry, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod logs_tests {
    use super::*;

    fn files<'a>(newest_first: &'a [&'a str]) -> impl Iterator<Item = String> + 'a {
        newest_first.iter().map(|f| f.to_string())
    }

    #[test]
    fn the_tail_spans_files_and_comes_out_oldest_first() {
        let got = tail_across(files(&["d\ne\n", "a\nb\nc\n"]), 3);
        assert_eq!(got, ["c", "d", "e"]);
    }

    #[test]
    fn asking_for_more_than_exists_gives_everything() {
        let got = tail_across(files(&["c\n", "a\nb\n"]), 50);
        assert_eq!(got, ["a", "b", "c"]);
    }

    #[test]
    fn zero_means_all() {
        let got = tail_across(files(&["c\n", "a\nb\n"]), 0);
        assert_eq!(got, ["a", "b", "c"]);
    }

    #[test]
    fn stops_reading_once_it_has_enough() {
        // The older file is never touched: it is not needed for two lines.
        let mut touched = 0;
        let newest_first = ["y\nz\n", "a\n"].into_iter().map(|f| {
            touched += 1;
            f.to_string()
        });
        assert_eq!(tail_across(newest_first, 2), ["y", "z"]);
        assert_eq!(touched, 1);
    }
}
