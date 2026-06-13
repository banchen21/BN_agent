//! main-app — entry point for the actix-actor plugin system.
//!
//! ## Endpoints
//!
//! | Method | Path                         | Description                      |
//! |--------|------------------------------|----------------------------------|
//! | GET    | `/api/health`                | Health check                     |
//! | GET    | `/api/plugins`               | List loaded plugins              |
//! | POST   | `/api/plugins/load`          | Load a plugin from a path        |
//! | POST   | `/api/plugins/unload/{name}` | Unload a plugin by name          |
//! | POST   | `/api/plugins/reload/{name}` | Reload a plugin by name          |
//! | POST   | `/api/plugins/scan`          | Scan dir and auto-load plugins   |
//! | POST   | `/api/events`                | Publish an event to the bus      |
//! | POST   | `/api/llm/chat`              | Simple LLM chat                  |
//! | POST   | `/api/chat`                  | Full chat with tools + history   |
//! | GET    | `/api/tools`                 | List registered tools            |
//! | POST   | `/api/tools/call`            | Call a tool directly             |
//! | GET    | `/api/metrics`               | Prometheus-format metrics        |
//! | GET    | `/api/metrics/json`          | JSON-format metrics              |
//! | GET    | `/api/token-usage`           | Global token usage summary       |
//! | POST   | `/api/cancel`                | Cancel in-flight request         |
//! | GET    | `/api/retry/state`           | Circuit breaker state            |
//! | ANY    | `/api/plugin/{name}/{path:.*}` | Proxy to plugin API handler    |
//! | POST   | `/api/shutdown`              | Graceful shutdown                |

mod cancellation_actor;
mod chat_store;
mod claude_backend;
mod llm_actor;

mod metrics_actor;
mod pipeline;
mod plugin_manager;
mod rate_limit_actor;
mod retry_actor;
mod token_usage_actor;
mod plugin_tools;
mod message_router;

use actix::prelude::*;
use actix_web::{web, HttpRequest, HttpResponse, HttpServer, Responder};
use cancellation_actor::CancellationActor;
use chat_store::{ChatStoreActor, ClearAll};
use llm_actor::{LlmActor, LlmConfig};

use metrics_actor::MetricsActor;
use plugin_interface::*;
use plugin_manager::PluginManager;
use rate_limit_actor::RateLimitActor;
use retry_actor::RetryActor;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use token_usage_actor::TokenUsageActor;
use message_router::MessageRouter;

// ── App state ────────────────────────────────────────────────────────────────

struct AppState {
    plugin_manager: Addr<PluginManager>,
    event_bus: Addr<EventBus>,
    llm: Option<Recipient<LlmRequest>>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
    metrics_addr: Option<Addr<MetricsActor>>,
    token_usage_addr: Option<Addr<TokenUsageActor>>,
    retry_addr: Option<Addr<RetryActor>>,
    cancellation_addr: Option<Addr<CancellationActor>>,
    chat_store: Option<Recipient<AppendChatRecord>>,
}

// ── Health ───────────────────────────────────────────────────────────────────

async fn health() -> impl Responder {
    HttpResponse::Ok().json(serde_json::json!({ "status": "ok" }))
}

// ── Plugin handlers ──────────────────────────────────────────────────────────

async fn list_plugins(state: web::Data<AppState>) -> impl Responder {
    match state.plugin_manager.send(ListPlugins).await {
        Ok(plugins) => HttpResponse::Ok().json(plugins),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({
            "error": format!("{}", e)
        })),
    }
}

#[derive(Deserialize)]
struct LoadRequest { path: String }

async fn load_plugin(state: web::Data<AppState>, body: web::Json<LoadRequest>) -> impl Responder {
    match state.plugin_manager.send(LoadPlugin { path: body.path.clone() }).await {
        Ok(Ok(info)) => HttpResponse::Ok().json(info),
        Ok(Err(e)) => HttpResponse::BadRequest().json(serde_json::json!({ "error": e })),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({ "error": format!("{}", e) })),
    }
}

