#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT_DIR"

mkdir -p dist/matrix

build() {
  local goos="$1"
  local goarch="$2"
  local goarm="${3:-}"
  local ext=""
  local ldflags="-s -w"
  local cgo=0

  if [[ "$goos" == "windows" ]]; then
    ext=".exe"
    ldflags="-s -w -H=windowsgui -buildid="
    cgo=1
  fi

  local out="dist/matrix/lanPrint-${goos}-${goarch}"
  if [[ -n "$goarm" ]]; then
    out+="v${goarm}"
  fi
  out+="$ext"

  echo "Building $out"
  if [[ -n "$goarm" ]]; then
    GOOS="$goos" GOARCH="$goarch" GOARM="$goarm" CGO_ENABLED="$cgo" go build -trimpath -ldflags "$ldflags" -o "$out" ./cmd/lanPrint
  else
    GOOS="$goos" GOARCH="$goarch" CGO_ENABLED="$cgo" go build -trimpath -ldflags "$ldflags" -o "$out" ./cmd/lanPrint
  fi
}

build windows amd64
build windows 386
build darwin amd64
build darwin arm64
build linux amd64
build linux 386
build linux arm64
build linux arm 6
build linux arm 7

echo "Build matrix done: dist/matrix"
