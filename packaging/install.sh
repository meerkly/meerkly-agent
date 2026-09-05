#!/bin/sh
# meerkly installer.
#
#   curl -fsSL https://meerkly.com/install | sh
#   curl -fsSL https://meerkly.com/install | PUBLISHER_ID=pub_… sh
#
# The variable goes after the pipe, on the sh side: `PUBLISHER_ID=… curl … | sh`
# would hand it to curl, which has no use for it, and sh would never see it.
#
# Installs through whatever package manager the machine already has — Homebrew
# on macOS, apt on Debian and Ubuntu, dnf on Fedora and RHEL — so that upgrading
# and removing meerkly work the way they do for everything else installed here.
# Only when there is no package manager to use does it fall back to dropping a
# static binary in place.
#
# `usage()` below is the one copy of the option list; --help prints it. It is a
# heredoc rather than a slice of this comment block because the script is
# normally run from a pipe, where $0 is "sh" and there is no file to read back.
#
# Re-running upgrades in place and keeps the existing configuration.
set -eu

REPO="meerkly/meerkly-agent"
TAP="meerkly/tap/meerkly"
VERSION="${MEERKLY_VERSION:-latest}"
# MEERKLY_PUBLISHER_ID is the long-standing name; bare PUBLISHER_ID is accepted
# because it is what a person reaching for a one-liner will type.
PUBLISHER_ID="${MEERKLY_PUBLISHER_ID:-${PUBLISHER_ID:-}}"
METHOD="${MEERKLY_METHOD:-auto}"
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
have() { command -v "$1" >/dev/null 2>&1; }

usage() {
  cat <<'USAGE'
meerkly installer.

  curl -fsSL https://meerkly.com/install | sh
  curl -fsSL https://meerkly.com/install | PUBLISHER_ID=pub_... sh
  curl -fsSL https://meerkly.com/install | sh -s -- --id pub_...

Options:
  --id <pub_...>   publisher id; otherwise $PUBLISHER_ID, otherwise asked
  --version <v>    install a specific version instead of the latest
  --method <m>     auto (default), brew, apt, dnf or binary
  --no-service     install and configure, but do not start the background service
  --help           show this

Installs with Homebrew, apt or dnf when one of them is present, and falls back
to a static binary otherwise.
USAGE
  exit 0
}

while [ $# -gt 0 ]; do
  case "$1" in
    --id)         PUBLISHER_ID="${2:-}"; shift 2 ;;
    --version)    VERSION="${2:-}"; shift 2 ;;
    --method)     METHOD="${2:-}"; shift 2 ;;
    --no-service) INSTALL_SERVICE=0; shift ;;
    --help|-h)    usage ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

have curl || die "curl is required but not installed"

# ---- what are we installing on? ---------------------------------------------

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)  os_target="unknown-linux-musl" ;;   # static: no glibc version floor
  Darwin) os_target="apple-darwin" ;;
  *) die "unsupported operating system: $os. Windows builds are on https://github.com/$REPO/releases" ;;
esac
case "$arch" in
  x86_64|amd64)  arch_target="x86_64";  deb_arch="amd64"; rpm_arch="x86_64" ;;
  aarch64|arm64) arch_target="aarch64"; deb_arch="arm64"; rpm_arch="aarch64" ;;
  *) die "unsupported architecture: $arch" ;;
esac
TARGET="${arch_target}-${os_target}"

# ---- how are we installing? --------------------------------------------------

# Homebrew first: it is the one package manager that manages its own upgrades on
# both macOS and Linux, and it is never run under sudo. apt and dnf come next
# because a distribution package is what makes `apt remove meerkly` work. The
# static binary is the fallback for everything else — Alpine, a bare container,
# a machine with no package manager on PATH.
if [ "$METHOD" = "auto" ]; then
  if have brew; then METHOD=brew
  elif have apt-get; then METHOD=apt
  elif have dnf || have yum; then METHOD=dnf
  else METHOD=binary
  fi
