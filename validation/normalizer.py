"""
Data validation and normalization for trading execution.

Provides comprehensive validation, normalization, and position sizing
for trading orders to ensure data quality and risk management.
"""

import re
from decimal import Decimal, ROUND_DOWN
from typing import Any, Dict, Optional
import math

from ..metrics import (
    validation_errors,
    normalization_operations,
    nan_replacements,
)


class ValidationError(Exception):
    """Raised when validation fails."""

    pass


class DataNormalizer:
    """
    Normalize and validate trading data.

    Handles:
    - NaN/None/Inf values
    - Type conversions
    - Symbol normalization
    - Price/quantity precision
    - Range validation
    """

    def __init__(
        self,
        max_price_deviation: float = 0.1,  # 10% max deviation from market
        min_quantity: float = 0.0001,
        max_quantity: float = 1000.0,
        price_precision: int = 8,
        quantity_precision: int = 8,
    ):
        """
        Initialize normalizer.

        Args:
            max_price_deviation: Maximum allowed price deviation from market (0.1 = 10%)
            min_quantity: Minimum order quantity
            max_quantity: Maximum order quantity
            price_precision: Number of decimal places for prices
            quantity_precision: Number of decimal places for quantities
        """
        self.max_price_deviation = max_price_deviation
        self.min_quantity = min_quantity
        self.max_quantity = max_quantity
        self.price_precision = price_precision
        self.quantity_precision = quantity_precision

    def normalize_symbol(self, symbol: str) -> str:
        """
        Normalize trading pair symbols to CCXT format (BASE/QUOTE).

        Examples:
            BTC-USDT → BTC/USDT
            BTCUSDT → BTC/USDT
            btc_usdt → BTC/USDT
            BTC/USDT → BTC/USDT (unchanged)

        Args:
            symbol: Raw symbol string

        Returns:
            Normalized symbol in BASE/QUOTE format

        Raises:
            ValidationError: If symbol cannot be normalized
        """
        normalization_operations.labels(operation_type="symbol").inc()
        
        if not symbol or not isinstance(symbol, str):
            validation_errors.labels(validation_type="symbol", field="symbol").inc()
            raise ValidationError(f"Invalid symbol: {symbol}")

        # Already in correct format
        if "/" in symbol:
            return symbol.upper()

        # Remove common separators
        clean = symbol.upper().replace("-", "").replace("_", "")

        # Known quote currencies (order matters - check longer first)
        quote_currencies = ["USDT", "BUSD", "USDC", "USD", "BTC", "ETH", "BNB"]

        for quote in quote_currencies:
            if clean.endswith(quote):
                base = clean[: -len(quote)]
                if base:  # Ensure base is not empty
                    return f"{base}/{quote}"

        validation_errors.labels(validation_type="symbol", field="symbol").inc()
        raise ValidationError(f"Cannot normalize symbol: {symbol}")

    def clean_numeric(self, value: Any, field_name: str = "value") -> float:
        """
        Clean and validate numeric values.

        Handles:
        - None → raises ValidationError
        - NaN/Inf → raises ValidationError
        - String numbers → converted to float
        - Negative values → allowed (caller should validate if needed)

        Args:
            value: Value to clean
            field_name: Name of field (for error messages)

        Returns:
            Cleaned float value

        Raises:
            ValidationError: If value is None, NaN, Inf, or cannot be converted
        """
        if value is None:
            validation_errors.labels(validation_type="numeric", field=field_name).inc()
            raise ValidationError(f"{field_name} cannot be None")

        # Convert to float
        try:
            if isinstance(value, str):
                value = float(value.strip())
            else:
                value = float(value)
        except (ValueError, TypeError) as e:
            validation_errors.labels(validation_type="numeric", field=field_name).inc()
            raise ValidationError(f"Invalid {field_name}: {value} ({e})")

        # Check for NaN/Inf
        if math.isnan(value):
            nan_replacements.labels(field=field_name).inc()
            validation_errors.labels(validation_type="nan", field=field_name).inc()
            raise ValidationError(f"{field_name} is NaN")
        if math.isinf(value):
            validation_errors.labels(validation_type="inf", field=field_name).inc()
            raise ValidationError(f"{field_name} is infinite")

        return value

    def round_to_precision(self, value: float, precision: int) -> float:
        """
        Round value to specified decimal precision (ROUND_DOWN for safety).

        Args:
            value: Value to round
            precision: Number of decimal places

        Returns:
            Rounded value
        """
        if precision < 0:
            raise ValidationError(f"Invalid precision: {precision}")

        decimal_value = Decimal(str(value))
        quantize_str = "0." + "0" * (precision - 1) + "1" if precision > 0 else "1"
        return float(decimal_value.quantize(Decimal(quantize_str), rounding=ROUND_DOWN))

    def normalize_price(
        self, price: Any, market_price: Optional[float] = None
    ) -> float:
        """
        Normalize and validate price.

        Args:
            price: Raw price value
            market_price: Current market price (for deviation check)

        Returns:
            Normalized price

        Raises:
            ValidationError: If price is invalid or deviates too much
        """
        normalization_operations.labels(operation_type="price").inc()
        price = self.clean_numeric(price, "price")

        if price <= 0:
            validation_errors.labels(validation_type="price", field="price").inc()
            raise ValidationError(f"Price must be positive: {price}")

        # Check deviation from market price
        if market_price is not None:
            market_price = self.clean_numeric(market_price, "market_price")
            deviation = abs(price - market_price) / market_price
            if deviation > self.max_price_deviation:
                validation_errors.labels(validation_type="price_deviation", field="price").inc()
                raise ValidationError(
                    f"Price {price} deviates {deviation:.1%} from market {market_price} "
                    f"(max allowed: {self.max_price_deviation:.1%})"
                )

        return self.round_to_precision(price, self.price_precision)

    def normalize_quantity(self, quantity: Any) -> float:
        """
        Normalize and validate quantity.

        Args:
            quantity: Raw quantity value

        Returns:
            Normalized quantity

        Raises:
            ValidationError: If quantity is invalid or out of range
        """
        normalization_operations.labels(operation_type="quantity").inc()
        quantity = self.clean_numeric(quantity, "quantity")

        if quantity <= 0:
            validation_errors.labels(validation_type="quantity", field="quantity").inc()
            raise ValidationError(f"Quantity must be positive: {quantity}")

        if quantity < self.min_quantity:
            validation_errors.labels(validation_type="quantity_range", field="quantity").inc()
            raise ValidationError(
                f"Quantity {quantity} below minimum {self.min_quantity}"
            )

        if quantity > self.max_quantity:
            validation_errors.labels(validation_type="quantity_range", field="quantity").inc()
            raise ValidationError(
                f"Quantity {quantity} above maximum {self.max_quantity}"
            )

        return self.round_to_precision(quantity, self.quantity_precision)

    def normalize_order(
        self, order: Dict[str, Any], market_price: Optional[float] = None
    ) -> Dict[str, Any]:
        """
        Normalize complete order data.

        Args:
            order: Raw order dictionary
            market_price: Current market price

        Returns:
            Normalized order

        Raises:
            ValidationError: If any field is invalid
        """
        normalized = {}

        # Required fields
        if "symbol" not in order:
            raise ValidationError("Missing required field: symbol")
        normalized["symbol"] = self.normalize_symbol(order["symbol"])

        if "quantity" not in order:
            raise ValidationError("Missing required field: quantity")
        normalized["quantity"] = self.normalize_quantity(order["quantity"])

        # Optional price (required for limit orders)
        if "price" in order and order["price"] is not None:
            normalized["price"] = self.normalize_price(order["price"], market_price)

        # Optional stop loss / take profit
        if "stop_loss" in order and order["stop_loss"] is not None:
            normalized["stop_loss"] = self.normalize_price(order["stop_loss"])

        if "take_profit" in order and order["take_profit"] is not None:
            normalized["take_profit"] = self.normalize_price(order["take_profit"])

        # Pass through other fields
        for key in ["side", "order_type", "confidence", "timestamp"]:
            if key in order:
                normalized[key] = order[key]

        return normalized


