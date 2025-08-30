# Custom Dockerfile for FKS Execution Service
FROM rust:slim AS build
WORKDIR /src

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src

# Copy and build real source
COPY src ./src
RUN cargo build --release --locked

# Runtime stage
FROM debian:bookworm-slim AS final
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Copy binary
COPY --from=build /src/target/release/fks_execution /usr/local/bin/fks_execution

# Create non-root user
# RUN useradd -r -u 1088 appuser
# USER appuser

EXPOSE 4700

CMD ["/usr/local/bin/fks_execution", "--listen", "0.0.0.0:4700"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:4700/health || exit 1
