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
//! | `ListChatSessions`  | `Vec<ChatSessionSummary>` | List per-peer sessions |
//! | `ClearAll`          | `usize`            | Delete all records             |

use actix::prelude::*;
use plugin_interface::{
    AppendChatRecord, ChatHistoryRecord, ChatStoreMsg, ChatStoreResponse, FetchChatHistory,
};
use rusqlite::{params, Connection, OptionalExtension, Result as SqlResult};

// ── Records ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Record {
    pub role: String,
    pub content: String,
    pub created_at: String,
    /// Full OpenAI message JSON (supports tool_calls, tool role, etc.).
    /// NULL for legacy records.
    pub message_json: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatSessionSummary {
    pub peer_id: String,
    pub title: String,
    pub summary: String,
    pub message_count: usize,
    pub first_message_at: String,
    pub last_message_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ChatLongSummary {
    pub summary: String,
    pub summarized_until_id: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct LongSummaryRecord {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct LongSummaryBatch {
    pub peer_id: String,
    pub existing_summary: String,
    pub summarized_until_id: i64,
    pub records: Vec<LongSummaryRecord>,
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

#[derive(Message)]
#[rtype(result = "Vec<ChatSessionSummary>")]
pub struct ListChatSessions {
    pub peer_id: Option<String>,
    pub limit: usize,
}

#[derive(Message)]
#[rtype(result = "Option<ChatSessionSummary>")]
pub struct RefreshChatSession {
    pub peer_id: String,
}

#[derive(Message)]
#[rtype(result = "Option<ChatLongSummary>")]
pub struct GetChatLongSummary {
    pub peer_id: String,
}

#[derive(Message)]
#[rtype(result = "Option<LongSummaryBatch>")]
pub struct FetchLongSummaryBatch {
    pub peer_id: String,
    pub keep_recent: usize,
    pub batch_size: usize,
}

#[derive(Message)]
#[rtype(result = "Option<ChatLongSummary>")]
pub struct UpdateLongSummary {
    pub peer_id: String,
    pub summary: String,
    pub summarized_until_id: i64,
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
            );",
        )?;
        // 平滑升级旧库：追加 message_json/peer_id 列（已存在则忽略）
        let _ = conn
            .execute_batch("ALTER TABLE chat_history ADD COLUMN message_json TEXT DEFAULT NULL");
        let _ = conn.execute_batch("ALTER TABLE chat_history ADD COLUMN peer_id TEXT DEFAULT ''");
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_created ON chat_history(created_at);
            CREATE INDEX IF NOT EXISTS idx_peer_id_id ON chat_history(peer_id, id);
            CREATE TABLE IF NOT EXISTS owner_binding (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                owner_peer_id TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS chat_sessions (
                peer_id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                summary TEXT NOT NULL DEFAULT '',
                long_summary TEXT NOT NULL DEFAULT '',
                summarized_until_id INTEGER NOT NULL DEFAULT 0,
                long_summary_updated_at DATETIME DEFAULT NULL,
                message_count INTEGER NOT NULL DEFAULT 0,
                first_message_at DATETIME DEFAULT NULL,
                last_message_at DATETIME DEFAULT NULL,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        )?;
        let _ = conn.execute_batch(
            "ALTER TABLE chat_sessions ADD COLUMN long_summary TEXT NOT NULL DEFAULT ''",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE chat_sessions ADD COLUMN summarized_until_id INTEGER NOT NULL DEFAULT 0",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE chat_sessions ADD COLUMN long_summary_updated_at DATETIME DEFAULT NULL",
        );
        let _ = rebuild_all_session_metadata(&conn);
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
        let sql_scoped = "SELECT role, content, created_at, message_json FROM (
            SELECT id, role, content, created_at, message_json FROM chat_history
                WHERE peer_id = ?2
                ORDER BY id DESC
                LIMIT ?1
            ) ORDER BY id ASC";
        let sql_global = "SELECT role, content, created_at, message_json FROM (
            SELECT id, role, content, created_at, message_json FROM chat_history
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
                    created_at: row.get(2)?,
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
                    created_at: row.get(2)?,
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
                    params![msg.peer_id.clone()],
                ) {
                    log::error!("[ChatStoreActor] EnsureOwnerPeer backfill failed: {}", e);
                }
                refresh_session_metadata_best_effort(&self.conn, &msg.peer_id);
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
        let peer_id = msg.peer_id;
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, peer_id) VALUES (?1, ?2, ?3)",
            params![msg.role, msg.content, &peer_id],
        ) {
            log::error!("[ChatStoreActor] AppendRecord failed: {}", e);
            return;
        }
        prune_and_refresh_peer(&self.conn, &peer_id);
    }
}

