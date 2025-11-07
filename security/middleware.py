"""
Security middleware and utilities for trading execution.

Provides rate limiting, circuit breakers, IP whitelisting, and audit logging.
"""

import time
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Callable, Any
from collections import defaultdict, deque
from dataclasses import dataclass, field
from enum import Enum
import logging
import asyncio

from ..metrics import (
    rate_limit_requests,
    rate_limit_rejections,
    circuit_breaker_state,
    circuit_breaker_transitions,
    circuit_breaker_rejections,
    ip_whitelist_checks,
    ip_whitelist_rejections,
    audit_events,
)


logger = logging.getLogger(__name__)


class CircuitState(Enum):
    """Circuit breaker states."""

    CLOSED = "closed"  # Normal operation
    OPEN = "open"  # Blocking requests
    HALF_OPEN = "half_open"  # Testing if service recovered


@dataclass
class RateLimitConfig:
    """Configuration for rate limiting."""

    max_requests: int = 100  # Max requests per window
    window_seconds: int = 60  # Time window in seconds
    burst_allowance: int = 10  # Extra requests allowed in burst


@dataclass
class CircuitBreakerConfig:
    """Configuration for circuit breaker."""

    failure_threshold: int = 5  # Failures before opening circuit
    timeout_seconds: int = 60  # Timeout before retry (OPEN → HALF_OPEN)
    success_threshold: int = 2  # Successes needed to close (HALF_OPEN → CLOSED)


class RateLimiter:
    """
    Token bucket rate limiter with burst support.

    Tracks requests per identifier (e.g., IP address, API key) and enforces
    rate limits with configurable windows and burst allowances.
    """

    def __init__(self, config: Optional[RateLimitConfig] = None):
        """
        Initialize rate limiter.

        Args:
            config: Rate limit configuration (uses defaults if None)
        """
        self.config = config or RateLimitConfig()
        # Store (requests, window_start) per identifier
        self._requests: Dict[str, deque] = defaultdict(lambda: deque())
        self._lock = asyncio.Lock()

    async def check_rate_limit(self, identifier: str) -> bool:
        """
        Check if request is within rate limit.

        Args:
            identifier: Unique identifier (e.g., IP, API key)

        Returns:
            True if allowed, False if rate limited
        """
        async with self._lock:
            now = time.time()
            window_start = now - self.config.window_seconds
            requests = self._requests[identifier]

            # Remove old requests outside window
            while requests and requests[0] < window_start:
                requests.popleft()

            # Check if under limit
            allowed = len(requests) < self.config.max_requests + self.config.burst_allowance
            
            # Record metrics
            rate_limit_requests.labels(
                client_ip=identifier,
                allowed="true" if allowed else "false"
            ).inc()
            
            if allowed:
                requests.append(now)
            else:
                rate_limit_rejections.labels(client_ip=identifier).inc()
                logger.warning(
                    f"Rate limit exceeded for {identifier}: "
                    f"{len(requests)} requests in {self.config.window_seconds}s "
                    f"(max: {self.config.max_requests} + {self.config.burst_allowance} burst)"
                )
            
            return allowed

    async def get_stats(self, identifier: str) -> Dict[str, Any]:
        """
        Get rate limit statistics for identifier.

        Args:
            identifier: Unique identifier

        Returns:
            Stats dictionary with requests count, remaining, reset time
        """
        async with self._lock:
            now = time.time()
            window_start = now - self.config.window_seconds
            requests = self._requests[identifier]

            # Count requests in current window
            recent_requests = [r for r in requests if r >= window_start]
            remaining = max(
                0, self.config.max_requests + self.config.burst_allowance - len(recent_requests)
            )

            # Calculate when limit resets
            reset_at = (
                recent_requests[0] + self.config.window_seconds
                if recent_requests
                else now
            )

            return {
                "requests": len(recent_requests),
                "limit": self.config.max_requests,
                "remaining": remaining,
                "reset_at": datetime.fromtimestamp(reset_at).isoformat(),
            }

    def reset(self, identifier: Optional[str] = None):
        """
        Reset rate limit for identifier (or all if None).

        Args:
            identifier: Identifier to reset (None = reset all)
        """
        if identifier is None:
            self._requests.clear()
        elif identifier in self._requests:
            del self._requests[identifier]


