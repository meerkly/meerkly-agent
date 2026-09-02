# meerkly

Share this machine's connection as a **proxy exit node** and earn per GB shared.

The agent is a console-only background service: no window, no tray icon. It holds one QUIC
connection to a meerkly gateway and dials outbound TCP on the gateway's behalf, so traffic the
network routes through you leaves via your own IP. You install it, give it a publisher id, and
it runs.

> The repository is `meerkly-agent`; everything you type is just `meerkly`.

## Install

**Homebrew** (macOS, Linux)

```bash
brew install meerkly/tap/meerkly
```

**Debian / Ubuntu**

```bash
sudo apt install ./meerkly_1.0.0_amd64.deb
```

**Fedora / RHEL**

```bash
sudo dnf install ./meerkly-1.0.0.x86_64.rpm
```

**One-liner** (downloads, configures and starts)

```bash
curl -fsSL https://meerkly.com/install | sh
```

**From source**

```bash
cargo install meerkly
```

`.deb`, `.rpm`, `.tar.gz` and Windows `.zip` builds for x86-64 and arm64 are on the
[releases page](https://github.com/meerkly/meerkly-agent/releases).

## Getting started

```bash
sudo meerkly init
```

That is the whole setup: it asks for your publisher id, stores it, and installs and starts the
background service. Pass the id instead of being asked with `meerkly init pub_…`, and skip the
service with `--no-service`. (`sudo` is only needed on Linux, to register the systemd unit — the
service still runs as you.)

Then, any time:

```bash
meerkly status
meerkly login pub_…    # change which account earns what this machine shares
```

Get a publisher id at [dashboard.meerkly.com](https://dashboard.meerkly.com). It is a
**public identifier, not a secret**. It says which account earns the
bandwidth this machine shares, and grants access to nothing.

## Running it

The service is registered with your platform's own service manager, so use that if you prefer:

```bash
systemctl start|stop|status meerkly     # Linux
journalctl -u meerkly -f                # Linux logs
brew services start|stop meerkly        # macOS via Homebrew
```

`meerkly start|stop|restart` does the same thing on all three platforms — one command shape for
scripts and for the desktop app.

| | mechanism | starts |
|---|---|---|
| Linux | systemd system unit running as you | at boot |
| macOS | per-user LaunchAgent | at login |
| Windows | Scheduled Task at logon | at logon |

**The service always runs as the user who installed it**, which is what keeps its configuration
in your own home directory and editable without `sudo`. macOS and Windows start later than Linux
for the same reason: starting at boot there means running as root or as a service account with a
stored password, which would put the configuration back behind elevation.

## Configuration

| OS | Path |
|---|---|
| Linux, macOS | `~/.config/meerkly/config.toml` |
| Windows | `%APPDATA%\meerkly\config.toml` |

```toml
publisher_id = "pub_XXXXXXXXXXXXXXXXXXXXXXXX"

# gateway_addresses = ["gw.meerkly.com:4443"]   # defaults to the production gateway
# ca_cert_path = "/path/to/ca.crt"              # pin a development gateway's CA
# log = "info"
```

Edit it by hand, or:

```bash
meerkly login pub_…                     # the publisher id, with validation
meerkly config set log debug --restart
meerkly config show
meerkly config path
```

`meerkly login` is `config set publisher-id` with a prompt and a friendlier name; `config set`
remains the low-level way to reach every other setting.

Settings resolve **CLI flag → environment → config file → default**. The environment names are
`MEERKLY_PUBLISHER_ID`, `MEERKLY_GATEWAY_ADDRESSES`, `MEERKLY_CA_CERT_PATH`, `MEERKLY_LOG` and
`MEERKLY_CONFIG` (which points at a different file). The agent reads its configuration once at
startup, so a change needs a restart — `meerkly config set --restart` does both.

## Status

```bash
meerkly status
meerkly status --json
```

`--json` is the supported interface for other programs, including the desktop app:

```json
{
  "service": "running",
  "config": "/home/you/.config/meerkly/config.toml",
  "connection": {
    "state": "connected",
    "publisher_id": "pub_…",
    "gateway_id": "gw-1",
    "client_key": "…",
    "uptime_seconds": 3600,
    "pid": 4211,
    "version": "1.0.0"
  }
}
```

`client_key` is **ephemeral** — every reconnect gets a new one, because an exit node has no
persistent device identity by design. Earnings accrue to the account behind the publisher id.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -- run
```

The agent depends on [`meerkly-sdk`](https://crates.io/crates/meerkly-sdk) from crates.io. To
work on the SDK and the agent together, check out the monorepo as a sibling directory and add a
local override — `.cargo/` is gitignored, so this never reaches a release:

```toml
# .cargo/config.toml
[patch.crates-io]
meerkly-sdk = { path = "../meerkly/crates/client-core" }
```

To test against a local gateway rather than production:

```bash
meerkly login pub_devtest123
meerkly config set gateway-addresses 127.0.0.1:4443
meerkly config set ca-cert-path ../meerkly/certs/dev/ca.crt
```

## Licence

MIT OR Apache-2.0
