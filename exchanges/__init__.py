"""
Exchanges module - CCXT integration for cryptocurrency exchanges.
"""

from .manager import ExchangeManager, get_exchange_manager
from .ccxt_plugin import CCXTPlugin, create_ccxt_plugin, OrderSide, OrderType, OrderStatus

__all__ = [
    'ExchangeManager',
    'get_exchange_manager',
    'CCXTPlugin',
    'create_ccxt_plugin',
    'OrderSide',
    'OrderType',
    'OrderStatus',
]
