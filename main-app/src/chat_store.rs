//! ChatStoreActor — actix actor wrapping SQLite-backed chat history.
//!
//! Each row is keyed by `(chat_id, timestamp)` and stores role + content.
//!
//! ## Messages
//!
//! | Message             | Response           | Description                    |
//! |---------------------|--------------------|--------------------------------|
//! | `FetchRecent`       | `Vec<Record>`      | Load recent N records          |
//! | `AppendRecord`      | `()`               | Insert one message             |
//! | `AppendPair`        | `()`               | Insert user + assistant pair   |
//! | `ClearSession`      | `usize`            | Delete all records for chat_id |
//! | `ClearAll`          | `usize`            | Delete all records             |

use actix::prelude::*;
use rusqlite::{params, Connection, Result as SqlResult};

// ── Records ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Record {
    pub role: String,
    pub content: String,
    pub reasoning_content: Option<String>,
}

// ── Messages ─────────────────────────────────────────────────────────────────

/// Fetch the most recent N records for a chat (oldest first).
#[derive(Message)]
#[rtype(result = "Vec<Record>")]
pub struct FetchRecent {
    pub chat_id: i64,
    pub limit: usize,
}

/// Append a single message.
#[derive(Message)]
#[rtype(result = "()")]
pub struct AppendRecord {
    pub chat_id: i64,
    pub role: String,
    pub content: String,
}

/// Append a user + assistant pair atomically.
#[derive(Message)]
#[rtype(result = "()")]
pub struct AppendPair {
    pub chat_id: i64,
    pub user_msg: String,
    pub assistant_msg: String,
    pub reasoning_content: Option<String>,
}

/// Clear all records for a chat_id.
#[derive(Message)]
#[rtype(result = "usize")]
pub struct ClearSession(pub i64);

/// Clear all records in the database.
#[derive(Message)]
#[rtype(result = "usize")]
pub struct ClearAll;

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
                chat_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                reasoning_content TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_chat_id ON chat_history(chat_id);
            CREATE INDEX IF NOT EXISTS idx_created ON chat_history(created_at);",
        )?;
        // Add reasoning_content column if upgrading from old schema
        let _ = conn.execute_batch(
            "ALTER TABLE chat_history ADD COLUMN reasoning_content TEXT;",
        );
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
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT role, content, reasoning_content FROM chat_history
             WHERE chat_id = ?1
             ORDER BY id ASC
             LIMIT ?2"
        ) else { return vec![]; };

        let rows = stmt.query_map(params![msg.chat_id, msg.limit as i64], |row| {
            Ok(Record {
                role: row.get(0)?,
                content: row.get(1)?,
                reasoning_content: row.get(2).ok(),
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
            "INSERT INTO chat_history (chat_id, role, content) VALUES (?1, ?2, ?3)",
            params![msg.chat_id, msg.role, msg.content],
        ) {
            log::error!("[ChatStoreActor] AppendRecord failed: {}", e);
        }
    }
}

impl Handler<AppendPair> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendPair, _ctx: &mut Self::Context) {
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (chat_id, role, content) VALUES (?1, 'user', ?2)",
            params![msg.chat_id, msg.user_msg],
        ) {
            log::error!("[ChatStoreActor] AppendPair user failed: {}", e);
        }
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (chat_id, role, content, reasoning_content) VALUES (?1, 'assistant', ?2, ?3)",
            params![msg.chat_id, msg.assistant_msg, msg.reasoning_content],
        ) {
            log::error!("[ChatStoreActor] AppendPair assistant failed: {}", e);
        }
    }
}

impl Handler<ClearSession> for ChatStoreActor {
    type Result = usize;

    fn handle(&mut self, msg: ClearSession, _ctx: &mut Self::Context) -> Self::Result {
        match self.conn.execute(
            "DELETE FROM chat_history WHERE chat_id = ?1",
            params![msg.0],
        ) {
            Ok(n) => n,
            Err(e) => {
                log::error!("[ChatStoreActor] ClearSession failed: {}", e);
                0
            }
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
