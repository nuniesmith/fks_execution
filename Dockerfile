# Multi-stage build for Rust service
FROM rust:1.83-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy Cargo files (Cargo.lock is optional)
COPY Cargo.toml ./
COPY Cargo.lock* ./

# Copy source
COPY src/ ./src/

# Build release
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /app/target/release/fks_execution /app/fks_execution

# Create non-root user
RUN useradd -u 1000 -m appuser && chown -R appuser /app
USER appuser

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8004/health || exit 1

# Expose port
EXPOSE 8004

# Run service
CMD ["./fks_execution"]
