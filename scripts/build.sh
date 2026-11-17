#!/usr/bin/env bash
# One-click release build for Linux and macOS.
#
#   bash scripts/build.sh
#
# Outputs packages to dist/. Requires a Rust toolchain (rustup) and the usual
# platform build tools. Linux also needs libxkbcommon-dev, libwayland-dev,
# libx11-dev and libfontconfig1-dev.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install Rust from https://rustup.rs/ first." >&2
  exit 1
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
VERSION="${VERSION:-0.0.0}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

echo "==> Building lanPrint $VERSION (release)"
cargo build --release --locked -p lanprint-desktop -p lanprint-bridge

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
esac

OUT="dist/lanPrint_${VERSION}_$( [[ "$OS" == Darwin ]] && echo Mac || echo Linux )_${ARCH}"
mkdir -p "$OUT"

cp "$TARGET_DIR/release/lanPrint" "$OUT/lanPrint"
chmod +x "$OUT/lanPrint"
cp crates/desktop/ui/assets/lanprint-icon.png "$OUT/lanprint.png"
cp README.md README_EN.md LICENSE "$OUT/"

if [[ "$OS" == Darwin ]]; then
  APP="dist/LanPrint.app"
  rm -rf "$APP"
  mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
  cp "$TARGET_DIR/release/lanPrint" "$APP/Contents/MacOS/lanPrint"
  cp crates/desktop/ui/assets/lanprint-icon.png "$APP/Contents/Resources/lanprint.png"
  cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>lanPrint</string>
  <key>CFBundleDisplayName</key><string>lanPrint</string>
  <key>CFBundleIdentifier</key><string>com.kaiyuan.lanprint</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>lanPrint</string>
  <key>CFBundleIconFile</key><string>lanprint.png</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>10.15</string>
</dict></plist>
EOF
  tar -C dist -czf "dist/lanPrint_${VERSION}_Mac_${ARCH}.tar.gz" LanPrint.app
  rm -rf "$APP"
else
  cp packaging/linux/lanprint.desktop "$OUT/lanprint.desktop"
  cp packaging/linux/lanprint.service "$OUT/lanprint.service"
  cp packaging/linux/install-linux.sh "$OUT/install.sh"
  chmod +x "$OUT/install.sh"
  tar -C dist -czf "dist/lanPrint_${VERSION}_Linux_${ARCH}.tar.gz" "$(basename "$OUT")"
fi

cp "$TARGET_DIR/release/lanprint-bridge" "dist/lanprint-bridge_${VERSION}_$( [[ "$OS" == Darwin ]] && echo Darwin || echo Linux )_${ARCH}"
chmod +x "dist/lanprint-bridge_"*

echo
echo "Build complete:"
find dist -maxdepth 1 -type f -printf '  %f\n' 2>/dev/null || ls -1 dist
echo
echo "Desktop binary: $TARGET_DIR/release/lanPrint"
echo "Bridge binary:  $TARGET_DIR/release/lanprint-bridge"
