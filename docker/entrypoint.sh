#!/bin/sh
# Entrypoint script for fks_execution service
# Standardized entrypoint for FKS services

set -e

echo "🚀 Starting fks_execution service..." >&2

# Run the service with proper output handling
# Use unbuffered output and capture both stdout and stderr
exec stdbuf -o0 -e0 ./fks_execution 2>&1 || {
    EXIT=$?
    echo "❌ Process exited with code: $EXIT" >&2
    sleep 10
    exit $EXIT
}