fi
case "$METHOD" in
  brew)   have brew || die "--method brew, but brew is not on PATH" ;;
  apt)    have apt-get || die "--method apt, but apt-get is not on PATH" ;;
  dnf)    have dnf || have yum || die "--method dnf, but neither dnf nor yum is on PATH" ;;
  binary) have tar || die "installing the plain binary needs tar" ;;
  *) die "unknown method: $METHOD (try --help)" ;;
esac

# Root is needed to install a distribution package, and must never be used for
# Homebrew, which refuses to run as root and would leave a broken prefix if it
# did not.
SUDO=''
if [ "$METHOD" = "apt" ] || [ "$METHOD" = "dnf" ]; then
  if [ "$(id -u)" != "0" ]; then
    have sudo || die "installing a system package needs root; run this as root or install sudo"
    SUDO=sudo
  fi
fi

# ---- publisher id ------------------------------------------------------------

# stdin is the curl pipe, so an interactive prompt has to read the terminal
# directly. Asked for before anything is downloaded: better to stop here than
# after installing a service with nothing to earn for.
if [ -z "$PUBLISHER_ID" ] && [ -r /dev/tty ]; then
  say ""
  say "${BOLD}Your publisher id${OFF} — find it at https://dashboard.meerkly.com"
  printf 'publisher id (pub_…): '
  read -r PUBLISHER_ID < /dev/tty || true
fi
[ -n "$PUBLISHER_ID" ] || die "no publisher id given. Pass --id pub_… or set PUBLISHER_ID."
case "$PUBLISHER_ID" in
  pub_*) ;;
  *) die "that does not look like a publisher id — expected a 'pub_' prefix" ;;
esac

# ---- version -----------------------------------------------------------------

# Homebrew resolves its own version from the tap, so only the paths that build a
# download URL need to ask GitHub.
if [ "$METHOD" != "brew" ] && [ "$VERSION" = "latest" ]; then
  VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name" *: *"v\{0,1\}\([^"]*\)".*/\1/p' | head -n1)
  [ -n "$VERSION" ] || die "cannot determine the latest version; pass --version"
fi

BASE="https://github.com/$REPO/releases/download/v${VERSION}"

# Download one release asset and check it against the release's own SHA256SUMS.
# A truncated download must never become an installed package.
fetch_asset() {
  asset="$1"; out="$2"
  info "downloading ${asset}…"
  curl -fsSL "$BASE/$asset" -o "$out" || die "cannot download $BASE/$asset"

  sums="${out%/*}/SHA256SUMS"
  if curl -fsSL "$BASE/SHA256SUMS" -o "$sums" 2>/dev/null; then
    expected=$(grep " $asset\$" "$sums" | awk '{print $1}')
    if [ -n "$expected" ]; then
      if have sha256sum; then actual=$(sha256sum "$out" | awk '{print $1}')
      else actual=$(shasum -a 256 "$out" | awk '{print $1}')
      fi
      [ "$expected" = "$actual" ] || die "checksum mismatch for $asset — refusing to install"
      info "checksum ok"
    fi
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

# ---- install -----------------------------------------------------------------

MEERKLY=meerkly

case "$METHOD" in
  brew)
    # `brew install` on an already-installed formula is an error, not a no-op,
    # so an upgrade has to be asked for by name. "already installed" and "up to
    # date" are both success as far as this script is concerned.
    if brew list --formula meerkly >/dev/null 2>&1; then
      info "upgrading with Homebrew…"
      brew upgrade "$TAP" 2>/dev/null || brew upgrade meerkly 2>/dev/null || info "already up to date"
    else
      info "installing with Homebrew…"
      brew install "$TAP" || die "brew install $TAP failed"
    fi
    MEERKLY="$(brew --prefix)/bin/meerkly"
    ;;

  apt)
    asset="meerkly_${VERSION}_${deb_arch}.deb"
    fetch_asset "$asset" "$tmp/$asset"
    info "installing with apt…"
    # apt-get takes a local path directly and pulls in dependencies; dpkg is the
    # fallback for the older apt on long-lived LTS boxes, with a follow-up fix
    # for anything it could not resolve on its own.
    $SUDO apt-get install -y "$tmp/$asset" 2>/dev/null \
      || { $SUDO dpkg -i "$tmp/$asset" || true; $SUDO apt-get install -f -y; }
    MEERKLY=/usr/bin/meerkly
    ;;

  dnf)
    asset="meerkly-${VERSION}-1.${rpm_arch}.rpm"
    fetch_asset "$asset" "$tmp/$asset"
    info "installing with dnf…"
    if have dnf; then $SUDO dnf install -y "$tmp/$asset"
    else $SUDO yum localinstall -y "$tmp/$asset"
    fi
    MEERKLY=/usr/bin/meerkly
    ;;

  binary)
    asset="meerkly-${VERSION}-${TARGET}.tar.gz"
    fetch_asset "$asset" "$tmp/$asset"
    tar -xzf "$tmp/$asset" -C "$tmp"
    [ -f "$tmp/meerkly" ] || die "$asset did not contain a meerkly binary"
    chmod +x "$tmp/meerkly"

    # /usr/local/bin when it is writable or sudo is available, otherwise the
    # user's own bin — placing a binary must not require root.
    if [ -w /usr/local/bin ]; then
      BIN_DIR=/usr/local/bin; BSUDO=''
    elif have sudo && [ -d /usr/local/bin ]; then
      BIN_DIR=/usr/local/bin; BSUDO=sudo
    else
      BIN_DIR="$HOME/.local/bin"; BSUDO=''
      mkdir -p "$BIN_DIR"
    fi
    ${BSUDO:-} install -m 0755 "$tmp/meerkly" "$BIN_DIR/meerkly"
    MEERKLY="$BIN_DIR/meerkly"
    info "installed $MEERKLY"

    case ":$PATH:" in
      *":$BIN_DIR:"*) ;;
      *) NOT_ON_PATH="$BIN_DIR" ;;
    esac
    ;;
