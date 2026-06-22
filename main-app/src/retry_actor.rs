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
use rusqlite::Connection;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 当前 Unix 毫秒时间戳。用于把单调时钟 `Instant` 的冷却截止时间持久化为绝对时间。
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

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

impl CircuitState {
    /// 转为可持久化表示：(状态名, Open 的绝对冷却截止 Unix ms)。
    fn persist_repr(&self) -> (&'static str, Option<i64>) {
        match self {
            CircuitState::Closed => ("closed", None),
            CircuitState::HalfOpen => ("half-open", None),
            CircuitState::Open { until } => {
                let remaining = until.saturating_duration_since(Instant::now()).as_millis() as u64;
                ("open", Some((now_unix_ms() + remaining) as i64))
            }
        }
    }
}

/// 熔断状态的 SQLite 持久化（单行 id=1），用于进程重启后保持 open/half-open。
struct CircuitPersist {
    conn: Connection,
}

impl CircuitPersist {
    fn open() -> Option<Self> {
        let path = std::env::var("CIRCUIT_BREAKER_DB_PATH")
            .unwrap_or_else(|_| "data/circuit_breaker.db".to_string());
        Self::open_path(&path)
    }

    fn open_path(path: &str) -> Option<Self> {
        let p = std::path::Path::new(path);
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let conn = match Connection::open(p) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[RetryActor] circuit breaker persistence disabled: {}", e);
                return None;
            }
        };
        if let Err(e) = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS circuit_breaker (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                state TEXT NOT NULL,
                open_until_ms INTEGER,
                consecutive_failures INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        ) {
            log::warn!("[RetryActor] circuit breaker table init failed: {}", e);
            return None;
        }
        Some(Self { conn })
    }

    fn save(&self, state: &CircuitState, failures: u32) {
        let (s, until) = state.persist_repr();
        if let Err(e) = self.conn.execute(
            "INSERT INTO circuit_breaker (id, state, open_until_ms, consecutive_failures, updated_at)
             VALUES (1, ?1, ?2, ?3, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
                state = ?1, open_until_ms = ?2, consecutive_failures = ?3, updated_at = datetime('now')",
            rusqlite::params![s, until, failures as i64],
        ) {
            log::warn!("[RetryActor] circuit breaker save failed: {}", e);
        }
    }

    fn load(&self) -> Option<(CircuitState, u32)> {
        let (s, until_ms, failures): (String, Option<i64>, i64) = self
            .conn
            .query_row(
                "SELECT state, open_until_ms, consecutive_failures FROM circuit_breaker WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok()?;
        let state = match s.as_str() {
            "open" => {
                let now = now_unix_ms() as i64;
                match until_ms {
                    Some(u) if u > now => CircuitState::Open {
                        until: Instant::now() + Duration::from_millis((u - now) as u64),
                    },
                    // 冷却已过期 → 半开，允许一次试探请求
                    _ => CircuitState::HalfOpen,
                }
            }
            "half-open" => CircuitState::HalfOpen,
            _ => CircuitState::Closed,
        };
        Some((state, failures.max(0) as u32))
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
    persist: Option<CircuitPersist>,
}

impl RetryActor {
    pub fn new(llm_recipient: Recipient<ChatRequest>, config: RetryConfig) -> Self {
        let persist = CircuitPersist::open();
        let (circuit, consecutive_failures) = persist
            .as_ref()
            .and_then(|p| p.load())
            .unwrap_or((CircuitState::Closed, 0));
        if !matches!(circuit, CircuitState::Closed) {
            log::info!(
                "[RetryActor] restored circuit state: {} (consecutive_failures={})",
                circuit, consecutive_failures,
            );
        }
        Self {
            llm_recipient,
            config,
            circuit,
            consecutive_failures,
            persist,
        }
    }

    /// 把当前熔断状态写入持久化存储。
    fn persist_state(&self) {
        if let Some(ref p) = self.persist {
            p.save(&self.circuit, self.consecutive_failures);
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
                self.persist_state();
            }
            _ => {}
        }

        let llm = self.llm_recipient.clone();
        let max_retries = msg.max_retries.min(10).max(1);
        let base_delay_ms = self.config.base_delay_ms;
        let max_delay_ms = self.config.max_delay_ms;

        let fut = async move {
            let mut last_error;
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
            this.persist_state();
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

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_path(tag: &str) -> String {
        let mut p = std::env::temp_dir();
        p.push(format!("bn_agent_cb_test_{}_{}.db", tag, std::process::id()));
        let _ = std::fs::remove_file(&p);
        p.to_string_lossy().to_string()
    }

    #[test]
    fn persist_repr_closed_and_halfopen() {
        assert_eq!(CircuitState::Closed.persist_repr(), ("closed", None));
        assert_eq!(CircuitState::HalfOpen.persist_repr(), ("half-open", None));
    }

    #[test]
    fn persist_repr_open_is_future_ms() {
        let state = CircuitState::Open {
            until: Instant::now() + Duration::from_millis(50_000),
        };
        let (name, until) = state.persist_repr();
        assert_eq!(name, "open");
        assert!(until.expect("open has until") > now_unix_ms() as i64);
    }

    #[test]
    fn roundtrip_closed() {
        let path = temp_db_path("closed");
        let p = CircuitPersist::open_path(&path).expect("open");
        p.save(&CircuitState::Closed, 0);
        let (state, failures) = p.load().expect("load");
        assert_eq!(format!("{}", state), "closed");
        assert_eq!(failures, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn roundtrip_open_not_expired_restores_open() {
        let path = temp_db_path("open_live");
        let p = CircuitPersist::open_path(&path).expect("open");
        p.save(
            &CircuitState::Open {
                until: Instant::now() + Duration::from_millis(60_000),
            },
            5,
        );
        let (restored, failures) = p.load().expect("load");
        assert_eq!(format!("{}", restored), "open");
        assert_eq!(failures, 5);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_expired_restores_halfopen() {
        let path = temp_db_path("open_expired");
        let p = CircuitPersist::open_path(&path).expect("open");
        // 直接写入一个已经过期的 open_until_ms
        p.conn
            .execute(
                "INSERT INTO circuit_breaker (id, state, open_until_ms, consecutive_failures, updated_at)
                 VALUES (1, 'open', ?1, 7, datetime('now'))",
                rusqlite::params![now_unix_ms() as i64 - 1000],
            )
            .unwrap();
        let (restored, failures) = p.load().expect("load");
        assert_eq!(format!("{}", restored), "half-open");
        assert_eq!(failures, 7);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_overwrites_single_row() {
        let path = temp_db_path("single_row");
        let p = CircuitPersist::open_path(&path).expect("open");
        p.save(&CircuitState::Closed, 0);
        p.save(&CircuitState::HalfOpen, 2);
        let (state, failures) = p.load().expect("load");
        assert_eq!(format!("{}", state), "half-open");
        assert_eq!(failures, 2);
        let _ = std::fs::remove_file(&path);
    }
}
