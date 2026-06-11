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

use crate::models::plugin_loader::{ApiRequest, PluginManager};

/// 聊天请求（供后续扩展）
#[derive(Deserialize)]
pub struct ChatPayload {
    pub chat_id: Option<i64>,
    pub message: String,
    pub voice: Option<bool>,
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
    tool_registry: std::sync::Arc<std::sync::Mutex<ToolRegistry>>,
) -> std::io::Result<()> {
    let port: u16 = std::env::var("API_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);

    tracing::info!("HTTP API 启动: http://0.0.0.0:{}/v1", port);

    actix_web::HttpServer::new(move || {
        actix_web::App::new()
            .app_data(web::Data::new(pm.clone()))
            .app_data(web::Data::new(tool_registry.clone()))
            .route("/v1/health", web::get().to(health))
            .route("/v1/tools", web::get().to(tools))
            .route("/v1/{plugin:.*}", web::method(actix_web::http::Method::GET).to(plugin_proxy))
            .route("/v1/{plugin:.*}", web::method(actix_web::http::Method::POST).to(plugin_proxy))
            .route("/v1/{plugin:.*}", web::method(actix_web::http::Method::PUT).to(plugin_proxy))
            .route("/v1/{plugin:.*}", web::method(actix_web::http::Method::DELETE).to(plugin_proxy))
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
