//! BN Agent 主程序

mod models { pub mod event_bus; pub mod llm; pub mod plugin_loader; }
mod core;
mod runtime;
mod api_server;
mod logger;

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

    logger::init();
    use crate::runtime;

    runtime::run()
}