class CircuitBreaker:
    """
    Circuit breaker pattern implementation.

    Prevents cascading failures by monitoring error rates and temporarily
    blocking requests to failing services.
    """

    def __init__(
        self,
        name: str,
        config: Optional[CircuitBreakerConfig] = None,
        on_state_change: Optional[Callable[[CircuitState, CircuitState], None]] = None,
    ):
        """
        Initialize circuit breaker.

        Args:
            name: Circuit breaker name (for logging)
            config: Configuration (uses defaults if None)
            on_state_change: Callback when state changes (old_state, new_state)
        """
        self.name = name
        self.config = config or CircuitBreakerConfig()
        self.on_state_change = on_state_change

        self._state = CircuitState.CLOSED
        self._failure_count = 0
        self._success_count = 0
        self._last_failure_time: Optional[float] = None
        self._lock = asyncio.Lock()

    @property
    def state(self) -> CircuitState:
        """Get current circuit state."""
        return self._state

    async def call(self, func: Callable, *args, **kwargs) -> Any:
        """
        Execute function with circuit breaker protection.

        Args:
            func: Function to execute
            *args: Positional arguments
            **kwargs: Keyword arguments

        Returns:
            Function result

        Raises:
            Exception: If circuit is OPEN or function fails
        """
        async with self._lock:
            # Check state and potentially transition
            await self._check_state()

            if self._state == CircuitState.OPEN:
                circuit_breaker_rejections.labels(exchange=self.name).inc()
                raise Exception(
                    f"Circuit breaker '{self.name}' is OPEN "
                    f"(waiting {self.config.timeout_seconds}s)"
                )

        # Execute function
        try:
            if asyncio.iscoroutinefunction(func):
                result = await func(*args, **kwargs)
            else:
                result = func(*args, **kwargs)

            # Record success
            await self._record_success()
            return result

        except Exception as e:
            # Record failure
            await self._record_failure()
            raise

    async def _check_state(self):
        """Check and update circuit state based on timeout."""
        if self._state == CircuitState.OPEN:
            if self._last_failure_time is not None:
                elapsed = time.time() - self._last_failure_time
                if elapsed >= self.config.timeout_seconds:
                    await self._transition_state(CircuitState.HALF_OPEN)

    async def _record_success(self):
        """Record successful call."""
        async with self._lock:
            if self._state == CircuitState.HALF_OPEN:
                self._success_count += 1
                if self._success_count >= self.config.success_threshold:
                    await self._transition_state(CircuitState.CLOSED)
                    self._success_count = 0
                    self._failure_count = 0

    async def _record_failure(self):
        """Record failed call."""
        async with self._lock:
            self._failure_count += 1
            self._last_failure_time = time.time()

            if self._state == CircuitState.HALF_OPEN:
                # Failed during half-open → back to open
                await self._transition_state(CircuitState.OPEN)
                self._success_count = 0

            elif self._state == CircuitState.CLOSED:
                # Check if failures exceed threshold
                if self._failure_count >= self.config.failure_threshold:
                    await self._transition_state(CircuitState.OPEN)

    async def _transition_state(self, new_state: CircuitState):
        """
        Transition to new state.

        Args:
            new_state: New circuit state
        """
        old_state = self._state
        self._state = new_state

        # Record metrics
        circuit_breaker_transitions.labels(
            exchange=self.name,
            from_state=old_state.value,
            to_state=new_state.value
        ).inc()
        circuit_breaker_state.labels(exchange=self.name).state(new_state.value)

        logger.info(f"Circuit breaker '{self.name}': {old_state.value} → {new_state.value}")

        if self.on_state_change:
            try:
                self.on_state_change(old_state, new_state)
            except Exception as e:
                logger.error(f"Error in state change callback: {e}")

    async def get_stats(self) -> Dict[str, Any]:
        """
        Get circuit breaker statistics.

        Returns:
            Stats dictionary
        """
        async with self._lock:
            return {
                "name": self.name,
                "state": self._state.value,
                "failure_count": self._failure_count,
                "success_count": self._success_count,
                "threshold": self.config.failure_threshold,
                "last_failure": (
                    datetime.fromtimestamp(self._last_failure_time).isoformat()
                    if self._last_failure_time
                    else None
                ),
            }

    async def reset(self):
        """Reset circuit breaker to CLOSED state."""
        async with self._lock:
            old_state = self._state
            self._state = CircuitState.CLOSED
            self._failure_count = 0
            self._success_count = 0
            self._last_failure_time = None

            if old_state != CircuitState.CLOSED:
                logger.info(f"Circuit breaker '{self.name}' manually reset to CLOSED")
                if self.on_state_change:
                    try:
                        self.on_state_change(old_state, CircuitState.CLOSED)
                    except Exception as e:
                        logger.error(f"Error in state change callback: {e}")