async fn unload_plugin(state: web::Data<AppState>, path: web::Path<String>) -> impl Responder {
    let name = path.into_inner();
    match state.plugin_manager.send(UnloadPlugin { name: name.clone() }).await {
        Ok(Ok(())) => HttpResponse::Ok().json(serde_json::json!({ "status": "unloaded", "name": name })),
        Ok(Err(e)) => HttpResponse::NotFound().json(serde_json::json!({ "error": e })),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({ "error": format!("{}", e) })),
    }
}

async fn reload_plugin(state: web::Data<AppState>, path: web::Path<String>) -> impl Responder {
    let name = path.into_inner();
    match state.plugin_manager.send(ReloadPlugin { name: name.clone() }).await {
        Ok(Ok(info)) => HttpResponse::Ok().json(info),
        Ok(Err(e)) => HttpResponse::NotFound().json(serde_json::json!({ "error": e })),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({ "error": format!("{}", e) })),
    }
}

#[derive(Deserialize)]
struct ScanRequest { plugin_dir: String }

async fn scan_plugins(state: web::Data<AppState>, body: web::Json<ScanRequest>) -> impl Responder {
    let ctx = PluginContext {
        event_bus: state.event_bus.clone(),
        plugin_name: "host".into(),
        llm: state.llm.clone(),
        tool_registry: Some(state.tool_registry.clone()),
        logger: PluginLogger::new(state.event_bus.clone(), "host".into()),
        chat_store: state.chat_store.clone(),
    };
    match state.plugin_manager.send(ScanAndLoad { plugin_dir: body.plugin_dir.clone(), host_context: ctx }).await {
        Ok(Ok(n)) => HttpResponse::Ok().json(serde_json::json!({ "loaded": n })),
        Ok(Err(e)) => HttpResponse::BadRequest().json(serde_json::json!({ "error": e })),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({ "error": format!("{}", e) })),
    }
}

// ── Event handler ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PublishRequest { topic: String, data: serde_json::Value }

async fn publish_event(state: web::Data<AppState>, body: web::Json<PublishRequest>) -> impl Responder {
    let event = Event::new(&body.topic, body.data.clone(), "http-api");
    match state.event_bus.send(event).await {
        Ok(()) => HttpResponse::Ok().json(serde_json::json!({ "status": "published", "topic": body.topic })),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({ "error": format!("{}", e) })),
    }
}

// ── Simple LLM ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LlmChatRequest { messages: Vec<ChatMessage>, model: Option<String>, temperature: Option<f32>, max_tokens: Option<u32> }

async fn llm_chat(state: web::Data<AppState>, body: web::Json<LlmChatRequest>) -> impl Responder {
    let llm = match &state.llm {
        Some(llm) => llm.clone(),
        None => return HttpResponse::ServiceUnavailable().json(serde_json::json!({ "error": "LLM not configured" })),
    };
    match llm.send(LlmRequest { messages: body.messages.clone(), model: body.model.clone(), temperature: body.temperature, max_tokens: body.max_tokens }).await {
        Ok(Ok(resp)) => HttpResponse::Ok().json(resp),
        Ok(Err(e)) => HttpResponse::BadGateway().json(serde_json::json!({ "error": e })),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({ "error": format!("{}", e) })),
    }
}

// ── Full Chat (with tools + history) ─────────────────────────────────────────

#[derive(Deserialize)]
struct ChatPayload { message: String }

