//! SQLite 聊天记录持久化存储

use rusqlite::{params, Connection};
use std::path::PathBuf;

/// 聊天记录行
#[derive(Clone, Debug)]
pub struct ChatRecord {
    pub id: i64,
    pub chat_id: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// SQLite 聊天记录存储
pub struct ChatStore {
    conn: Connection,
}

impl ChatStore {
    /// 打开或创建数据库
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path)
            .map_err(|e| format!("打开数据库失败: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chat_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id     INTEGER NOT NULL,
                role        TEXT NOT NULL,
                content     TEXT NOT NULL,
                created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_chat_id ON chat_history(chat_id);
            CREATE INDEX IF NOT EXISTS idx_created_at ON chat_history(created_at);"
        ).map_err(|e| format!("建表失败: {}", e))?;

        Ok(Self { conn })
    }

    /// 追加一条消息
    pub fn append(&self, chat_id: i64, role: &str, content: &str) -> Result<(), String> {
        self.conn.execute(
            "INSERT INTO chat_history (chat_id, role, content) VALUES (?1, ?2, ?3)",
            params![chat_id, role, content],
        ).map_err(|e| format!("写入失败: {}", e))?;
        Ok(())
    }

    /// 获取指定会话的最近 N 条消息（按时间正序）
    pub fn recent(&self, chat_id: i64, limit: usize) -> Result<Vec<ChatRecord>, String> {
        let mut stmt = self.conn.prepare(
            "SELECT id, chat_id, role, content, created_at
             FROM chat_history
             WHERE chat_id = ?1
             ORDER BY id DESC
             LIMIT ?2"
        ).map_err(|e| format!("查询失败: {}", e))?;

        let rows = stmt.query_map(
            params![chat_id, limit as i64],
            |row| {
                Ok(ChatRecord {
                    id: row.get(0)?,
                    chat_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    created_at: row.get(4)?,
                })
            }
        ).map_err(|e| format!("读取失败: {}", e))?;

        let mut records: Vec<ChatRecord> = Vec::new();
        for r in rows {
            records.push(r.map_err(|e| format!("行解析失败: {}", e))?);
        }
        // 反转回正序
        records.reverse();
        Ok(records)
    }

    /// 清除指定会话
    pub fn clear(&self, chat_id: i64) -> Result<usize, String> {
        let n = self.conn.execute(
            "DELETE FROM chat_history WHERE chat_id = ?1",
            params![chat_id],
        ).map_err(|e| format!("清除失败: {}", e))?;
        Ok(n)
    }

    /// 获取数据库文件路径
    pub fn db_path() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join("chat_history.db")
    }
}
