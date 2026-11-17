#!/usr/bin/env bash
# Install lanPrint from a release tarball on Linux.
# Run after extracting the archive: sudo ./install.sh
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "Please run with sudo: sudo ./install.sh"
  exit 1
fi

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$DIR/lanPrint"
if [[ ! -x "$BIN" ]]; then
  echo "lanPrint binary not found next to install.sh"
  exit 1
fi

install -Dm755 "$BIN" /usr/bin/lanPrint
install -Dm644 "$DIR/lanprint.png" /usr/share/icons/hicolor/256x256/apps/lanprint.png
install -Dm644 "$DIR/lanprint.desktop" /usr/share/applications/lanprint.desktop
install -Dm644 "$DIR/lanprint.service" /usr/lib/systemd/user/lanprint.service

# CUPS backend symlink. It is only used for client queues created by lanPrint.
if [[ -d /usr/lib/cups/backend ]]; then
  ln -sf /usr/bin/lanPrint /usr/lib/cups/backend/lanprint
elif [[ -d /usr/libexec/cups/backend ]]; then
  ln -sf /usr/bin/lanPrint /usr/libexec/cups/backend/lanprint
fi

update-desktop-database /usr/share/applications 2>/dev/null || true
gtk-update-icon-cache -q /usr/share/icons/hicolor 2>/dev/null || true

echo "lanPrint installed."
echo "Start it from the application menu, or run: lanPrint --tray"
