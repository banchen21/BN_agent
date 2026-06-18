//! memory-plugin — extract long-term memories from conversations.
//!
//! Listens to `user.message` / `assistant.message`, buffers dialogue,
//! and periodically asks the LLM to extract key facts. Facts are stored
//! in a local SQLite database and injected into every LLM request via
//! the `snapshot()` mechanism with the `[记忆]` prefix.
//!
//! ## Environment variables
//!
//! | Variable               | Default            | Description                      |
//! |------------------------|--------------------|----------------------------------|
//! | `MEMORY_EXTRACT_EVERY` | `10`               | Messages between extractions     |
//! | `MEMORY_MAX_FACTS`     | `20`               | Max facts returned in snapshot   |
//! | `MEMORY_DB_PATH`       | `data/memories.db` | SQLite database path             |
//!
//! ## How it works
//!
//! 1. Buffer user + assistant messages in-memory
//! 2. Every N messages → ask LLM "extract key facts about the user"
//! 3. LLM returns JSON → store in SQLite (UPSERT by fact text)
//! 4. `snapshot()` returns `[记忆] fact …` lines → injected near system prompt
//! 5. Old facts rotated out when exceeding MEMORY_MAX_FACTS

use plugin_interface::*;
use serde::Deserialize;
use rusqlite::{params, Connection};
use std::sync::{Arc, Mutex};

// ── Constants ────────────────────────────────────────────────────────────────

const DEFAULT_EXTRACT_EVERY: usize = 10;
const DEFAULT_MAX_FACTS: usize = 20;

// ── LLM extraction output ────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct ExtractedFact {
    fact: String,
    #[serde(default)]
    category: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ExtractionResult {
    facts: Vec<ExtractedFact>,
}

// ── Shared state ─────────────────────────────────────────────────────────────

