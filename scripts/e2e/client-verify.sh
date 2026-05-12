#!/usr/bin/env bash
set -euo pipefail

SERVER_HOST="${1:-}"
API_PORT="${2:-52333}"
if [[ -z "$SERVER_HOST" ]]; then
  echo "Usage: scripts/e2e/client-verify.sh <server-host> [api-port]"
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT_DIR"

go build -o dist/lanPrint ./cmd/lanPrint
./dist/lanPrint &
PID=$!
sleep 2

curl -sS -X POST "http://127.0.0.1:${API_PORT}/api/v1/client/devices" \
  -H "Content-Type: application/json" \
  -d "{\"name\":\"remote-${SERVER_HOST}\",\"address\":\"${SERVER_HOST}\",\"port\":${API_PORT}}" >/dev/null

DEVICE_ID="$(curl -sS "http://127.0.0.1:${API_PORT}/api/v1/client/devices" | sed -n 's/.*"id":\([0-9][0-9]*\).*"address":"'"${SERVER_HOST}"'".*/\1/p' | head -n1)"
if [[ -z "$DEVICE_ID" ]]; then
  echo "Device add failed"
  kill "$PID"
  exit 1
fi

echo "Client verify started. PID=${PID}, device=${DEVICE_ID}" 
