#!/bin/sh
# meerkly installer.
#
#   curl -fsSL https://meerkly.com/install | sh
#   curl -fsSL https://meerkly.com/install | sh -s -- --id pub_…
#
# Downloads the release build for this machine, installs it, stores your
# publisher id and starts the background service.
#
# Options:
#   --id <pub_…>   publisher id; otherwise $MEERKLY_PUBLISHER_ID, otherwise asked
#   --version <v>  install a specific version instead of the latest
#   --no-service   install the binary and write the config, but do not start it
#   --help         show this
#
# Re-running upgrades in place and keeps your existing configuration.
set -eu

REPO="meerkly/meerkly-agent"
VERSION="${MEERKLY_VERSION:-latest}"
PUBLISHER_ID="${MEERKLY_PUBLISHER_ID:-}"
INSTALL_SERVICE=1

# Colour only for a terminal — a piped or logged run stays plain text.
if [ -t 1 ]; then
  BOLD=$(printf '\033[1m'); DIM=$(printf '\033[2m'); RED=$(printf '\033[31m'); OFF=$(printf '\033[0m')
else
  BOLD=''; DIM=''; RED=''; OFF=''
fi
say()  { printf '%s\n' "$*"; }
info() { printf '%s%s%s\n' "$DIM" "$*" "$OFF"; }
die()  { printf '%serror:%s %s\n' "$RED" "$OFF" "$*" >&2; exit 1; }

# The header above IS the help text, so the two can never drift apart.
usage() { sed -n '2,/^set -eu$/p' "$0" | sed 's/^# \{0,1\}//; /^set -eu$/d'; exit 0; }

while [ $# -gt 0 ]; do
  case "$1" in
    --id)      PUBLISHER_ID="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    --no-service) INSTALL_SERVICE=0; shift ;;
    --help|-h) usage ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required but not installed"; }
need curl
need tar

# ---- what are we installing on? ---------------------------------------------

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)  os_target="unknown-linux-musl" ;;   # static: no glibc version floor
  Darwin) os_target="apple-darwin" ;;
  *) die "unsupported operating system: $os. Windows builds are on the releases page." ;;
esac
case "$arch" in
  x86_64|amd64) arch_target="x86_64" ;;
  aarch64|arm64) arch_target="aarch64" ;;
  *) die "unsupported architecture: $arch" ;;
esac
TARGET="${arch_target}-${os_target}"

if [ "$VERSION" = "latest" ]; then
  VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name" *: *"v\{0,1\}\([^"]*\)".*/\1/p' | head -n1)
  [ -n "$VERSION" ] || die "cannot determine the latest version; pass --version"
fi

# ---- publisher id ------------------------------------------------------------

# stdin is the curl pipe, so an interactive prompt has to read the terminal.
if [ -z "$PUBLISHER_ID" ] && [ -r /dev/tty ]; then
  say ""
  say "${BOLD}Your publisher id${OFF} — find it at https://dashboard.meerkly.com"
  printf 'publisher id (pub_…): '
  read -r PUBLISHER_ID < /dev/tty || true
fi
[ -n "$PUBLISHER_ID" ] || die "no publisher id given. Pass --id pub_… or set MEERKLY_PUBLISHER_ID."
case "$PUBLISHER_ID" in
  pub_*) ;;
  *) die "that does not look like a publisher id — expected a 'pub_' prefix" ;;
esac

# ---- download ----------------------------------------------------------------

ARCHIVE="meerkly-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/$REPO/releases/download/v${VERSION}/${ARCHIVE}"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

info "downloading meerkly ${VERSION} for ${TARGET}…"
curl -fsSL "$URL" -o "$tmp/$ARCHIVE" || die "cannot download $URL"

# Verify against the release's own checksum file; a truncated download must not
# become an installed binary.
if curl -fsSL "https://github.com/$REPO/releases/download/v${VERSION}/SHA256SUMS" -o "$tmp/SHA256SUMS" 2>/dev/null; then
  expected=$(grep " $ARCHIVE\$" "$tmp/SHA256SUMS" | awk '{print $1}')
  if [ -n "$expected" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      actual=$(sha256sum "$tmp/$ARCHIVE" | awk '{print $1}')
    else
      actual=$(shasum -a 256 "$tmp/$ARCHIVE" | awk '{print $1}')
    fi
    [ "$expected" = "$actual" ] || die "checksum mismatch for $ARCHIVE — refusing to install"
    info "checksum ok"
  fi
fi

tar -xzf "$tmp/$ARCHIVE" -C "$tmp"
[ -f "$tmp/meerkly" ] || die "$ARCHIVE did not contain a meerkly binary"
chmod +x "$tmp/meerkly"

# ---- install -----------------------------------------------------------------

# /usr/local/bin where we can write it, otherwise ~/.local/bin — installing must
# not require root just to place a binary.
if [ -w /usr/local/bin ]; then
  BIN_DIR=/usr/local/bin
elif command -v sudo >/dev/null 2>&1 && [ -d /usr/local/bin ]; then
  BIN_DIR=/usr/local/bin
  SUDO=sudo
else
  BIN_DIR="$HOME/.local/bin"
  mkdir -p "$BIN_DIR"
fi
${SUDO:-} install -m 0755 "$tmp/meerkly" "$BIN_DIR/meerkly"
info "installed $BIN_DIR/meerkly"

# ---- configure and start -----------------------------------------------------

"$BIN_DIR/meerkly" login "$PUBLISHER_ID" >/dev/null
info "publisher id stored in $("$BIN_DIR/meerkly" config path)"

if [ "$INSTALL_SERVICE" = "1" ]; then
  if [ "$os" = "Linux" ] && [ "$(id -u)" != "0" ] && command -v sudo >/dev/null 2>&1; then
    # Registering a system unit needs root; the service still runs as this user.
    sudo -E "$BIN_DIR/meerkly" service install
  else
    "$BIN_DIR/meerkly" service install
  fi
else
  say ""
  say "Start it when you are ready:  meerkly service install"
fi

say ""
say "${BOLD}meerkly is installed.${OFF}"
say "  meerkly status        how it is doing"
say "  meerkly config show   what it is configured with"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say ""; say "  Note: $BIN_DIR is not on your PATH." ;;
esac
