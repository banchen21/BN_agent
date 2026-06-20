//! memory-plugin — Engram-style bi-temporal memory extraction.
//!
//! Listens to `user.message` / `assistant.message`, buffers dialogue,
//! and periodically asks the LLM to extract key facts. Facts are stored
//! in a local SQLite database with contradiction tracking:
//! old facts are never deleted — they're marked `is_active=0` with a
//! `superseded_by` pointer to the newer fact.
//!
//! ## Environment variables
//!
//! | Variable               | Default            | Description                      |
//! |------------------------|--------------------|----------------------------------|
//! | `MEMORY_EXTRACT_EVERY` | `10`               | Messages between extractions     |
//! | `MEMORY_DB_PATH`       | `data/memories.db` | SQLite database path             |
//!
//! ## How it works
//!
//! 1. Buffer user + assistant messages in-memory
//! 2. Every N messages → ask LLM "extract key facts; mark contradictions"
//! 3. LLM returns JSON with optional `supersedes` ids → mark old inactive
//! 4. `snapshot()` returns active facts + recently superseded ones (7-day window)

use chrono::Datelike;
use plugin_interface::*;
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ── Constants ────────────────────────────────────────────────────────────────

const DEFAULT_EXTRACT_EVERY: usize = 10;
/// Days a superseded fact remains visible in snapshot (Engram window).
const SUPERSEDED_VISIBLE_DAYS: i64 = 7;

// ── LLM extraction output ────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct ExtractedFact {
    fact: String,
    #[serde(default)]
    category: String,
    /// ID of an existing fact this one contradicts / replaces.
    #[serde(default)]
    supersedes: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExtractionResult {
    facts: Vec<ExtractedFact>,
}

// ── Shared state ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct PeerBuffer {
    /// Buffered messages: Vec<(role, text)>
    buffer: Vec<(String, String)>,
    /// Total messages buffered since last extraction.
    msg_count: usize,
    /// Whether an extraction is in flight.
    extracting: bool,
}

struct SharedState {
    /// Per-peer buffered messages and extraction state.
    peers: HashMap<String, PeerBuffer>,
    /// Extraction threshold (env MEMORY_EXTRACT_EVERY).
    extract_every: usize,
    /// SQLite connection.
    db: Option<Connection>,
    /// LLM backend.
    llm: Option<Recipient<LlmRequest>>,
    /// Plugin logger.
    logger: Option<PluginLogger>,
}

// ── Plugin struct ────────────────────────────────────────────────────────────

struct MemoryPlugin {
    info: PluginInfo,
    state: Arc<Mutex<SharedState>>,
}

