# Multi-stage build for fks_execution Rust service
FROM rust:1.83-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy Cargo files first (for better dependency caching)
COPY Cargo.toml Cargo.lock* ./

# Copy actual source code
COPY src/ ./src/

# Build the application with BuildKit cache mount for Cargo registry
# This will build dependencies on first run, then use cache on subsequent builds
# Explicitly specify --bin to ensure the binary is built
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    RUSTFLAGS="-C debuginfo=1" cargo build --release --bin fks_execution && \
    cp target/release/fks_execution /app/fks_execution

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user first
RUN useradd -u 1000 -m -s /bin/bash appuser

# Copy binary from builder with correct ownership
COPY --from=builder --chown=appuser:appuser /app/fks_execution /app/fks_execution

# Copy entrypoint script
COPY docker/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

# Environment variables
ENV SERVICE_NAME=fks_execution_main \
    SERVICE_PORT=8004 \
    RUST_LOG=info

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8004/health || exit 1

# Expose port
EXPOSE 8004

# Switch to non-root user
USER appuser

# Use entrypoint script
ENTRYPOINT ["./entrypoint.sh"]
