//! ChatStoreActor — actix actor wrapping SQLite-backed chat history.
//!
//! Each row stores role + content with a timestamp.
//!
//! ## Messages
//!
//! | Message             | Response           | Description                    |
//! |---------------------|--------------------|--------------------------------|
//! | `FetchRecent`       | `Vec<Record>`      | Load recent N records          |
//! | `AppendRecord`      | `()`               | Insert one message             |
//! | `AppendPair`        | `()`               | Insert user + assistant pair   |
//! | `ClearAll`          | `usize`            | Delete all records             |

use actix::prelude::*;
use plugin_interface::{AppendChatRecord, ChatHistoryRecord, ChatStoreMsg, ChatStoreResponse, FetchChatHistory};
use rusqlite::{params, Connection, Result as SqlResult};

// ── Records ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Record {
    pub role: String,
    pub content: String,
    /// Full OpenAI message JSON (supports tool_calls, tool role, etc.).
    /// NULL for legacy records.
    pub message_json: Option<String>,
}

// ── Messages ─────────────────────────────────────────────────────────────────

/// Fetch the most recent N records (oldest first).
#[derive(Message)]
#[rtype(result = "Vec<Record>")]
pub struct FetchRecent {
    pub limit: usize,
}

/// Append a single message.
#[derive(Message)]
#[rtype(result = "()")]
pub struct AppendRecord {
    pub role: String,
    pub content: String,
}

/// Append a user + assistant pair atomically.
#[derive(Message)]
#[rtype(result = "()")]
pub struct AppendPair {
    pub user_msg: String,
    pub assistant_msg: String,
}

/// Clear all records in the database.
#[derive(Message)]
#[rtype(result = "usize")]
pub struct ClearAll;

/// Append a full OpenAI-format message JSON to the history.
/// Supports tool_calls, tool role, and standard user/assistant/system messages.
/// Also extracts role+content for backward compatibility with legacy queries.
#[derive(Message)]
#[rtype(result = "()")]
pub struct AppendJsonMessage {
    /// Serialized OpenAI message JSON string.
    pub message_json: String,
}

// ── Actor ────────────────────────────────────────────────────────────────────

pub struct ChatStoreActor {
    conn: Connection,
}

impl ChatStoreActor {
    /// Open or create the SQLite database.
    pub fn open(path: &str) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chat_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                message_json TEXT DEFAULT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_created ON chat_history(created_at);",
        )?;
        // 平滑升级旧库：追加 message_json 列（已存在则忽略）
        let _ = conn.execute_batch("ALTER TABLE chat_history ADD COLUMN message_json TEXT DEFAULT NULL");
        Ok(Self { conn })
    }

    /// Default database file path.
    pub fn db_path() -> std::path::PathBuf {
        std::path::PathBuf::from("data/chat_history.db")
    }
}

impl Actor for ChatStoreActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!("[ChatStoreActor] started");
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

impl Handler<FetchRecent> for ChatStoreActor {
    type Result = Vec<Record>;

    fn handle(&mut self, msg: FetchRecent, _ctx: &mut Self::Context) -> Self::Result {
        // 子查询先取最新 N 条，外层按 id ASC 恢复时间序
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT role, content, message_json FROM (
                SELECT id, role, content, message_json FROM chat_history
                ORDER BY id DESC
                LIMIT ?1
            ) ORDER BY id ASC"
        ) else { return vec![]; };

        let rows = stmt.query_map(params![msg.limit as i64], |row| {
            Ok(Record {
                role: row.get(0)?,
                content: row.get(1)?,
                message_json: row.get(2)?,
            })
        });

        match rows {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                log::error!("[ChatStoreActor] FetchRecent failed: {}", e);
                vec![]
            }
        }
    }
}

impl Handler<AppendRecord> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendRecord, _ctx: &mut Self::Context) {
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content) VALUES (?1, ?2)",
            params![msg.role, msg.content],
        ) {
            log::error!("[ChatStoreActor] AppendRecord failed: {}", e);
        }
    }
}

impl Handler<AppendChatRecord> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendChatRecord, _ctx: &mut Self::Context) {
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content) VALUES (?1, ?2)",
            params![msg.role, msg.content],
        ) {
            log::error!("[ChatStoreActor] AppendChatRecord failed: {}", e);
        }
    }
}

