"""
Prometheus metrics for execution pipeline monitoring.

Tracks webhook processing, order execution, security events, and pipeline performance.
"""

from prometheus_client import Counter, Histogram, Gauge, Enum
from typing import Optional
import time


# ============================================================================
# Webhook Metrics
# ============================================================================

webhook_requests_total = Counter(
    "execution_webhook_requests_total",
    "Total number of webhook requests received",
    ["source", "symbol", "side"],
)

webhook_processing_duration = Histogram(
    "execution_webhook_processing_duration_seconds",
    "Time spent processing webhook requests",
    ["source", "status"],
    buckets=(0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0),
)

webhook_validation_failures = Counter(
    "execution_webhook_validation_failures_total",
    "Total number of webhook validation failures",
    ["source", "reason"],
)

webhook_signature_failures = Counter(
    "execution_webhook_signature_failures_total",
    "Total number of signature verification failures",
    ["source"],
)

webhook_confidence_filtered = Counter(
    "execution_webhook_confidence_filtered_total",
    "Total number of webhooks filtered by confidence threshold",
    ["source", "symbol"],
)

webhook_stale_rejected = Counter(
    "execution_webhook_stale_rejected_total",
    "Total number of stale webhooks rejected",
    ["source", "symbol"],
)


# ============================================================================
# Order Execution Metrics
# ============================================================================

orders_total = Counter(
    "execution_orders_total",
    "Total number of orders executed",
    ["exchange", "symbol", "side", "order_type", "status"],
)

order_execution_duration = Histogram(
    "execution_order_duration_seconds",
    "Time spent executing orders",
    ["exchange", "order_type"],
    buckets=(0.1, 0.25, 0.5, 0.75, 1.0, 2.0, 3.0, 5.0, 10.0, 30.0),
)

order_failures = Counter(
    "execution_order_failures_total",
    "Total number of order execution failures",
    ["exchange", "symbol", "reason"],
)

order_size_usd = Histogram(
    "execution_order_size_usd",
    "Order size in USD",
    ["exchange", "symbol", "side"],
    buckets=(10, 50, 100, 500, 1000, 5000, 10000, 50000, 100000),
)

position_size_pct = Histogram(
    "execution_position_size_pct",
    "Position size as percentage of capital",
    ["method"],
    buckets=(0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 2.5, 3.0, 5.0),
)


# ============================================================================
# Security Metrics
# ============================================================================

rate_limit_requests = Counter(
    "execution_rate_limit_requests_total",
    "Total number of rate limit checks",
    ["client_ip", "allowed"],
)

rate_limit_rejections = Counter(
    "execution_rate_limit_rejections_total",
    "Total number of requests blocked by rate limiting",
    ["client_ip"],
)

circuit_breaker_state = Enum(
    "execution_circuit_breaker_state",
    "Current state of circuit breaker",
    ["exchange"],
    states=["closed", "open", "half_open"],
)

circuit_breaker_transitions = Counter(
    "execution_circuit_breaker_transitions_total",
    "Total number of circuit breaker state transitions",
    ["exchange", "from_state", "to_state"],
)

circuit_breaker_rejections = Counter(
    "execution_circuit_breaker_rejections_total",
    "Total number of requests blocked by circuit breaker",
    ["exchange"],
)

ip_whitelist_checks = Counter(
    "execution_ip_whitelist_checks_total",
    "Total number of IP whitelist checks",
    ["client_ip", "allowed"],
)

ip_whitelist_rejections = Counter(
    "execution_ip_whitelist_rejections_total",
    "Total number of requests blocked by IP whitelist",
    ["client_ip"],
)

audit_events = Counter(
    "execution_audit_events_total",
    "Total number of audit events logged",
    ["event_type", "client_ip", "result"],
)


# ============================================================================
# Validation Metrics
# ============================================================================

validation_errors = Counter(
    "execution_validation_errors_total",
    "Total number of validation errors",
    ["validation_type", "field"],
)

normalization_operations = Counter(
    "execution_normalization_operations_total",
    "Total number of normalization operations",
    ["operation_type"],
)

nan_replacements = Counter(
    "execution_nan_replacements_total",
    "Total number of NaN values replaced",
    ["field"],
)


# ============================================================================
# Performance Metrics
# ============================================================================

active_requests = Gauge(
    "execution_active_requests",
    "Number of requests currently being processed",
)

exchange_connections = Gauge(
    "execution_exchange_connections",
    "Number of active exchange connections",
    ["exchange"],
)

