"""
FKS Execution Service - CCXT Integration (Simplified)
Production-ready trading execution with CCXT exchange integration.

Differences from app_ccxt.py:
- Uses direct imports instead of Phase 3 relative imports
- Simplified ExchangeManager integration
- Full CCXT functionality with TradingView webhooks
"""

import os
import logging
import time
import hmac
import hashlib
import asyncio
import ipaddress
from contextlib import asynccontextmanager
from typing import Dict, Any, Optional, Set
from datetime import datetime
from collections import defaultdict, deque
from enum import Enum

from fastapi import FastAPI, Request, HTTPException, status
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field
import ccxt.async_support as ccxt
from prometheus_client import Counter, Histogram, Gauge, generate_latest, CONTENT_TYPE_LATEST

# Configure logging
log_format = os.getenv("LOG_FORMAT", "%(asctime)s - %(name)s - %(levelname)s - %(message)s")
if log_format.lower() == "json":
    log_format = "%(asctime)s - %(name)s - %(levelname)s - %(message)s"  # Fall back to standard format

logging.basicConfig(
    level=os.getenv("LOG_LEVEL", "INFO"),
    format=log_format
)
logger = logging.getLogger(__name__)

# Prometheus Metrics
webhook_requests = Counter('webhook_requests_total', 'Total webhook requests', ['status'])
order_executions = Counter('order_executions_total', 'Total order executions', ['exchange', 'status'])
processing_duration = Histogram('processing_duration_seconds', 'Request processing duration')
active_exchanges = Gauge('active_exchanges', 'Number of active exchange connections')
rate_limited_requests = Counter('rate_limited_requests_total', 'Total rate limited requests', ['ip'])
circuit_breaker_state = Gauge('circuit_breaker_state', 'Circuit breaker state (0=closed, 1=half_open, 2=open)')
ip_whitelist_rejections = Counter('ip_whitelist_rejections_total', 'Total IP whitelist rejections', ['ip'])

# Configuration
WEBHOOK_SECRET = os.getenv("WEBHOOK_SECRET", "default-secret-change-me")
MIN_CONFIDENCE = float(os.getenv("MIN_CONFIDENCE", "0.6"))
MAX_ORDER_SIZE_USD = float(os.getenv("MAX_ORDER_SIZE_USD", "10000"))
RISK_PER_TRADE = float(os.getenv("RISK_PER_TRADE", "0.02"))
MAX_POSITION_SIZE_USD = float(os.getenv("MAX_POSITION_SIZE_USD", "1000"))
DEFAULT_EXCHANGE = os.getenv("DEFAULT_EXCHANGE", "binance")
TESTNET = os.getenv("TESTNET", "true").lower() == "true"

# Security Configuration
RATE_LIMIT_REQUESTS = int(os.getenv("RATE_LIMIT_REQUESTS", "100"))
RATE_LIMIT_WINDOW = int(os.getenv("RATE_LIMIT_WINDOW", "60"))
CIRCUIT_BREAKER_THRESHOLD = int(os.getenv("CIRCUIT_BREAKER_THRESHOLD", "5"))
CIRCUIT_BREAKER_TIMEOUT = int(os.getenv("CIRCUIT_BREAKER_TIMEOUT", "60"))
IP_WHITELIST = os.getenv("IP_WHITELIST", "").split(",") if os.getenv("IP_WHITELIST") else []


# Security Middleware Classes

class CircuitBreakerState(str, Enum):
    """Circuit breaker states."""
    CLOSED = "closed"
    HALF_OPEN = "half_open"
    OPEN = "open"


class CircuitBreaker:
    """
    Circuit breaker pattern implementation.
    
    States:
    - CLOSED: Normal operation, requests pass through
    - OPEN: Too many failures, reject all requests
    - HALF_OPEN: Testing if service recovered
    """
    
    def __init__(self, threshold: int = 5, timeout: int = 60):
        self.threshold = threshold
        self.timeout = timeout
        self.failures = 0
        self.last_failure_time = 0
        self.state = CircuitBreakerState.CLOSED
        self._update_metric()
    
    def _update_metric(self):
        """Update Prometheus metric for circuit breaker state."""
        state_map = {
            CircuitBreakerState.CLOSED: 0,
            CircuitBreakerState.HALF_OPEN: 1,
            CircuitBreakerState.OPEN: 2
        }
        circuit_breaker_state.set(state_map[self.state])
    
    def record_success(self):
        """Record successful request."""
        if self.state == CircuitBreakerState.HALF_OPEN:
            self.state = CircuitBreakerState.CLOSED
            self.failures = 0
            logger.info("Circuit breaker closed - service recovered")
        self._update_metric()
    
    def record_failure(self):
        """Record failed request."""
        self.failures += 1
        self.last_failure_time = time.time()
        
        if self.failures >= self.threshold:
            self.state = CircuitBreakerState.OPEN
            logger.warning(f"Circuit breaker opened - {self.failures} failures")
        
        self._update_metric()
    
    def can_proceed(self) -> bool:
        """Check if request can proceed."""
        if self.state == CircuitBreakerState.CLOSED:
            return True
        
        if self.state == CircuitBreakerState.OPEN:
            # Check if timeout has passed
            if time.time() - self.last_failure_time >= self.timeout:
                self.state = CircuitBreakerState.HALF_OPEN
                logger.info("Circuit breaker half-open - testing service")
                self._update_metric()
                return True
            return False
        
        # HALF_OPEN state - allow one request through
        return True


