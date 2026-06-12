//! ChatStore — SQLite-backed chat history for multi-turn LLM conversations.
//!
//! Each row is keyed by `(chat_id, timestamp)` and stores role + content.
//! The store supports append, recent (last N turns), clear (per chat_id), and clear_all.

use rusqlite::{params, Connection, Result as SqlResult};

pub struct ChatStore {
    conn: Connection,
}

impl ChatStore {
    /// Open (or create) the SQLite database at `path`.
    pub fn open(path: &str) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chat_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_chat_id ON chat_history(chat_id);
            CREATE INDEX IF NOT EXISTS idx_created ON chat_history(created_at);",
        )?;
        Ok(Self { conn })
    }

    /// Append a message (role = "user" or "assistant").
    pub fn append(&self, chat_id: i64, role: &str, content: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO chat_history (chat_id, role, content) VALUES (?1, ?2, ?3)",
            params![chat_id, role, content],
        )?;
        Ok(())
    }

    /// Return the most recent `limit` records for a chat_id, oldest first.
    pub fn recent(&self, chat_id: i64, limit: usize) -> SqlResult<Vec<Record>> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content FROM chat_history
             WHERE chat_id = ?1
             ORDER BY id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![chat_id, limit as i64], |row| {
            Ok(Record {
                role: row.get(0)?,
                content: row.get(1)?,
            })
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    /// Delete all records for a chat_id.
    pub fn clear(&self, chat_id: i64) -> SqlResult<usize> {
        self.conn
            .execute("DELETE FROM chat_history WHERE chat_id = ?1", params![chat_id])
    }

    /// Delete all records.
    pub fn clear_all(&self) -> SqlResult<usize> {
        self.conn.execute("DELETE FROM chat_history", [])
    }

    /// Return the database file path.
    pub fn db_path() -> std::path::PathBuf {
        std::path::PathBuf::from("data/chat_history.db")
    }
}

#[derive(Debug)]
pub struct Record {
    pub role: String,
    pub content: String,
}
