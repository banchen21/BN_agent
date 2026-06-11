//! 日志初始化

use tracing_subscriber::prelude::*;

pub fn init() {
    let log_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("logs");
    std::fs::create_dir_all(&log_dir).ok();
    let log_file = std::fs::File::create(log_dir.join("bn-agent.log"))
        .expect("无法创建日志文件");

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_target(false).with_ansi(true))
        .with(tracing_subscriber::fmt::layer().with_target(true).with_ansi(false)
            .with_writer(std::sync::Mutex::new(log_file)))
        .init();
}
