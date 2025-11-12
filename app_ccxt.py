"""
FKS Execution Service - Real CCXT Trading

Integrates Phase 3 components:
- CCXT ExchangeManager for multi-exchange support
- TradingView webhook validation (HMAC-SHA256)
- Data normalization and position sizing  
- Security middleware (rate limiting, circuit breaker)
- Real order execution (no simulation)
"""

import sys
import os
import logging
import asyncio
from typing import Dict, Any, Optional
from contextlib import asynccontextmanager

# Add current directory to path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from fastapi import FastAPI, Request, HTTPException, Depends
from fastapi.responses import JSONResponse
import uvicorn
from prometheus_client import make_asgi_app, Counter, Histogram, Gauge

# Import Phase 3 execution components
from exchanges.manager import ExchangeManager
from webhooks.tradingview import TradingViewWebhookHandler
from validation.normalizer import DataNormalizer, PositionSizer
from security.middleware import RateLimiter, CircuitBreaker, IPWhitelist, AuditLogger

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Prometheus metrics
webhook_requests_total = Counter(
    'webhook_requests_total',
    'Total webhook requests',
    ['status', 'exchange']
)
webhook_processing_duration = Histogram(
    'webhook_processing_duration_seconds',
    'Webhook processing duration'
)
webhook_validation_failures = Counter(
    'webhook_validation_failures_total',
    'Webhook validation failures',
    ['reason']
)
order_executions_total = Counter(
    'order_executions_total',
    'Total order executions',
    ['exchange', 'side', 'status']
)
order_execution_duration = Histogram(
    'order_execution_duration_seconds',
    'Order execution duration',
    ['exchange']
)
active_exchanges = Gauge(
    'active_exchanges_total',
    'Number of active exchange connections'
)

# Global instances
exchange_manager: Optional[ExchangeManager] = None
webhook_handler: Optional[TradingViewWebhookHandler] = None
data_normalizer: Optional[DataNormalizer] = None
position_sizer: Optional[PositionSizer] = None
rate_limiter: Optional[RateLimiter] = None
circuit_breaker: Optional[CircuitBreaker] = None
ip_whitelist: Optional[IPWhitelist] = None
audit_logger: Optional[AuditLogger] = None

# Configuration from environment
WEBHOOK_SECRET = os.getenv('WEBHOOK_SECRET', 'fks-tradingview-webhook-secret-dev-2025')
MIN_CONFIDENCE = float(os.getenv('MIN_CONFIDENCE', '0.6'))
MAX_ORDER_SIZE_USD = float(os.getenv('MAX_ORDER_SIZE_USD', '1000'))
DEFAULT_EXCHANGE = os.getenv('DEFAULT_EXCHANGE', 'binance')
TESTNET = os.getenv('TESTNET', 'true').lower() == 'true'

# Exchange credentials from environment
EXCHANGE_API_KEY = os.getenv('EXCHANGE_API_KEY', '')
EXCHANGE_API_SECRET = os.getenv('EXCHANGE_API_SECRET', '')

