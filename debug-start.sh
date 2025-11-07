#!/bin/bash
echo "Starting FKS Execution Service..."
echo "Binary path: $(which fks_execution)"
echo "Binary exists: $(test -f /usr/local/bin/fks_execution && echo 'YES' || echo 'NO')"
echo "Binary permissions: $(ls -la /usr/local/bin/fks_execution)"

# Try to run the binary
echo "Attempting to start service..."
exec /usr/local/bin/fks_execution --listen 0.0.0.0:4700
