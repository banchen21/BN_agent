//! TokenUsageActor — tracks LLM token usage per chat_id, per model, total.
//!
//! Persisted in SQLite at `data/token_usage.db`.
//!
//! ## Messages
//!
//! - `RecordTokenUsage` — record tokens from an LLM call.
//! - `GetTokenUsage` — query usage summary for a chat_id (or global).
//! - `GetGlobalTokenUsage` — total across all chat_ids.

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
            chat_id     INTEGER NOT NULL,
            model       TEXT NOT NULL,
            prompt_tokens   INTEGER NOT NULL,
            completion_tokens INTEGER NOT NULL,
            total_tokens    INTEGER NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_token_usage_chat_id ON token_usage(chat_id);
        CREATE INDEX IF NOT EXISTS idx_token_usage_model ON token_usage(model);"
    ).map_err(|e| format!("create table: {}", e))?;
    Ok(conn)
}

// ── Messages ─────────────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "()")]
pub struct RecordTokenUsage {
    pub chat_id: i64,
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Message)]
#[rtype(result = "TokenUsageSummary")]
pub struct GetTokenUsage {
    pub chat_id: i64,
}

#[derive(Message)]
#[rtype(result = "TokenUsageSummary")]
pub struct GetGlobalTokenUsage;

// ── Responses ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize)]
pub struct ModelUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub call_count: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct TokenUsageSummary {
    pub chat_id: Option<i64>,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
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
            "INSERT INTO token_usage (chat_id, model, prompt_tokens, completion_tokens, total_tokens) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![msg.chat_id, msg.model, msg.prompt_tokens, msg.completion_tokens, total],
        ) {
            log::error!("[TokenUsageActor] insert failed: {}", e);
        }
    }
}

impl Handler<GetTokenUsage> for TokenUsageActor {
    type Result = MessageResult<GetTokenUsage>;

    fn handle(&mut self, msg: GetTokenUsage, _ctx: &mut Self::Context) -> Self::Result {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => {
                log::error!("[TokenUsageActor] lock failed: {}", e);
                return MessageResult(TokenUsageSummary {
                    chat_id: Some(msg.chat_id),
                    total_prompt_tokens: 0,
                    total_completion_tokens: 0,
                    total_tokens: 0,
                    total_calls: 0,
                    by_model: std::collections::HashMap::new(),
                });
            }
        };
        MessageResult(build_summary(&db, Some(msg.chat_id)))
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
                    chat_id: None,
                    total_prompt_tokens: 0,
                    total_completion_tokens: 0,
                    total_tokens: 0,
                    total_calls: 0,
                    by_model: std::collections::HashMap::new(),
                });
            }
        };
        MessageResult(build_summary(&db, None))
    }
}

fn build_summary(db: &Connection, chat_id: Option<i64>) -> TokenUsageSummary {
    let cid_storage: i64;
    let (where_clause, params): (&str, Vec<&dyn rusqlite::types::ToSql>) = match chat_id {
        Some(cid) => {
            cid_storage = cid;
            ("WHERE chat_id = ?1", vec![&cid_storage as &dyn rusqlite::types::ToSql])
        }
        None => ("", vec![]),
    };

    let totals: (u64, u64, u64, u64) = db
        .query_row(
            &format!(
                "SELECT COALESCE(SUM(prompt_tokens),0), COALESCE(SUM(completion_tokens),0),
                        COALESCE(SUM(total_tokens),0), COUNT(*) FROM token_usage {where_clause}"
            ),
            rusqlite::params_from_iter(params.iter().copied()),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap_or((0, 0, 0, 0));

    let mut stmt = db
        .prepare(&format!(
            "SELECT model, SUM(prompt_tokens), SUM(completion_tokens), SUM(total_tokens), COUNT(*)
             FROM token_usage {where_clause} GROUP BY model"
        ))
        .unwrap();

    let mut by_model = std::collections::HashMap::new();
    if let Ok(mut rows) = stmt.query(rusqlite::params_from_iter(params.iter().copied())) {
        while let Ok(Some(row)) = rows.next() {
            let model: String = row.get(0).unwrap_or_default();
            by_model.insert(model, ModelUsage {
                prompt_tokens: row.get(1).unwrap_or(0),
                completion_tokens: row.get(2).unwrap_or(0),
                total_tokens: row.get(3).unwrap_or(0),
                call_count: row.get(4).unwrap_or(0),
            });
        }
    }

    TokenUsageSummary {
        chat_id,
        total_prompt_tokens: totals.0,
        total_completion_tokens: totals.1,
        total_tokens: totals.2,
        total_calls: totals.3,
        by_model,
    }
}
