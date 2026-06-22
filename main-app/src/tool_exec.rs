//! 工具执行超时封装：在 blocking 线程池执行同步工具，避免阻塞 actix arbiter，并施加 per-tool 超时。

use plugin_interface::{ToolExecutor, ToolResult};
use std::sync::Arc;
use std::time::Duration;

/// 每工具超时秒数（env `TOOL_TIMEOUT_SECS`，默认 180）；0 表示禁用超时
/// （仍走 `spawn_blocking` 以免阻塞 async executor 线程）。
pub fn tool_timeout_secs() -> u64 {
    std::env::var("TOOL_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180)
}

/// 在 blocking 线程池执行工具；超过 `timeout_secs` 返回超时错误。
///
/// 注意：工具是同步 `execute()`，超时后其后台任务仍会继续运行直到自身结束
/// （`spawn_blocking` 不可取消），但主流程不再被阻塞。工具被设计为线程无关
/// （内部自建 runtime），故在 blocking 线程执行是安全的。
pub async fn execute_with_timeout(
    exec: Arc<dyn ToolExecutor>,
    args: serde_json::Value,
    tool_name: &str,
    timeout_secs: u64,
) -> ToolResult {
    let handle = tokio::task::spawn_blocking(move || exec.execute(&args));
    if timeout_secs == 0 {
        return match handle.await {
            Ok(r) => r,
            Err(_) => ToolResult::err(&format!("tool '{}' panicked", tool_name)),
        };
    }
    match tokio::time::timeout(Duration::from_secs(timeout_secs), handle).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => ToolResult::err(&format!("tool '{}' panicked", tool_name)),
        Err(_) => ToolResult::err(&format!(
            "tool '{}' timed out after {}s",
            tool_name, timeout_secs
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_interface::ToolDef;

    struct SleepTool {
        def: ToolDef,
        millis: u64,
    }
    impl ToolExecutor for SleepTool {
        fn def(&self) -> &ToolDef {
            &self.def
        }
        fn execute(&self, _args: &serde_json::Value) -> ToolResult {
            std::thread::sleep(Duration::from_millis(self.millis));
            ToolResult::ok("done")
        }
    }

    fn sleep_tool(millis: u64) -> Arc<dyn ToolExecutor> {
        Arc::new(SleepTool {
            def: ToolDef {
                name: "sleep".into(),
                description: "sleeps".into(),
                parameters: serde_json::json!({}),
                internal: false,
            },
            millis,
        })
    }

    #[actix_rt::test]
    async fn fast_tool_completes() {
        let r = execute_with_timeout(sleep_tool(10), serde_json::json!({}), "sleep", 5).await;
        assert!(r.success);
        assert_eq!(r.content, "done");
    }

    #[actix_rt::test]
    async fn slow_tool_times_out() {
        let r = execute_with_timeout(sleep_tool(2000), serde_json::json!({}), "sleep", 1).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("timed out"));
    }

    #[actix_rt::test]
    async fn zero_timeout_disables_limit() {
        let r = execute_with_timeout(sleep_tool(50), serde_json::json!({}), "sleep", 0).await;
        assert!(r.success);
    }
}