esac

# The package puts the binary at a known path, but a distribution may relocate
# it and Homebrew's prefix moves between Intel and Apple silicon. Trust the
# expected path only if something executable is actually there, and fall back to
# whatever the shell now resolves — an install that succeeded but left nothing
# runnable deserves a sentence, not "No such file or directory".
if [ ! -x "$MEERKLY" ]; then
  MEERKLY=$(command -v meerkly 2>/dev/null || true)
fi
if [ -z "$MEERKLY" ] || [ ! -x "$MEERKLY" ]; then
  die "installed, but no meerkly binary was found afterwards. Open a shell and try: meerkly init $PUBLISHER_ID"
fi

# ---- configure and start -----------------------------------------------------

# `init` is the agent's own one-command setup: it stores the id and registers
# the service. Using it here rather than reimplementing either step means this
# script cannot drift from what `meerkly init` does.
if [ "$INSTALL_SERVICE" = "1" ]; then
  if [ "$os" = "Linux" ] && [ "$(id -u)" != "0" ] && have sudo; then
    # Registering a system unit needs root. The service still runs as the user
    # who installed it, which is why the config stays in that user's home.
    sudo -E "$MEERKLY" init "$PUBLISHER_ID"
  else
    "$MEERKLY" init "$PUBLISHER_ID"
  fi
else
  "$MEERKLY" init "$PUBLISHER_ID" --no-service
fi

say ""
say "${BOLD}meerkly is installed.${OFF}"
say "  meerkly status        how it is doing"
say "  meerkly config show   what it is configured with"
case "$METHOD" in
  brew) say "  brew upgrade meerkly  update it" ;;
  apt)  say "  sudo apt remove meerkly   remove it" ;;
  dnf)  say "  sudo dnf remove meerkly   remove it" ;;
esac
if [ -n "${NOT_ON_PATH:-}" ]; then
  say ""
  say "  Note: $NOT_ON_PATH is not on your PATH."
fi
