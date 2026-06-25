//! TokenUsageActor — tracks LLM token usage per model, globally.
//!
//! Persisted in SQLite at `data/token_usage.db`.
//!
//! ## Messages
//!
//! - `RecordTokenUsage` — record tokens from an LLM call.
//! - `GetGlobalTokenUsage` — total across all calls.

use actix::prelude::*;
use plugin_interface::*;
use rusqlite::Connection;
use std::sync::Mutex;

// ── Database ─────────────────────────────────────────────────────────────────

fn open_db() -> Result<Connection, String> {
    let path = std::path::Path::new("data/token_usage.db");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {}", e))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("open db: {}", e))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS token_usage (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            model       TEXT NOT NULL,
            prompt_tokens   INTEGER NOT NULL,
            completion_tokens INTEGER NOT NULL,
            prompt_cache_hit_tokens INTEGER NOT NULL DEFAULT 0,
            prompt_cache_miss_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens    INTEGER NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_token_usage_model ON token_usage(model);",
    )
    .map_err(|e| format!("create table: {}", e))?;
    // Migrate: add cache hit/miss columns if upgrading from old schema
    let _ = conn.execute_batch(
        "ALTER TABLE token_usage ADD COLUMN prompt_cache_hit_tokens INTEGER NOT NULL DEFAULT 0;",
    );
    let _ = conn.execute_batch(
        "ALTER TABLE token_usage ADD COLUMN prompt_cache_miss_tokens INTEGER NOT NULL DEFAULT 0;",
    );
    Ok(conn)
}

// ── Messages ─────────────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "()")]
pub struct RecordTokenUsage {
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub prompt_cache_hit_tokens: u32,
    pub prompt_cache_miss_tokens: u32,
}

#[derive(Message)]
#[rtype(result = "TokenUsageSummary")]
pub struct GetGlobalTokenUsage;

/// 查询当前 token 预算是否还允许新请求。
#[derive(Message)]
#[rtype(result = "BudgetCheck")]
pub struct CheckTokenBudget;

/// 预算检查结果。`allowed=false` 时 `period/used/limit` 给出首个超限的周期。
#[derive(Clone, Debug, MessageResponse)]
pub struct BudgetCheck {
    pub allowed: bool,
    pub period: Option<String>,
    pub used: u64,
    pub limit: u64,
}

impl BudgetCheck {
    fn allow() -> Self {
        Self {
            allowed: true,
            period: None,
            used: 0,
            limit: 0,
        }
    }
}

/// Token 预算配置（滚动窗口：日=24h、周=7d、月=30d）。None/0 表示该周期无限制。
#[derive(Clone, Debug, Default)]
pub struct TokenBudgetConfig {
    pub daily: Option<u64>,
    pub weekly: Option<u64>,
    pub monthly: Option<u64>,
}

impl TokenBudgetConfig {
    pub fn from_env() -> Self {
        let parse = |k: &str| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|&n| n > 0)
        };
        Self {
            daily: parse("TOKEN_BUDGET_DAILY"),
            weekly: parse("TOKEN_BUDGET_WEEKLY"),
            monthly: parse("TOKEN_BUDGET_MONTHLY"),
        }
    }
    pub fn is_unlimited(&self) -> bool {
        self.daily.is_none() && self.weekly.is_none() && self.monthly.is_none()
    }
}

/// 纯逻辑：给定配置与“某窗口内已用 token”查询闭包，返回是否超限（首个超限周期）。
fn evaluate_budget<F: Fn(&str) -> u64>(config: &TokenBudgetConfig, used_since: F) -> BudgetCheck {
    if config.is_unlimited() {
        return BudgetCheck::allow();
    }
    let checks = [
        (config.daily, "-1 day", "daily"),
        (config.weekly, "-7 days", "weekly"),
        (config.monthly, "-30 days", "monthly"),
    ];
    for (limit, window, name) in checks {
        if let Some(limit) = limit {
            let used = used_since(window);
            if used >= limit {
                return BudgetCheck {
                    allowed: false,
                    period: Some(name.to_string()),
                    used,
                    limit,
                };
            }
        }
    }
    BudgetCheck::allow()
}

// ── Responses ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize)]
pub struct ModelUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub prompt_cache_hit_tokens: u64,
    pub prompt_cache_miss_tokens: u64,
    pub total_tokens: u64,
    pub call_count: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct TokenUsageSummary {
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_prompt_cache_hit_tokens: u64,
    pub total_prompt_cache_miss_tokens: u64,
    pub total_tokens: u64,
    pub total_calls: u64,
    pub by_model: std::collections::HashMap<String, ModelUsage>,
}

// ── Actor ────────────────────────────────────────────────────────────────────

pub struct TokenUsageActor {
    db: Mutex<Connection>,
    budget: TokenBudgetConfig,
}

impl TokenUsageActor {
    pub fn new() -> Result<Self, String> {
        let db = open_db()?;
        let budget = TokenBudgetConfig::from_env();
        if !budget.is_unlimited() {
            log::info!(
                "[TokenUsageActor] token budget: daily={:?} weekly={:?} monthly={:?}",
                budget.daily,
                budget.weekly,
                budget.monthly
            );
        }
        log::info!("[TokenUsageActor] started");
        Ok(Self {
            db: Mutex::new(db),
            budget,
        })
    }
}

impl Actor for TokenUsageActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!("[TokenUsageActor] actor started");
    }
}

impl Handler<RecordTokenUsage> for TokenUsageActor {
    type Result = ();

