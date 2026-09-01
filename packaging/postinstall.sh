#!/bin/sh
# The service runs as the user who installed the package, not as root, so its
# configuration lives in that user's own home and never needs sudo to edit.
# apt/dnf run this as root; $SUDO_USER is how we recover who that actually was.
set -e

TARGET_USER="${SUDO_USER:-}"

if [ -z "$TARGET_USER" ] || [ "$TARGET_USER" = "root" ]; then
  cat <<'MSG'

meerkly is installed.

  Could not tell which user the service should run as, so it was not started.
  Finish the install as yourself:

    meerkly config set publisher-id pub_…
    sudo meerkly service install

MSG
  exit 0
fi

# Only auto-start when there is already something to connect with; otherwise the
# service would come up, fail to find a publisher id and restart in a loop.
if sudo -u "$TARGET_USER" /usr/bin/meerkly config get publisher-id >/dev/null 2>&1; then
  /usr/bin/meerkly service install || true
  echo "meerkly is running. Check it with: meerkly status"
else
  cat <<'MSG'

meerkly is installed. Two steps to start earning:

  meerkly config set publisher-id pub_…    # from https://dashboard.meerkly.com
  sudo meerkly service install

MSG
fi
