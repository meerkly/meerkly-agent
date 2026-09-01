#!/bin/sh
# Remove the unit but never the user's configuration — an upgrade runs this too,
# and deleting a publisher id on upgrade would silently stop the machine earning.
set -e
if [ -x /usr/bin/meerkly ]; then
  /usr/bin/meerkly service uninstall >/dev/null 2>&1 || true
fi
