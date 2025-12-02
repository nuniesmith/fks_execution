#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "[execution] Stopping existing containers..."
docker compose down

echo "[execution] Rebuilding images..."
docker compose build

echo "[execution] Starting containers in detached mode..."
docker compose up -d
