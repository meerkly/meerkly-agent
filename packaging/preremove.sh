#!/bin/sh
# Remove the unit only when the package itself is being removed. Never the
# user's configuration, and never on an upgrade: an upgrade used to run this
# too, which took the service down on every `apt upgrade` — and left it down
# whenever nobody was there to run `meerkly init` again.
#
# Argument conventions differ. dpkg's prerm gets a word: remove, upgrade,
# deconfigure or failed-upgrade. rpm's %preun gets a count of installed
# versions remaining: 0 for a removal, 1 or more for an upgrade.
set -e

case "${1:-}" in
  upgrade|deconfigure|failed-upgrade) exit 0 ;;
  [1-9]*) exit 0 ;;
esac

MEERKLY="${MEERKLY_BIN:-/usr/bin/meerkly}"
if [ -x "$MEERKLY" ]; then
  "$MEERKLY" service uninstall >/dev/null 2>&1 || true
fi