class RateLimiter:
    """
    Token bucket rate limiter.
    
    Allows burst traffic up to bucket capacity, then enforces rate limit.
    """
    
    def __init__(self, requests: int = 100, window: int = 60):
        self.requests = requests
        self.window = window
        self.buckets: Dict[str, deque] = defaultdict(deque)
    
    def is_allowed(self, client_id: str) -> bool:
        """Check if request is allowed for client."""
        now = time.time()
        bucket = self.buckets[client_id]
        
        # Remove expired tokens
        while bucket and bucket[0] <= now - self.window:
            bucket.popleft()
        
        # Check if under limit
        if len(bucket) < self.requests:
            bucket.append(now)
            return True
        
        rate_limited_requests.labels(ip=client_id).inc()
        return False


class IPWhitelist:
    """
    IP whitelist with CIDR support.
    
    If whitelist is empty, all IPs are allowed.
    """
    
    def __init__(self, allowed_ips: list):
        self.networks: Set[ipaddress.IPv4Network | ipaddress.IPv6Network] = set()
        
        for ip in allowed_ips:
            if not ip.strip():
                continue
            try:
                # Try parsing as network (CIDR)
                self.networks.add(ipaddress.ip_network(ip.strip(), strict=False))
            except ValueError:
                logger.warning(f"Invalid IP/CIDR in whitelist: {ip}")
        
        if self.networks:
            logger.info(f"IP whitelist enabled with {len(self.networks)} entries")
    
    def is_allowed(self, client_ip: str) -> bool:
        """Check if IP is whitelisted."""
        if not self.networks:
            return True  # Empty whitelist = allow all
        
        try:
            ip = ipaddress.ip_address(client_ip)
            for network in self.networks:
                if ip in network:
                    return True
            
            ip_whitelist_rejections.labels(ip=client_ip).inc()
            return False
        except ValueError:
            logger.warning(f"Invalid client IP: {client_ip}")
            return False


# Global security middleware instances
circuit_breaker = CircuitBreaker(
    threshold=CIRCUIT_BREAKER_THRESHOLD,
    timeout=CIRCUIT_BREAKER_TIMEOUT
)
rate_limiter = RateLimiter(
    requests=RATE_LIMIT_REQUESTS,
    window=RATE_LIMIT_WINDOW
)
ip_whitelist = IPWhitelist(IP_WHITELIST)


