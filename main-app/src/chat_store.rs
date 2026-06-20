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
use plugin_interface::{
    AppendChatRecord, ChatHistoryRecord, ChatStoreMsg, ChatStoreResponse, FetchChatHistory,
};
use rusqlite::{params, Connection, Result as SqlResult};

// ── Records ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Record {
    pub role: String,
    pub content: String,
    pub peer_id: Option<String>,
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
    pub peer_id: String,
}

/// Append a single message.
#[derive(Message)]
#[rtype(result = "()")]
pub struct AppendRecord {
    pub role: String,
    pub content: String,
    pub peer_id: String,
}

/// Append a user + assistant pair atomically.
#[derive(Message)]
#[rtype(result = "()")]
pub struct AppendPair {
    pub user_msg: String,
    pub assistant_msg: String,
    pub peer_id: String,
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
    /// Peer identity, format: "{source}:{platform_unique_id}".
    pub peer_id: String,
}

/// Ensure owner peer is bound. First valid peer becomes owner permanently.
#[derive(Message)]
#[rtype(result = "Option<String>")]
pub struct EnsureOwnerPeer {
    pub peer_id: String,
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
                peer_id TEXT DEFAULT '',
                message_json TEXT DEFAULT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_created ON chat_history(created_at);
            CREATE INDEX IF NOT EXISTS idx_peer_id_id ON chat_history(peer_id, id);
            CREATE TABLE IF NOT EXISTS owner_binding (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                owner_peer_id TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        )?;
        // 平滑升级旧库：追加 message_json/peer_id 列（已存在则忽略）
        let _ = conn
            .execute_batch("ALTER TABLE chat_history ADD COLUMN message_json TEXT DEFAULT NULL");
        let _ = conn.execute_batch("ALTER TABLE chat_history ADD COLUMN peer_id TEXT DEFAULT ''");
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
        let sql_scoped = "SELECT role, content, peer_id, message_json FROM (
                SELECT id, role, content, peer_id, message_json FROM chat_history
                WHERE peer_id = ?2
                ORDER BY id DESC
                LIMIT ?1
            ) ORDER BY id ASC";
        let sql_global = "SELECT role, content, peer_id, message_json FROM (
                SELECT id, role, content, peer_id, message_json FROM chat_history
                ORDER BY id DESC
                LIMIT ?1
            ) ORDER BY id ASC";
        let sql = if msg.peer_id.is_empty() {
            sql_global
        } else {
            sql_scoped
        };

        let Ok(mut stmt) = self.conn.prepare(sql) else {
            return vec![];
        };

        if msg.peer_id.is_empty() {
            let rows = stmt.query_map(params![msg.limit as i64], |row| {
                Ok(Record {
                    role: row.get(0)?,
                    content: row.get(1)?,
                    peer_id: row.get(2)?,
                    message_json: row.get(3)?,
                })
            });
            match rows {
                Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
                Err(e) => {
                    log::error!("[ChatStoreActor] FetchRecent failed: {}", e);
                    vec![]
                }
            }
        } else {
            let rows = stmt.query_map(params![msg.limit as i64, msg.peer_id], |row| {
                Ok(Record {
                    role: row.get(0)?,
                    content: row.get(1)?,
                    peer_id: row.get(2)?,
                    message_json: row.get(3)?,
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
}

impl Handler<EnsureOwnerPeer> for ChatStoreActor {
    type Result = Option<String>;

    fn handle(&mut self, msg: EnsureOwnerPeer, _ctx: &mut Self::Context) -> Self::Result {
        if msg.peer_id.trim().is_empty() {
            return None;
        }

        let existing = self.conn.query_row(
            "SELECT owner_peer_id FROM owner_binding WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        );

        match existing {
            Ok(owner) => Some(owner),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                if let Err(e) = self.conn.execute(
                    "INSERT OR IGNORE INTO owner_binding (id, owner_peer_id) VALUES (1, ?1)",
                    params![msg.peer_id.clone()],
                ) {
                    log::error!("[ChatStoreActor] EnsureOwnerPeer insert failed: {}", e);
                    return None;
                }
                // 首次绑定主人后，把旧的无 peer 历史归档给主人。
                if let Err(e) = self.conn.execute(
                    "UPDATE chat_history SET peer_id = ?1 WHERE peer_id IS NULL OR peer_id = ''",
                    params![msg.peer_id],
                ) {
                    log::error!("[ChatStoreActor] EnsureOwnerPeer backfill failed: {}", e);
                }
                self.conn
                    .query_row(
                        "SELECT owner_peer_id FROM owner_binding WHERE id = 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .ok()
            }
            Err(e) => {
                log::error!("[ChatStoreActor] EnsureOwnerPeer query failed: {}", e);
                None
            }
        }
    }
}

impl Handler<AppendRecord> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendRecord, _ctx: &mut Self::Context) {
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, peer_id) VALUES (?1, ?2, ?3)",
            params![msg.role, msg.content, msg.peer_id],
        ) {
            log::error!("[ChatStoreActor] AppendRecord failed: {}", e);
        }
    }
}

impl Handler<AppendChatRecord> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendChatRecord, _ctx: &mut Self::Context) {
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, peer_id) VALUES (?1, ?2, ?3)",
            params![msg.role, msg.content, msg.peer_id.unwrap_or_default()],
        ) {
            log::error!("[ChatStoreActor] AppendChatRecord failed: {}", e);
        }
    }
}

