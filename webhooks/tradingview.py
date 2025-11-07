"""
TradingView Webhook Handler

Receives trading alerts from TradingView and executes orders via CCXTPlugin.

Features:
- Webhook signature verification
- Payload validation
- Confidence threshold filtering (Phase 2 integration)
- Risk checks (position sizing, symbol whitelist)
- Order execution via CCXT
- Detailed logging and error handling

TradingView Alert Format:
{
    "symbol": "BTC/USDT",
    "side": "buy",
    "order_type": "market",
    "quantity": 0.1,
    "price": 67000.0,  // Optional, for limit orders
    "stop_loss": 66000.0,  // Optional
    "take_profit": 69000.0,  // Optional
    "confidence": 0.85,  // Optional, defaults to 1.0
    "exchange": "binance",  // Optional, uses default if not provided
    "timestamp": 1699113600000,  // Optional
    "signature": "sha256_hmac_signature"  // Required if secret configured
}
"""

import hmac
import hashlib
import json
import logging
import time
from typing import Dict, List, Optional, Any, Set
from datetime import datetime
from enum import Enum

from ..metrics import (
    webhook_requests_total,
    webhook_processing_duration,
    webhook_validation_failures,
    webhook_signature_failures,
    webhook_confidence_filtered,
    webhook_stale_rejected,
    active_requests,
    MetricsTimer,
)

logger = logging.getLogger(__name__)


class ValidationError(Exception):
    """Raised when webhook payload validation fails."""
    pass


class SignatureError(Exception):
    """Raised when webhook signature verification fails."""
    pass


