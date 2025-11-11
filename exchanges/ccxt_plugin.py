"""
CCXT Plugin - ExecutionPlugin implementation for CCXT exchanges.

Wraps the ExchangeManager to provide a unified plugin interface
for cryptocurrency exchange interactions.
"""

import logging
import time
from typing import Dict, List, Optional, Any
from datetime import datetime
from enum import Enum

from .manager import ExchangeManager, get_exchange_manager
from ..metrics import (
    orders_total,
    order_execution_duration,
    order_failures,
    order_size_usd,
    exchange_connections,
    exchange_api_calls,
    exchange_errors,
)


logger = logging.getLogger(__name__)


class OrderSide(str, Enum):
    """Order side enum."""
    BUY = "buy"
    SELL = "sell"


class OrderType(str, Enum):
    """Order type enum."""
    MARKET = "market"
    LIMIT = "limit"
    STOP_LOSS = "stop_loss"
    TAKE_PROFIT = "take_profit"


class OrderStatus(str, Enum):
    """Order status enum."""
    PENDING = "pending"
    OPEN = "open"
    CLOSED = "closed"
    CANCELED = "canceled"
    FAILED = "failed"


class CCXTPlugin:
    """
    CCXT execution plugin.
    
    Implements the ExecutionPlugin interface (Python version)
    to provide unified cryptocurrency exchange access via CCXT.
    
    Attributes:
        exchange_id: Default exchange to use (e.g., 'binance')
        manager: ExchangeManager instance
        config: Plugin configuration
    """
    
    def __init__(self, exchange_id: str = "binance", config: Optional[Dict] = None):
        """
        Initialize CCXT plugin.
        
        Args:
            exchange_id: Default exchange ID (e.g., 'binance', 'coinbase')
            config: Configuration dict with optional:
                - api_key: Exchange API key
                - api_secret: Exchange API secret
                - passphrase: API passphrase (for exchanges that need it)
                - testnet: Use testnet environment (bool)
        """
        self.exchange_id = exchange_id
        self.config = config or {}
        self.manager = get_exchange_manager()
        self._initialized = False
        self.logger = logging.getLogger(__name__)
    
    async def init(self) -> bool:
        """
        Initialize the plugin by connecting to the exchange.
        
        Returns:
            True if initialization successful
        """
        try:
            credentials = None
            if self.config.get('api_key') and self.config.get('api_secret'):
                credentials = {
                    'api_key': self.config['api_key'],
                    'api_secret': self.config['api_secret'],
                }
                if 'passphrase' in self.config:
                    credentials['passphrase'] = self.config['passphrase']
            
            testnet = self.config.get('testnet', False)
            
            await self.manager.init_exchange(
                self.exchange_id,
                credentials=credentials,
                testnet=testnet
            )
            
            self._initialized = True
            exchange_connections.labels(exchange=self.exchange_id).inc()
            self.logger.info(f"CCXT plugin initialized with exchange: {self.exchange_id}")
            return True
            
        except Exception as e:
            exchange_errors.labels(exchange=self.exchange_id, error_type="initialization").inc()
            self.logger.error(f"Failed to initialize CCXT plugin: {e}")
            return False
    
    async def execute_order(self, order: Dict[str, Any]) -> Dict[str, Any]:
        """
        Execute a trading order.
        
        Args:
            order: Order dict with required fields:
                - symbol: Trading pair (e.g., 'BTC/USDT')
                - side: 'buy' or 'sell'
                - order_type: 'market', 'limit', etc.
                - quantity: Order size
                - price: Limit price (optional, required for limit orders)
                - stop_loss: Stop-loss price (optional)
                - take_profit: Take-profit price (optional)
                - confidence: Confidence score 0-1 (optional, for filtering)
        
        Returns:
            ExecutionResult dict:
                - success: bool
                - order_id: str
                - filled_quantity: float
                - average_price: float
                - error: str (if failed)
                - timestamp: int (milliseconds)
        """
        start_time = time.time()
        exchange_id = order.get('exchange', self.exchange_id)
        symbol = order.get('symbol', 'unknown')
        side = order.get('side', 'unknown')
        order_type = order.get('order_type', 'market')
        
        if not self._initialized:
            order_failures.labels(
                exchange=exchange_id,
                symbol=symbol,
                reason="not_initialized"
            ).inc()
            return {
                'success': False,
                'order_id': None,
                'filled_quantity': 0.0,
                'average_price': 0.0,
                'error': 'Plugin not initialized',
                'timestamp': int(datetime.utcnow().timestamp() * 1000)
            }
        
        try:
            # Validate confidence threshold (Phase 2 integration)
            confidence = order.get('confidence', 1.0)
            min_confidence = self.config.get('min_confidence', 0.6)
            
            if confidence < min_confidence:
                order_failures.labels(
                    exchange=exchange_id,
                    symbol=symbol,
                    reason="low_confidence"
                ).inc()
                return {
                    'success': False,
                    'order_id': None,
                    'filled_quantity': 0.0,
                    'average_price': 0.0,
                    'error': f'Confidence {confidence:.2f} below threshold {min_confidence:.2f}',
                    'timestamp': int(datetime.utcnow().timestamp() * 1000)
                }
            
            # Place order via ExchangeManager
            exchange_api_calls.labels(
                exchange=exchange_id,
                endpoint="create_order",
                status="attempt"
            ).inc()
            
            result = await self.manager.place_order(
                exchange_id,
                symbol=order['symbol'],
                side=order['side'],
                order_type=order['order_type'],
                amount=order['quantity'],
                price=order.get('price'),
                stop_loss=order.get('stop_loss'),
                take_profit=order.get('take_profit'),
                params=order.get('params', {})
            )
            
            # Calculate order size in USD
            avg_price = result.get('average', result.get('price', 0.0))
            filled_qty = result.get('filled', order['quantity'])
            size_usd = filled_qty * avg_price
            
            # Record success metrics
            duration = time.time() - start_time
            orders_total.labels(
                exchange=exchange_id,
                symbol=symbol,
                side=side,
                order_type=order_type,
                status="success"
            ).inc()
            order_execution_duration.labels(
                exchange=exchange_id,
                order_type=order_type
            ).observe(duration)
            order_size_usd.labels(
                exchange=exchange_id,
                symbol=symbol,
                side=side
            ).observe(size_usd)
            exchange_api_calls.labels(
                exchange=exchange_id,
                endpoint="create_order",
                status="success"
            ).inc()
            
            return {
                'success': True,
                'order_id': result['id'],
                'filled_quantity': filled_qty,
                'average_price': avg_price,
                'error': None,
                'timestamp': result.get('timestamp', int(datetime.utcnow().timestamp() * 1000)),
                'raw_result': result  # Include full CCXT response
            }
            
        except Exception as e:
            # Record failure metrics
            duration = time.time() - start_time
            error_type = type(e).__name__
            
            orders_total.labels(
                exchange=exchange_id,
                symbol=symbol,
                side=side,
                order_type=order_type,
                status="failure"
            ).inc()
            order_execution_duration.labels(
                exchange=exchange_id,
                order_type=order_type
            ).observe(duration)
            order_failures.labels(
                exchange=exchange_id,
                symbol=symbol,
                reason=error_type
            ).inc()
            exchange_api_calls.labels(
                exchange=exchange_id,
                endpoint="create_order",
                status="failure"
            ).inc()
            exchange_errors.labels(
                exchange=exchange_id,
                error_type=error_type
            ).inc()
            
            self.logger.error(f"Failed to execute order: {e}")
            return {
                'success': False,
                'order_id': None,
                'filled_quantity': 0.0,
                'average_price': 0.0,
                'error': str(e),
                'timestamp': int(datetime.utcnow().timestamp() * 1000)
            }
    
    async def fetch_data(self, symbol: str, exchange_id: Optional[str] = None) -> Dict[str, Any]:
        """
        Fetch market data for a symbol.
        
        Args:
            symbol: Trading pair (e.g., 'BTC/USDT')
            exchange_id: Exchange to query (uses default if None)
        
        Returns:
            MarketData dict:
                - bid: float
                - ask: float
                - last: float
                - volume: float
                - timestamp: int
        """
        if not self._initialized:
            raise RuntimeError("Plugin not initialized")
        
        exchange_id = exchange_id or self.exchange_id
        
        try:
            ticker = await self.manager.fetch_ticker(exchange_id, symbol)
            
            return {
                'bid': ticker.get('bid', 0.0),
                'ask': ticker.get('ask', 0.0),
                'last': ticker.get('last', 0.0),
                'volume': ticker.get('volume', 0.0),
                'timestamp': ticker.get('timestamp', 0),
                'symbol': ticker.get('symbol', symbol),
                'raw_ticker': ticker  # Include full ticker data
            }
            
        except Exception as e:
            self.logger.error(f"Failed to fetch data for {symbol}: {e}")
            raise
    
    async def fetch_balance(self, exchange_id: Optional[str] = None) -> Dict[str, Any]:
        """
        Fetch account balance.
        
        Args:
            exchange_id: Exchange to query (uses default if None)
        
        Returns:
            Dict of balances by currency
        """
        if not self._initialized:
            raise RuntimeError("Plugin not initialized")
        
        exchange_id = exchange_id or self.exchange_id
        return await self.manager.fetch_balance(exchange_id)
    
    async def cancel_order(self, order_id: str, symbol: str, exchange_id: Optional[str] = None) -> bool:
        """
        Cancel an open order.
        
        Args:
            order_id: Order ID to cancel
            symbol: Trading pair
            exchange_id: Exchange (uses default if None)
        
        Returns:
            True if canceled successfully
        """
        if not self._initialized:
            return False
        
        exchange_id = exchange_id or self.exchange_id
        return await self.manager.cancel_order(exchange_id, order_id, symbol)
    
    async def fetch_order(self, order_id: str, symbol: str, exchange_id: Optional[str] = None) -> Dict[str, Any]:
        """
        Fetch order details.
        
        Args:
            order_id: Order ID
            symbol: Trading pair
            exchange_id: Exchange (uses default if None)
        
        Returns:
            Order details dict
        """
        if not self._initialized:
            raise RuntimeError("Plugin not initialized")
        
        exchange_id = exchange_id or self.exchange_id
        return await self.manager.fetch_order(exchange_id, order_id, symbol)
    
    def name(self) -> str:
        """Get plugin name."""
        return f"ccxt:{self.exchange_id}"
    
    async def health_check(self) -> bool:
        """
        Check if plugin is healthy.
        
        Returns:
            True if exchange connection is active
        """
        if not self._initialized:
            return False
        
        try:
            # Try fetching ticker for a common pair as health check
            await self.manager.fetch_ticker(self.exchange_id, 'BTC/USDT')
            return True
        except Exception as e:
            self.logger.warning(f"Health check failed: {e}")
            return False
    
    async def close(self):
        """Close plugin and cleanup resources."""
        if self._initialized:
            # Don't close manager (singleton), just mark as not initialized
            self._initialized = False
            exchange_connections.labels(exchange=self.exchange_id).dec()
            self.logger.info(f"CCXT plugin closed for {self.exchange_id}")


# Convenience function for creating CCXT plugins
def create_ccxt_plugin(exchange_id: str = "binance", **config) -> CCXTPlugin:
    """
    Create a CCXT plugin instance.
    
    Args:
        exchange_id: Exchange ID (e.g., 'binance', 'coinbase')
        **config: Configuration options (api_key, api_secret, testnet, etc.)
    
    Returns:
        Initialized CCXTPlugin instance
    
    Example:
        plugin = create_ccxt_plugin(
            'binance',
            api_key='xxx',
            api_secret='yyy',
            testnet=True,
            min_confidence=0.7
        )
        await plugin.init()
    """
    return CCXTPlugin(exchange_id, config)