async fn chat(state: web::Data<AppState>, body: web::Json<ChatPayload>) -> impl Responder {
    let retry_addr = match &state.retry_addr {
        Some(a) => a.clone(),
        None => return HttpResponse::ServiceUnavailable().json(serde_json::json!({ "error": "LLM not configured" })),
    };

    // Refresh snapshots.
    let _ = state.plugin_manager.send(RefreshSnapshots).await;
    let contexts: Vec<String> = state.snapshots.lock().unwrap().clone();

    let tools: Vec<serde_json::Value> = match state.tool_registry.lock() {
        Ok(reg) => reg.all_defs().iter()
            .filter(|d| !d.internal)
            .map(|d| serde_json::json!({
                "type": "function",
                "function": { "name": d.name, "description": d.description, "parameters": d.parameters }
            }))
            .collect(),
        Err(_) => vec![],
    };

    let request_id = uuid::Uuid::new_v4().to_string();

    match retry_addr.send(retry_actor::RetryChatRequest {
        request: ChatRequest {
            message: body.message.clone(),
            tools,
            skip_store: false,
            contexts,
            jailbreak_index: None,
            image_base64: None,
            video_base64: None,
            video_mime: None,
            file_base64: None,
            file_name: None,
            stream: true,
            request_id: request_id.clone(),
            source: String::new(),
            user_name: String::new(),
            max_tokens: None,
            original_user_msg: None,
            assistant_tool_calls: vec![],
            tool_results: vec![],
        },
        max_retries: 3,
    }).await {
        Ok(Ok(resp)) => {
            // Record token usage if available.
            if let Some(ref tu) = state.token_usage_addr {
                tu.do_send(token_usage_actor::RecordTokenUsage {
                    model: resp.model.clone(),
                    prompt_tokens: resp.prompt_tokens,
                    completion_tokens: resp.completion_tokens,
                    prompt_cache_hit_tokens: resp.prompt_cache_hit_tokens,
                    prompt_cache_miss_tokens: resp.prompt_cache_miss_tokens,
                });
            }
            HttpResponse::Ok().json(resp)
        }
        Ok(Err(e)) => HttpResponse::BadGateway().json(serde_json::json!({ "error": e })),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({ "error": format!("{}", e) })),
    }
}

// ── Tools ────────────────────────────────────────────────────────────────────

async fn list_tools(state: web::Data<AppState>) -> impl Responder {
    match state.tool_registry.lock() {
        Ok(reg) => HttpResponse::Ok().json(reg.all_defs()),
        Err(_) => HttpResponse::InternalServerError().json(serde_json::json!({ "error": "registry locked" })),
    }
}

#[derive(Deserialize)]
struct ToolCallPayload { tool_name: String, arguments: serde_json::Value }

async fn tool_call(state: web::Data<AppState>, body: web::Json<ToolCallPayload>) -> impl Responder {
    let result = match state.tool_registry.lock() {
        Ok(reg) => reg.execute(&body.tool_name, &body.arguments),
        Err(_) => None,
    };
    match result {
        Some(r) => HttpResponse::Ok().json(serde_json::json!({
            "success": r.success, "content": r.content, "error": r.error,
        })),
        None => HttpResponse::NotFound().json(serde_json::json!({ "error": format!("tool '{}' not found", body.tool_name) })),
    }
}

// ── Metrics ──────────────────────────────────────────────────────────────────

async fn get_metrics(state: web::Data<AppState>) -> impl Responder {
    let metrics = match &state.metrics_addr {
        Some(a) => a.send(metrics_actor::GetMetrics).await.unwrap_or_else(|_| "".into()),
        None => "Metrics not available".into(),
    };
    HttpResponse::Ok()
        .content_type("text/plain; version=0.0.4; charset=utf-8")
        .body(metrics)
}

async fn get_metrics_json(state: web::Data<AppState>) -> impl Responder {
    let json = match &state.metrics_addr {
        Some(a) => a.send(metrics_actor::GetMetricsJson).await.unwrap_or_default(),
        None => serde_json::json!({ "error": "Metrics not available" }),
    };
    HttpResponse::Ok().json(json)
}

// ── Token usage ──────────────────────────────────────────────────────────────

async fn get_global_token_usage(state: web::Data<AppState>) -> impl Responder {
    let summary = match &state.token_usage_addr {
        Some(a) => a.send(token_usage_actor::GetGlobalTokenUsage).await.unwrap_or_else(|_| {
            token_usage_actor::TokenUsageSummary {
                total_prompt_tokens: 0, total_completion_tokens: 0,
                total_prompt_cache_hit_tokens: 0, total_prompt_cache_miss_tokens: 0,
                total_tokens: 0, total_calls: 0, by_model: std::collections::HashMap::new(),
            }
        }),
        None => return HttpResponse::NotFound().json(serde_json::json!({ "error": "Token usage tracking not available" })),
    };
    HttpResponse::Ok().json(summary)
}

// ── Cancellation ─────────────────────────────────────────────────────────────

