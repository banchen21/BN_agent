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
use plugin_interface::AppendChatRecord;
use rusqlite::{params, Connection, Result as SqlResult};

// ── Records ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Record {
    pub role: String,
    pub content: String,
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
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_created ON chat_history(created_at);",
        )?;
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
            "SELECT role, content FROM (
                SELECT id, role, content FROM chat_history
                ORDER BY id DESC
                LIMIT ?1
            ) ORDER BY id ASC"
        ) else { return vec![]; };

        let rows = stmt.query_map(params![msg.limit as i64], |row| {
            Ok(Record {
                role: row.get(0)?,
                content: row.get(1)?,
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
