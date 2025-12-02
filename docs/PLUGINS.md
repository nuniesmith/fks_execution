# Execution Service Plugins

The execution service uses a plugin-based architecture for modular exchange/broker integration.

## Plugin Architecture

### Core Components

1. **ExecutionPlugin Trait** (`src/plugins/mod.rs`)
   - Defines the interface all plugins must implement
   - Methods: `init()`, `execute_order()`, `fetch_data()`, `health_check()`

2. **PluginRegistry** (`src/plugins/registry.rs`)
   - Manages multiple plugins
   - Routes orders to appropriate backend
   - Supports default plugin selection

3. **Plugin Implementations**
   - `mock.rs` - Mock plugin for testing
   - `ccxt.rs` - CCXT integration via HTTP API

## Available Plugins

### Mock Plugin

**Purpose**: Testing and development without real exchange connections

**Features**:
- Simulates order execution with realistic slippage
- Generates mock market data
- Fast execution for testing

**Usage**:
```rust
let mut plugin = MockPlugin::new("test");
plugin.init(serde_json::json!({})).await?;
```

### CCXT Plugin

**Purpose**: Integrate with external CCXT services via HTTP

**Configuration**:
```json
{
  "base_url": "http://localhost:8000",
  "webhook_secret": "your-secret",
  "exchange": "binance",
  "testnet": false
}
```

**Features**:
- HTTP-based integration (no direct exchange connection)
- TradingView webhook format support
- HMAC-SHA256 signature verification
- Health check on initialization

**Environment Variables**:
- `CCXT_BASE_URL` - Base URL of CCXT service
- `WEBHOOK_SECRET` - Secret for webhook signature verification
- `EXCHANGE` - Exchange name (default: "binance")
- `TESTNET` - Use testnet (default: "false")

## Plugin Registration

Plugins are registered in `main.rs` during service startup:

```rust
let registry = Arc::new(PluginRegistry::new());

// Register CCXT plugin
let mut ccxt = CCXTPlugin::new("binance");
ccxt.init(config).await?;
registry.register("binance".to_string(), Arc::new(ccxt)).await;
```

## Order Execution Flow

1. Webhook/API request received
2. Convert to `Order` struct
3. Route to plugin via `PluginRegistry::execute_order()`
4. Plugin executes order on exchange/broker
5. Return `ExecutionResult` with order details

## Adding New Plugins

To add a new plugin:

1. Implement `ExecutionPlugin` trait
2. Add plugin module to `src/plugins/mod.rs`
3. Register plugin in `main.rs`
4. Add tests in plugin module

Example:
```rust
pub struct MyPlugin {
    // Plugin state
}

#[async_trait]
impl ExecutionPlugin for MyPlugin {
    async fn init(&mut self, config: serde_json::Value) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Initialize plugin
        Ok(())
    }
    
    async fn execute_order(&self, order: Order) -> Result<ExecutionResult, Box<dyn Error + Send + Sync>> {
        // Execute order
    }
    
    // ... implement other methods
}
```

## Testing

All plugins include unit tests. Run with:
```bash
cargo test
```

Mock plugin is used for integration tests to avoid real exchange connections.

