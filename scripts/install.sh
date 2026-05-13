#!/bin/bash

# lanPrint One-Click Installer for Linux & macOS
# Usage: curl -fsSL https://raw.githubusercontent.com/kaiyuan/lanPrint/main/scripts/install.sh | bash

set -e

# 1. Detect OS and Architecture
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$ARCH" in
    x86_64) ARCH="amd64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    armv7*) ARCH="armv7" ;;
    *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

# 2. Fetch Latest Version from GitHub
REPO="kaiyuan/lanPrint"
LATEST_TAG=$(curl -s https://api.github.com/repos/$REPO/releases/latest | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$LATEST_TAG" ]; then
    echo "Failed to fetch latest version tag."
    exit 1
fi

echo "Installing lanPrint $LATEST_TAG for $OS ($ARCH)..."

# 3. Download and Extract
PLATFORM_NAME=""
if [ "$OS" == "darwin" ]; then
    PLATFORM_NAME="Mac"
else
    PLATFORM_NAME="$(echo $OS | sed 's/./\u&/')"
fi

FILENAME="lanPrint_${PLATFORM_NAME}_${ARCH}.zip"
URL="https://github.com/$REPO/releases/download/$LATEST_TAG/$FILENAME"

TMP_DIR=$(mktemp -d)
curl -L "$URL" -o "$TMP_DIR/$FILENAME"
unzip -q "$TMP_DIR/$FILENAME" -d "$TMP_DIR"

# 4. Move Binary
BINARY_NAME="lanPrint"
if [ "$OS" == "windows" ]; then
    BINARY_NAME="lanPrint.exe"
fi

sudo mv "$TMP_DIR/$BINARY_NAME" /usr/local/bin/lanPrint
sudo chmod +x /usr/local/bin/lanPrint

echo "lanPrint installed successfully to /usr/local/bin/lanPrint"
echo "You can now run 'lanPrint -service install && lanPrint -service start' to start the service."

rm -rf "$TMP_DIR"
