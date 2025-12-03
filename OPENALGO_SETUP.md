# OpenAlgo Integration Guide

## Overview

OpenAlgo is a self-hosted algo trading bridge for Indian markets that supports 24+ brokers including:
- Zerodha (Kite Connect)
- Fyers
- Dhan
- Angel One
- Upstox
- And many more...

## Quick Start

### 1. Paper Trading (Sandbox Mode)

Start the execution service with OpenAlgo in sandbox mode:

```bash
# Start with OpenAlgo enabled
cd services/execution
docker compose --profile indian-markets up -d

# Check health
curl http://localhost:5000/api/v1/health
```

### 2. Environment Configuration

Create a `.env` file in `services/execution/`:

```env
# OpenAlgo Configuration
OPENALGO_URL=http://openalgo:5000
OPENALGO_SANDBOX=true          # Set to false for live trading
OPENALGO_BROKER=paper          # Change to: zerodha, fyers, dhan, angel

# Broker API Credentials (required for live trading)
OPENALGO_API_KEY=your_api_key
OPENALGO_API_SECRET=your_api_secret
```

### 3. Trading Modes

| Mode | `OPENALGO_SANDBOX` | Description |
|------|-------------------|-------------|
| Paper Trading | `true` | Simulated execution, no real orders |
| Forward Testing | `true` | Live data, virtual fills for validation |
| Live Trading | `false` | Real execution with broker |

## Using the Plugin

### Rust Code Example

```rust
use fks_execution::plugins::openalgo::{OpenAlgoPlugin, OpenAlgoConfig};
use fks_execution::plugins::{ExecutionPlugin, Order, OrderSide, OrderType};

// Initialize plugin
let mut plugin = OpenAlgoPlugin::new("openalgo");
plugin.init(serde_json::json!({
    "base_url": "http://localhost:5000",
    "api_key": "your_api_key",
    "sandbox_mode": true,
    "broker": "paper"
})).await?;

// Execute an order
let order = Order {
    symbol: "RELIANCE".to_string(),  // NSE:RELIANCE
    side: OrderSide::Buy,
    order_type: OrderType::Market,
    quantity: 10.0,
    price: None,
    stop_loss: Some(2400.0),
    take_profit: Some(2600.0),
    confidence: 0.85,
};

let result = plugin.execute_order(order).await?;
println!("Order ID: {:?}", result.order_id);

// Fetch market data
let quote = plugin.fetch_data("RELIANCE").await?;
println!("LTP: {}", quote.last);
```

### Symbol Formats

| FKS Format | OpenAlgo Format | Exchange |
|------------|-----------------|----------|
| `RELIANCE` | `RELIANCE` | NSE (default) |
| `RELIANCE-BSE` | `RELIANCE` | BSE |
| `INFY` | `INFY` | NSE |
| `NIFTYFUT` | `NIFTYFUT` | NFO |

## API Endpoints

The plugin uses these OpenAlgo API endpoints:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/orders` | POST | Place new order |
| `/api/v1/quote` | GET | Get market quote |
| `/api/v1/health` | GET | Health check |
| `/api/v1/positions` | GET | Get open positions |
| `/api/v1/orders` | GET | Get order book |

## Product Types

| Type | Code | Description |
|------|------|-------------|
| Intraday | `MIS` | Margin Intraday Square-off |
| Delivery | `CNC` | Cash and Carry |
| F&O Normal | `NRML` | Futures & Options |

## Safety Features

1. **Sandbox by Default**: `OPENALGO_SANDBOX=true` is the default
2. **Health Check**: Plugin verifies OpenAlgo connection on init
3. **Retry Logic**: Automatic retries for failed requests
4. **Timeout**: Configurable request timeout (default: 30s)

## Troubleshooting

### OpenAlgo not connecting

```bash
# Check if OpenAlgo is running
docker ps | grep openalgo

# Check logs
docker logs fks-openalgo

# Test health endpoint directly
curl http://localhost:5000/api/v1/health
```

### Orders not executing

1. Check `OPENALGO_SANDBOX` setting
2. Verify API credentials
3. Ensure market hours (NSE: 9:15 AM - 3:30 PM IST)
4. Check broker connection in OpenAlgo dashboard

### Broker-specific setup

Visit: http://localhost:5000 after starting OpenAlgo to configure your broker connection through the web interface.

## Production Checklist

Before going live:

- [ ] Test thoroughly in sandbox mode
- [ ] Set appropriate position size limits
- [ ] Configure daily loss limits
- [ ] Set up monitoring alerts
- [ ] Implement kill switch
- [ ] Document risk management rules
- [ ] Test during market hours

## Related Files

- Plugin: `src/plugins/openalgo.rs`
- Docker: `docker-compose.yml`
- Tasks: `/infrastructure/docs/01-PLANNING/CURRENT_ACTIVE_TASKS.md`