@asynccontextmanager
async def lifespan(app: FastAPI):
    """Initialize and cleanup resources."""
    global exchange_manager, webhook_handler, data_normalizer, position_sizer
    global rate_limiter, circuit_breaker, ip_whitelist, audit_logger
    
    logger.info("🚀 Starting FKS Execution Service...")
    
    try:
        # Initialize ExchangeManager
        exchange_manager = ExchangeManager()
        
        # Initialize default exchange if credentials provided
        if EXCHANGE_API_KEY and EXCHANGE_API_SECRET:
            logger.info(f"Initializing {DEFAULT_EXCHANGE} exchange (testnet={TESTNET})...")
            await exchange_manager.init_exchange(
                DEFAULT_EXCHANGE,
                credentials={
                    'api_key': EXCHANGE_API_KEY,
                    'api_secret': EXCHANGE_API_SECRET
                },
                testnet=TESTNET
            )
            active_exchanges.set(len(exchange_manager.exchanges))
            logger.info(f"✅ {DEFAULT_EXCHANGE} initialized")
        else:
            logger.warning("⚠️  No exchange credentials - running in dry-run mode")
        
        # Initialize webhook handler
        webhook_handler = TradingViewWebhookHandler(
            webhook_secret=WEBHOOK_SECRET,
            min_confidence=MIN_CONFIDENCE
        )
        logger.info("✅ TradingView webhook handler initialized")
        
        # Initialize data normalizer
        data_normalizer = DataNormalizer()
        logger.info("✅ Data normalizer initialized")
        
        # Initialize position sizer
        position_sizer = PositionSizer(
            default_method='risk_based',
            risk_per_trade=0.02,  # 2% risk per trade
            max_position_size_usd=MAX_ORDER_SIZE_USD
        )
        logger.info("✅ Position sizer initialized")
        
        # Initialize security middleware
        rate_limiter = RateLimiter(
            max_requests=100,
            window_seconds=60
        )
        circuit_breaker = CircuitBreaker(
            failure_threshold=5,
            recovery_timeout=60
        )
        ip_whitelist = IPWhitelist(
            whitelist=['127.0.0.1', '10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']
        )
        audit_logger = AuditLogger()
        logger.info("✅ Security middleware initialized")
        
        logger.info("✅ FKS Execution Service ready!")
        
    except Exception as e:
        logger.error(f"❌ Failed to initialize: {e}")
        raise
    
    yield  # Application runs
    
    # Cleanup
    logger.info("Shutting down FKS Execution Service...")
    if exchange_manager:
        await exchange_manager.close_all()

# Create FastAPI app
app = FastAPI(
    title="FKS Execution Service",
    description="Real-time cryptocurrency trading via TradingView webhooks + CCXT",
    version="2.0.0",
    lifespan=lifespan
)

# Mount Prometheus metrics
metrics_app = make_asgi_app()
app.mount("/metrics", metrics_app)

@app.get("/health")
async def health_check():
    """Health check endpoint."""
    return {
        "status": "healthy",
        "service": "fks-execution",
        "version": "2.0.0",
        "mode": "testnet" if TESTNET else "production"
    }

@app.get("/ready")
async def readiness_check():
    """Readiness check endpoint."""
    exchanges_ready = exchange_manager and len(exchange_manager.exchanges) > 0
    return {
        "status": "ready" if exchanges_ready else "not-ready",
        "exchanges": list(exchange_manager.exchanges.keys()) if exchange_manager else [],
        "service": "fks-execution"
    }

