"""
FKS Execution Service - Main Application
Handles TradingView webhooks and executes orders via CCXT
"""
import os
import logging
from typing import Dict, Any
from fastapi import FastAPI, Request, HTTPException, status
from fastapi.responses import JSONResponse
from prometheus_client import make_asgi_app
import uvicorn

from webhooks.tradingview import TradingViewWebhookHandler
from exchanges.manager import ExchangeManager
from exchanges.ccxt_plugin import CCXTPlugin
from security.middleware import RateLimiter, CircuitBreaker, IPWhitelist, AuditLogger
from validation.normalizer import DataNormalizer, PositionSizer
from metrics import (
    webhook_requests_total,
    webhook_processing_duration,
    orders_total,
    active_requests
)

# Configure logging
logging.basicConfig(
    level=os.getenv('LOG_LEVEL', 'INFO'),
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Initialize FastAPI app
app = FastAPI(
    title="FKS Execution Service",
    description="Trading execution pipeline with TradingView webhooks and CCXT integration",
    version="1.0.0"
)

# Initialize components
webhook_handler = TradingViewWebhookHandler(
    secret=os.getenv('WEBHOOK_SECRET', 'default-dev-secret'),
    min_confidence=float(os.getenv('MIN_CONFIDENCE', '0.6'))
)

exchange_manager = ExchangeManager()
ccxt_plugin = CCXTPlugin(exchange_manager)

# Security middleware
rate_limiter = RateLimiter(
    max_requests=int(os.getenv('RATE_LIMIT_REQUESTS', '100')),
    window_seconds=int(os.getenv('RATE_LIMIT_WINDOW', '60'))
)
circuit_breaker = CircuitBreaker(
    failure_threshold=int(os.getenv('CIRCUIT_BREAKER_THRESHOLD', '5')),
    timeout_seconds=int(os.getenv('CIRCUIT_BREAKER_TIMEOUT', '60'))
)
ip_whitelist = IPWhitelist(whitelist=[])  # Empty = allow all for dev
audit_logger = AuditLogger()

# Data processing
data_normalizer = DataNormalizer()
position_sizer = PositionSizer()


@app.get("/health")
async def health_check():
    """Health check endpoint for K8s probes"""
    return {"status": "healthy", "service": "fks-execution"}


@app.get("/ready")
async def readiness_check():
    """Readiness check endpoint for K8s probes"""
    # Check if exchange connections are available
    try:
        # Simple check - can expand to verify exchange connectivity
        return {
            "status": "ready",
            "exchanges": len(exchange_manager.exchanges),
            "circuit_breaker": circuit_breaker.state
        }
    except Exception as e:
        raise HTTPException(status_code=503, detail=f"Not ready: {str(e)}")


@app.post("/webhook/tradingview")
async def tradingview_webhook(request: Request):
    """
    Handle TradingView webhook alerts
    Pipeline: Validate → Security → Normalize → Size → Execute
    """
    client_ip = request.client.host
    
    # Track active requests
    active_requests.inc()
    webhook_requests_total.labels(source="tradingview", status="received").inc()
    
    try:
        # Rate limiting
        if not rate_limiter.allow_request(client_ip):
            webhook_requests_total.labels(source="tradingview", status="rate_limited").inc()
            raise HTTPException(status_code=429, detail="Rate limit exceeded")
        
        # IP whitelist (disabled for dev)
        if not ip_whitelist.is_allowed(client_ip):
            webhook_requests_total.labels(source="tradingview", status="blocked").inc()
            raise HTTPException(status_code=403, detail="IP not whitelisted")
        
        # Circuit breaker check
        if not circuit_breaker.allow_request():
            webhook_requests_total.labels(source="tradingview", status="circuit_open").inc()
            raise HTTPException(status_code=503, detail="Circuit breaker open")
        
        # Parse request
        body = await request.body()
        headers = dict(request.headers)
        
        # Validate webhook
        is_valid, payload = webhook_handler.validate_webhook(body, headers)
        
        if not is_valid:
            webhook_requests_total.labels(source="tradingview", status="invalid").inc()
            circuit_breaker.record_failure()
            raise HTTPException(status_code=400, detail="Invalid webhook")
        
        # Normalize data
        normalized = data_normalizer.normalize(payload)
        
        # Calculate position size
        sized_order = position_sizer.calculate(
            normalized,
            method=os.getenv('POSITION_SIZE_METHOD', 'fixed_percentage')
        )
        
        # Execute order (demo mode for now)
        logger.info(f"Would execute order: {sized_order}")
        
        # Record success
        circuit_breaker.record_success()
        webhook_requests_total.labels(source="tradingview", status="success").inc()
        orders_total.labels(
            exchange=sized_order.get('exchange', 'unknown'),
            symbol=sized_order.get('symbol', 'unknown'),
            side=sized_order.get('side', 'unknown'),
            status="simulated"
        ).inc()
        
        # Audit log
        audit_logger.log_event(
            event_type="order_executed",
            user_id="tradingview",
            ip_address=client_ip,
            details=sized_order
        )
        
        return JSONResponse({
            "status": "success",
            "message": "Order processed (simulation mode)",
            "order": sized_order
        })
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"Error processing webhook: {str(e)}", exc_info=True)
        webhook_requests_total.labels(source="tradingview", status="error").inc()
        circuit_breaker.record_failure()
        raise HTTPException(status_code=500, detail=f"Internal error: {str(e)}")
    finally:
        active_requests.dec()


# Mount Prometheus metrics
metrics_app = make_asgi_app()
app.mount("/metrics", metrics_app)


if __name__ == "__main__":
    port = int(os.getenv('PORT', '8000'))
    uvicorn.run(
        app,
        host="0.0.0.0",
        port=port,
        log_level=os.getenv('LOG_LEVEL', 'info').lower()
    )