pipeline_latency_percentile = Histogram(
    "execution_pipeline_latency_seconds",
    "End-to-end pipeline latency percentiles",
    buckets=(0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0),
)


# ============================================================================
# Exchange-Specific Metrics
# ============================================================================

exchange_api_calls = Counter(
    "execution_exchange_api_calls_total",
    "Total number of exchange API calls",
    ["exchange", "endpoint", "status"],
)

exchange_api_duration = Histogram(
    "execution_exchange_api_duration_seconds",
    "Time spent calling exchange APIs",
    ["exchange", "endpoint"],
    buckets=(0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0),
)

exchange_errors = Counter(
    "execution_exchange_errors_total",
    "Total number of exchange errors",
    ["exchange", "error_type"],
)

exchange_rate_limits = Counter(
    "execution_exchange_rate_limits_total",
    "Total number of exchange rate limit hits",
    ["exchange"],
)


# ============================================================================
# Helper Functions
# ============================================================================


class MetricsTimer:
    """Context manager for timing operations with Prometheus histograms."""

    def __init__(self, histogram: Histogram, **labels):
        """
        Initialize timer.

        Args:
            histogram: Prometheus histogram to record duration
            **labels: Label values for the histogram
        """
        self.histogram = histogram
        self.labels = labels
        self.start_time: Optional[float] = None

    def __enter__(self):
        """Start timer."""
        self.start_time = time.time()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Stop timer and record duration."""
        if self.start_time is not None:
            duration = time.time() - self.start_time
            self.histogram.labels(**self.labels).observe(duration)


def record_webhook(
    source: str,
    symbol: str,
    side: str,
    status: str,
    duration: float,
    validation_error: Optional[str] = None,
):
    """
    Record webhook processing metrics.

    Args:
        source: Webhook source (e.g., "tradingview")
        symbol: Trading symbol
        side: Order side (buy/sell)
        status: Processing status (success/failure)
        duration: Processing duration in seconds
        validation_error: Validation error reason (if failed)
    """
    webhook_requests_total.labels(source=source, symbol=symbol, side=side).inc()
    webhook_processing_duration.labels(source=source, status=status).observe(duration)

    if validation_error:
        webhook_validation_failures.labels(source=source, reason=validation_error).inc()


def record_order(
    exchange: str,
    symbol: str,
    side: str,
    order_type: str,
    status: str,
    duration: float,
    size_usd: float,
    failure_reason: Optional[str] = None,
):
    """
    Record order execution metrics.

    Args:
        exchange: Exchange name
        symbol: Trading symbol
        side: Order side (buy/sell)
        order_type: Order type (market/limit/stop_loss/take_profit)
        status: Order status (success/failure)
        duration: Execution duration in seconds
        size_usd: Order size in USD
        failure_reason: Failure reason (if failed)
    """
    orders_total.labels(
        exchange=exchange, symbol=symbol, side=side, order_type=order_type, status=status
    ).inc()
    order_execution_duration.labels(exchange=exchange, order_type=order_type).observe(
        duration
    )
    order_size_usd.labels(exchange=exchange, symbol=symbol, side=side).observe(size_usd)

    if failure_reason:
        order_failures.labels(
            exchange=exchange, symbol=symbol, reason=failure_reason
        ).inc()


def record_rate_limit(client_ip: str, allowed: bool):
    """
    Record rate limit check.

    Args:
        client_ip: Client IP address
        allowed: Whether request was allowed
    """
    rate_limit_requests.labels(
        client_ip=client_ip, allowed="true" if allowed else "false"
    ).inc()

    if not allowed:
        rate_limit_rejections.labels(client_ip=client_ip).inc()


def record_circuit_breaker_transition(
    exchange: str, from_state: str, to_state: str
):
    """
    Record circuit breaker state transition.

    Args:
        exchange: Exchange name
        from_state: Previous state
        to_state: New state
    """
    circuit_breaker_transitions.labels(
        exchange=exchange, from_state=from_state, to_state=to_state
    ).inc()
    circuit_breaker_state.labels(exchange=exchange).state(to_state)


def record_audit_event(event_type: str, client_ip: str, result: str):
    """
    Record audit event.

    Args:
        event_type: Type of event (webhook_received, order_placed, etc.)
        client_ip: Client IP address
        result: Event result (success/failure)
    """
    audit_events.labels(
        event_type=event_type, client_ip=client_ip, result=result
    ).inc()