impl MemoryPlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "memory-plugin".into(),
                version: "0.1.0".into(),
                description:
                    "Engram-style bi-temporal memory: marks contradictions instead of deleting"
                        .into(),
                author: "bn-agent".into(),
                min_host_version: "0.1.0".into(),
            },
            state: Arc::new(Mutex::new(SharedState {
                peers: HashMap::new(),
                extract_every: DEFAULT_EXTRACT_EVERY,
                db: None,
                llm: None,
                logger: None,
            })),
        }
    }

    fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                peer_id TEXT NOT NULL DEFAULT '',
                fact TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT '',
                is_active INTEGER DEFAULT 1,
                superseded_by INTEGER DEFAULT NULL,
                importance INTEGER DEFAULT 1,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(peer_id, fact)
            );",
        )?;

        let _ = conn.execute(
            "ALTER TABLE memories ADD COLUMN peer_id TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE memories ADD COLUMN is_active INTEGER DEFAULT 1",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE memories ADD COLUMN superseded_by INTEGER DEFAULT NULL",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE memories ADD COLUMN importance INTEGER DEFAULT 1",
            [],
        );

        let create_sql = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_default()
            .to_lowercase()
            .replace(' ', "");

        if !create_sql.contains("unique(peer_id,fact)") {
            conn.execute_batch(
                "CREATE TABLE memories_v2 (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    peer_id TEXT NOT NULL DEFAULT '',
                    fact TEXT NOT NULL,
                    category TEXT NOT NULL DEFAULT '',
                    is_active INTEGER DEFAULT 1,
                    superseded_by INTEGER DEFAULT NULL,
                    importance INTEGER DEFAULT 1,
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    UNIQUE(peer_id, fact)
                );
                INSERT OR IGNORE INTO memories_v2 (
                    id, peer_id, fact, category, is_active, superseded_by, importance, created_at
                )
                SELECT
                    id,
                    COALESCE(peer_id, ''),
                    fact,
                    category,
                    COALESCE(is_active, 1),
                    superseded_by,
                    COALESCE(importance, 1),
                    created_at
                FROM memories;
                DROP TABLE memories;
                ALTER TABLE memories_v2 RENAME TO memories;",
            )?;
        }

        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_memories_peer_active ON memories(peer_id, is_active);
             CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);",
        )?;
        Ok(())
    }

    fn owner_peer_id_from_chat_store() -> Option<String> {
        let path =
            std::env::var("CHAT_HISTORY_DB_PATH").unwrap_or_else(|_| "data/chat_history.db".into());
        if !std::path::Path::new(&path).exists() {
            return None;
        }
        let conn = Connection::open(path).ok()?;
        conn.query_row(
            "SELECT owner_peer_id FROM owner_binding WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .filter(|s| !s.trim().is_empty())
    }

    fn backfill_legacy_memories_to_owner(db: &Connection, logger: &Option<PluginLogger>) {
        let Some(owner_peer_id) = Self::owner_peer_id_from_chat_store() else {
            return;
        };
        match db.execute(
            "UPDATE memories SET peer_id = ?1 WHERE peer_id IS NULL OR peer_id = ''",
            params![owner_peer_id],
        ) {
            Ok(n) if n > 0 => {
                if let Some(ref l) = logger {
                    l.info(format!("backfilled {} legacy memories to owner peer", n));
                }
            }
            Ok(_) => {}
            Err(e) => {
                if let Some(ref l) = logger {
                    l.error(format!("failed to backfill legacy memories: {}", e));
                }
            }
        }
    }

    fn peer_id_from_event(data: &serde_json::Value) -> Option<String> {
        if let Some(peer_id) = data.get("peer_id").and_then(|v| v.as_str()) {
            let peer_id = peer_id.trim();
            if !peer_id.is_empty() {
                return Some(peer_id.to_string());
            }
        }

        let source = data
            .get("source")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("platform").and_then(|v| v.as_str()))
            .unwrap_or("")
            .trim();
        if source.is_empty() {
            return None;
        }

        let raw_id = data
            .get("chat_id")
            .and_then(|v| v.as_str().map(String::from))
            .or_else(|| {
                data.get("chat_id")
                    .and_then(|v| v.as_i64().map(|n| n.to_string()))
            })
            .or_else(|| {
                data.get("from_user_id")
                    .and_then(|v| v.as_str().map(String::from))
            })
            .or_else(|| {
                data.get("user_id")
                    .and_then(|v| v.as_str().map(String::from))
            });

        raw_id.and_then(|id| {
            let id = id.trim();
            if id.is_empty() {
                None
            } else {
                Some(format!("{}:{}", source, id))
            }
        })
    }

    /// Read active facts from DB and format them for the extraction prompt.
    fn existing_facts_context(db: &Connection, peer_id: &str) -> String {
        let mut stmt = match db.prepare(
            "SELECT id, fact, category FROM memories
             WHERE peer_id=?1 AND is_active=1
             ORDER BY importance DESC, created_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return String::new(),
        };
        let rows: Vec<String> = stmt
            .query_map(params![peer_id], |row| {
                let id: i64 = row.get(0)?;
                let fact: String = row.get(1)?;
                let cat: String = row.get(2)?;
                Ok(format!("[id:{}] {} ({})", id, fact, cat))
            })
            .ok()
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        if rows.is_empty() {
            String::new()
        } else {
            format!(
                "已有记忆（带 id，如新事实与某条矛盾请引用其 id）：\n{}\n",
                rows.join("\n")
            )
        }
    }

    /// Build the LLM prompt for extracting facts from a conversation batch.
    fn build_extraction_prompt(conversation: &str, existing: &str) -> String {
        let existing_section = if existing.is_empty() {
            "（尚无已有记忆）\n".to_string()
        } else {
            format!(
                "{}\n矛盾检测规则：\n\
                 - 如果新事实与某条已有记忆矛盾或更新了它（如年龄变了、名字改了），\n\
                   在 supersedes 字段填那条记忆的 id\n\
                 - 被取代的旧记忆不会被删除，只是标记为过时\n\
                 - 如果新事实是全新的，supersedes 留空\n\n",
                existing,
            )
        };

        format!(
            "你是一个记忆助手。分析以下对话，提取关于用户的关键事实。\n\
             \n\
             提取规则：\n\
             - 只提取关于用户的信息（身份、偏好、习惯、经历、关系等）\n\
             - 每条事实用一句话简洁表达\n\
             - 如果对话中没有新的事实，返回空数组\n\
             - category 用简短的类别标签（如：个人信息、偏好、经历、关系）\n\
             \n\
             {}\
             对话：\n\
             {}\n\
             \n\
             以 JSON 格式输出，只输出 JSON 不要解释：\n\
             {{\n\
               \"facts\": [\n\
                 {{\"fact\": \"用户叫小明\", \"category\": \"个人信息\", \"supersedes\": null}},\n\
                 {{\"fact\": \"小明喜欢打游戏\", \"category\": \"偏好\", \"supersedes\": null}},\n\
                 {{\"fact\": \"小明今年21岁\", \"category\": \"个人信息\", \"supersedes\": 2}}\n\
               ]\n\
             }}",
            existing_section, conversation,
        )
    }

    /// Store extracted facts with Engram-style contradiction tracking.
    fn store_facts(
        db: &Connection,
        peer_id: &str,
        facts: &[ExtractedFact],
        logger: &Option<PluginLogger>,
    ) {
        for f in facts {
            // UPSERT: new fact, or bump importance + refresh timestamp on re-extraction.
            let result = db.execute(
                "INSERT INTO memories (peer_id, fact, category, is_active, importance) VALUES (?1, ?2, ?3, 1, 1)
                 ON CONFLICT(peer_id, fact) DO UPDATE SET
                     category=excluded.category,
                     is_active=1,
                     importance=importance+1,
                     created_at=CURRENT_TIMESTAMP",
                params![peer_id, f.fact, f.category],
            );

            match result {
                Ok(_) => {
                    // Get the id of the just-inserted/updated fact.
                    let new_id: Option<i64> = db
                        .query_row(
                            "SELECT id FROM memories WHERE peer_id=?1 AND fact=?2",
                            params![peer_id, f.fact],
                            |row| row.get(0),
                        )
                        .ok();

                    // If this fact supersedes an old one, mark the old as inactive
                    // and link both directions.
                    if let (Some(old_id), Some(nid)) = (f.supersedes, new_id) {
                        let _ = db.execute(
                            "UPDATE memories SET is_active=0, superseded_by=?1
                             WHERE id=?2 AND peer_id=?3 AND is_active=1",
                            params![nid, old_id, peer_id],
                        );
                    }

                    let supersedes_note = if f.supersedes.is_some() {
                        " (supersedes old)"
                    } else {
                        ""
                    };
                    if let Some(ref l) = logger {
                        l.info(format!(
                            "memory stored{} for {}: [{}] {}",
                            supersedes_note, peer_id, f.category, f.fact
                        ));
                    }
                }
                Err(e) => {
                    if let Some(ref l) = logger {
                        l.error(format!("failed to store memory: {}", e));
                    }
                }
            }
        }

        // Clean up: deactivate superseded facts older than the visibility window.
        let cutoff = format!("-{} days", SUPERSEDED_VISIBLE_DAYS);
        if let Err(e) = db.execute(
            "UPDATE memories SET is_active=-1 WHERE is_active=0 AND created_at < datetime('now', ?1)",
            params![cutoff],
        ) {
            if let Some(ref l) = logger {
                l.error(format!("failed to archive old memories: {}", e));
            }
        }
    }
}