async fn cancel_handler(state: web::Data<AppState>) -> impl Responder {
    let cancelled = match &state.cancellation_addr {
        Some(a) => a.send(cancellation_actor::CancelCurrent).await.unwrap_or(false),
        None => false,
    };
    if cancelled {
        HttpResponse::Ok().json(serde_json::json!({ "status": "cancelled" }))
    } else {
        HttpResponse::Ok().json(serde_json::json!({ "status": "no_active_request" }))
    }
}

// ── Retry / Circuit Breaker state ────────────────────────────────────────────

async fn retry_state(state: web::Data<AppState>) -> impl Responder {
    let state_str = match &state.retry_addr {
        Some(a) => a.send(retry_actor::CircuitStateQuery).await.unwrap_or_else(|_| "query failed".into()),
        None => "Retry not available".into(),
    };
    HttpResponse::Ok().json(serde_json::json!({ "circuit_breaker": state_str }))
}

// ── Plugin API proxy ─────────────────────────────────────────────────────────

async fn plugin_proxy(
    state: web::Data<AppState>,
    req: HttpRequest,
    path: web::Path<String>,
) -> impl Responder {
    let full_path = path.into_inner();
    let (plugin_name, sub_path) = full_path.split_once('/')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .unwrap_or_else(|| (full_path, String::new()));

    let method = req.method().as_str().to_uppercase();

    match state.plugin_manager.send(ApiRequest {
        plugin: plugin_name,
        method,
        path: sub_path,
        body: None,
    }).await {
        Ok(Some((status, body))) => {
            HttpResponse::build(
                actix_web::http::StatusCode::from_u16(status).unwrap_or(actix_web::http::StatusCode::OK)
            ).body(body)
        }
        Ok(None) => HttpResponse::NotFound().json(serde_json::json!({ "error": "plugin not found or no API handler" })),
        Err(_) => HttpResponse::InternalServerError().json(serde_json::json!({ "error": "actor communication failed" })),
    }
}

