"""FKS Execution Service - FastAPI Application."""
import sys
import os
import logging
from typing import Dict, Any

# Add paths
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from fastapi import FastAPI, Request, HTTPException
from fastapi.responses import JSONResponse
import uvicorn
from prometheus_client import make_asgi_app, Counter, Histogram

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# Create FastAPI app
app = FastAPI(
    title="FKS Execution Service",
    description="TradingView webhook handler with CCXT execution",
    version="1.0.0"
)

# Prometheus metrics
webhook_requests = Counter('webhook_requests_total', 'Total webhook requests')
webhook_duration = Histogram('webhook_processing_duration_seconds', 'Webhook processing duration')

# Mount metrics endpoint
metrics_app = make_asgi_app()
app.mount("/metrics", metrics_app)

@app.get("/health")
async def health_check():
    """Health check endpoint."""
    return {"status": "healthy", "service": "fks-execution"}

@app.get("/ready")
async def readiness_check():
    """Readiness check endpoint."""
    return {"status": "ready", "service": "fks-execution"}

@app.post("/webhook/tradingview")
async def tradingview_webhook(request: Request):
    """Handle TradingView webhook."""
    webhook_requests.inc()
    
    try:
        # Get payload
        payload = await request.json()
        logger.info(f"Received webhook: {payload}")
        
        # Simple validation
        if not isinstance(payload, dict):
            raise HTTPException(status_code=400, detail="Invalid payload format")
        
        # Extract fields
        symbol = payload.get('symbol', 'UNKNOWN')
        side = payload.get('side', 'buy')
        confidence = payload.get('confidence', 0.0)
        
        logger.info(f"Processing order: {symbol} {side} (confidence: {confidence})")
        
        # Simulate order execution
        order_result = {
            "status": "simulated",
            "symbol": symbol,
            "side": side,
            "confidence": confidence,
            "message": "Order simulation successful (real execution disabled)"
        }
        
        return JSONResponse(content=order_result, status_code=200)
        
    except Exception as e:
        logger.error(f"Webhook processing error: {e}")
        raise HTTPException(status_code=500, detail=str(e))

if __name__ == "__main__":
    port = int(os.environ.get("PORT", 8000))
    uvicorn.run(app, host="0.0.0.0", port=port)
