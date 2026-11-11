//! 通话 HTTP 入口与浏览器通话页。
//!
//! - `/api/call/{start,end,health,rooms}` → 转发给 `call-livekit-plugin`（CORS 全开）
//! - `/call` → 浏览器通话页（PC / 手机同一页面）
//! - `/call/assets/{file}` → 页面静态资源（livekit-client）
//!
//! 页面文件默认读取 `deploy/livekit/call.html`，可用 `CALL_DEPLOY_DIR` 覆盖。

use crate::AppState;
use actix_web::http::StatusCode;
use actix_web::{web, HttpRequest, HttpResponse, Responder};
use plugin_interface::ApiRequest;
use std::path::PathBuf;

const CALL_PLUGIN: &str = "call-livekit-plugin";
// HeaderName::from_static 要求小写；HTTP 头名大小写不敏感。
const CORS_HEADERS: [(&str, &str); 3] = [
    ("access-control-allow-origin", "*"),
    ("access-control-allow-methods", "GET, POST, OPTIONS"),
    (
        "access-control-allow-headers",
        "Content-Type, Authorization, X-API-Key",
    ),
];

fn with_cors(mut response: HttpResponse) -> HttpResponse {
    for (name, value) in CORS_HEADERS {
        response.headers_mut().append(
            actix_web::http::header::HeaderName::from_static(name),
            actix_web::http::header::HeaderValue::from_static(value),
        );
    }
    response
}

/// 转发 `/api/call/{action}` 到 call 插件。
pub async fn call_api(
    state: web::Data<AppState>,
    req: HttpRequest,
    action: web::Path<String>,
    body: Option<web::Bytes>,
) -> impl Responder {
    let method = req.method().as_str().to_uppercase();
    let body_str = body.and_then(|bytes| String::from_utf8(bytes.to_vec()).ok());

    match state
        .plugin_manager
        .send(ApiRequest {
            plugin: CALL_PLUGIN.to_string(),
            method,
            path: action.into_inner(),
            body: body_str,
        })
        .await
    {
        Ok(Some((status, text))) => with_cors(
            HttpResponse::build(
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            )
            .content_type("application/json; charset=utf-8")
            .body(text),
        ),
        Ok(None) => with_cors(
            HttpResponse::NotFound()
                .content_type("application/json; charset=utf-8")
                .body(
                    serde_json::json!({
                        "error": format!(
                            "call 插件未加载或未提供 API（期望插件名 {}，检查 PLUGIN_DIR 与构建产物）",
                            CALL_PLUGIN
                        )
                    })
                    .to_string(),
                ),
        ),
        Err(e) => with_cors(
            HttpResponse::InternalServerError()
                .content_type("application/json; charset=utf-8")
                .body(
                    serde_json::json!({ "error": format!("plugin manager 通信失败: {e}") })
                        .to_string(),
                ),
        ),
    }
}

/// CORS 预检。
pub async fn call_preflight() -> impl Responder {
    with_cors(HttpResponse::NoContent().finish())
}

fn deploy_dir() -> PathBuf {
    std::env::var("CALL_DEPLOY_DIR")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../deploy/livekit")
        })
}

/// 浏览器通话页。
pub async fn call_page() -> impl Responder {
    let path = deploy_dir().join("call.html");
    match std::fs::read_to_string(&path) {
        Ok(html) => HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .insert_header(("Cache-Control", "no-store"))
            .body(html),
        Err(e) => HttpResponse::InternalServerError()
            .content_type("text/plain; charset=utf-8")
            .body(format!(
                "通话页缺失：{}（{}）。请确认 deploy/livekit/call.html 存在。",
                path.display(),
                e
            )),
    }
}

fn content_type_for(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "crt" | "pem" => "application/x-x509-ca-cert",
        _ => "application/octet-stream",
    }
}

/// 通话页静态资源（只允许单层文件名，防目录穿越）。
pub async fn call_asset(file: web::Path<String>) -> impl Responder {
    let name = file.into_inner();
    let valid = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !valid {
        return HttpResponse::NotFound().finish();
    }

    let path = deploy_dir().join("static").join(&name);
    match std::fs::read(&path) {
        Ok(bytes) => HttpResponse::Ok()
            .content_type(content_type_for(&name))
            .insert_header(("Cache-Control", "public, max-age=86400"))
            .body(bytes),
        Err(_) => HttpResponse::NotFound().finish(),
    }
}