class PositionSizer:
    """
    Calculate position sizes based on risk parameters.

    Implements various position sizing strategies:
    - Fixed percentage of capital
    - Risk-based (% of capital to risk per trade)
    - Volatility-adjusted
    """

    def __init__(
        self,
        account_balance: float,
        max_risk_per_trade: float = 0.01,  # 1% of capital per trade
        max_position_size: float = 0.1,  # 10% of capital max
    ):
        """
        Initialize position sizer.

        Args:
            account_balance: Total account balance
            max_risk_per_trade: Maximum % of capital to risk (0.01 = 1%)
            max_position_size: Maximum % of capital per position (0.1 = 10%)
        """
        if account_balance <= 0:
            raise ValueError("Account balance must be positive")
        if not 0 < max_risk_per_trade <= 1:
            raise ValueError("max_risk_per_trade must be between 0 and 1")
        if not 0 < max_position_size <= 1:
            raise ValueError("max_position_size must be between 0 and 1")

        self.account_balance = account_balance
        self.max_risk_per_trade = max_risk_per_trade
        self.max_position_size = max_position_size

    def calculate_fixed_percentage(self, percentage: float, price: float) -> float:
        """
        Calculate position size as fixed percentage of capital.

        Args:
            percentage: Percentage of capital (0.1 = 10%)
            price: Asset price

        Returns:
            Position size in asset units
        """
        if not 0 < percentage <= 1:
            raise ValueError("Percentage must be between 0 and 1")
        if price <= 0:
            raise ValueError("Price must be positive")

        position_value = self.account_balance * min(percentage, self.max_position_size)
        return position_value / price

    def calculate_risk_based(
        self, entry_price: float, stop_loss: float, risk_percentage: Optional[float] = None
    ) -> float:
        """
        Calculate position size based on risk per trade.

        Formula:
            Position Size = (Account Balance × Risk %) / (Entry Price - Stop Loss)

        Args:
            entry_price: Planned entry price
            stop_loss: Stop loss price
            risk_percentage: % of capital to risk (default: max_risk_per_trade)

        Returns:
            Position size in asset units

        Raises:
            ValidationError: If stop loss is invalid
        """
        if entry_price <= 0 or stop_loss <= 0:
            raise ValidationError("Prices must be positive")

        # Validate stop loss direction
        risk_per_unit = abs(entry_price - stop_loss)
        if risk_per_unit == 0:
            raise ValidationError("Stop loss cannot equal entry price")

        # Default to max risk per trade
        if risk_percentage is None:
            risk_percentage = self.max_risk_per_trade

        risk_percentage = min(risk_percentage, self.max_risk_per_trade)
        risk_amount = self.account_balance * risk_percentage

        position_size = risk_amount / risk_per_unit

        # Cap at max position size
        max_size = self.calculate_fixed_percentage(self.max_position_size, entry_price)
        return min(position_size, max_size)

    def calculate_volatility_adjusted(
        self, price: float, volatility: float, base_percentage: float = 0.05
    ) -> float:
        """
        Calculate position size adjusted for volatility (higher vol = smaller size).

        Args:
            price: Asset price
            volatility: Volatility measure (e.g., standard deviation, ATR)
            base_percentage: Base position size (adjusted by volatility)

        Returns:
            Position size in asset units
        """
        if price <= 0:
            raise ValueError("Price must be positive")
        if volatility < 0:
            raise ValueError("Volatility cannot be negative")

        # Normalize volatility to percentage
        volatility_pct = volatility / price if volatility > 0 else 0.01

        # Reduce position size for higher volatility
        # Example: 1% vol → 100% of base, 5% vol → 20% of base
        adjustment_factor = 0.01 / volatility_pct if volatility_pct > 0 else 1.0
        adjustment_factor = min(adjustment_factor, 1.0)  # Cap at 100%

        adjusted_percentage = base_percentage * adjustment_factor
        return self.calculate_fixed_percentage(adjusted_percentage, price)


def create_normalizer(**kwargs) -> DataNormalizer:
    """Factory function to create DataNormalizer instance."""
    return DataNormalizer(**kwargs)


def create_position_sizer(
    account_balance: float, **kwargs
) -> PositionSizer:
    """Factory function to create PositionSizer instance."""
    return PositionSizer(account_balance, **kwargs)