impl Handler<AppendPair> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendPair, _ctx: &mut Self::Context) {
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content) VALUES ('user', ?1)",
            params![msg.user_msg],
        ) {
            log::error!("[ChatStoreActor] AppendPair user failed: {}", e);
        }
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content) VALUES ('assistant', ?1)",
            params![msg.assistant_msg],
        ) {
            log::error!("[ChatStoreActor] AppendPair assistant failed: {}", e);
        }
    }
}

impl Handler<ClearAll> for ChatStoreActor {
    type Result = usize;

    fn handle(&mut self, _msg: ClearAll, _ctx: &mut Self::Context) -> Self::Result {
        match self.conn.execute("DELETE FROM chat_history", []) {
            Ok(n) => n,
            Err(e) => {
                log::error!("[ChatStoreActor] ClearAll failed: {}", e);
                0
            }
        }
    }
}

impl Handler<AppendJsonMessage> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendJsonMessage, _ctx: &mut Self::Context) {
        // Parse the JSON to extract role + content for backward compat columns.
        let (role, content) = match serde_json::from_str::<serde_json::Value>(&msg.message_json) {
            Ok(val) => {
                let r = val.get("role").and_then(|v| v.as_str()).unwrap_or("user").to_string();
                let c = val.get("content")
                    .and_then(|v| {
                        if v.is_null() { None }
                        else { v.as_str() }
                    })
                    .unwrap_or("")
                    .to_string();
                (r, c)
            }
            Err(_) => {
                log::error!("[ChatStoreActor] AppendJsonMessage: invalid JSON, storing as 'user' fallback");
                ("user".to_string(), msg.message_json.clone())
            }
        };

        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, message_json) VALUES (?1, ?2, ?3)",
            params![role, content, msg.message_json],
        ) {
            log::error!("[ChatStoreActor] AppendJsonMessage failed: {}", e);
        }
    }
}

impl Handler<FetchChatHistory> for ChatStoreActor {
    type Result = Vec<ChatHistoryRecord>;

    fn handle(&mut self, msg: FetchChatHistory, _ctx: &mut Self::Context) -> Self::Result {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT role, content FROM (
                SELECT id, role, content FROM chat_history
                ORDER BY id DESC
                LIMIT ?1
            ) ORDER BY id ASC"
        ) else { return vec![]; };

        let rows = stmt.query_map(params![msg.limit as i64], |row| {
            Ok(ChatHistoryRecord {
                role: row.get(0)?,
                content: row.get(1)?,
            })
        });

        match rows {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                log::error!("[ChatStoreActor] FetchChatHistory failed: {}", e);
                vec![]
            }
        }
    }
}

impl Handler<ChatStoreMsg> for ChatStoreActor {
    type Result = ChatStoreResponse;

    fn handle(&mut self, msg: ChatStoreMsg, _ctx: &mut Self::Context) -> Self::Result {
        match msg {
            ChatStoreMsg::Append { role, content } => {
                if let Err(e) = self.conn.execute(
                    "INSERT INTO chat_history (role, content) VALUES (?1, ?2)",
                    params![role, content],
                ) {
                    log::error!("[ChatStoreActor] ChatStoreMsg::Append failed: {}", e);
                }
                ChatStoreResponse::AppendOk
            }
            ChatStoreMsg::FetchRecent { limit } => {
                let Ok(mut stmt) = self.conn.prepare(
                    "SELECT role, content FROM (
                        SELECT id, role, content FROM chat_history
                        ORDER BY id DESC
                        LIMIT ?1
                    ) ORDER BY id ASC"
                ) else {
                    return ChatStoreResponse::FetchRecent(vec![]);
                };

                let rows = stmt.query_map(params![limit as i64], |row| {
                    Ok(ChatHistoryRecord {
                        role: row.get(0)?,
                        content: row.get(1)?,
                    })
                });

                let records = match rows {
                    Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
                    Err(e) => {
                        log::error!("[ChatStoreActor] ChatStoreMsg::FetchRecent failed: {}", e);
                        vec![]
                    }
                };
                ChatStoreResponse::FetchRecent(records)
            }
        }
    }
}
