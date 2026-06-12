# claude-bridge 做 LLM 后端方案

## 目标

让 claude-bridge 插件提供 `Handler<LlmRequest>` 的 actor，使 PipelineActor 可以直接把对话请求发给 Claude CLI 而非 LlmActor（OpenAI API）。

```
启动时 LLM_BACKEND=claude
                   │
        ┌──────────┴──────────┐
        │                     │
   LlmActor(openai)    ClaudeBridgeActor(CLI)
        │                     │
   pipeline → retry     pipeline → retry
        │                     │
   ChatStore            ChatStore
        │                     │
   IM 插件               IM 插件（不变）
```

## 改动

### 1. claude-bridge-plugin: 新增 ClaudeBridgeActor

```
ClaudeBridgeActor
  ├── 持久 Claude CLI 进程（stdin/stdout 管道）
  ├── Handler<LlmRequest> — 写入 stdin，读 stdout 解析
  ├── 支持 tool_calls 解析（Claude 的 JSON 格式）
  └── 重启机制（进程崩溃自动拉起）
```

### 2. plugin_manager.rs: 暴露 LLM 后端

PluginManager 新增方法 `get_llm_backend() → Option<Recipient<LlmRequest>>`，遍历已加载插件检查谁注册了 LLM 后端。

### 3. main.rs: 启动时按配置选后端

```
if LLM_BACKEND == "claude":
    先加载 claude-bridge plugin（此时它创建 ClaudeBridgeActor）
    plugin_manager.get_llm_backend() → 拿到 recipient
    retry_addr = 用此 recipient 包装 RetryActor
else:
    走当前流程（LlmActor → RetryActor）
```

## 关键代码

### claude-bridge-plugin: ClaudeBridgeActor

```rust
use actix::prelude::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Command, ChildStdin, ChildStdout};

struct ClaudeBridgeActor {
    stdin: Option<ChildStdin>,
    event_bus: Addr<EventBus>,
}

impl Actor for ClaudeBridgeActor {
    type Context = Context<Self>;
}

impl Handler<LlmRequest> for ClaudeBridgeActor {
    type Result = ResponseActFuture<Self, Result<LlmResponse, String>>;

    fn handle(&mut self, msg: LlmRequest, _ctx: &mut Self::Context) -> Self::Result {
        // 把 messages 拼成 prompt → 写入 claude stdin
        // 读 claude stdout → 解析成 LlmResponse
        // 如果含 tool_calls → 一并返回
    }
}
```

### 进程管理

```rust
fn start_claude_process(path: &str) -> (ChildStdin, ChildStdout) {
    let mut child = Command::new(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to start claude");

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    (stdin, stdout)
}
```

## 数据流

```
HTTP / IM → PipelineActor
    ↓ ChatRequest
RetryActor
    ↓ LlmRequest
ClaudeBridgeActor
    ↓ stdin
[claude CLI 进程]  ← 持久运行，保持会话
    ↓ stdout
ClaudeBridgeActor 解析
    ↓ LlmResponse { content, tool_calls }
PipelineActor 执行工具
    ↓ assistant.message
IM 插件 → 用户
ChatStore → 持久化
```

## 现状

- ✅ claude-bridge 已能用 `Command::new("claude").output()` 做一次性调用
- ❌ 没有持久进程（每次启动新进程，丢失会话记忆）
- ❌ 没有实现 `Handler<LlmRequest>` actor
- ❌ 没有 tool_calls 解析（Claude 回复含 `<function_calls>` 或 JSON）
- ❌ main.rs 没有 LLM 后端切换逻辑