impl Handler<AppendChatRecord> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendChatRecord, _ctx: &mut Self::Context) {
        let peer_id = msg.peer_id.unwrap_or_default();
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, peer_id) VALUES (?1, ?2, ?3)",
            params![msg.role, msg.content, &peer_id],
        ) {
            log::error!("[ChatStoreActor] AppendChatRecord failed: {}", e);
            return;
        }
        prune_and_refresh_peer(&self.conn, &peer_id);
    }
}

impl Handler<AppendPair> for ChatStoreActor {
    type Result = ();

    fn handle(&mut self, msg: AppendPair, _ctx: &mut Self::Context) {
        let peer_id = msg.peer_id;
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, peer_id) VALUES ('user', ?1, ?2)",
            params![msg.user_msg, &peer_id],
        ) {
            log::error!("[ChatStoreActor] AppendPair user failed: {}", e);
            return;
        }
        if let Err(e) = self.conn.execute(
            "INSERT INTO chat_history (role, content, peer_id) VALUES ('assistant', ?1, ?2)",
            params![msg.assistant_msg, &peer_id],
        ) {
            log::error!("[ChatStoreActor] AppendPair assistant failed: {}", e);
            return;
        }
        prune_and_refresh_peer(&self.conn, &peer_id);
    }
}

impl Handler<ClearAll> for ChatStoreActor {
    type Result = usize;

    fn handle(&mut self, _msg: ClearAll, _ctx: &mut Self::Context) -> Self::Result {
        match self.conn.execute("DELETE FROM chat_history", []) {
            Ok(n) => {
                if let Err(e) = self.conn.execute("DELETE FROM chat_sessions", []) {
                    log::error!("[ChatStoreActor] ClearAll sessions failed: {}", e);
                }
                n
            }
            Err(e) => {
                log::error!("[ChatStoreActor] ClearAll failed: {}", e);
                0
            }
        }
    }
}

/// 每 peer 保留的历史上限（env `CHAT_HISTORY_MAX_PER_PEER`，默认 1000，0=不限）。
fn chat_history_max_per_peer() -> usize {
    std::env::var("CHAT_HISTORY_MAX_PER_PEER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000)
}

const CHAT_SESSION_TITLE_MAX_CHARS: usize = 60;
const CHAT_SESSION_SUMMARY_MAX_CHARS: usize = 600;
const CHAT_SESSION_SUMMARY_RECENT_LIMIT: usize = 12;
const CHAT_SESSION_LIST_LIMIT_DEFAULT: usize = 100;
const CHAT_SESSION_LIST_LIMIT_MAX: usize = 500;

fn compact_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    if max_chars <= 3 {
        return input.chars().take(max_chars).collect();
    }
    let mut out: String = input.chars().take(max_chars - 3).collect();
    out.push_str("...");
    out
}

fn derive_session_title(first_user: Option<&str>, fallback: Option<&str>, peer_id: &str) -> String {
    let raw = first_user.or(fallback).unwrap_or(peer_id);
    let compact = compact_whitespace(raw);
    let title = if compact.is_empty() {
        peer_id.to_string()
    } else {
        compact
    };
    truncate_chars(&title, CHAT_SESSION_TITLE_MAX_CHARS)
}