impl Handler<AppendPair> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendPair, _ctx: &mut Self::Context) {
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, peer_id) VALUES ('user', ?1, ?2)",
            params![msg.user_msg, msg.peer_id],
        ) {
            log::error!("[ChatStoreActor] AppendPair user failed: {}", e);
        }
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, peer_id) VALUES ('assistant', ?1, ?2)",
            params![msg.assistant_msg, msg.peer_id],
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
                let r = val
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_string();
                let c = val
                    .get("content")
                    .and_then(|v| if v.is_null() { None } else { v.as_str() })
                    .unwrap_or("")
                    .to_string();
                (r, c)
            }
            Err(_) => {
                log::error!(
                    "[ChatStoreActor] AppendJsonMessage: invalid JSON, storing as 'user' fallback"
                );
                ("user".to_string(), msg.message_json.clone())
            }
        };

        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, peer_id, message_json) VALUES (?1, ?2, ?3, ?4)",
            params![role, content, msg.peer_id, msg.message_json],
        ) {
            log::error!("[ChatStoreActor] AppendJsonMessage failed: {}", e);
        }
    }
}

impl Handler<FetchChatHistory> for ChatStoreActor {
    type Result = Vec<ChatHistoryRecord>;

    fn handle(&mut self, msg: FetchChatHistory, _ctx: &mut Self::Context) -> Self::Result {
        let scoped = msg.peer_id.clone().unwrap_or_default();
        let sql_scoped = "SELECT role, content, peer_id FROM (
                SELECT id, role, content, peer_id FROM chat_history
                WHERE peer_id = ?2
                ORDER BY id DESC
                LIMIT ?1
            ) ORDER BY id ASC";
        let sql_global = "SELECT role, content, peer_id FROM (
                SELECT id, role, content, peer_id FROM chat_history
                ORDER BY id DESC
                LIMIT ?1
            ) ORDER BY id ASC";
        let sql = if scoped.is_empty() {
            sql_global
        } else {
            sql_scoped
        };

        let Ok(mut stmt) = self.conn.prepare(sql) else {
            return vec![];
        };

        if scoped.is_empty() {
            let rows = stmt.query_map(params![msg.limit as i64], |row| {
                Ok(ChatHistoryRecord {
                    role: row.get(0)?,
                    content: row.get(1)?,
                    peer_id: row.get(2)?,
                })
            });
            match rows {
                Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
                Err(e) => {
                    log::error!("[ChatStoreActor] FetchChatHistory failed: {}", e);
                    vec![]
                }
            }
        } else {
            let rows = stmt.query_map(params![msg.limit as i64, scoped], |row| {
                Ok(ChatHistoryRecord {
                    role: row.get(0)?,
                    content: row.get(1)?,
                    peer_id: row.get(2)?,
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
}

impl Handler<ChatStoreMsg> for ChatStoreActor {
    type Result = ChatStoreResponse;

    fn handle(&mut self, msg: ChatStoreMsg, _ctx: &mut Self::Context) -> Self::Result {
        match msg {
            ChatStoreMsg::Append {
                role,
                content,
                peer_id,
            } => {
                if let Err(e) = self.conn.execute(
                    "INSERT INTO chat_history (role, content, peer_id) VALUES (?1, ?2, ?3)",
                    params![role, content, peer_id.unwrap_or_default()],
                ) {
                    log::error!("[ChatStoreActor] ChatStoreMsg::Append failed: {}", e);
                }
                ChatStoreResponse::AppendOk
            }
            ChatStoreMsg::FetchRecent { limit, peer_id } => {
                let scoped = peer_id.unwrap_or_default();
                let sql_scoped = "SELECT role, content, peer_id FROM (
                        SELECT id, role, content, peer_id FROM chat_history
                        WHERE peer_id = ?2
                        ORDER BY id DESC
                        LIMIT ?1
                    ) ORDER BY id ASC";
                let sql_global = "SELECT role, content, peer_id FROM (
                        SELECT id, role, content, peer_id FROM chat_history
                        ORDER BY id DESC
                        LIMIT ?1
                    ) ORDER BY id ASC";
                let sql = if scoped.is_empty() {
                    sql_global
                } else {
                    sql_scoped
                };

                let Ok(mut stmt) = self.conn.prepare(sql) else {
                    return ChatStoreResponse::FetchRecent(vec![]);
                };

                let records = if scoped.is_empty() {
                    let rows = stmt.query_map(params![limit as i64], |row| {
                        Ok(ChatHistoryRecord {
                            role: row.get(0)?,
                            content: row.get(1)?,
                            peer_id: row.get(2)?,
                        })
                    });
                    match rows {
                        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
                        Err(e) => {
                            log::error!("[ChatStoreActor] ChatStoreMsg::FetchRecent failed: {}", e);
                            vec![]
                        }
                    }
                } else {
                    let rows = stmt.query_map(params![limit as i64, scoped], |row| {
                        Ok(ChatHistoryRecord {
                            role: row.get(0)?,
                            content: row.get(1)?,
                            peer_id: row.get(2)?,
                        })
                    });
                    match rows {
                        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
                        Err(e) => {
                            log::error!("[ChatStoreActor] ChatStoreMsg::FetchRecent failed: {}", e);
                            vec![]
                        }
                    }
                };
                ChatStoreResponse::FetchRecent(records)
            }
        }
    }
}
