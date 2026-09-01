# Working in this repository

`meerkly-agent` packages the meerkly exit-node SDK as a background service. The repository has
the long name; **the crate, the binary, the Homebrew formula, the deb/rpm package and the
service are all called `meerkly`** — keep it that way in anything user-facing.

## Shape

| Path | What |
|---|---|
| `src/main.rs` | command dispatch; every subcommand is a `cmd_*` function here |
| `src/cli.rs` | clap definitions only |
| `src/config.rs` | the TOML file, the resolution order, publisher-id validation |
| `src/paths.rs` | where the config, state and status files live, per platform |
| `src/run.rs` | the daemon: SDK lifecycle, signals, status writing |
| `src/status.rs` | the status file `meerkly status` reads back |
| `src/service/` | one backend per platform behind `ServiceManager` |
| `packaging/` | nfpm config, install.sh, and reference copies of the service definitions |

## Rules that are not obvious from the code

- **The service runs as the installing user.** This is the load-bearing decision: it is why the
  configuration lives in a home directory rather than `/etc`, why the systemd unit sets
  `Environment=HOME=`, and why macOS uses a LaunchAgent and Windows a logon task instead of
  something that starts at boot. Do not "fix" a platform to start earlier without moving its
  configuration too.
- **A publisher id is public, not a secret.** No keychain, no `0600`, no masking in output. Say
  so in a comment if you touch code that looks like it should be handling a credential.
- **Validate publisher ids by prefix, never by length.** `^pub_[A-Za-z0-9]+$` after trimming,
  matching the dashboard's own pairing-code parser. Ids are 28 characters today; nothing
  promises that.
- **`meerkly status --json` is the only supported interface for other programs.** The status
  file's path and shape are internal — the desktop app must never read it directly.
- **`MEERKLY_LOG`, not `RUST_LOG`.** That is the variable the rest of meerkly uses.
- **Config changes need a restart.** The agent reads its file once at startup. Anything that
  writes config should say so, or offer `--restart`.

## House style

Follows the monorepo: `anyhow` throughout with `.context()` and `ensure!`, and failure messages
long enough to name the fix. `//!` and `///` comments explain *why* something is the way it is,
not what the next line does. Unlike the monorepo, this repo does enforce `cargo fmt` and
`cargo clippy -D warnings` in CI.

## Before opening a PR

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
sh -n packaging/install.sh
```

End-to-end coverage is manual — it needs a gateway, which lives in the private monorepo. See the
Development section of the README.
