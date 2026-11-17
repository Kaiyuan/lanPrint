#!/usr/bin/env bash
# lanPrint desktop one-line installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install.sh | bash
#
# Environment:
#   LANPRINT_VERSION   release tag, default: latest
#   LANPRINT_PREFIX    install prefix, default: /usr/local

set -euo pipefail

REPO="Kaiyuan/lanPrint"
VERSION="${LANPRINT_VERSION:-latest}"
PREFIX="${LANPRINT_PREFIX:-/usr/local}"

os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) echo "Unsupported architecture: $arch"; exit 1 ;;
esac

case "$os" in
  Linux) platform="Linux" ;;
  Darwin) platform="Mac" ;;
  *) echo "Unsupported operating system: $os"; exit 1 ;;
esac

if [[ "$VERSION" == "latest" ]]; then
  url="https://github.com/${REPO}/releases/latest/download/lanPrint_latest_${platform}_${arch}.tar.gz"
  # The release workflow names assets with the actual version. Resolve it via
  # the GitHub API when possible, otherwise fall back to a stable redirect.
  tag="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1 || true)"
  if [[ -n "${tag:-}" ]]; then
    ver="${tag#v}"
    url="https://github.com/${REPO}/releases/download/${tag}/lanPrint_${ver}_${platform}_${arch}.tar.gz"
  fi
else
  tag="$VERSION"
  [[ "$tag" == v* ]] || tag="v$tag"
  ver="${tag#v}"
  url="https://github.com/${REPO}/releases/download/${tag}/lanPrint_${ver}_${platform}_${arch}.tar.gz"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $url"
curl -fL "$url" -o "$tmp/lanPrint.tar.gz"
tar -xzf "$tmp/lanPrint.tar.gz" -C "$tmp"
bin="$(find "$tmp" -type f -name lanPrint | head -n1)"
if [[ -z "$bin" ]]; then
  echo "The archive did not contain lanPrint"; exit 1
fi

if [[ "$platform" == "Mac" ]]; then
  app="$(find "$tmp" -maxdepth 3 -type d -name '*.app' | head -n1)"
  if [[ -n "$app" ]]; then
    sudo rm -rf "/Applications/$(basename "$app")"
    sudo cp -R "$app" /Applications/
    echo "Installed to /Applications/$(basename "$app")"
  else
    sudo install -m755 "$bin" "$PREFIX/bin/lanPrint"
    echo "Installed $PREFIX/bin/lanPrint"
  fi
else
  sudo install -Dm755 "$bin" "$PREFIX/bin/lanPrint"
  icon="$(find "$tmp" -type f -name lanprint.png | head -n1 || true)"
  desktop="$(find "$tmp" -type f -name lanprint.desktop | head -n1 || true)"
  service="$(find "$tmp" -type f -name lanprint.service | head -n1 || true)"
  [[ -n "$icon" ]] && sudo install -Dm644 "$icon" /usr/share/icons/hicolor/256x256/apps/lanprint.png
  [[ -n "$desktop" ]] && sudo install -Dm644 "$desktop" /usr/share/applications/lanprint.desktop
  [[ -n "$service" ]] && sudo install -Dm644 "$service" /usr/lib/systemd/user/lanprint.service
  # CUPS backend used by client queues
  if [[ -d /usr/lib/cups/backend ]]; then
    sudo ln -sf "$PREFIX/bin/lanPrint" /usr/lib/cups/backend/lanprint
  elif [[ -d /usr/libexec/cups/backend ]]; then
    sudo ln -sf "$PREFIX/bin/lanPrint" /usr/libexec/cups/backend/lanprint
  fi
  echo "Installed $PREFIX/bin/lanPrint"
  echo "Run 'lanPrint --tray' or start lanPrint from the application menu."
fi