fn role_label(role: &str) -> &'static str {
    match role {
        "user" => "用户",
        "assistant" => "助手",
        "tool" => "工具",
        "system" => "系统",
        _ => "消息",
    }
}

fn build_session_summary(records: &[(String, String)], max_chars: usize) -> String {
    let lines: Vec<String> = records
        .iter()
        .filter_map(|(role, content)| {
            let compact = compact_whitespace(content);
            if compact.is_empty() {
                None
            } else {
                Some(format!("{}：{}", role_label(role), compact))
            }
        })
        .collect();
    truncate_chars(&lines.join("\n"), max_chars)
}

fn normalize_session_list_limit(limit: usize) -> usize {
    if limit == 0 {
        CHAT_SESSION_LIST_LIMIT_DEFAULT
    } else {
        limit.min(CHAT_SESSION_LIST_LIMIT_MAX)
    }
}

fn chat_session_from_row(row: &rusqlite::Row<'_>) -> SqlResult<ChatSessionSummary> {
    let message_count: i64 = row.get(3)?;
    Ok(ChatSessionSummary {
        peer_id: row.get(0)?,
        title: row.get(1)?,
        summary: row.get(2)?,
        message_count: message_count.max(0) as usize,
        first_message_at: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        last_message_at: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        updated_at: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
    })
}

fn fetch_session_summary(
    conn: &Connection,
    peer_id: &str,
) -> SqlResult<Option<ChatSessionSummary>> {
    conn.query_row(
        "SELECT peer_id, title, summary, message_count, first_message_at, last_message_at, updated_at
            FROM chat_sessions WHERE peer_id = ?1",
        params![peer_id],
        chat_session_from_row,
    )
    .optional()
}

fn chat_long_summary_from_row(row: &rusqlite::Row<'_>) -> SqlResult<ChatLongSummary> {
    Ok(ChatLongSummary {
        summary: row.get(0)?,
        summarized_until_id: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
        updated_at: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
    })
}

fn fetch_chat_long_summary(conn: &Connection, peer_id: &str) -> SqlResult<Option<ChatLongSummary>> {
    conn.query_row(
        "SELECT long_summary, summarized_until_id, long_summary_updated_at
            FROM chat_sessions WHERE peer_id = ?1",
        params![peer_id],
        chat_long_summary_from_row,
    )
    .optional()
}

fn long_summary_cutoff_id(
    conn: &Connection,
    peer_id: &str,
    keep_recent: usize,
) -> SqlResult<Option<i64>> {
    if keep_recent == 0 {
        return conn
            .query_row(
                "SELECT COALESCE(MAX(id), 0) + 1 FROM chat_history WHERE peer_id = ?1",
                params![peer_id],
                |row| row.get::<_, i64>(0),
            )
            .map(Some);
    }

    conn.query_row(
        "SELECT MIN(id) FROM (
            SELECT id FROM chat_history WHERE peer_id = ?1 ORDER BY id DESC LIMIT ?2
        )",
        params![peer_id, keep_recent as i64],
        |row| row.get::<_, Option<i64>>(0),
    )
}