class TradingViewWebhook:
    """
    TradingView webhook handler.
    
    Validates and processes trading alerts from TradingView,
    executing orders via CCXTPlugin with confidence filtering
    and risk management.
    """
    
    def __init__(
        self,
        plugin,
        webhook_secret: Optional[str] = None,
        config: Optional[Dict] = None
    ):
        """
        Initialize webhook handler.
        
        Args:
            plugin: CCXTPlugin instance for order execution
            webhook_secret: Secret key for HMAC signature verification (optional)
            config: Configuration dict with:
                - min_confidence: Minimum confidence threshold (default 0.6)
                - max_quantity: Maximum order size (default None = unlimited)
                - symbol_whitelist: Allowed symbols (default None = all)
                - max_order_value: Maximum order value in USD (default None)
                - require_signature: Require HMAC signature (default True if secret provided)
                - stale_timeout: Reject orders older than N seconds (default 300)
        """
        self.plugin = plugin
        self.webhook_secret = webhook_secret
        self.config = config or {}
        self.logger = logging.getLogger(__name__)
        
        # Configuration with defaults
        self.min_confidence = self.config.get('min_confidence', 0.6)
        self.max_quantity = self.config.get('max_quantity')
        self.symbol_whitelist: Optional[Set[str]] = (
            set(self.config['symbol_whitelist']) 
            if 'symbol_whitelist' in self.config 
            else None
        )
        self.max_order_value = self.config.get('max_order_value')
        self.require_signature = self.config.get(
            'require_signature',
            webhook_secret is not None
        )
        self.stale_timeout = self.config.get('stale_timeout', 300)  # 5 minutes
    
    def verify_signature(self, payload: str, signature: str) -> bool:
        """
        Verify HMAC signature of webhook payload.
        
        Args:
            payload: Raw webhook payload string
            signature: HMAC signature from header
        
        Returns:
            True if signature valid
        
        Raises:
            SignatureError: If signature invalid or missing
        """
        if not self.webhook_secret:
            if self.require_signature:
                webhook_signature_failures.labels(source="tradingview").inc()
                raise SignatureError("Webhook secret not configured but signature required")
            return True
        
        if not signature:
            webhook_signature_failures.labels(source="tradingview").inc()
            raise SignatureError("Signature missing from webhook")
        
        # Compute expected signature
        expected = hmac.new(
            self.webhook_secret.encode('utf-8'),
            payload.encode('utf-8'),
            hashlib.sha256
        ).hexdigest()
        
        # Constant-time comparison
        if not hmac.compare_digest(signature, expected):
            webhook_signature_failures.labels(source="tradingview").inc()
            raise SignatureError("Invalid webhook signature")
        
        return True
    
    def validate_payload(self, data: Dict[str, Any]) -> None:
        """
        Validate webhook payload structure and values.
        
        Args:
            data: Parsed webhook data
        
        Raises:
            ValidationError: If payload invalid
        """
        # Required fields
        required_fields = ['symbol', 'side', 'order_type', 'quantity']
        for field in required_fields:
            if field not in data:
                raise ValidationError(f"Missing required field: {field}")
        
        # Validate symbol
        symbol = data['symbol']
        if self.symbol_whitelist and symbol not in self.symbol_whitelist:
            raise ValidationError(
                f"Symbol {symbol} not in whitelist: {self.symbol_whitelist}"
            )
        
        # Validate side
        if data['side'].lower() not in ['buy', 'sell']:
            raise ValidationError(f"Invalid side: {data['side']}")
        
        # Validate order_type
        valid_types = ['market', 'limit', 'stop_loss', 'take_profit']
        if data['order_type'].lower() not in valid_types:
            raise ValidationError(f"Invalid order_type: {data['order_type']}")
        
        # Validate quantity
        quantity = float(data['quantity'])
        if quantity <= 0:
            raise ValidationError(f"Invalid quantity: {quantity}")
        
        if self.max_quantity and quantity > self.max_quantity:
            raise ValidationError(
                f"Quantity {quantity} exceeds max {self.max_quantity}"
            )
        
        # Validate price for limit orders
        if data['order_type'].lower() == 'limit' and 'price' not in data:
            raise ValidationError("Limit orders require 'price' field")
        
        # Validate confidence
        if 'confidence' in data:
            confidence = float(data['confidence'])
            if not 0 <= confidence <= 1:
                raise ValidationError(f"Confidence must be 0-1: {confidence}")
            
            if confidence < self.min_confidence:
                webhook_confidence_filtered.labels(source="tradingview", symbol=symbol).inc()
                raise ValidationError(
                    f"Confidence {confidence:.2f} below threshold {self.min_confidence:.2f}"
                )
        
        # Validate timestamp (check for stale orders)
        if 'timestamp' in data:
            timestamp_ms = int(data['timestamp'])
            current_ms = int(datetime.utcnow().timestamp() * 1000)
            age_seconds = (current_ms - timestamp_ms) / 1000
            
            if age_seconds > self.stale_timeout:
                webhook_stale_rejected.labels(source="tradingview", symbol=symbol).inc()
                raise ValidationError(
                    f"Order too old: {age_seconds:.0f}s (max {self.stale_timeout}s)"
                )
            
            if age_seconds < -60:  # Allow 60s clock skew
                raise ValidationError("Order timestamp in the future")
        
        # Validate order value (price * quantity) if max_order_value set
        if self.max_order_value and 'price' in data:
            order_value = float(data['price']) * quantity
            if order_value > self.max_order_value:
                raise ValidationError(
                    f"Order value ${order_value:.2f} exceeds max ${self.max_order_value:.2f}"
                )
    
    async def process_webhook(
        self,
        payload: str,
        signature: Optional[str] = None
    ) -> Dict[str, Any]:
        """
        Process incoming TradingView webhook.
        
        Args:
            payload: Raw webhook payload (JSON string)
            signature: HMAC signature (optional)
        
        Returns:
            Response dict:
                - success: bool
                - order_id: str (if successful)
                - error: str (if failed)
                - message: str
        """
        start_time = time.time()
        active_requests.inc()
        status = "success"
        symbol = "unknown"
        side = "unknown"
        validation_error = None
        
        try:
            # Verify signature
            if self.require_signature:
                self.verify_signature(payload, signature)
            
            # Parse JSON
            try:
                data = json.loads(payload)
            except json.JSONDecodeError as e:
                validation_error = "invalid_json"
                raise ValidationError(f"Invalid JSON: {e}")
            
            # Extract symbol and side for metrics (before full validation)
            symbol = data.get('symbol', 'unknown')
            side = data.get('side', 'unknown')
            
            # Validate payload
            self.validate_payload(data)
            
            # Build order dict for plugin
            order = {
                'symbol': data['symbol'],
                'side': data['side'].lower(),
                'order_type': data['order_type'].lower(),
                'quantity': float(data['quantity']),
                'confidence': float(data.get('confidence', 1.0)),
            }
            
            # Optional fields
            if 'price' in data:
                order['price'] = float(data['price'])
            if 'stop_loss' in data:
                order['stop_loss'] = float(data['stop_loss'])
            if 'take_profit' in data:
                order['take_profit'] = float(data['take_profit'])
            if 'exchange' in data:
                order['exchange'] = data['exchange']
            
            # Execute order via plugin
            self.logger.info(
                f"Executing TradingView order: {order['side']} {order['quantity']} "
                f"{order['symbol']} @ confidence {order['confidence']:.2f}"
            )
            
            result = await self.plugin.execute_order(order)
            
            if result['success']:
                # Record webhook metrics
                webhook_requests_total.labels(
                    source="tradingview",
                    symbol=order['symbol'],
                    side=order['side']
                ).inc()
                
                self.logger.info(
                    f"Order executed successfully: {result['order_id']} "
                    f"(filled: {result['filled_quantity']}, "
                    f"avg price: {result['average_price']})"
                )
                return {
                    'success': True,
                    'order_id': result['order_id'],
                    'filled_quantity': result['filled_quantity'],
                    'average_price': result['average_price'],
                    'message': 'Order executed successfully'
                }
            else:
                status = "failure"
                self.logger.warning(f"Order execution failed: {result['error']}")
                return {
                    'success': False,
                    'error': result['error'],
                    'message': 'Order execution failed'
                }
        
        except SignatureError as e:
            status = "failure"
            self.logger.error(f"Signature verification failed: {e}")
            return {
                'success': False,
                'error': str(e),
                'message': 'Signature verification failed'
            }
        
        except ValidationError as e:
            status = "failure"
            if validation_error is None:
                validation_error = "validation_failed"
            webhook_validation_failures.labels(source="tradingview", reason=validation_error).inc()
            self.logger.warning(f"Validation failed: {e}")
            return {
                'success': False,
                'error': str(e),
                'message': 'Validation failed'
            }
        
        except Exception as e:
            status = "failure"
            self.logger.error(f"Webhook processing failed: {e}", exc_info=True)
            return {
                'success': False,
                'error': str(e),
                'message': 'Internal error'
            }
        
        finally:
            # Record processing duration
            duration = time.time() - start_time
            webhook_processing_duration.labels(source="tradingview", status=status).observe(duration)
            active_requests.dec()
    
    def get_stats(self) -> Dict[str, Any]:
        """
        Get webhook handler statistics.
        
        Returns:
            Stats dict with configuration and limits
        """
        return {
            'min_confidence': self.min_confidence,
            'max_quantity': self.max_quantity,
            'max_order_value': self.max_order_value,
            'symbol_whitelist': list(self.symbol_whitelist) if self.symbol_whitelist else None,
            'require_signature': self.require_signature,
            'stale_timeout': self.stale_timeout,
            'plugin': self.plugin.name() if self.plugin else None
        }


def create_webhook_handler(
    plugin,
    webhook_secret: Optional[str] = None,
    **config
) -> TradingViewWebhook:
    """
    Create a TradingView webhook handler.
    
    Args:
        plugin: CCXTPlugin instance
        webhook_secret: HMAC secret key (optional)
        **config: Configuration options
    
    Returns:
        TradingViewWebhook instance
    
    Example:
        handler = create_webhook_handler(
            plugin,
            webhook_secret='my_secret_key',
            min_confidence=0.7,
            symbol_whitelist=['BTC/USDT', 'ETH/USDT'],
            max_quantity=1.0,
            max_order_value=10000.0
        )
        
        result = await handler.process_webhook(
            payload='{"symbol": "BTC/USDT", ...}',
            signature='hmac_signature'
        )
    """
    return TradingViewWebhook(plugin, webhook_secret, config)
