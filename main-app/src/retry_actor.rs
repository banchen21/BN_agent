//! RetryActor — wraps LLM calls with exponential-backoff retry + circuit breaker.
//!
//! ## Circuit breaker states
//!
//! - **Closed** — normal operation, requests pass through.
//! - **Open** — after N consecutive failures; reject immediately for a cooldown.
//! - **HalfOpen** — after cooldown, allow one request; success → Closed, failure → Open.
//!
//! ## Messages
//!
//! - `RetryChatRequest` — wraps `ChatRequest` with retry config.

use actix::prelude::*;
use plugin_interface::*;
use std::time::{Duration, Instant};

// ── Configuration ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    pub circuit_breaker_threshold: u32,
    pub circuit_breaker_cooldown_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
            circuit_breaker_threshold: 5,
            circuit_breaker_cooldown_ms: 60_000,
        }
    }
}

impl RetryConfig {
    /// Load from environment variables.
    pub fn from_env() -> Self {
        Self {
            max_retries: std::env::var("RETRY_MAX_ATTEMPTS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(3),
            base_delay_ms: std::env::var("RETRY_BASE_DELAY_MS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(1_000),
            max_delay_ms: std::env::var("RETRY_MAX_DELAY_MS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(30_000),
            circuit_breaker_threshold: std::env::var("CIRCUIT_BREAKER_THRESHOLD")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(5),
            circuit_breaker_cooldown_ms: std::env::var("CIRCUIT_BREAKER_COOLDOWN_MS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(60_000),
        }
    }
}

// ── Circuit breaker ──────────────────────────────────────────────────────────

enum CircuitState {
    Closed,
    Open { until: Instant },
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitState::Closed => write!(f, "closed"),
            CircuitState::Open { .. } => write!(f, "open"),
            CircuitState::HalfOpen => write!(f, "half-open"),
        }
    }
}

// ── Messages ─────────────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "Result<LlmResponse, String>")]
pub struct RetryChatRequest {
    pub request: ChatRequest,
    pub max_retries: u32,
}

/// Query current circuit breaker state (for metrics / debugging).
#[derive(Message)]
#[rtype(result = "String")]
pub struct CircuitStateQuery;

// ── Actor ────────────────────────────────────────────────────────────────────

pub struct RetryActor {
    llm_recipient: Recipient<ChatRequest>,
    config: RetryConfig,
    circuit: CircuitState,
    consecutive_failures: u32,
}

impl RetryActor {
    pub fn new(llm_recipient: Recipient<ChatRequest>, config: RetryConfig) -> Self {
        Self {
            llm_recipient,
            config,
            circuit: CircuitState::Closed,
            consecutive_failures: 0,
        }
    }
}

impl Actor for RetryActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!(
            "[RetryActor] started (max_retries={}, circuit_threshold={})",
            self.config.max_retries,
            self.config.circuit_breaker_threshold,
        );
    }
}

impl Handler<RetryChatRequest> for RetryActor {
    type Result = ResponseActFuture<Self, Result<LlmResponse, String>>;

    fn handle(&mut self, msg: RetryChatRequest, _ctx: &mut Self::Context) -> Self::Result {
        // Check circuit breaker.
        match &self.circuit {
            CircuitState::Open { until } if Instant::now() < *until => {
                let remaining = until.saturating_duration_since(Instant::now()).as_secs();
                log::warn!("[RetryActor] circuit open, rejecting (cooldown {}s remaining)", remaining);
                return Box::pin(fut::ready(Err(format!(
                    "service temporarily unavailable (circuit open, {}s cooldown)", remaining
                ))));
            }
            CircuitState::Open { .. } => {
                // Cooldown expired → transition to half-open.
                log::info!("[RetryActor] circuit → half-open (cooldown expired)");
                self.circuit = CircuitState::HalfOpen;
            }
            _ => {}
        }

        let llm = self.llm_recipient.clone();
        let max_retries = msg.max_retries.min(10).max(1);
        let base_delay_ms = self.config.base_delay_ms;
        let max_delay_ms = self.config.max_delay_ms;
        let circuit_threshold = self.config.circuit_breaker_threshold;

        let fut = async move {
            let mut last_error = String::new();
            let mut attempt = 0u32;

            loop {
                attempt += 1;
                let start = Instant::now();

                let result = llm.send(msg.request.clone()).await;

                match result {
                    Ok(Ok(resp)) => {
                        // Success — record latency and return.
                        let elapsed = start.elapsed().as_secs_f64();
                        log::info!("[RetryActor] attempt {attempt} succeeded in {elapsed:.2}s");
                        return Ok(resp);
                    }
                    Ok(Err(e)) => {
                        last_error = e;
                        log::warn!("[RetryActor] attempt {attempt} failed: {last_error}");
                    }
                    Err(e) => {
                        last_error = format!("mailbox error: {}", e);
                        log::warn!("[RetryActor] attempt {attempt} mailbox: {last_error}");
                    }
                }

                if attempt >= max_retries {
                    log::error!("[RetryActor] exhausted {max_retries} attempts, giving up");
                    return Err(format!("retry exhausted after {max_retries} attempts: {last_error}"));
                }

                // Exponential backoff.
                let delay = (base_delay_ms as u64)
                    .saturating_mul(1u64 << (attempt - 1).min(10))
                    .min(max_delay_ms);
                log::info!("[RetryActor] retrying in {delay}ms (attempt {}/{max_retries})", attempt + 1);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
        .into_actor(self)
        .map(|result, this: &mut Self, _ctx| {
            match &result {
                Ok(_) => {
                    // Success: close circuit.
                    this.consecutive_failures = 0;
                    this.circuit = CircuitState::Closed;
                }
                Err(_) => {
                    this.consecutive_failures += 1;
                    if this.consecutive_failures >= this.config.circuit_breaker_threshold {
                        let cooldown = this.config.circuit_breaker_cooldown_ms;
                        let until = Instant::now() + Duration::from_millis(cooldown);
                        log::warn!(
                            "[RetryActor] {} consecutive failures → circuit open for {}ms",
                            this.consecutive_failures, cooldown,
                        );
                        this.circuit = CircuitState::Open { until };
                    } else if matches!(this.circuit, CircuitState::HalfOpen) {
                        // Half-open + failure → back to open.
                        let cooldown = this.config.circuit_breaker_cooldown_ms;
                        let until = Instant::now() + Duration::from_millis(cooldown);
                        log::warn!("[RetryActor] half-open test failed → circuit open for {cooldown}ms");
                        this.circuit = CircuitState::Open { until };
                    }
                }
            }
            result
        });

        Box::pin(fut)
    }
}

impl Handler<CircuitStateQuery> for RetryActor {
    type Result = String;
    fn handle(&mut self, _: CircuitStateQuery, _: &mut Self::Context) -> String {
        format!("circuit={}, consecutive_failures={}", self.circuit, self.consecutive_failures)
    }
}
