"""
Exchange Manager - Unified CCXT Interface

Provides a centralized interface for interacting with cryptocurrency exchanges
via the CCXT library. Supports multiple exchanges with consistent API.
"""

import asyncio
import ccxt.async_support as ccxt
from typing import Dict, List, Optional, Any
import logging

logger = logging.getLogger(__name__)


class ExchangeManager:
    """
    Unified interface for crypto exchanges via CCXT.
    
    Supports:
    - Multiple exchange connections (Binance, Coinbase, Kraken, etc.)
    - Market/Limit orders with TP/SL
    - Real-time market data
    - Rate limiting (built into CCXT)
    - Error handling and retries
    """
    
    def __init__(self):
        self.exchanges: Dict[str, ccxt.Exchange] = {}
        self.logger = logging.getLogger(__name__)
    
    async def init_exchange(
        self,
        exchange_id: str,
        credentials: Optional[Dict[str, str]] = None,
        testnet: bool = False,
        **options
    ) -> None:
        """
        Initialize an exchange connection.
        
        Args:
            exchange_id: CCXT exchange ID (e.g., 'binance', 'coinbase', 'kraken')
            credentials: API credentials {'api_key': '...', 'api_secret': '...'}
            testnet: Use testnet/sandbox environment
            **options: Additional CCXT exchange options
        
        Example:
            await manager.init_exchange(
                'binance',
                credentials={'api_key': 'xxx', 'api_secret': 'yyy'},
                testnet=True
            )
        """
        try:
            # Get exchange class
            exchange_class = getattr(ccxt, exchange_id)
            
            # Build config
            config = {
                'enableRateLimit': True,  # Automatic rate limiting
                'timeout': 30000,  # 30 second timeout
                **options
            }
            
            # Add credentials if provided
            if credentials:
                config.update({
                    'apiKey': credentials.get('api_key'),
                    'secret': credentials.get('api_secret'),
                })
                
                # Add passphrase for exchanges that need it (e.g., Coinbase)
                if 'passphrase' in credentials:
                    config['password'] = credentials['passphrase']
            
            # Enable testnet/sandbox if requested
            if testnet:
                config['options'] = {'defaultType': 'future'}  # For futures testnet
                # Some exchanges use different testnet approaches
                if exchange_id == 'binance':
                    config['urls'] = {
                        'api': {
                            'public': 'https://testnet.binance.vision/api',
                            'private': 'https://testnet.binance.vision/api',
                        }
                    }
            
            # Create exchange instance
            exchange = exchange_class(config)
            
            # Load markets (validates connection)
            await exchange.load_markets()
            
            self.exchanges[exchange_id] = exchange
            self.logger.info(f"Initialized {exchange_id} exchange (testnet={testnet})")
            
        except Exception as e:
            self.logger.error(f"Failed to initialize {exchange_id}: {e}")
            raise
    
    async def fetch_ticker(self, exchange_id: str, symbol: str) -> Dict[str, Any]:
        """
        Fetch current ticker data for a symbol.
        
        Args:
            exchange_id: Exchange to query
            symbol: Trading pair (e.g., 'BTC/USDT')
        
        Returns:
            {
                'symbol': 'BTC/USDT',
                'bid': 67500.0,
                'ask': 67505.0,
                'last': 67502.5,
                'volume': 12345.67,
                'timestamp': 1699113600000
            }
        """
        exchange = self._get_exchange(exchange_id)
        
        try:
            ticker = await exchange.fetch_ticker(symbol)
            
            return {
                'symbol': ticker['symbol'],
                'bid': ticker.get('bid'),
                'ask': ticker.get('ask'),
                'last': ticker.get('last'),
                'volume': ticker.get('baseVolume', 0),
                'timestamp': ticker.get('timestamp', 0),
                'high_24h': ticker.get('high'),
                'low_24h': ticker.get('low'),
                'change_24h': ticker.get('change'),
                'change_percent_24h': ticker.get('percentage'),
            }
            
        except Exception as e:
            self.logger.error(f"Failed to fetch ticker for {symbol} on {exchange_id}: {e}")
            raise
    
    async def fetch_balance(self, exchange_id: str) -> Dict[str, Any]:
        """
        Fetch account balance.
        
        Args:
            exchange_id: Exchange to query
        
        Returns:
            {
                'BTC': {'free': 0.5, 'used': 0.1, 'total': 0.6},
                'USDT': {'free': 10000, 'used': 0, 'total': 10000},
                ...
            }
        """
        exchange = self._get_exchange(exchange_id)
        
        try:
            balance = await exchange.fetch_balance()
            
            # Return only non-zero balances
            result = {}
            for currency, amounts in balance['total'].items():
                if amounts and amounts > 0:
                    result[currency] = {
                        'free': balance['free'].get(currency, 0),
                        'used': balance['used'].get(currency, 0),
                        'total': amounts
                    }
            
            return result
            
        except Exception as e:
            self.logger.error(f"Failed to fetch balance on {exchange_id}: {e}")
            raise
    
    async def place_order(
        self,
        exchange_id: str,
        symbol: str,
        side: str,
        order_type: str,
        amount: float,
        price: Optional[float] = None,
        stop_loss: Optional[float] = None,
        take_profit: Optional[float] = None,
        params: Optional[Dict] = None
    ) -> Dict[str, Any]:
        """
        Place an order with optional TP/SL.
        
        Args:
            exchange_id: Exchange to use
            symbol: Trading pair (e.g., 'BTC/USDT')
            side: 'buy' or 'sell'
            order_type: 'market', 'limit', 'stop_loss', 'take_profit'
            amount: Order quantity
            price: Limit price (required for limit orders)
            stop_loss: Stop-loss price (creates separate stop order)
            take_profit: Take-profit price (creates separate limit order)
            params: Additional exchange-specific parameters
        
        Returns:
            {
                'id': '12345',
                'symbol': 'BTC/USDT',
                'side': 'buy',
                'type': 'limit',
                'status': 'closed',
                'filled': 0.1,
                'average': 67500.0,
                'timestamp': 1699113600000
            }
        """
        exchange = self._get_exchange(exchange_id)
        params = params or {}
        
        try:
            # Validate inputs
            if order_type == 'limit' and price is None:
                raise ValueError("Price required for limit orders")
            
            # Place main order
            self.logger.info(f"Placing {side} {order_type} order for {amount} {symbol} on {exchange_id}")
            
            order = await exchange.create_order(
                symbol=symbol,
                type=order_type,
                side=side,
                amount=amount,
                price=price,
                params=params
            )
            
            # Place stop-loss if provided
            if stop_loss:
                sl_side = 'sell' if side == 'buy' else 'buy'
                self.logger.info(f"Placing stop-loss at {stop_loss}")
                
                try:
                    await exchange.create_order(
                        symbol=symbol,
                        type='stop_loss_limit' if exchange.has['createStopLossOrder'] else 'stop_market',
                        side=sl_side,
                        amount=amount,
                        price=stop_loss,
                        params={'stopPrice': stop_loss}
                    )
                except Exception as e:
                    self.logger.warning(f"Failed to place stop-loss: {e}")
            
            # Place take-profit if provided
            if take_profit:
                tp_side = 'sell' if side == 'buy' else 'buy'
                self.logger.info(f"Placing take-profit at {take_profit}")
                
                try:
                    await exchange.create_order(
                        symbol=symbol,
                        type='take_profit_limit' if exchange.has['createTakeProfitOrder'] else 'limit',
                        side=tp_side,
                        amount=amount,
                        price=take_profit,
                        params={'stopPrice': take_profit} if 'createTakeProfitOrder' in exchange.has else {}
                    )
                except Exception as e:
                    self.logger.warning(f"Failed to place take-profit: {e}")
            
            # Return standardized order info
            return {
                'id': order['id'],
                'symbol': order['symbol'],
                'side': order['side'],
                'type': order['type'],
                'status': order['status'],
                'filled': order.get('filled', 0),
                'average': order.get('average'),
                'price': order.get('price'),
                'timestamp': order.get('timestamp', 0),
                'info': order.get('info', {}),  # Raw exchange response
            }
            
        except Exception as e:
            self.logger.error(f"Failed to place order on {exchange_id}: {e}")
            raise
    
    async def cancel_order(self, exchange_id: str, order_id: str, symbol: str) -> bool:
        """
        Cancel an open order.
        
        Args:
            exchange_id: Exchange
            order_id: Order ID to cancel
            symbol: Trading pair
        
        Returns:
            True if cancelled successfully
        """
        exchange = self._get_exchange(exchange_id)
        
        try:
            await exchange.cancel_order(order_id, symbol)
            self.logger.info(f"Cancelled order {order_id} on {exchange_id}")
            return True
            
        except Exception as e:
            self.logger.error(f"Failed to cancel order {order_id}: {e}")
            return False
    
    async def fetch_order(self, exchange_id: str, order_id: str, symbol: str) -> Dict[str, Any]:
        """
        Fetch order details.
        
        Args:
            exchange_id: Exchange
            order_id: Order ID
            symbol: Trading pair
        
        Returns:
            Order details dict
        """
        exchange = self._get_exchange(exchange_id)
        
        try:
            order = await exchange.fetch_order(order_id, symbol)
            
            return {
                'id': order['id'],
                'symbol': order['symbol'],
                'side': order['side'],
                'type': order['type'],
                'status': order['status'],
                'filled': order.get('filled', 0),
                'remaining': order.get('remaining', 0),
                'average': order.get('average'),
                'price': order.get('price'),
                'timestamp': order.get('timestamp', 0),
            }
            
        except Exception as e:
            self.logger.error(f"Failed to fetch order {order_id}: {e}")
            raise
    
    def list_exchanges(self) -> List[str]:
        """
        Get list of all CCXT-supported exchanges.
        
        Returns:
            List of exchange IDs
        """
        return ccxt.exchanges
    
    def get_initialized_exchanges(self) -> List[str]:
        """
        Get list of initialized exchanges.
        
        Returns:
            List of exchange IDs that are initialized
        """
        return list(self.exchanges.keys())
    
    async def close_all(self):
        """Close all exchange connections."""
        for exchange_id, exchange in self.exchanges.items():
            try:
                await exchange.close()
                self.logger.info(f"Closed {exchange_id} connection")
            except Exception as e:
                self.logger.warning(f"Error closing {exchange_id}: {e}")
        
        self.exchanges.clear()
    
    def _get_exchange(self, exchange_id: str) -> ccxt.Exchange:
        """Get an initialized exchange or raise error."""
        if exchange_id not in self.exchanges:
            raise ValueError(f"Exchange '{exchange_id}' not initialized. Call init_exchange() first.")
        return self.exchanges[exchange_id]


# Singleton instance
_manager = None

def get_exchange_manager() -> ExchangeManager:
    """Get the global ExchangeManager instance."""
    global _manager
    if _manager is None:
        _manager = ExchangeManager()
    return _manager