class IPWhitelist:
    """
    IP address whitelist with CIDR support.

    Allows requests only from whitelisted IP addresses or ranges.
    """

    def __init__(self, allowed_ips: Optional[List[str]] = None):
        """
        Initialize IP whitelist.

        Args:
            allowed_ips: List of allowed IPs/CIDR ranges (None = allow all)
        """
        self.allowed_ips = set(allowed_ips) if allowed_ips else None

    def is_allowed(self, ip: str) -> bool:
        """
        Check if IP is whitelisted.

        Args:
            ip: IP address to check

        Returns:
            True if allowed, False otherwise
        """
        if self.allowed_ips is None:
            allowed = True  # No whitelist = allow all
        else:
            # Exact match
            if ip in self.allowed_ips:
                allowed = True
            else:
                # Check CIDR ranges (simplified - production should use ipaddress module)
                allowed = False
                for allowed_ip in self.allowed_ips:
                    if "/" in allowed_ip:
                        # CIDR notation - simplified check
                        # Production: use ipaddress.ip_address(ip) in ipaddress.ip_network(allowed_ip)
                        network = allowed_ip.split("/")[0]
                        if ip.startswith(network.rsplit(".", 1)[0]):
                            allowed = True
                            break
        
        # Record metrics
        ip_whitelist_checks.labels(
            client_ip=ip,
            allowed="true" if allowed else "false"
        ).inc()
        
        if not allowed:
            ip_whitelist_rejections.labels(client_ip=ip).inc()
            logger.warning(f"IP {ip} not in whitelist")
        
        return allowed

    def add_ip(self, ip: str):
        """Add IP to whitelist."""
        if self.allowed_ips is None:
            self.allowed_ips = set()
        self.allowed_ips.add(ip)

    def remove_ip(self, ip: str):
        """Remove IP from whitelist."""
        if self.allowed_ips and ip in self.allowed_ips:
            self.allowed_ips.remove(ip)


@dataclass
class AuditLogEntry:
    """Audit log entry."""

    timestamp: datetime
    action: str
    identifier: str  # IP, user ID, etc.
    success: bool
    details: Dict[str, Any] = field(default_factory=dict)
    error: Optional[str] = None


class AuditLogger:
    """
    Audit logging for security events.

    Tracks all security-relevant actions (auth, rate limits, circuit breaks).
    """

    def __init__(self, max_entries: int = 10000):
        """
        Initialize audit logger.

        Args:
            max_entries: Maximum log entries to keep in memory
        """
        self.max_entries = max_entries
        self._entries: deque[AuditLogEntry] = deque(maxlen=max_entries)
        self._lock = asyncio.Lock()

    async def log(
        self,
        action: str,
        identifier: str,
        success: bool,
        details: Optional[Dict[str, Any]] = None,
        error: Optional[str] = None,
    ):
        """
        Log audit event.

        Args:
            action: Action type (e.g., "webhook_request", "rate_limit", "auth")
            identifier: Identifier (IP, user ID, etc.)
            success: Whether action succeeded
            details: Additional details
            error: Error message if failed
        """
        async with self._lock:
            entry = AuditLogEntry(
                timestamp=datetime.utcnow(),
                action=action,
                identifier=identifier,
                success=success,
                details=details or {},
                error=error,
            )
            self._entries.append(entry)
            
            # Record metric
            audit_events.labels(
                event_type=action,
                client_ip=identifier,
                result="success" if success else "failure"
            ).inc()

            # Also log to standard logger
            level = logging.INFO if success else logging.WARNING
            logger.log(
                level,
                f"AUDIT: {action} by {identifier} - "
                f"{'SUCCESS' if success else 'FAILED'}"
                + (f": {error}" if error else ""),
            )

    async def get_recent(self, count: int = 100) -> List[AuditLogEntry]:
        """
        Get recent audit log entries.

        Args:
            count: Number of entries to retrieve

        Returns:
            List of recent entries
        """
        async with self._lock:
            return list(self._entries)[-count:]

    async def get_by_identifier(
        self, identifier: str, count: int = 100
    ) -> List[AuditLogEntry]:
        """
        Get audit entries for specific identifier.

        Args:
            identifier: Identifier to filter by
            count: Max entries to return

        Returns:
            Filtered entries
        """
        async with self._lock:
            matches = [e for e in self._entries if e.identifier == identifier]
            return matches[-count:]


def create_rate_limiter(config: Optional[RateLimitConfig] = None) -> RateLimiter:
    """Factory function to create RateLimiter."""
    return RateLimiter(config)


def create_circuit_breaker(
    name: str,
    config: Optional[CircuitBreakerConfig] = None,
    on_state_change: Optional[Callable[[CircuitState, CircuitState], None]] = None,
) -> CircuitBreaker:
    """Factory function to create CircuitBreaker."""
    return CircuitBreaker(name, config, on_state_change)


def create_ip_whitelist(allowed_ips: Optional[List[str]] = None) -> IPWhitelist:
    """Factory function to create IPWhitelist."""
    return IPWhitelist(allowed_ips)


def create_audit_logger(max_entries: int = 10000) -> AuditLogger:
    """Factory function to create AuditLogger."""
    return AuditLogger(max_entries)
