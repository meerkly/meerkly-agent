#!/bin/sh
# The service runs as the user who installed the package, not as root, so its
# configuration lives in that user's own home and never needs sudo to edit.
# apt/dnf run this as root; $SUDO_USER is how we recover who that actually was.
#
# Argument conventions differ. dpkg's postinst gets "configure <old-version>",
# the version being empty on a fresh install. rpm's %post gets a count: 1 for a
# fresh install, 2 or more for an upgrade.
set -e

MEERKLY="${MEERKLY_BIN:-/usr/bin/meerkly}"

UPGRADE=''
case "${1:-}" in
  configure) [ -n "${2:-}" ] && UPGRADE=1 ;;
  [2-9]*) UPGRADE=1 ;;
esac

# On an upgrade the unit is still registered (prerm leaves it alone now) but the
# running process is the old binary until it is restarted. Restart it — as root,
# straight through systemctl, which needs no $SUDO_USER.
if [ -n "$UPGRADE" ] && command -v systemctl >/dev/null 2>&1; then
  if systemctl cat meerkly >/dev/null 2>&1; then
    systemctl daemon-reload || true
    systemctl restart meerkly || true
  fi
  exit 0
fi

# The meerkly.com installer drives the whole first run itself and sets this. It
# is about to store the id and register the service; instructions printed from
# inside apt telling the person to do that by hand would only confuse.
if [ -n "${MEERKLY_INSTALLER:-}" ]; then
  exit 0
fi

TARGET_USER="${SUDO_USER:-}"

if [ -z "$TARGET_USER" ] || [ "$TARGET_USER" = "root" ]; then
  cat <<'MSG'

meerkly is installed.

  Could not tell which user the service should run as, so it was not started.
  Finish the install as yourself:

    sudo meerkly init

MSG
  exit 0
fi

# Only auto-start when there is already something to connect with; otherwise the
# service would come up, fail to find a publisher id and restart in a loop.
if sudo -u "$TARGET_USER" "$MEERKLY" config get publisher-id >/dev/null 2>&1; then
  "$MEERKLY" service install || true
  echo "meerkly is running. Check it with: meerkly status"
else
  cat <<'MSG'

meerkly is installed. One command to start earning:

  sudo meerkly init      # asks for your publisher id, then starts the service

MSG
fi
