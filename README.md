# FKS Execution - Rust Execution Engine

**Port**: 8004  
**Framework**: Rust + Actix-web (or Axum)  
**Role**: High-performance order execution - ONLY service that communicates with exchanges/brokers

## Overview

FKS Execution is the **ONLY service** in the FKS Trading Platform that directly communicates with exchanges and brokers. It provides:

- **Order Lifecycle Management**: FSM-based order state tracking
- **Position Tracking**: Real-time position updates
- **Exchange Integration**: Binance, Coinbase, Kraken APIs
- **High Performance**: Rust for low-latency execution
- **Fault Tolerance**: Circuit breakers and retry logic

**Critical Principle**: NO OTHER SERVICE should talk to exchanges directly. All market orders flow through fks_execution.

## Architecture Principles

### What FKS Execution DOES

✅ Execute market/limit orders on exchanges  
✅ Track order states (pending → filled → settled)  
✅ Manage positions and update balances  
✅ Handle exchange API rate limits  
✅ Retry failed orders with exponential backoff  
✅ Validate orders before submission  
✅ Report execution results to fks_app  

### What FKS Execution DOES NOT DO

❌ NO trading logic or signal generation (use fks_app)  
❌ NO portfolio optimization (use fks_app)  
❌ NO data collection for market data (use fks_data)  
❌ ONLY executes orders, does not decide what to trade  

## Tech Stack

- **Language**: Rust (stable)
- **Web Framework**: Actix-web 4.x or Axum 0.7.x
- **Async Runtime**: Tokio
- **HTTP Client**: reqwest (async)
- **Serialization**: serde, serde_json
- **Exchange APIs**: ccxt-rust, custom wrappers
- **Database**: PostgreSQL client (tokio-postgres)
- **Monitoring**: Prometheus metrics

## API Endpoints

### Orders

- `POST /orders` - Submit new order
- `GET /orders/{order_id}` - Get order status
- `PUT /orders/{order_id}/cancel` - Cancel order
- `GET /orders` - List orders with filters

### Positions

- `GET /positions` - Get all positions
- `GET /positions/{symbol}` - Get position for symbol
- `PUT /positions/{symbol}/close` - Close position

### Balances

- `GET /balances` - Get account balances
- `GET /balances/{asset}` - Get balance for specific asset

### Health

- `GET /health` - Service health check
- `GET /metrics` - Prometheus metrics

## Directory Structure

```
repo/execution/
├── src/
│   ├── main.rs              # Entry point
│   ├── lib.rs               # Library root
│   ├── api/                 # HTTP endpoints
│   │   ├── mod.rs
│   │   ├── orders.rs        # Order endpoints
│   │   ├── positions.rs     # Position endpoints
│   │   └── health.rs        # Health check
│   ├── exchanges/           # Exchange integrations
│   │   ├── mod.rs
│   │   ├── binance.rs       # Binance API wrapper
│   │   ├── coinbase.rs      # Coinbase API wrapper
│   │   └── traits.rs        # Common exchange traits
│   ├── models/              # Data models
│   │   ├── mod.rs
│   │   ├── order.rs         # Order model + FSM
│   │   ├── position.rs      # Position model
│   │   └── balance.rs       # Balance model
│   ├── state/               # Order state machine
│   │   ├── mod.rs
│   │   └── fsm.rs           # Finite State Machine
│   ├── db/                  # Database client
│   │   ├── mod.rs
│   │   └── postgres.rs      # PostgreSQL queries
│   └── utils/
│       ├── mod.rs
│       ├── retry.rs         # Retry logic
│       └── circuit_breaker.rs # Circuit breaker
├── tests/
│   ├── integration/         # Integration tests
│   └── unit/                # Unit tests
├── Cargo.toml               # Rust dependencies
├── Dockerfile               # Container definition
└── README.md                # This file
```

## Development Setup

### Prerequisites

- Rust (stable, 1.70+)
- Docker + Docker Compose
- PostgreSQL (for order/position storage)
- Exchange API keys (Binance, Coinbase, etc.)

### Local Development

```bash
# Install Rust (if not installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build
cargo build

# Run tests
cargo test

# Run locally (dev mode)
cargo run

# Run in Docker
docker-compose up fks_execution
```

### Environment Variables

```bash
# Service configuration
FKS_EXECUTION_PORT=8004
FKS_EXECUTION_HOST=0.0.0.0

# Database
DATABASE_URL=postgresql://fks_user:password@db:5432/trading_db

# Exchange API keys (Binance)
BINANCE_API_KEY=your_api_key
BINANCE_SECRET_KEY=your_secret_key
BINANCE_TESTNET=true

# Exchange API keys (Coinbase)
COINBASE_API_KEY=your_api_key
COINBASE_SECRET_KEY=your_secret_key

# Rate limiting
MAX_ORDERS_PER_SECOND=10
CIRCUIT_BREAKER_THRESHOLD=5
CIRCUIT_BREAKER_TIMEOUT_SECS=60

# Feature flags
ENABLE_PAPER_TRADING=true
ENABLE_LIVE_TRADING=false

# Logging
RUST_LOG=info
```