    fn handle(&mut self, msg: RecordTokenUsage, _ctx: &mut Self::Context) {
        let total = msg.prompt_tokens + msg.completion_tokens;
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => {
                log::error!("[TokenUsageActor] lock failed: {}", e);
                return;
            }
        };
        if let Err(e) = db.execute(
            "INSERT INTO token_usage (model, prompt_tokens, completion_tokens, prompt_cache_hit_tokens, prompt_cache_miss_tokens, total_tokens) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![msg.model, msg.prompt_tokens, msg.completion_tokens, msg.prompt_cache_hit_tokens, msg.prompt_cache_miss_tokens, total],
        ) {
            log::error!("[TokenUsageActor] insert failed: {}", e);
        }
    }
}

impl Handler<GetGlobalTokenUsage> for TokenUsageActor {
    type Result = MessageResult<GetGlobalTokenUsage>;

    fn handle(&mut self, _: GetGlobalTokenUsage, _ctx: &mut Self::Context) -> Self::Result {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => {
                log::error!("[TokenUsageActor] lock failed: {}", e);
                return MessageResult(TokenUsageSummary {
                    total_prompt_tokens: 0,
                    total_completion_tokens: 0,
                    total_prompt_cache_hit_tokens: 0,
                    total_prompt_cache_miss_tokens: 0,
                    total_tokens: 0,
                    total_calls: 0,
                    by_model: std::collections::HashMap::new(),
                });
            }
        };
        MessageResult(build_summary(&db))
    }
}

impl Handler<CheckTokenBudget> for TokenUsageActor {
    type Result = MessageResult<CheckTokenBudget>;

    fn handle(&mut self, _: CheckTokenBudget, _ctx: &mut Self::Context) -> Self::Result {
        if self.budget.is_unlimited() {
            return MessageResult(BudgetCheck::allow());
        }
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => {
                log::error!("[TokenUsageActor] budget lock failed: {}", e);
                return MessageResult(BudgetCheck::allow());
            }
        };
        let result = evaluate_budget(&self.budget, |window| {
            db.query_row(
                "SELECT COALESCE(SUM(total_tokens),0) FROM token_usage WHERE created_at >= datetime('now', ?1)",
                [window],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            .max(0) as u64
        });
        MessageResult(result)
    }
}

fn build_summary(db: &Connection) -> TokenUsageSummary {
    let totals: (u64, u64, u64, u64, u64, u64) = db
        .query_row(
            "SELECT COALESCE(SUM(prompt_tokens),0), COALESCE(SUM(completion_tokens),0),
                    COALESCE(SUM(prompt_cache_hit_tokens),0), COALESCE(SUM(prompt_cache_miss_tokens),0),
                    COALESCE(SUM(total_tokens),0), COUNT(*) FROM token_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap_or((0, 0, 0, 0, 0, 0));

    let mut stmt = db
        .prepare(
            "SELECT model, SUM(prompt_tokens), SUM(completion_tokens), SUM(prompt_cache_hit_tokens), SUM(prompt_cache_miss_tokens), SUM(total_tokens), COUNT(*)
             FROM token_usage GROUP BY model"
        )
        .unwrap();

    let mut by_model = std::collections::HashMap::new();
    if let Ok(mut rows) = stmt.query([]) {
        while let Ok(Some(row)) = rows.next() {
            let model: String = row.get(0).unwrap_or_default();
            by_model.insert(
                model,
                ModelUsage {
                    prompt_tokens: row.get(1).unwrap_or(0),
                    completion_tokens: row.get(2).unwrap_or(0),
                    prompt_cache_hit_tokens: row.get(3).unwrap_or(0),
                    prompt_cache_miss_tokens: row.get(4).unwrap_or(0),
                    total_tokens: row.get(5).unwrap_or(0),
                    call_count: row.get(6).unwrap_or(0),
                },
            );
        }
    }

    TokenUsageSummary {
        total_prompt_tokens: totals.0,
        total_completion_tokens: totals.1,
        total_prompt_cache_hit_tokens: totals.2,
        total_prompt_cache_miss_tokens: totals.3,
        total_tokens: totals.4,
        total_calls: totals.5,
        by_model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_allows() {
        let r = evaluate_budget(&TokenBudgetConfig::default(), |_| 1_000_000);
        assert!(r.allowed);
    }

    #[test]
    fn under_limit_allows() {
        let cfg = TokenBudgetConfig {
            daily: Some(1000),
            weekly: None,
            monthly: None,
        };
        assert!(evaluate_budget(&cfg, |_| 500).allowed);
    }

    #[test]
    fn daily_over_limit_blocks() {
        let cfg = TokenBudgetConfig {
            daily: Some(1000),
            weekly: None,
            monthly: None,
        };
        let r = evaluate_budget(&cfg, |w| if w == "-1 day" { 1500 } else { 0 });
        assert!(!r.allowed);
        assert_eq!(r.period.as_deref(), Some("daily"));
        assert_eq!(r.used, 1500);
        assert_eq!(r.limit, 1000);
    }

    #[test]
    fn monthly_blocks_when_daily_ok() {
        let cfg = TokenBudgetConfig {
            daily: Some(10000),
            weekly: None,
            monthly: Some(5000),
        };
        let r = evaluate_budget(&cfg, |w| match w {
            "-1 day" => 1000,
            "-30 days" => 6000,
            _ => 0,
        });
        assert!(!r.allowed);
        assert_eq!(r.period.as_deref(), Some("monthly"));
    }

    #[test]
    fn at_exactly_limit_blocks() {
        let cfg = TokenBudgetConfig {
            daily: Some(1000),
            weekly: None,
            monthly: None,
        };
        assert!(!evaluate_budget(&cfg, |_| 1000).allowed);
    }
}
