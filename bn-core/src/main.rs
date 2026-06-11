//! BN Agent 主程序

mod models { pub mod event_bus; pub mod plugin_loader; }
mod llm;
mod api;
mod core_loop;
mod runtime;

use tracing_subscriber::prelude::*;

fn main() -> std::io::Result<()> {
    // .env
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    if let Err(e) = dotenvy::from_path(&env_path) {
        eprintln!("[main] 警告: .env 加载失败: {}", e);
    }

    // 清除终端代理变量
    for var in &["HTTP_PROXY","HTTPS_PROXY","ALL_PROXY","http_proxy","https_proxy","all_proxy"] {
        std::env::remove_var(var);
    }

    // 日志
    let log_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("logs");
    std::fs::create_dir_all(&log_dir).ok();
    let log_file = std::fs::File::create(log_dir.join("bn-agent.log"))?;
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_target(false).with_ansi(true))
        .with(tracing_subscriber::fmt::layer().with_target(true).with_ansi(false)
            .with_writer(std::sync::Mutex::new(log_file)))
        .init();

    runtime::run()
}