struct SharedState {
    /// Buffered messages: Vec<(role, text)>
    buffer: Vec<(String, String)>,
    /// Total messages buffered since last extraction.
    msg_count: usize,
    /// Extraction threshold (env MEMORY_EXTRACT_EVERY).
    extract_every: usize,
    /// Max facts returned in snapshot (env MEMORY_MAX_FACTS).
    max_facts: usize,
    /// SQLite connection.
    db: Option<Connection>,
    /// LLM backend.
    llm: Option<Recipient<LlmRequest>>,
    /// EventBus — to fire internal events for LLM calls.
    event_bus: Option<Addr<EventBus>>,
    /// Plugin logger.
    logger: Option<PluginLogger>,
    /// Whether an extraction is in flight.
    extracting: bool,
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
                description: "Extracts long-term memories from conversations".into(),
                author: "bn-agent".into(),
                min_host_version: "0.1.0".into(),
            },
            state: Arc::new(Mutex::new(SharedState {
                buffer: Vec::new(),
                msg_count: 0,
                extract_every: DEFAULT_EXTRACT_EVERY,
                max_facts: DEFAULT_MAX_FACTS,
                db: None,
                llm: None,
                event_bus: None,
                logger: None,
                extracting: false,
            })),
        }
    }

    /// Build the LLM prompt for extracting facts from a conversation batch.
    fn build_extraction_prompt(conversation: &str) -> String {
        format!(
            "你是一个记忆助手。分析以下对话，提取关于用户的关键事实。\n\
             \n\
             提取规则：\n\
             - 只提取关于用户的信息（身份、偏好、习惯、经历、关系等）\n\
             - 每条事实用一句话简洁表达\n\
             - 如果对话中没有新的事实，返回空数组\n\
             - category 用简短的类别标签（如：个人信息、偏好、经历、关系）\n\
             \n\
             对话：\n\
             {}\n\
             \n\
             以 JSON 格式输出，只输出 JSON 不要解释：\n\
             {{\n\
               \"facts\": [\n\
                 {{\"fact\": \"用户叫小明\", \"category\": \"个人信息\"}},\n\
                 {{\"fact\": \"小明喜欢打游戏\", \"category\": \"偏好\"}}\n\
               ]\n\
             }}",
            conversation,
        )
    }

    /// Store extracted facts in SQLite (UPSERT by fact text).
    fn store_facts(db: &Connection, facts: &[ExtractedFact], max_facts: usize, logger: &Option<PluginLogger>) {
        for f in facts {
            let result = db.execute(
                "INSERT INTO memories (fact, category) VALUES (?1, ?2)
                 ON CONFLICT(fact) DO UPDATE SET created_at = CURRENT_TIMESTAMP",
                params![f.fact, f.category],
            );
            match result {
                Ok(_) => {
                    if let Some(ref l) = logger {
                        l.info(format!("memory stored: [{}] {}", f.category, f.fact));
                    }
                }
                Err(e) => {
                    if let Some(ref l) = logger {
                        l.error(format!("failed to store memory: {}", e));
                    }
                }
            }
        }

        // Prune old facts if over limit.
        if let Err(e) = db.execute(
            "DELETE FROM memories WHERE id NOT IN (
                SELECT id FROM memories ORDER BY created_at DESC LIMIT ?1
            )",
            params![max_facts as i64],
        ) {
            if let Some(ref l) = logger {
                l.error(format!("failed to prune old memories: {}", e));
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
            .ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_EXTRACT_EVERY);
        let max_facts: usize = std::env::var("MEMORY_MAX_FACTS")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_MAX_FACTS);
        let db_path = std::env::var("MEMORY_DB_PATH")
            .unwrap_or_else(|_| "data/memories.db".into());

        // Ensure the data directory exists.
        if let Some(parent) = std::path::Path::new(&db_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                fact TEXT NOT NULL UNIQUE,
                category TEXT NOT NULL DEFAULT '',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_memories_category ON memories(category);
            CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);",
        )?;

        let logger = ctx.logger.clone();
        logger.info(format!(
            "started (extract_every={}, max_facts={}, db={})",
            extract_every, max_facts, db_path,
        ));

        {
            let mut s = self.state.lock().unwrap();
            s.extract_every = extract_every;
            s.max_facts = max_facts;
            s.db = Some(conn);
            s.llm = ctx.llm.clone();
            s.event_bus = Some(ctx.event_bus.clone());
            s.logger = Some(logger);
        }

        Ok(())
    }

    fn stop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            s.llm = None;
            s.event_bus = None;
            s.logger = None;
            s.db = None;
        }
        log::info!("[memory-plugin] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        match event.topic.as_str() {
            // ── Buffer dialogue ──────────────────────────────────────────
            "user.message" | "assistant.message" => {
                let text = event
                    .data
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if text.is_empty() {
                    return true;
                }
                let role = if event.topic == "user.message" { "user" } else { "assistant" };

                let (should_extract, conversation, llm) = {
                    let mut s = match self.state.lock() {
                        Ok(s) => s,
                        Err(_) => return true,
                    };

                    if s.extracting {
                        return true;
                    }

                    s.buffer.push((role.to_string(), text));
                    s.msg_count += 1;

                    if s.msg_count >= s.extract_every {
                        s.extracting = true;
                        let conversation = s.buffer.iter()
                            .map(|(r, t)| format!("{}: {}", r, t))
                            .collect::<Vec<_>>()
                            .join("\n");
                        (
                            true,
                            conversation,
                            s.llm.clone(),
                        )
                    } else {
                        (false, String::new(), None)
                    }
                };

                if should_extract {
                    if let Some(llm) = llm {
                        let state = Arc::clone(&self.state);
                        let prompt = Self::build_extraction_prompt(&conversation);
                        let request = LlmRequest {
                            messages: vec![ChatMessage::user(prompt)],
                            model: None,
                            temperature: Some(0.3),   // low temp for extraction
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
                                        s.extracting = false;
                                    }
                                    return;
                                }
                                Err(e) => {
                                    if let Ok(mut s) = state.lock() {
                                        if let Some(ref l) = s.logger {
                                            l.error(format!("LLM mailbox error: {}", e));
                                        }
                                        s.extracting = false;
                                    }
                                    return;
                                }
                            };

                            // Parse JSON from LLM response.
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
                                    // Lock state to access db for storage, then clear buffer.
                                    let result = {
                                        let mut s = state.lock().unwrap();
                                        let db = s.db.as_ref();
                                        let max_facts = s.max_facts;
                                        let logger = s.logger.clone();
                                        if let Some(db) = db {
                                            Self::store_facts(db, &extracted.facts, max_facts, &logger);
                                        }
                                        s.buffer.clear();
                                        s.msg_count = 0;
                                        s.extracting = false;
                                        let fact_count = extracted.facts.len();
                                        (fact_count, logger)
                                    };
                                    if let Some(ref l) = result.1 {
                                        l.info(format!(
                                            "extracted {} facts, buffer cleared",
                                            result.0
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
                                        s.extracting = false;
                                    }
                                }
                            }
                            });
                        });
                    }
                }
                true
            }
            _ => true,
        }
    }

    fn snapshot(&self) -> Option<String> {
        let s = self.state.lock().ok()?;
        let db = s.db.as_ref()?;
        let limit = s.max_facts as i64;

        let mut stmt = db
            .prepare("SELECT fact FROM memories ORDER BY created_at DESC LIMIT ?1")
            .ok()?;

        let facts: Vec<String> = stmt
            .query_map(params![limit], |row| row.get::<_, String>(0))
            .ok()?
            .filter_map(|r| r.ok())
            .collect();

        if facts.is_empty() {
            return None;
        }

        Some(
            facts
                .iter()
                .map(|f| format!("[记忆] {}", f))
                .collect::<Vec<_>>()
                .join("\n"),
        )
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
