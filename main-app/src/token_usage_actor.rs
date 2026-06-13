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
        CREATE INDEX IF NOT EXISTS idx_token_usage_model ON token_usage(model);"
    ).map_err(|e| format!("create table: {}", e))?;
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
}

impl TokenUsageActor {
    pub fn new() -> Result<Self, String> {
        let db = open_db()?;
        log::info!("[TokenUsageActor] started");
        Ok(Self { db: Mutex::new(db) })
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
            by_model.insert(model, ModelUsage {
                prompt_tokens: row.get(1).unwrap_or(0),
                completion_tokens: row.get(2).unwrap_or(0),
                prompt_cache_hit_tokens: row.get(3).unwrap_or(0),
                prompt_cache_miss_tokens: row.get(4).unwrap_or(0),
                total_tokens: row.get(5).unwrap_or(0),
                call_count: row.get(6).unwrap_or(0),
            });
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