@app.post("/webhook/tradingview")
async def tradingview_webhook(request: Request):
    """
    Process TradingView webhook and execute orders.
    
    Expected payload:
    {
        "symbol": "BTCUSDT",
        "side": "buy" | "sell",
        "confidence": 0.85,
        "strategy": "momentum",
        "timeframe": "1h",
        "price": 67500.0,
        "stop_loss": 66000.0,
        "take_profit": 70000.0
    }
    """
    client_ip = request.client.host
    
    with webhook_processing_duration.time():
        try:
            # Security checks
            if not ip_whitelist.is_allowed(client_ip):
                webhook_validation_failures.labels(reason='ip_blocked').inc()
                audit_logger.log_event('ip_blocked', {'ip': client_ip})
                raise HTTPException(status_code=403, detail="IP not whitelisted")
            
            if not rate_limiter.is_allowed(client_ip):
                webhook_validation_failures.labels(reason='rate_limit').inc()
                audit_logger.log_event('rate_limit', {'ip': client_ip})
                raise HTTPException(status_code=429, detail="Rate limit exceeded")
            
            # Circuit breaker check
            if circuit_breaker.state == 'OPEN':
                webhook_validation_failures.labels(reason='circuit_open').inc()
                raise HTTPException(status_code=503, detail="Circuit breaker open - service degraded")
            
            # Get payload
            payload = await request.json()
            headers = dict(request.headers)
            
            logger.info(f"Received webhook from {client_ip}: {payload}")
            
            # Validate webhook signature
            if not webhook_handler.validate_signature(payload, headers):
                webhook_validation_failures.labels(reason='signature').inc()
                audit_logger.log_event('invalid_signature', {'ip': client_ip, 'payload': payload})
                raise HTTPException(status_code=401, detail="Invalid webhook signature")
            
            # Validate payload
            if not webhook_handler.validate_payload(payload):
                webhook_validation_failures.labels(reason='payload').inc()
                raise HTTPException(status_code=400, detail="Invalid payload format")
            
            # Normalize data
            normalized = data_normalizer.normalize_webhook_data(payload)
            
            # Extract fields
            symbol = normalized.get('symbol')
            side = normalized.get('side')
            confidence = normalized.get('confidence', 0)
            price = normalized.get('price')
            stop_loss = normalized.get('stop_loss')
            take_profit = normalized.get('take_profit')
            
            # Check confidence threshold
            if confidence < MIN_CONFIDENCE:
                webhook_validation_failures.labels(reason='low_confidence').inc()
                return JSONResponse(
                    content={
                        "status": "rejected",
                        "reason": f"Confidence {confidence} below threshold {MIN_CONFIDENCE}",
                        "symbol": symbol
                    },
                    status_code=200
                )
            
            # Calculate position size
            position_size = position_sizer.calculate_size(
                symbol=symbol,
                entry_price=price or 0,
                stop_loss=stop_loss,
                account_balance=10000  # TODO: Fetch from exchange
            )
            
            # Check if we have exchanges configured
            if not exchange_manager or not exchange_manager.exchanges:
                logger.warning("No exchanges configured - dry run mode")
                webhook_requests_total.labels(status='dry_run', exchange='none').inc()
                return JSONResponse(
                    content={
                        "status": "dry_run",
                        "message": "No exchange configured - order not executed",
                        "symbol": symbol,
                        "side": side,
                        "size": position_size,
                        "confidence": confidence
                    },
                    status_code=200
                )
            
            # Execute order
            exchange_id = DEFAULT_EXCHANGE
            
            with order_execution_duration.labels(exchange=exchange_id).time():
                try:
                    order_result = await exchange_manager.place_order(
                        exchange_id=exchange_id,
                        symbol=symbol,
                        side=side,
                        order_type='market',
                        amount=position_size,
                        stop_loss=stop_loss,
                        take_profit=take_profit
                    )
                    
                    circuit_breaker.record_success()
                    order_executions_total.labels(
                        exchange=exchange_id,
                        side=side,
                        status='success'
                    ).inc()
                    webhook_requests_total.labels(status='success', exchange=exchange_id).inc()
                    
                    audit_logger.log_event('order_executed', {
                        'exchange': exchange_id,
                        'symbol': symbol,
                        'side': side,
                        'size': position_size,
                        'confidence': confidence,
                        'order_id': order_result.get('id')
                    })
                    
                    logger.info(f"✅ Order executed: {order_result}")
                    
                    return JSONResponse(
                        content={
                            "status": "success",
                            "exchange": exchange_id,
                            "symbol": symbol,
                            "side": side,
                            "size": position_size,
                            "confidence": confidence,
                            "order": order_result
                        },
                        status_code=200
                    )
                    
                except Exception as order_error:
                    circuit_breaker.record_failure()
                    order_executions_total.labels(
                        exchange=exchange_id,
                        side=side,
                        status='failure'
                    ).inc()
                    webhook_requests_total.labels(status='failure', exchange=exchange_id).inc()
                    
                    logger.error(f"Order execution failed: {order_error}")
                    
                    return JSONResponse(
                        content={
                            "status": "failure",
                            "error": str(order_error),
                            "symbol": symbol,
                            "side": side
                        },
                        status_code=500
                    )
        
        except HTTPException:
            raise
        except Exception as e:
            logger.error(f"Webhook processing error: {e}", exc_info=True)
            webhook_requests_total.labels(status='error', exchange='unknown').inc()
            raise HTTPException(status_code=500, detail=str(e))

if __name__ == "__main__":
    port = int(os.getenv("PORT", 8000))
    uvicorn.run(app, host="0.0.0.0", port=port)
