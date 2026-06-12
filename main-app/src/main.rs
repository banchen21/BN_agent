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
//! | ANY    | `/api/plugin/{name}/{path:.*}` | Proxy to plugin API handler    |
//! | POST   | `/api/shutdown`              | Graceful shutdown                |

mod chat_store;
mod llm_actor;
mod pipeline;
mod plugin_manager;

use actix::prelude::*;
use actix_web::{web, HttpRequest, HttpResponse, HttpServer, Responder};
use llm_actor::LlmActor;
use plugin_interface::*;
use plugin_manager::PluginManager;
use serde::Deserialize;
use std::sync::{Arc, Mutex};

// ── App state ────────────────────────────────────────────────────────────────

struct AppState {
    plugin_manager: Addr<PluginManager>,
    event_bus: Addr<EventBus>,
    llm: Option<Recipient<LlmRequest>>,
    llm_addr: Option<Addr<LlmActor>>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
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
struct ChatPayload { chat_id: Option<i64>, message: String }

async fn chat(state: web::Data<AppState>, body: web::Json<ChatPayload>) -> impl Responder {
    let llm_addr = match &state.llm_addr {
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

    match llm_addr.send(ChatRequest {
        chat_id: body.chat_id.unwrap_or(0),
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
    }).await {
        Ok(Ok(resp)) => HttpResponse::Ok().json(resp),
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
    // .env 路径：编译时固定在 main-app/ 目录。
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    if let Err(e) = dotenvy::from_path(&env_path) {
        eprintln!("[main] .env load skipped: {}", e);
    }
    env_logger::init();
    log::info!("=== actix-actor plugin system starting ===");

    let sys = actix_rt::System::new();

    sys.block_on(async {
        // 1. EventBus
        let event_bus = EventBus::new().start();
        log::info!("EventBus actor started");

        // 2. LlmActor (optional — requires LLM_API_KEY).
        let (llm_addr, llm_recipient): (Option<Addr<LlmActor>>, Option<Recipient<LlmRequest>>) = {
            match LlmActor::from_env(event_bus.clone()) {
                Some(actor) => {
                    let addr = actor.start();
                    log::info!("LlmActor started");
                    let recipient = addr.clone().recipient();
                    (Some(addr), Some(recipient))
                }
                None => {
                    log::warn!("LLM not configured — LLM endpoints will return 503");
                    (None, None)
                }
            }
        };

        // 3. Plugin directory (auto-scan happens in PluginManager::started).
        let plugin_dir = std::env::var("PLUGIN_DIR").unwrap_or_else(|_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../target/debug")
                .to_string_lossy()
                .to_string()
        });
        log::info!("Plugin dir: {}", plugin_dir);

        // 4. Shared ToolRegistry.
        let tool_registry: Arc<Mutex<ToolRegistry>> = Arc::new(Mutex::new(ToolRegistry::new()));

        // 5. PluginManager — auto-scans in started() callback.
        let pm = PluginManager::new(
            event_bus.clone(),
            llm_recipient.clone(),
            tool_registry.clone(),
            plugin_dir,
        );
        let snapshots = pm.snapshots_arc();
        let plugin_manager = pm.start();
        log::info!("PluginManager actor started");

        // 6. PipelineActor (if LLM is available).
        if let Some(ref llm_addr) = llm_addr {
            let pipeline = pipeline::PipelineActor::new(
                llm_addr.clone(),
                plugin_manager.clone(),
                tool_registry.clone(),
                snapshots.clone(),
                event_bus.clone(),
            );
            let pipeline_addr = pipeline.start();
            log::info!("PipelineActor started");

            // Subscribe PipelineActor to user.message events.
            event_bus.do_send(Subscribe {
                topic: "user.message".into(),
                recipient: pipeline_addr.recipient(),
            });
            log::info!("PipelineActor subscribed to 'user.message'");
        }

        // Build shared state.
        let state = web::Data::new(AppState {
            plugin_manager: plugin_manager.clone(),
            event_bus: event_bus.clone(),
            llm: llm_recipient,
            llm_addr,
            tool_registry,
            snapshots,
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
                .route("/api/shutdown", web::post().to(shutdown))
                .route("/api/plugin/{name}/{path:.*}", web::method(actix_web::http::Method::GET).to(plugin_proxy))
                .route("/api/plugin/{name}/{path:.*}", web::method(actix_web::http::Method::POST).to(plugin_proxy))
        })
        .bind("127.0.0.1:8080")?
        .run()
        .await
    })
}