## Order Lifecycle (FSM)

```
          ┌──────────┐
          │  PENDING │
          └────┬─────┘
               │
     ┌─────────┴─────────┐
     │                   │
┌────▼────┐        ┌────▼────┐
│ FILLED  │        │ REJECTED│
└────┬────┘        └─────────┘
     │
┌────▼────┐
│ SETTLED │
└─────────┘
```

**States**:
- `Pending`: Order submitted to exchange, awaiting confirmation
- `Filled`: Order executed successfully
- `Settled`: Order confirmed and position updated
- `Rejected`: Order rejected by exchange
- `Cancelled`: Order cancelled by user

## Exchange Integration

### Binance API

```rust
use crate::exchanges::binance::BinanceClient;

let client = BinanceClient::new(api_key, secret_key);

// Market order
let order = client.create_market_order("BTCUSDT", "BUY", 0.01).await?;

// Limit order
let order = client.create_limit_order("BTCUSDT", "SELL", 0.01, 50000.0).await?;

// Check order status
let status = client.get_order_status("BTCUSDT", order_id).await?;
```

### Rate Limiting

```rust
use crate::utils::RateLimiter;

let limiter = RateLimiter::new(10, Duration::from_secs(1)); // 10 req/sec

limiter.wait().await; // Blocks until rate limit allows
let result = exchange.submit_order(order).await?;
```

### Circuit Breaker

```rust
use crate::utils::CircuitBreaker;

let breaker = CircuitBreaker::new(5, Duration::from_secs(60)); // 5 failures, 60s timeout

match breaker.call(|| exchange.submit_order(order)).await {
    Ok(result) => println!("Order submitted: {:?}", result),
    Err(e) => eprintln!("Circuit open: {}", e),
}
```

## Testing

```bash
# Unit tests (no external dependencies)
cargo test --lib

# Integration tests (requires testnet API keys)
export BINANCE_TESTNET=true
cargo test --test integration_tests

# Coverage (requires tarpaulin)
cargo install cargo-tarpaulin
cargo tarpaulin --out Html

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt --check
```

## Deployment

### Docker Build

```bash
docker build -t fks_execution:latest .
```

### Health Checks

- **Endpoint**: `GET /health`
- **Expected**: `{"status": "healthy", "service": "fks_execution", "exchange_connected": true}`
- **Dependencies**: Exchange APIs, PostgreSQL

## Performance Considerations

- **Low Latency**: Rust provides <1ms overhead for order submission
- **Concurrent Orders**: Tokio async runtime handles thousands of concurrent requests
- **Connection Pooling**: Reuse HTTP connections to exchanges
- **Rate Limiting**: Respect exchange limits (Binance: 10 req/sec, 100,000 req/day)
- **Circuit Breaker**: Prevent cascading failures during exchange outages

## Common Issues

**Exchange connection fails**:
- Verify API keys are correct
- Check if IP is whitelisted on exchange
- Ensure TESTNET mode is enabled for development
- Check exchange status page for outages

**Order rejected**:
- Insufficient balance
- Invalid order parameters (price, quantity)
- Symbol not tradable
- Exchange maintenance window

**Rate limit exceeded**:
- Reduce MAX_ORDERS_PER_SECOND
- Implement request batching
- Use WebSocket for market data (not REST)

**Position tracking mismatch**:
- Reconcile positions with exchange every 5 minutes
- Check for manual trades outside FKS
- Verify database consistency

## Security Considerations

- **API Keys**: Store in environment variables, NEVER in code
- **Testnet First**: Always test on testnet before live trading
- **Paper Trading**: Use paper trading mode for validation
- **Position Limits**: Enforce maximum position size limits
- **Order Validation**: Validate all orders before submission

## Integration with FKS App

FKS App sends execution signals to fks_execution:

```json
POST http://fks_execution:8004/orders
{
  "symbol": "BTCUSDT",
  "side": "BUY",
  "order_type": "MARKET",
  "quantity": 0.01,
  "strategy_id": "rsi_btc_001",
  "signal_id": "signal_123"
}
```

FKS Execution responds with order confirmation:

```json
{
  "order_id": "order_456",
  "status": "PENDING",
  "exchange_order_id": "binance_789",
  "timestamp": "2025-10-24T12:00:00Z"
}
```

## Contributing

1. Write tests for new exchange integrations
2. Follow Rust best practices (clippy, rustfmt)
3. Document error handling and retry logic
4. Test on testnet before live trading
5. Update FSM for new order states

## License

MIT License - See LICENSE file for details

---

**Status**: Active Development  
**Maintainer**: FKS Trading Platform Team  
**Last Updated**: October 2025
