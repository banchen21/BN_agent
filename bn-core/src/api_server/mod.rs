//! HTTP API — 基于 actix-web 的 REST 接口
//!
//! 路由:
//!   GET  /v1/tools      → 工具列表
//!   GET  /v1/health     → 健康检查
//!   ANY  /v1/{plugin}/... → 转发到插件 API

use actix::Addr;
use actix_web::{web, HttpRequest, HttpResponse};
use plugin_core::ToolRegistry;
use serde::Deserialize;
use std::sync::{Arc, Mutex};

use super::llm::client::{ChatRequest, LlmActor};
use super::models::plugin_loader::{ApiRequest, PluginManager, RefreshSnapshots};

/// 聊天请求
#[derive(Deserialize)]
pub struct ChatPayload {
    pub chat_id: Option<i64>,
    pub message: String,
    pub voice: Option<bool>,
}

/// 工具调用请求
#[derive(Deserialize)]
pub struct ToolPayload {
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

/// /v1/health
async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// /v1/tools
async fn tools(tool_registry: web::Data<std::sync::Arc<std::sync::Mutex<ToolRegistry>>>) -> HttpResponse {
    let tools = match tool_registry.lock() {
        Ok(reg) => reg.all_defs(),
        Err(_) => return HttpResponse::InternalServerError().json(serde_json::json!({"error": "registry locked"})),
    };
    HttpResponse::Ok().json(tools)
}

/// POST /v1/chat — LLM 对话（带工具调用）
async fn chat(
    pm: web::Data<Addr<PluginManager>>,
    llm: web::Data<Addr<LlmActor>>,
    tool_registry: web::Data<Arc<Mutex<ToolRegistry>>>,
    snapshots: web::Data<Arc<Mutex<Vec<String>>>>,
    payload: web::Json<ChatPayload>,
) -> HttpResponse {
    // 刷新被动上下文快照
    let _ = pm.send(RefreshSnapshots).await;
    let contexts: Vec<String> = snapshots.lock().unwrap().clone();

    let tools: Vec<serde_json::Value> = match tool_registry.lock() {
        Ok(reg) => reg.all_defs().iter()
            .filter(|d| !d.internal)
            .map(|d| serde_json::json!({
                "type": "function",
                "function": {
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.parameters,
                }
            }))
            .collect(),
        Err(_) => vec![],
    };

    let req = ChatRequest {
        chat_id: payload.chat_id.unwrap_or(0),
        message: payload.message.clone(),
        json_mode: false,
        tools,
        skip_store: false,
        contexts,
    };

    match llm.send(req).await {
        Ok(Ok(resp)) => HttpResponse::Ok().json(serde_json::json!({
            "content": resp.content,
            "tool_calls": resp.tool_calls,
            "cache_hit_tokens": resp.cache_hit_tokens,
        })),
        Ok(Err(e)) => HttpResponse::InternalServerError().json(serde_json::json!({"error": e})),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({"error": format!("{}", e)})),
    }
}

/// POST /v1/tools/call — 直接调用工具
async fn tool_call(
    tool_registry: web::Data<std::sync::Arc<std::sync::Mutex<ToolRegistry>>>,
    payload: web::Json<ToolPayload>,
) -> HttpResponse {
    let result = match tool_registry.lock() {
        Ok(reg) => reg.execute(&payload.tool_name, &payload.arguments),
        Err(_) => None,
    };

    match result {
        Some(r) => HttpResponse::Ok().json(serde_json::json!({
            "success": r.success,
            "content": r.content,
            "error": r.error,
        })),
        None => HttpResponse::NotFound().json(serde_json::json!({
            "error": format!("tool '{}' not found", payload.tool_name)
        })),
    }
}

/// /v1/{plugin}/* → 转发到插件 API
async fn plugin_proxy(
    pm: web::Data<Addr<PluginManager>>,
    req: HttpRequest,
    path: web::Path<String>,
    body: Option<String>,
) -> HttpResponse {
    let full_path = path.into_inner();
    let parts: Vec<&str> = full_path.splitn(2, '/').collect();
    let plugin_name = parts[0].to_string();
    let sub_path = if parts.len() > 1 { parts[1].to_string() } else { String::new() };

    let method = req.method().as_str().to_uppercase();

    let body_str = body.or_else(|| {
        // try to read body from request (simple approach)
        None
    });

    match pm.send(ApiRequest {
        plugin: plugin_name,
        method,
        path: sub_path,
        body: body_str,
    }).await {
        Ok(Some((status, body))) => {
            HttpResponse::build(
                actix_web::http::StatusCode::from_u16(status).unwrap_or(actix_web::http::StatusCode::OK)
            ).body(body)
        }
        Ok(None) => HttpResponse::NotFound().json(serde_json::json!({
            "error": "plugin not found or no API handler"
        })),
        Err(_) => HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "actor communication failed"
        })),
    }
}

/// 启动 HTTP server
pub async fn start_server(
    pm: Addr<PluginManager>,
    llm: Addr<LlmActor>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    let port: u16 = std::env::var("API_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);

    tracing::info!("HTTP API 启动: http://0.0.0.0:{}/v1", port);

    actix_web::HttpServer::new(move || {
        actix_web::App::new()
            .app_data(web::Data::new(pm.clone()))
            .app_data(web::Data::new(llm.clone()))
            .app_data(web::Data::new(tool_registry.clone()))
            .app_data(web::Data::new(snapshots.clone()))
            // Core
            .route("/v1/health", web::get().to(health))
            .route("/v1/tools", web::get().to(tools))
            .route("/v1/chat", web::post().to(chat))
            .route("/v1/tools/call", web::post().to(tool_call))
            // Plugin proxy
            .route("/v1/{plugin:.*}", web::method(actix_web::http::Method::GET).to(plugin_proxy))
            .route("/v1/{plugin:.*}", web::method(actix_web::http::Method::POST).to(plugin_proxy))
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
