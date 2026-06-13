//! RateLimitActor — global token-bucket rate limiter.
//!
//! ## Messages
//!
//! - `CheckRateLimit` → `bool` (true = allowed, false = rate-limited).

use actix::prelude::*;
use plugin_interface::*;
use std::time::Instant;

// ── Configuration ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct RateLimitConfig {
    /// Max requests per minute (global).
    pub requests_per_minute: u32,
    /// Max burst size (allow short bursts above the sustained rate).
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 30,
            burst_size: 5,
        }
    }
}

impl RateLimitConfig {
    pub fn from_env() -> Self {
        Self {
            requests_per_minute: std::env::var("RATE_LIMIT_PER_MIN")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(30),
            burst_size: std::env::var("RATE_LIMIT_BURST")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(5),
        }
    }
}

// ── Token bucket ─────────────────────────────────────────────────────────────

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    refill_rate: f64,  // tokens per second
}

impl TokenBucket {
    fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            tokens: capacity,
            last_refill: Instant::now(),
            capacity,
            refill_rate,
        }
    }

    fn try_consume(&mut self, count: f64) -> bool {
        self.refill();
        if self.tokens >= count {
            self.tokens -= count;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
            self.last_refill = now;
        }
    }
}

// ── Messages ─────────────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "bool")]
pub struct CheckRateLimit;

// ── Actor ────────────────────────────────────────────────────────────────────

pub struct RateLimitActor {
    bucket: TokenBucket,
    config: RateLimitConfig,
}

impl RateLimitActor {
    pub fn new(config: RateLimitConfig) -> Self {
        let capacity = config.burst_size as f64;
        let refill_rate = config.requests_per_minute as f64 / 60.0;
        Self {
            bucket: TokenBucket::new(capacity, refill_rate),
            config,
        }
    }
}

impl Actor for RateLimitActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!(
            "[RateLimitActor] started ({} req/min, burst {})",
            self.config.requests_per_minute,
            self.config.burst_size,
        );
    }
}

impl Handler<CheckRateLimit> for RateLimitActor {
    type Result = bool;

    fn handle(&mut self, _msg: CheckRateLimit, _ctx: &mut Self::Context) -> bool {
        let allowed = self.bucket.try_consume(1.0);
        if !allowed {
            log::warn!("[RateLimit] rate-limited (tokens={:.1})", self.bucket.tokens);
        }
        allowed
    }
}