async fn shutdown() -> impl Responder {
    log::info!("Shutdown requested via API");
    actix_rt::System::current().stop();
    HttpResponse::Ok().json(serde_json::json!({ "status": "shutting down" }))
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() -> std::io::Result<()> {
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    if let Err(e) = dotenvy::from_path(&env_path) {
        eprintln!("[main] .env load skipped: {}", e);
    }
    // ── Logging: stdout + rotating file ──
    let log_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file = log_dir.join("app.log");
    // 启动前清理旧日志
    // RUST_LOG=debug 时直接清掉，否则超过 10MB 才轮转（最多留 3 个备份）
    let is_debug = std::env::var("RUST_LOG")
        .map(|v| v.to_lowercase().contains("debug"))
        .unwrap_or(false);
    if is_debug {
        let _ = std::fs::remove_file(&log_file);
    } else {
        const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
        const LOG_MAX_BACKUPS: u32 = 3;
        if let Ok(meta) = std::fs::metadata(&log_file) {
            if meta.len() > LOG_MAX_BYTES {
                for i in (1..=LOG_MAX_BACKUPS).rev() {
                    let old = log_dir.join(format!("app.log.{}.old", i));
                    if i == LOG_MAX_BACKUPS {
                        let _ = std::fs::remove_file(&old);
                    } else {
                        let dst = log_dir.join(format!("app.log.{}.old", i + 1));
                        let _ = std::fs::rename(&old, &dst);
                    }
                }
                let _ = std::fs::rename(&log_file, log_dir.join("app.log.1.old"));
            }
        }
    }
    let log_path = log_file.to_string_lossy().to_string();
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{} {} {}] {}",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .level_for("actix_server", log::LevelFilter::Warn)
        .chain(std::io::stdout())
        .chain(fern::log_file(&log_file).expect("failed to open log file"))
        .apply()
        .expect("failed to initialize logging");
    log::info!("Logging to stdout + {}", log_path);
    log::info!("=== actix-actor plugin system starting ===");

    let sys = actix_rt::System::new();

    sys.block_on(async {
        // 1. EventBus.
        let event_bus = EventBus::new().start();
        log::info!("EventBus actor started");

        // 1b. MessageRouter（统一消息路由层）。
        let message_router_addr = MessageRouter::new(event_bus.clone()).start();
        event_bus.do_send(Subscribe {
            topic: "user.message".into(),
            recipient: message_router_addr.clone().recipient(),
        });
        event_bus.do_send(Subscribe {
            topic: "route.message".into(),
            recipient: message_router_addr.recipient(),
        });
        log::info!("MessageRouter actor started, subscribed to 'user.message' + 'route.message'");

        // 2. ChatStoreActor (SQLite history).
        let store_addr = {
            let db_path = ChatStoreActor::db_path();
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            match ChatStoreActor::open(db_path.to_str().unwrap_or("data/chat_history.db")) {
                Ok(actor) => {
                    let addr = actor.start();
                    log::info!("ChatStoreActor started");
                    // Clear history on startup (development convenience).
                    addr.do_send(ClearAll);
                    addr
                }
                Err(e) => {
                    panic!("ChatStoreActor failed to open: {}", e);
                }
            }
        };

        // ── 3. Shared ToolRegistry + PluginDirectory ──
        let tool_registry: Arc<Mutex<ToolRegistry>> = Arc::new(Mutex::new(ToolRegistry::new()));
        let plugin_dir = std::env::var("PLUGIN_DIR").unwrap_or_else(|_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../target/debug")
                .to_string_lossy()
                .to_string()
        });
        log::info!("Plugin dir: {}", plugin_dir);

        // ── 4. PluginManager ──
        // Defer start until after LLM backend is ready, so plugins can access ctx.llm.
        let mut pm = PluginManager::new(
            event_bus.clone(),
            None,  // llm_recipient — will be set below
            tool_registry.clone(),
            plugin_dir,
            Some(store_addr.clone().recipient()),
        );
        let snapshots = pm.snapshots_arc();

        // ── 5. LLM Backend — openai (default) or claude ──
        let use_claude = std::env::var("LLM_BACKEND").as_deref() == Ok("claude");
        let llm_recipient: Option<Recipient<LlmRequest>>;
        let retry_addr: Option<Addr<RetryActor>>;

        if use_claude {
            // Claude backend: actor created in the main process (avoids DLL TLS issue)
            let (available, claude_path) = claude_backend::probe_claude();
            if available {
                log::info!("LLM_BACKEND=claude — creating ClaudeBridgeActor in main process");
                let addr = claude_backend::ClaudeBridgeActor::new(claude_path).start();
                let chat_rec: Recipient<ChatRequest> = addr.clone().recipient();
                llm_recipient = Some(addr.recipient());
                let config = retry_actor::RetryConfig::from_env();
                let actor = RetryActor::new(chat_rec, config);
                let addr = actor.start();
                log::info!("RetryActor (claude backend) started");
                retry_addr = Some(addr);
            } else {
                log::warn!("[main] Claude CLI not available — no LLM for pipeline");
                let rec = LlmConfig::from_env().ok().map(|config| {
                    let addr = LlmActor::new(config, event_bus.clone(), store_addr.clone()).start();
                    log::info!("LlmActor started (fallback, for plugins)");
                    addr.recipient()
                });
                llm_recipient = rec;
                retry_addr = None;
            }
        } else {
            // OpenAI 兼容后端（默认）
            let (llm_rec, chat_rec) = match LlmConfig::from_env() {
                Ok(config) => {
                    let addr = LlmActor::new(config, event_bus.clone(), store_addr.clone()).start();
                    log::info!("LlmActor started");
                    let llm_rec: Recipient<LlmRequest> = addr.clone().recipient();
                    let chat_rec: Recipient<ChatRequest> = addr.clone().recipient();
                    (Some(llm_rec), Some(chat_rec))
                }
                Err(e) => {
                    log::warn!("LLM not configured — {}", e);
                    (None, None)
                }
            };

            llm_recipient = llm_rec;
            retry_addr = chat_rec.map(|chat| {
                let config = retry_actor::RetryConfig::from_env();
                let addr = RetryActor::new(chat, config).start();
                log::info!("RetryActor started");
                addr
            });
        }

        // ── Start PluginManager now that LLM is ready ──
        pm.set_llm_recipient(llm_recipient.clone());
        let plugin_manager = pm.start();
        log::info!("PluginManager actor started");

        // ── 6. TokenUsageActor ──
        let token_usage_addr = match TokenUsageActor::new() {
            Ok(actor) => {
                let addr = actor.start();
                log::info!("TokenUsageActor started");
                Some(addr)
            }
            Err(e) => {
                log::warn!("TokenUsageActor failed: {}", e);
                None
            }
        };

        // 5. RateLimitActor.
        let rate_limit_addr = {
            let config = rate_limit_actor::RateLimitConfig::from_env();
            let actor = RateLimitActor::new(config);
            let addr = actor.start();
            log::info!("RateLimitActor started");
            addr
        };

        // 6. MetricsActor.
        let metrics_addr = {
            let actor = MetricsActor::new();
            let addr = actor.start();
            log::info!("MetricsActor started");
            Some(addr)
        };

        // 7. CancellationActor.
        let cancellation_addr = {
            let actor = CancellationActor::new();
            let addr = actor.start();
            log::info!("CancellationActor started");
            Some(addr)
        };

        // ── 11. PipelineActor (if LLM is available). ──
        if let (Some(ref retry), Some(ref tu), Some(ref metrics)) =
            (&retry_addr, &token_usage_addr, &metrics_addr)
        {
            let pipeline = pipeline::PipelineActor::new(
                retry.clone(),
                plugin_manager.clone(),
                tool_registry.clone(),
                snapshots.clone(),
                event_bus.clone(),
                rate_limit_addr.clone(),
                tu.clone(),
                metrics.clone(),
                store_addr.clone(),
            );
            let pipeline_addr = pipeline.start();
            log::info!("PipelineActor started");

            event_bus.do_send(Subscribe {
                topic: "user.message".into(),
                recipient: pipeline_addr.recipient(),
            });
            log::info!("PipelineActor subscribed to 'user.message'");
        } else {
            log::warn!("PipelineActor not started — missing LLM or infrastructure actors");
        }

        // ── Register host-level tools (plugin management for LLM) ──
        {
            use std::sync::Arc;
            let mut reg = tool_registry.lock().unwrap();
            reg.register(Arc::new(plugin_tools::LoadPluginTool::new(plugin_manager.clone())));
            reg.register(Arc::new(plugin_tools::UnloadPluginTool::new(plugin_manager.clone())));
            reg.register(Arc::new(plugin_tools::ReloadPluginTool::new(plugin_manager.clone())));
            log::info!("Registered 3 plugin management tools");
        }

        // Build shared state.
        let state = web::Data::new(AppState {
            plugin_manager: plugin_manager.clone(),
            event_bus: event_bus.clone(),
            llm: llm_recipient,
            tool_registry,
            snapshots,
            metrics_addr,
            token_usage_addr,
            retry_addr,
            cancellation_addr,
            chat_store: Some(store_addr.clone().recipient()),
        });

        log::info!("HTTP API listening on http://127.0.0.1:8080");
        HttpServer::new(move || {
            actix_web::App::new()
                .app_data(state.clone())
                .route("/api/health", web::get().to(health))
                .route("/api/plugins", web::get().to(list_plugins))
                .route("/api/plugins/load", web::post().to(load_plugin))
                .route("/api/plugins/unload/{name}", web::post().to(unload_plugin))
                .route("/api/plugins/reload/{name}", web::post().to(reload_plugin))
                .route("/api/plugins/scan", web::post().to(scan_plugins))
                .route("/api/events", web::post().to(publish_event))
                .route("/api/llm/chat", web::post().to(llm_chat))
                .route("/api/chat", web::post().to(chat))
                .route("/api/tools", web::get().to(list_tools))
                .route("/api/tools/call", web::post().to(tool_call))
                .route("/api/metrics", web::get().to(get_metrics))
                .route("/api/metrics/json", web::get().to(get_metrics_json))
                .route("/api/token-usage", web::get().to(get_global_token_usage))
                .route("/api/cancel", web::post().to(cancel_handler))
                .route("/api/retry/state", web::get().to(retry_state))
                .route("/api/shutdown", web::post().to(shutdown))
                .route("/api/plugin/{name}/{path:.*}", web::method(actix_web::http::Method::GET).to(plugin_proxy))
                .route("/api/plugin/{name}/{path:.*}", web::method(actix_web::http::Method::POST).to(plugin_proxy))
        })
        .bind("127.0.0.1:8080")?
        .run()
        .await
    })
}