fn fetch_long_summary_batch(
    conn: &Connection,
    peer_id: &str,
    keep_recent: usize,
    batch_size: usize,
) -> SqlResult<Option<LongSummaryBatch>> {
    let peer_id = peer_id.trim();
    if peer_id.is_empty() || batch_size == 0 {
        return Ok(None);
    }

    let _ = refresh_session_metadata(conn, peer_id)?;
    let summary = fetch_chat_long_summary(conn, peer_id)?.unwrap_or(ChatLongSummary {
        summary: String::new(),
        summarized_until_id: 0,
        updated_at: String::new(),
    });
    let Some(cutoff_id) = long_summary_cutoff_id(conn, peer_id, keep_recent)? else {
        return Ok(None);
    };

    let mut stmt = conn.prepare(
        "SELECT id, role, content, created_at FROM chat_history
            WHERE peer_id = ?1
              AND id > ?2
              AND id < ?3
              AND role IN ('user', 'assistant')
              AND TRIM(content) <> ''
            ORDER BY id ASC
            LIMIT ?4",
    )?;
    let rows = stmt.query_map(
        params![
            peer_id,
            summary.summarized_until_id,
            cutoff_id,
            batch_size as i64
        ],
        |row| {
            Ok(LongSummaryRecord {
                id: row.get(0)?,
                role: row.get(1)?,
                content: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )?;
    let records = rows.collect::<SqlResult<Vec<_>>>()?;
    if records.is_empty() {
        return Ok(None);
    }

    Ok(Some(LongSummaryBatch {
        peer_id: peer_id.to_string(),
        existing_summary: summary.summary,
        summarized_until_id: summary.summarized_until_id,
        records,
    }))
}

fn update_chat_long_summary(
    conn: &Connection,
    peer_id: &str,
    summary: &str,
    summarized_until_id: i64,
) -> SqlResult<Option<ChatLongSummary>> {
    let peer_id = peer_id.trim();
    if peer_id.is_empty() || summarized_until_id <= 0 {
        return Ok(None);
    }

    let _ = refresh_session_metadata(conn, peer_id)?;
    conn.execute(
        "UPDATE chat_sessions SET
            long_summary = ?2,
            summarized_until_id = ?3,
            long_summary_updated_at = CURRENT_TIMESTAMP
        WHERE peer_id = ?1 AND summarized_until_id <= ?3",
        params![peer_id, summary, summarized_until_id],
    )?;

    fetch_chat_long_summary(conn, peer_id)
}

fn fetch_recent_session_snippets(
    conn: &Connection,
    peer_id: &str,
    limit: usize,
) -> SqlResult<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT role, content FROM (
            SELECT id, role, content FROM chat_history
            WHERE peer_id = ?1 AND TRIM(content) <> ''
            ORDER BY id DESC
            LIMIT ?2
        ) ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![peer_id, limit as i64], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.collect()
}

fn refresh_session_metadata(
    conn: &Connection,
    peer_id: &str,
) -> SqlResult<Option<ChatSessionSummary>> {
    let peer_id = peer_id.trim();
    if peer_id.is_empty() {
        return Ok(None);
    }

    let message_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chat_history WHERE peer_id = ?1",
        params![peer_id],
        |row| row.get(0),
    )?;
    if message_count == 0 {
        conn.execute(
            "DELETE FROM chat_sessions WHERE peer_id = ?1",
            params![peer_id],
        )?;
        return Ok(None);
    }

    let (first_message_at, last_message_at): (Option<String>, Option<String>) = conn.query_row(
        "SELECT MIN(created_at), MAX(created_at) FROM chat_history WHERE peer_id = ?1",
        params![peer_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let first_user = conn
        .query_row(
            "SELECT content FROM chat_history
            WHERE peer_id = ?1 AND role = 'user' AND TRIM(content) <> ''
            ORDER BY id ASC LIMIT 1",
            params![peer_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let fallback_content = conn
        .query_row(
            "SELECT content FROM chat_history
            WHERE peer_id = ?1 AND TRIM(content) <> ''
            ORDER BY id ASC LIMIT 1",
            params![peer_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let title = derive_session_title(first_user.as_deref(), fallback_content.as_deref(), peer_id);
    let snippets = fetch_recent_session_snippets(conn, peer_id, CHAT_SESSION_SUMMARY_RECENT_LIMIT)?;
    let summary = build_session_summary(&snippets, CHAT_SESSION_SUMMARY_MAX_CHARS);

    conn.execute(
        "INSERT INTO chat_sessions (
            peer_id, title, summary, message_count, first_message_at, last_message_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
        ON CONFLICT(peer_id) DO UPDATE SET
            title = excluded.title,
            summary = excluded.summary,
            message_count = excluded.message_count,
            first_message_at = excluded.first_message_at,
            last_message_at = excluded.last_message_at,
            updated_at = CURRENT_TIMESTAMP",
        params![
            peer_id,
            &title,
            &summary,
            message_count,
            first_message_at.as_deref(),
            last_message_at.as_deref(),
        ],
    )?;

    fetch_session_summary(conn, peer_id)
}

fn refresh_session_metadata_best_effort(conn: &Connection, peer_id: &str) {
    if let Err(e) = refresh_session_metadata(conn, peer_id) {
        log::error!(
            "[ChatStoreActor] refresh session metadata failed for peer '{}': {}",
            peer_id,
            e
        );
    }
}

fn rebuild_all_session_metadata(conn: &Connection) -> SqlResult<usize> {
    let peer_ids = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT peer_id FROM chat_history
            WHERE peer_id IS NOT NULL AND TRIM(peer_id) <> ''",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<SqlResult<Vec<_>>>()?
    };

    let mut refreshed = 0;
    for peer_id in peer_ids {
        if refresh_session_metadata(conn, &peer_id)?.is_some() {
            refreshed += 1;
        }
    }
    Ok(refreshed)
}

fn list_session_summaries(
    conn: &Connection,
    peer_id: Option<String>,
    limit: usize,
) -> SqlResult<Vec<ChatSessionSummary>> {
    if let Some(peer_id) = peer_id.filter(|p| !p.trim().is_empty()) {
        return Ok(fetch_session_summary(conn, peer_id.trim())?
            .into_iter()
            .collect());
    }

    let limit = normalize_session_list_limit(limit);
    let mut stmt = conn.prepare(
        "SELECT peer_id, title, summary, message_count, first_message_at, last_message_at, updated_at
            FROM chat_sessions
            ORDER BY COALESCE(last_message_at, updated_at) DESC, peer_id ASC
            LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], chat_session_from_row)?;
    rows.collect()
}

/// 删除某 peer 超出保留上限的旧记录（仅保留最近 `keep` 条）。返回删除行数。keep=0 不删。
fn prune_peer_history(conn: &Connection, peer_id: &str, keep: usize) -> usize {
    if keep == 0 {
        return 0;
    }
    conn.execute(
        "DELETE FROM chat_history WHERE peer_id = ?1 AND id NOT IN (
            SELECT id FROM chat_history WHERE peer_id = ?1 ORDER BY id DESC LIMIT ?2
        )",
        params![peer_id, keep as i64],
    )
    .unwrap_or(0)
}

fn prune_and_refresh_peer(conn: &Connection, peer_id: &str) {
    let removed = prune_peer_history(conn, peer_id, chat_history_max_per_peer());
    if removed > 0 {
        log::debug!(
            "[ChatStoreActor] pruned {} old record(s) for peer '{}'",
            removed,
            peer_id
        );
    }
    refresh_session_metadata_best_effort(conn, peer_id);
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
            return;
        }
        prune_and_refresh_peer(&self.conn, &msg.peer_id);
    }
}

impl Handler<ListChatSessions> for ChatStoreActor {
    type Result = Vec<ChatSessionSummary>;

    fn handle(&mut self, msg: ListChatSessions, _ctx: &mut Self::Context) -> Self::Result {
        match list_session_summaries(&self.conn, msg.peer_id, msg.limit) {
            Ok(sessions) => sessions,
            Err(e) => {
                log::error!("[ChatStoreActor] ListChatSessions failed: {}", e);
                vec![]
            }
        }
    }
}

impl Handler<RefreshChatSession> for ChatStoreActor {
    type Result = Option<ChatSessionSummary>;

    fn handle(&mut self, msg: RefreshChatSession, _ctx: &mut Self::Context) -> Self::Result {
        match refresh_session_metadata(&self.conn, &msg.peer_id) {
            Ok(summary) => summary,
            Err(e) => {
                log::error!("[ChatStoreActor] RefreshChatSession failed: {}", e);
                None
            }
        }
    }
}

impl Handler<GetChatLongSummary> for ChatStoreActor {
    type Result = Option<ChatLongSummary>;

    fn handle(&mut self, msg: GetChatLongSummary, _ctx: &mut Self::Context) -> Self::Result {
        match fetch_chat_long_summary(&self.conn, msg.peer_id.trim()) {
            Ok(summary) => summary,
            Err(e) => {
                log::error!("[ChatStoreActor] GetChatLongSummary failed: {}", e);
                None
            }
        }
    }
}

impl Handler<FetchLongSummaryBatch> for ChatStoreActor {
    type Result = Option<LongSummaryBatch>;

    fn handle(&mut self, msg: FetchLongSummaryBatch, _ctx: &mut Self::Context) -> Self::Result {
        match fetch_long_summary_batch(&self.conn, &msg.peer_id, msg.keep_recent, msg.batch_size) {
            Ok(batch) => batch,
            Err(e) => {
                log::error!("[ChatStoreActor] FetchLongSummaryBatch failed: {}", e);
                None
            }
        }
    }
}

impl Handler<UpdateLongSummary> for ChatStoreActor {
    type Result = Option<ChatLongSummary>;

    fn handle(&mut self, msg: UpdateLongSummary, _ctx: &mut Self::Context) -> Self::Result {
        match update_chat_long_summary(
            &self.conn,
            &msg.peer_id,
            &msg.summary,
            msg.summarized_until_id,
        ) {
            Ok(summary) => summary,
            Err(e) => {
                log::error!("[ChatStoreActor] UpdateLongSummary failed: {}", e);
                None
            }
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
                let peer_id = peer_id.unwrap_or_default();
                if let Err(e) = self.conn.execute(
                    "INSERT INTO chat_history (role, content, peer_id) VALUES (?1, ?2, ?3)",
                    params![role, content, &peer_id],
                ) {
                    log::error!("[ChatStoreActor] ChatStoreMsg::Append failed: {}", e);
                } else {
                    prune_and_refresh_peer(&self.conn, &peer_id);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE chat_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                role TEXT NOT NULL DEFAULT 'user',
                content TEXT NOT NULL DEFAULT '',
                peer_id TEXT DEFAULT '',
                message_json TEXT DEFAULT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE chat_sessions (
                peer_id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                summary TEXT NOT NULL DEFAULT '',
                long_summary TEXT NOT NULL DEFAULT '',
                summarized_until_id INTEGER NOT NULL DEFAULT 0,
                long_summary_updated_at DATETIME DEFAULT NULL,
                message_count INTEGER NOT NULL DEFAULT 0,
                first_message_at DATETIME DEFAULT NULL,
                last_message_at DATETIME DEFAULT NULL,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .unwrap();
        conn
    }

    fn insert_n(conn: &Connection, peer: &str, n: usize) {
        for i in 0..n {
            conn.execute(
                "INSERT INTO chat_history (role, content, peer_id) VALUES ('user', ?1, ?2)",
                params![format!("msg-{}", i), peer],
            )
            .unwrap();
        }
    }

    fn insert_role(conn: &Connection, peer: &str, role: &str, content: &str) {
        conn.execute(
            "INSERT INTO chat_history (role, content, peer_id) VALUES (?1, ?2, ?3)",
            params![role, content, peer],
        )
        .unwrap();
    }

    fn count(conn: &Connection, peer: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM chat_history WHERE peer_id = ?1",
            params![peer],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn prune_keeps_when_under_limit() {
        let conn = mem_db();
        insert_n(&conn, "p1", 5);
        assert_eq!(prune_peer_history(&conn, "p1", 10), 0);
        assert_eq!(count(&conn, "p1"), 5);
    }

    #[test]
    fn prune_removes_excess_oldest() {
        let conn = mem_db();
        insert_n(&conn, "p1", 10);
        assert_eq!(prune_peer_history(&conn, "p1", 3), 7);
        assert_eq!(count(&conn, "p1"), 3);
    }

    #[test]
    fn prune_zero_keep_disables() {
        let conn = mem_db();
        insert_n(&conn, "p1", 5);
        assert_eq!(prune_peer_history(&conn, "p1", 0), 0);
        assert_eq!(count(&conn, "p1"), 5);
    }

    #[test]
    fn prune_only_affects_target_peer() {
        let conn = mem_db();
        insert_n(&conn, "p1", 10);
        insert_n(&conn, "p2", 4);
        prune_peer_history(&conn, "p1", 2);
        assert_eq!(count(&conn, "p1"), 2);
        assert_eq!(count(&conn, "p2"), 4);
    }

    #[test]
    fn prune_keeps_most_recent() {
        let conn = mem_db();
        insert_n(&conn, "p1", 5);
        prune_peer_history(&conn, "p1", 2);
        let mut stmt = conn
            .prepare("SELECT content FROM chat_history WHERE peer_id = 'p1' ORDER BY id ASC")
            .unwrap();
        let contents: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(contents, vec!["msg-3", "msg-4"]);
    }

    #[test]
    fn derive_session_title_prefers_first_user_message() {
        let title = derive_session_title(
            Some("  hello\nthere  "),
            Some("assistant fallback"),
            "telegram:1",
        );
        assert_eq!(title, "hello there");
    }

    #[test]
    fn refresh_session_metadata_builds_title_and_summary() {
        let conn = mem_db();
        insert_role(&conn, "telegram:1", "assistant", "先打个招呼");
        insert_role(&conn, "telegram:1", "user", "  我 想 记录 一件事  ");
        insert_role(&conn, "telegram:1", "assistant", "好的，继续说");

        let session = refresh_session_metadata(&conn, "telegram:1")
            .unwrap()
            .expect("session summary");

        assert_eq!(session.peer_id, "telegram:1");
        assert_eq!(session.title, "我 想 记录 一件事");
        assert_eq!(session.message_count, 3);
        assert!(session.summary.contains("助手：先打个招呼"));
        assert!(session.summary.contains("用户：我 想 记录 一件事"));
    }

    #[test]
    fn rebuild_all_session_metadata_indexes_existing_peers() {
        let conn = mem_db();
        insert_role(&conn, "telegram:1", "user", "第一段对话");
        insert_role(&conn, "wechat:2", "user", "第二段对话");

        assert_eq!(rebuild_all_session_metadata(&conn).unwrap(), 2);
        let sessions = list_session_summaries(&conn, None, 0).unwrap();

        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.peer_id == "telegram:1"));
        assert!(sessions.iter().any(|s| s.peer_id == "wechat:2"));
    }

    #[test]
    fn list_session_summaries_can_filter_exact_peer() {
        let conn = mem_db();
        insert_role(&conn, "telegram:1", "user", "第一段对话");
        insert_role(&conn, "wechat:2", "user", "第二段对话");
        rebuild_all_session_metadata(&conn).unwrap();

        let sessions = list_session_summaries(&conn, Some("wechat:2".into()), 100).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].peer_id, "wechat:2");
        assert_eq!(sessions[0].title, "第二段对话");
    }

    #[test]
    fn fetch_long_summary_batch_excludes_recent_window() {
        let conn = mem_db();
        insert_n(&conn, "telegram:1", 6);

        let batch = fetch_long_summary_batch(&conn, "telegram:1", 2, 10)
            .unwrap()
            .expect("summary batch");

        assert_eq!(batch.peer_id, "telegram:1");
        assert_eq!(batch.summarized_until_id, 0);
        let contents: Vec<_> = batch.records.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(contents, vec!["msg-0", "msg-1", "msg-2", "msg-3"]);
    }

    #[test]
    fn update_long_summary_advances_cursor() {
        let conn = mem_db();
        insert_n(&conn, "telegram:1", 6);
        let batch = fetch_long_summary_batch(&conn, "telegram:1", 2, 10)
            .unwrap()
            .expect("summary batch");
        let until = batch.records.last().unwrap().id;

        let summary = update_chat_long_summary(&conn, "telegram:1", "长期摘要", until)
            .unwrap()
            .expect("long summary");

        assert_eq!(summary.summary, "长期摘要");
        assert_eq!(summary.summarized_until_id, until);
        assert!(fetch_long_summary_batch(&conn, "telegram:1", 2, 10)
            .unwrap()
            .is_none());
    }
}
