#!/usr/bin/env sh
set -eu
LOG=/app/logs/exec-wrapper.log
mkdir -p /app/logs || true
{
  echo "[wrapper] start $(date -u +%FT%TZ)"
  echo "[wrapper] launching fks_execution $*"
} >>"$LOG" 2>&1
/usr/local/bin/fks_execution --listen 0.0.0.0:4700 >>"$LOG" 2>&1 || echo "[wrapper] process exited code $?" >>"$LOG"
# Sleep indefinitely so container stays up for inspection
while true; do sleep 300; done