# Simplified ExchangeManager
class SimpleExchangeManager:
    """Simplified CCXT exchange manager."""
    
    def __init__(self):
        self.exchanges: Dict[str, ccxt.Exchange] = {}
        self.api_key = os.getenv("EXCHANGE_API_KEY")
        self.api_secret = os.getenv("EXCHANGE_API_SECRET")
        logger.info(f"ExchangeManager initialized (testnet={TESTNET}, has_credentials={bool(self.api_key)})")
    
    async def initialize(self):
        """Initialize default exchange connection."""
        try:
            if not self.api_key or not self.api_secret:
                logger.warning("No exchange credentials provided - running in dry-run mode")
                return
            
            # Initialize default exchange
            exchange_class = getattr(ccxt, DEFAULT_EXCHANGE)
            config = {
                'apiKey': self.api_key,
                'secret': self.api_secret,
                'enableRateLimit': True,
            }
            
            if TESTNET:
                config['options'] = {'defaultType': 'future'}  # Use testnet if available
                if DEFAULT_EXCHANGE == 'binance':
                    config['options']['sandbox'] = True
            
            exchange = exchange_class(config)
            await exchange.load_markets()
            self.exchanges[DEFAULT_EXCHANGE] = exchange
            active_exchanges.set(len(self.exchanges))
            logger.info(f"Connected to {DEFAULT_EXCHANGE} (testnet={TESTNET})")
        except Exception as e:
            logger.error(f"Failed to initialize exchange: {e}")
            raise
    
    async def place_order(
        self,
        exchange: str,
        symbol: str,
        side: str,
        order_type: str,
        quantity: float,
        price: Optional[float] = None,
        stop_loss: Optional[float] = None,
        take_profit: Optional[float] = None
    ) -> Dict[str, Any]:
        """Place order on exchange."""
        start_time = time.time()
        
        try:
            if exchange not in self.exchanges:
                raise ValueError(f"Exchange {exchange} not connected")
            
            exch = self.exchanges[exchange]
            
            # Place main order
            order = await exch.create_order(
                symbol=symbol,
                type=order_type,
                side=side,
                amount=quantity,
                price=price
            )
            
            # Add TP/SL if provided
            if stop_loss:
                await exch.create_order(
                    symbol=symbol,
                    type='stop_loss',
                    side='sell' if side == 'buy' else 'buy',
                    amount=quantity,
                    price=stop_loss
                )
            
            if take_profit:
                await exch.create_order(
                    symbol=symbol,
                    type='take_profit',
                    side='sell' if side == 'buy' else 'buy',
                    amount=quantity,
                    price=take_profit
                )
            
            duration = time.time() - start_time
            processing_duration.observe(duration)
            order_executions.labels(exchange=exchange, status='success').inc()
            
            logger.info(f"Order placed: {order['id']} ({side} {quantity} {symbol})")
            return order
        
        except Exception as e:
            order_executions.labels(exchange=exchange, status='failure').inc()
            logger.error(f"Order placement failed: {e}")
            raise
    
    async def close(self):
        """Close all exchange connections."""
        for name, exchange in self.exchanges.items():
            try:
                await exchange.close()
                logger.info(f"Closed connection to {name}")
            except Exception as e:
                logger.error(f"Error closing {name}: {e}")
        self.exchanges.clear()
        active_exchanges.set(0)


