#!/usr/bin/env bash
set -euo pipefail

API_PORT="${1:-52333}"
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT_DIR"

go build -o dist/lanPrint ./cmd/lanPrint
./dist/lanPrint &
PID=$!
sleep 2

curl -sS "http://127.0.0.1:${API_PORT}/api/v1/stats" >/dev/null

echo "Server verify done. PID=${PID}. Keep machine online for client test."
