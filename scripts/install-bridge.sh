#!/usr/bin/env bash
# One-line installer for the lanPrint cloud bridge server.
#
# Linux (systemd):
#   curl -fsSL https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install-bridge.sh | sudo bash
#
# macOS (launchd, run with sudo):
#   curl -fsSL https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install-bridge.sh | sudo bash
#
# Environment:
#   LANPRINT_VERSION        release tag, default: latest
#   LANPRINT_BRIDGE_AUTH_CODE  server access code; generated when omitted
#   LANPRINT_BRIDGE_PUBLIC_URL public https URL for responses
#   LANPRINT_BRIDGE_PORT    listen port, default 52334

set -euo pipefail

REPO="Kaiyuan/lanPrint"
VERSION="${LANPRINT_VERSION:-latest}"
PORT="${LANPRINT_BRIDGE_PORT:-52334}"
AUTH_CODE="${LANPRINT_BRIDGE_AUTH_CODE:-}"
PUBLIC_URL="${LANPRINT_BRIDGE_PUBLIC_URL:-}"
DATA_DIR="/var/lib/lanprint-bridge"
CONF_DIR="/etc/lanprint-bridge"
BIN="/usr/local/bin/lanprint-bridge"

if [[ $EUID -ne 0 ]]; then
  echo "Please run this installer with sudo."
  exit 1
fi

os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) echo "Unsupported architecture: $arch"; exit 1 ;;
esac
case "$os" in
  Linux) platform="Linux" ;;
  Darwin) platform="Darwin" ;;
  *) echo "Unsupported operating system: $os"; exit 1 ;;
esac

if [[ -z "$AUTH_CODE" ]]; then
  if command -v openssl >/dev/null 2>&1; then
    AUTH_CODE="$(openssl rand -base64 24 | tr -d '\n')"
  else
    AUTH_CODE="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
  fi
  echo "Generated server access code: $AUTH_CODE"
fi

if [[ "$VERSION" == "latest" ]]; then
  tag="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1 || true)"
  [[ -n "${tag:-}" ]] || { echo "Could not resolve the latest release"; exit 1; }
else
  tag="$VERSION"
  [[ "$tag" == v* ]] || tag="v$tag"
fi
ver="${tag#v}"
url="https://github.com/${REPO}/releases/download/${tag}/lanprint-bridge_${ver}_${platform}_${arch}.tar.gz"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $url"
curl -fL "$url" -o "$tmp/bridge.tar.gz"
tar -xzf "$tmp/bridge.tar.gz" -C "$tmp"
found="$(find "$tmp" -type f -name lanprint-bridge | head -n1)"
[[ -n "$found" ]] || { echo "Archive did not contain lanprint-bridge"; exit 1; }
install -Dm755 "$found" "$BIN"
install -d -m700 "$DATA_DIR" "$CONF_DIR"
if [[ "$os" == "Linux" ]]; then
  if ! id -u lanprint >/dev/null 2>&1; then
    useradd --system --home-dir "$DATA_DIR" --shell /usr/sbin/nologin lanprint 2>/dev/null || true
  fi
  chown -R lanprint:lanprint "$DATA_DIR" 2>/dev/null || true
fi

cat > "$CONF_DIR/bridge.env" <<EOF
LANPRINT_BRIDGE_ADDR=0.0.0.0:${PORT}
LANPRINT_BRIDGE_AUTH_CODE=${AUTH_CODE}
LANPRINT_BRIDGE_DATA=${DATA_DIR}/hosts.json
LANPRINT_BRIDGE_PUBLIC_URL=${PUBLIC_URL}
RUST_LOG=info
EOF
chmod 600 "$CONF_DIR/bridge.env"

if [[ "$os" == "Linux" ]]; then
  cat > /etc/systemd/system/lanprint-bridge.service <<EOF
[Unit]
Description=lanPrint cloud bridge
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=${CONF_DIR}/bridge.env
ExecStart=${BIN}
Restart=on-failure
RestartSec=3
User=lanprint
Group=lanprint
StateDirectory=lanprint-bridge
# Allow the service to write the persistent share-code database.
ReadWritePaths=${DATA_DIR}

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable --now lanprint-bridge
  ufw allow "${PORT}/tcp" 2>/dev/null || true
  firewall-cmd --permanent --add-port="${PORT}/tcp" 2>/dev/null || true
  firewall-cmd --reload 2>/dev/null || true
  echo "lanPrint bridge is running on port ${PORT}."
  echo "Check: systemctl status lanprint-bridge"
else
  cat > /Library/LaunchDaemons/com.kaiyuan.lanprint-bridge.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.kaiyuan.lanprint-bridge</string>
  <key>ProgramArguments</key><array><string>${BIN}</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>LANPRINT_BRIDGE_ADDR</key><string>0.0.0.0:${PORT}</string>
    <key>LANPRINT_BRIDGE_AUTH_CODE</key><string>${AUTH_CODE}</string>
    <key>LANPRINT_BRIDGE_DATA</key><string>${DATA_DIR}/hosts.json</string>
    <key>LANPRINT_BRIDGE_PUBLIC_URL</key><string>${PUBLIC_URL}</string>
    <key>RUST_LOG</key><string>info</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${DATA_DIR}/bridge.log</string>
  <key>StandardErrorPath</key><string>${DATA_DIR}/bridge.log</string>
</dict></plist>
EOF
  launchctl unload /Library/LaunchDaemons/com.kaiyuan.lanprint-bridge.plist 2>/dev/null || true
  launchctl load /Library/LaunchDaemons/com.kaiyuan.lanprint-bridge.plist
  echo "lanPrint bridge is running on port ${PORT}."
fi

cat <<EOF

Bridge server:  http://<this-host>:${PORT}
Server code:    ${AUTH_CODE}
Data file:      ${DATA_DIR}/hosts.json

Put this service behind HTTPS (Caddy, Nginx or Traefik) before exposing it
to the internet. Clients then enter https://your-domain as the bridge URL.
EOF