# Global exchange manager
exchange_manager: Optional[SimpleExchangeManager] = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan manager."""
    global exchange_manager
    
    # Startup
    logger.info("Starting FKS Execution Service with CCXT")
    exchange_manager = SimpleExchangeManager()
    await exchange_manager.initialize()
    
    yield
    
    # Shutdown
    logger.info("Shutting down FKS Execution Service")
    if exchange_manager:
        await exchange_manager.close()


app = FastAPI(
    title="FKS Execution Service",
    description="Production trading execution with CCXT",
    version="2.0.0-ccxt",
    lifespan=lifespan
)


# Pydantic Models
class TradingViewWebhook(BaseModel):
    """TradingView webhook payload."""
    symbol: str = Field(..., description="Trading symbol (e.g., BTCUSDT)")
    action: str = Field(..., description="Trade action: buy, sell, close")
    confidence: float = Field(..., ge=0.0, le=1.0, description="Signal confidence")
    price: Optional[float] = Field(None, description="Entry price")
    stop_loss: Optional[float] = Field(None, description="Stop loss price")
    take_profit: Optional[float] = Field(None, description="Take profit price")
    timestamp: int = Field(..., description="Signal timestamp")


def verify_webhook_signature(payload: bytes, signature: str) -> bool:
    """Verify HMAC-SHA256 webhook signature."""
    expected = hmac.new(
        WEBHOOK_SECRET.encode(),
        payload,
        hashlib.sha256
    ).hexdigest()
    return hmac.compare_digest(signature, expected)


def calculate_position_size(
    signal: TradingViewWebhook,
    account_balance: float = 10000.0  # Default for dry-run
) -> float:
    """Calculate position size based on risk parameters."""
    # Simple risk-based sizing: risk 2% of balance per trade
    risk_amount = account_balance * RISK_PER_TRADE
    
    # Calculate position size based on stop loss distance
    if signal.price and signal.stop_loss:
        stop_distance = abs(signal.price - signal.stop_loss) / signal.price
        position_size_usd = risk_amount / stop_distance if stop_distance > 0 else 0
    else:
        # No stop loss - use fixed percentage of balance
        position_size_usd = account_balance * 0.05  # 5% of balance
    
    # Cap at max position size
    return min(position_size_usd, MAX_POSITION_SIZE_USD)


@app.post("/webhook/tradingview")
async def tradingview_webhook(request: Request):
    """
    TradingView webhook endpoint with comprehensive security.
    
    Security layers (in order):
    1. IP whitelist check
    2. Rate limiting (token bucket)
    3. Circuit breaker check
    4. Signature verification (HMAC-SHA256)
    5. Payload validation
    """
    start_time = time.time()
    
    try:
        # Security Layer 1: IP Whitelist
        client_ip = request.client.host if request.client else "unknown"
        if not ip_whitelist.is_allowed(client_ip):
            webhook_requests.labels(status='ip_blocked').inc()
            logger.warning(f"Request from non-whitelisted IP: {client_ip}")
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="IP not whitelisted"
            )
        
        # Security Layer 2: Rate Limiting
        if not rate_limiter.is_allowed(client_ip):
            webhook_requests.labels(status='rate_limited').inc()
            logger.warning(f"Rate limit exceeded for IP: {client_ip}")
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail="Rate limit exceeded"
            )
        
        # Security Layer 3: Circuit Breaker
        if not circuit_breaker.can_proceed():
            webhook_requests.labels(status='circuit_open').inc()
            logger.warning("Circuit breaker is open - rejecting request")
            raise HTTPException(
                status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
                detail="Service temporarily unavailable"
            )
        
        # Security Layer 4: Signature Verification
        # Get signature from headers
        signature = request.headers.get("X-Webhook-Signature", "")
        
        # Read raw body for signature verification
        body = await request.body()
        
        # Verify signature
        if not verify_webhook_signature(body, signature):
            webhook_requests.labels(status='invalid_signature').inc()
            circuit_breaker.record_failure()
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Invalid webhook signature"
            )
        
        # Security Layer 5: Payload Validation
        # Parse payload
        signal = TradingViewWebhook.parse_raw(body)
        
        # Validate confidence threshold
        if signal.confidence < MIN_CONFIDENCE:
            webhook_requests.labels(status='low_confidence').inc()
            logger.info(f"Signal ignored - low confidence: {signal.confidence}")
            circuit_breaker.record_success()
            return {"status": "ignored", "reason": "confidence below threshold"}
        
        # Check timestamp staleness (5 minutes)
        now = int(time.time())
        if abs(now - signal.timestamp) > 300:
            webhook_requests.labels(status='stale').inc()
            circuit_breaker.record_success()
            return {"status": "ignored", "reason": "stale signal"}
        
        # Calculate position size
        position_size_usd = calculate_position_size(signal)
        
        # Execute order (or simulate if no credentials)
        if exchange_manager and exchange_manager.api_key:
            # Real execution
            # Convert USD size to quantity (simplified)
            quantity = position_size_usd / signal.price if signal.price else 0
            
            order = await exchange_manager.place_order(
                exchange=DEFAULT_EXCHANGE,
                symbol=signal.symbol,
                side=signal.action,
                order_type='market',
                quantity=quantity,
                stop_loss=signal.stop_loss,
                take_profit=signal.take_profit
            )
            
            webhook_requests.labels(status='executed').inc()
            circuit_breaker.record_success()
            
            return {
                "status": "executed",
                "order_id": order.get('id'),
                "symbol": signal.symbol,
                "action": signal.action,
                "quantity": quantity,
                "position_size_usd": position_size_usd,
                "exchange": DEFAULT_EXCHANGE
            }
        else:
            # Dry-run mode
            webhook_requests.labels(status='dry_run').inc()
            circuit_breaker.record_success()
            logger.info(f"DRY-RUN: Would execute {signal.action} {signal.symbol} for ${position_size_usd}")
            
            return {
                "status": "dry_run",
                "message": "No exchange credentials - simulating order",
                "signal": {
                    "symbol": signal.symbol,
                    "action": signal.action,
                    "confidence": signal.confidence,
                    "position_size_usd": position_size_usd
                }
            }
    
    except HTTPException:
        raise
    except Exception as e:
        webhook_requests.labels(status='error').inc()
        circuit_breaker.record_failure()
        logger.error(f"Webhook processing error: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=str(e)
        )
    finally:
        duration = time.time() - start_time
        processing_duration.observe(duration)


@app.get("/health")
async def health():
    """Health check endpoint."""
    return {
        "status": "healthy",
        "service": "fks-execution",
        "version": "2.0.0-ccxt",
        "exchange": DEFAULT_EXCHANGE,
        "testnet": TESTNET,
        "has_credentials": bool(exchange_manager and exchange_manager.api_key)
    }


@app.get("/ready")
async def readiness():
    """Readiness check endpoint."""
    if exchange_manager is None:
        raise HTTPException(status_code=503, detail="Exchange manager not initialized")
    return {"status": "ready"}


@app.get("/metrics")
async def metrics():
    """Prometheus metrics endpoint."""
    return JSONResponse(
        content=generate_latest().decode('utf-8'),
        media_type=CONTENT_TYPE_LATEST
    )


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8000)