impl Plugin for MemoryPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let extract_every: usize = std::env::var("MEMORY_EXTRACT_EVERY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_EXTRACT_EVERY);
        let db_path = std::env::var("MEMORY_DB_PATH").unwrap_or_else(|_| "data/memories.db".into());

        // Ensure the data directory exists.
        if let Some(parent) = std::path::Path::new(&db_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let conn = Connection::open(&db_path)?;
        Self::init_schema(&conn)?;

        let logger = ctx.logger.clone();
        logger.info(format!(
            "started (extract_every={}, db={})",
            extract_every, db_path,
        ));

        {
            let mut s = self.state.lock().unwrap();
            s.extract_every = extract_every;
            s.db = Some(conn);
            s.llm = ctx.llm.clone();
            s.logger = Some(logger);
        }

        Ok(())
    }

    fn stop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            s.llm = None;
            s.logger = None;
            s.db = None;
        }
        log::info!("[memory-plugin] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        match event.topic.as_str() {
            "user.message" | "assistant.message" => {
                let peer_id = match Self::peer_id_from_event(&event.data) {
                    Some(id) => id,
                    None => return true,
                };
                let text = event
                    .data
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if text.is_empty() {
                    return true;
                }
                let role = if event.topic == "user.message" {
                    "user"
                } else {
                    "assistant"
                };

                let (should_extract, conversation, llm, existing_facts, logger) = {
                    let mut s = match self.state.lock() {
                        Ok(s) => s,
                        Err(_) => return true,
                    };

                    let extract_every = s.extract_every;
                    let (should_extract, conversation) = {
                        let peer = s.peers.entry(peer_id.clone()).or_default();
                        if peer.extracting {
                            return true;
                        }

                        peer.buffer.push((role.to_string(), text));
                        peer.msg_count += 1;

                        if peer.msg_count >= extract_every {
                            peer.extracting = true;
                            let conversation = peer
                                .buffer
                                .iter()
                                .map(|(r, t)| format!("{}: {}", r, t))
                                .collect::<Vec<_>>()
                                .join("\n");
                            (true, conversation)
                        } else {
                            (false, String::new())
                        }
                    };

                    if should_extract {
                        let logger = s.logger.clone();
                        if let Some(db) = s.db.as_ref() {
                            Self::backfill_legacy_memories_to_owner(db, &logger);
                        }
                        let existing =
                            s.db.as_ref()
                                .map(|db| Self::existing_facts_context(db, &peer_id))
                                .unwrap_or_default();
                        (true, conversation, s.llm.clone(), existing, logger)
                    } else {
                        (false, String::new(), None, String::new(), s.logger.clone())
                    }
                };

                if should_extract {
                    if let Some(llm) = llm {
                        let state = Arc::clone(&self.state);
                        let peer_id_for_task = peer_id.clone();
                        let prompt = Self::build_extraction_prompt(&conversation, &existing_facts);
                        let request = LlmRequest {
                            messages: vec![ChatMessage::user(prompt)],
                            model: None,
                            temperature: Some(0.3),
                            max_tokens: Some(1024),
                        };

                        std::thread::spawn(move || {
                            let rt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .expect("tokio runtime for memory extraction");
                            rt.block_on(async {
                                let result = llm.send(request).await;
                                let response_text = match result {
                                    Ok(Ok(resp)) => resp.content,
                                    Ok(Err(e)) => {
                                        if let Ok(mut s) = state.lock() {
                                            if let Some(ref l) = s.logger {
                                                l.error(format!("LLM extraction error: {}", e));
                                            }
                                            if let Some(peer) = s.peers.get_mut(&peer_id_for_task) {
                                                peer.extracting = false;
                                            }
                                        }
                                        return;
                                    }
                                    Err(e) => {
                                        if let Ok(mut s) = state.lock() {
                                            if let Some(ref l) = s.logger {
                                                l.error(format!("LLM mailbox error: {}", e));
                                            }
                                            if let Some(peer) = s.peers.get_mut(&peer_id_for_task) {
                                                peer.extracting = false;
                                            }
                                        }
                                        return;
                                    }
                                };

                                let content = response_text.trim();
                                let json_str = if let Some(start) = content.find('{') {
                                    if let Some(end) = content.rfind('}') {
                                        &content[start..=end]
                                    } else {
                                        content
                                    }
                                } else {
                                    content
                                };

                                match serde_json::from_str::<ExtractionResult>(json_str) {
                                    Ok(extracted) => {
                                        let result = {
                                            let mut s = state.lock().unwrap();
                                            let db = s.db.as_ref();
                                            let logger = s.logger.clone();
                                            if let Some(db) = db {
                                                Self::store_facts(
                                                    db,
                                                    &peer_id_for_task,
                                                    &extracted.facts,
                                                    &logger,
                                                );
                                            }
                                            if let Some(peer) = s.peers.get_mut(&peer_id_for_task) {
                                                peer.buffer.clear();
                                                peer.msg_count = 0;
                                                peer.extracting = false;
                                            }
                                            let fact_count = extracted.facts.len();
                                            (fact_count, logger)
                                        };
                                        if let Some(ref l) = result.1 {
                                            l.info(format!(
                                                "extracted {} facts for {}, buffer cleared",
                                                result.0, peer_id_for_task
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        if let Ok(mut s) = state.lock() {
                                            if let Some(ref l) = s.logger {
                                                l.error(format!(
                                                    "failed to parse extraction JSON: {} — raw: {}",
                                                    e, content
                                                ));
                                            }
                                            if let Some(peer) = s.peers.get_mut(&peer_id_for_task) {
                                                peer.extracting = false;
                                            }
                                        }
                                    }
                                }
                            });
                        });
                    } else {
                        if let Ok(mut s) = self.state.lock() {
                            if let Some(ref l) = logger {
                                l.error(format!(
                                    "LLM unavailable for memory extraction: {}",
                                    peer_id
                                ));
                            }
                            if let Some(peer) = s.peers.get_mut(&peer_id) {
                                peer.extracting = false;
                            }
                        }
                    }
                }
                true
            }
            _ => true,
        }
    }

    fn snapshot(&self) -> Option<String> {
        None
    }

    fn snapshot_for_peer(&self, peer_id: &str) -> Option<String> {
        let peer_id = peer_id.trim();
        if peer_id.is_empty() {
            return None;
        }

        let s = self.state.lock().ok()?;
        let db = s.db.as_ref()?;
        let logger = s.logger.clone();
        Self::backfill_legacy_memories_to_owner(db, &logger);

        let mut stmt = db
            .prepare(
                "SELECT fact, is_active, created_at FROM memories
                 WHERE peer_id=?1 AND is_active >= 0
                 ORDER BY is_active DESC, importance DESC, created_at DESC",
            )
            .ok()?;

        let mut labeled: Vec<(String, Vec<String>)> = Vec::new();

        let now = chrono::Local::now();
        let today = now.date_naive();
        let weekday = today.weekday().num_days_from_monday();
        let week_start = today - chrono::Duration::days(weekday as i64);
        let last_week_start = week_start - chrono::Duration::days(7);
        let month_start = today.with_day(1).unwrap();
        let last_month_start = if today.month() == 1 {
            chrono::NaiveDate::from_ymd_opt(today.year() - 1, 12, 1).unwrap()
        } else {
            chrono::NaiveDate::from_ymd_opt(today.year(), today.month() - 1, 1).unwrap()
        };

        let rows: Vec<(String, bool, chrono::NaiveDate)> = stmt
            .query_map(params![peer_id], |row| {
                let fact: String = row.get(0)?;
                let active: i64 = row.get(1)?;
                let created: String = row.get(2)?;
                let date = chrono::NaiveDateTime::parse_from_str(&created, "%Y-%m-%d %H:%M:%S")
                    .map(|dt| dt.date())
                    .unwrap_or(today);
                Ok((fact, active != 0, date))
            })
            .ok()?
            .filter_map(|r| r.ok())
            .collect();

        if rows.is_empty() {
            return None;
        }

        for (fact, is_active, date) in rows {
            let label: String = if date >= week_start {
                "本周".into()
            } else if date >= last_week_start {
                "上周".into()
            } else if date >= month_start {
                "本月".into()
            } else if date >= last_month_start {
                "上月".into()
            } else if date.year() == today.year() {
                format!("{}月", date.month())
            } else {
                "更早".into()
            };

            let prefix = if is_active {
                "[记忆]"
            } else {
                "[记忆][已过时]"
            };
            let line = format!("{} {}", prefix, fact);

            if let Some((_, facts)) = labeled.iter_mut().find(|(l, _)| l == &label) {
                facts.push(line);
            } else {
                labeled.push((label.clone(), vec![line]));
            }
        }

        if labeled.is_empty() {
            return None;
        }

        // Each bucket: top 3 by original order (already importance-sorted from DB).
        let lines: Vec<String> = labeled
            .into_iter()
            .flat_map(|(label, facts)| {
                let top: Vec<String> = facts.into_iter().take(3).collect();
                std::iter::once(format!("[记忆][{}]", label))
                    .chain(top.into_iter())
                    .chain(std::iter::once(String::new()))
            })
            .collect();

        Some(lines.join("\n").trim_end().to_string())
    }
}

// ── FFI exports ──────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(MemoryPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
