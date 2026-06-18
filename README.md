# BN Agent — 插件化 LLM Agent 框架

一个基于 Rust + actix actor 模型的插件化 LLM Agent 框架。通过动态加载的 `cdylib` 插件注册工具（tools），由 LLM 自主调用，实现多平台 IM 接入、多模态交互、外部服务桥接等能力。

## 架构总览

```
┌──────────────────────────────────────────────────────────┐
│                     HTTP API (actix-web)                  │
│  GET /api/health  POST /api/chat  GET /api/tools  ...   │
└──────────────────────┬───────────────────────────────────┘
                       │
┌──────────────────────▼───────────────────────────────────┐
│                    EventBus (Actor)                       │
│        全局发布/订阅 — topic: user.message, llm.*        │
└────┬──────┬──────┬──────┬──────┬──────┬──────┬──────────┘
     │      │      │      │      │      │      │
┌────▼──┐ ┌▼─────┐ ┌▼────┐ ┌▼────┐ ┌▼────┐ ┌▼────┐ ┌──────┐
│tg-im  │ │feishu│ │tui  │ │webr │ │asr  │ │...  │ │PLUGIN│
│plugin │ │plugin│ │plugn│ │plugn│ │plugin│ │plugs│ │MGR   │
└──┬────┘ └──────┘ └─────┘ └─────┘ └─────┘ └─────┘ └──────┘
   │   user.message / assistant.message
   │
┌──▼───────────────────────────────────────────────────────┐
│  PipelineActor (核心编排)                                 │
│                                                          │
│  RateLimitActor ─▶ RetryActor ─▶ LlmActor ─▶ Tool exec   │
│  (频率限制)      (重试+熔断)    (LLM API)  (工具调用)    │
│                                                          │
│  TokenUsageActor    MetricsActor    CancellationActor     │
│  (Token SQLite)     (Prometheus)   (请求取消)            │
└──────────────────────────────────────────────────────────┘
```

### Actor 说明

| Actor | 职责 |
|---|---|
| **EventBus** | 全局事件总线，topic 匹配发布/订阅 |
| **PluginManager** | 动态加载/卸载 `cdylib` 插件，广播事件 |
| **PipelineActor** | 编排 LLM 工具调用循环：限流→重试→LLM→工具→回复 |
| **LlmActor** | OpenAI 兼容 API 封装，支持流式 SSE、多模态、SQLite 历史 |
| **RetryActor** | 指数退避重试 + 熔断器（Circuit Breaker） |
| **TokenUsageActor** | Token 用量 SQLite 持久化及查询 |
| **RateLimitActor** | 令牌桶频率限制 |
| **MetricsActor** | Prometheus 格式结构化观测 |
| **CancellationActor** | 按 chat_id 追踪/取消进行中的请求 |

## 插件一览

| 插件 | 功能 | 注册的工具 |
|---|---|---|
| `hello-plugin` | 演示插件 | — |
| `logger-plugin` | 日志记录所有事件 | — |
| `time-plugin` | 注入当前时间到 LLM 上下文 | — |
| `tg-im-plugin` | Telegram Bot | `tg_send_message`, `tg_send_voice`, `tg_send_photo`... |
| `feishu-im-plugin` | 飞书 Bot | `feishu_send_message` |
| `wechat-claw-plugin` | 微信 Bot | `wechat_send_message`, `wechat_send_voice` |
| `claude-bridge-plugin` | 调用 Claude CLI | `claude_chat` |
| `asr-tts-plugin` | 语音识别 + 合成 | `asr_transcribe`, `tts_synthesize` |
| `audio-capture-plugin` | 麦克风音频捕获 | — |
| `webrtc-plugin` | WebRTC P2P | `webrtc_create_peer`, `webrtc_answer_peer`... |
| `image-plugin` | 图片处理 | `image_info`, `image_resize`, `image_convert`, `image_compress`, `image_understand`, `image_describe` |
| `image-gen-plugin` | 本地 SD 生图 (ComfyUI) | `generate_image` |
| `video-plugin` | 视频分析 | `video_analyze` |
| `tui-plugin` | 终端聊天界面 | — |
| `mcp-plugin` | MCP 服务器桥接 | 动态 — 来自 MCP 服务器的 `tools/list` |
| `skill-plugin` | Markdown Skill 工具 | `skill__{name}` 按 .md 文件注册 |
| `proactive-plugin` | 主动消息推送 | — |

## 快速开始

### 环境要求

- Rust 1.75+
- 一个兼容 OpenAI API 的 LLM 端点

### 配置

```bash
# 复制示例配置
cp main-app/.env.example main-app/.env
# 编辑 main-app/.env，至少设置：
# LLM_API_KEY=your_key
# LLM_MODEL=your_model
# LLM_BASE_URL=https://api.example.com/v1
```

### 构建 & 运行

```bash
# 编译所有插件
cargo build

# 运行主应用
cargo run -p main-app
```

### 完整环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `LLM_API_KEY` | — | **必填** LLM API Key |
| `LLM_MODEL` | `deepseek-chat` | LLM 模型名 |
| `LLM_BASE_URL` | `https://api.deepseek.com/v1` | API 端点 |
| `LLM_MAX_HISTORY` | `20` | 历史对话轮数 |
| `PLUGIN_LOAD` | (全部) | 插件白名单，逗号分隔 |
| `PLUGIN_SKIP` | (空) | 插件黑名单 |
| `RETRY_MAX_ATTEMPTS` | `3` | 最大重试次数 |
| `RETRY_BASE_DELAY_MS` | `1000` | 初始重试延迟(ms) |
| `RETRY_MAX_DELAY_MS` | `30000` | 最大重试延迟(ms) |
| `CIRCUIT_BREAKER_THRESHOLD` | `5` | 熔断触发连续失败数 |
| `CIRCUIT_BREAKER_COOLDOWN_MS` | `60000` | 熔断冷却时间(ms) |
| `RATE_LIMIT_PER_MIN` | `30` | 每分钟每会话请求上限 |
| `RATE_LIMIT_BURST` | `5` | 令牌桶突发容量 |
| `SKILL_DIR` | `data/skills/` | Skill 文件目录 |
| `COMFYUI_URL` | `http://127.0.0.1:8188` | ComfyUI API 地址（image-gen-plugin） |
| `COMFYUI_OUTPUT_DIR` | `output` | ComfyUI 输出目录（image-gen-plugin） |
| `IMAGE_GEN_OUTPUT_DIR` | `./temp_images` | 生图本地副本目录 |

### HTTP API

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | 健康检查 |
| GET | `/api/plugins` | 插件列表 |
| POST | `/api/plugins/load` | 加载插件 |
| POST | `/api/plugins/unload/{name}` | 卸载插件 |
| POST | `/api/plugins/reload/{name}` | 重载插件 |
| POST | `/api/plugins/scan` | 扫描目录自动加载 |
| POST | `/api/events` | 发布事件 |
| POST | `/api/llm/chat` | 简单 LLM 调用 |
| POST | `/api/chat` | 带工具+历史的完整对话 |
| GET | `/api/tools` | 列出所有已注册工具 |
| POST | `/api/tools/call` | 直接调用工具 |
| GET | `/api/metrics` | Prometheus 格式指标 |
| GET | `/api/metrics/json` | JSON 格式指标 |
| GET | `/api/token-usage` | 全局 Token 用量 |
| GET | `/api/token-usage/{chat_id}` | 按会话 Token 用量 |
| POST | `/api/cancel/{chat_id}` | 取消进行中的请求 |
| GET | `/api/retry/state` | 熔断器状态 |
| ANY | `/api/plugin/{name}/{path:.*}` | 代理到插件 API |
| POST | `/api/shutdown` | 优雅关闭 |

## 开发一个新插件

```rust
use plugin_interface::*;

pub struct MyPlugin;

impl Plugin for MyPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "my-plugin".into(),
            version: "0.1.0".into(),
            description: "does something useful".into(),
            author: "me".into(),
            min_host_version: "0.1.0".into(),
        }
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        // 注册工具
        if let Some(ref registry) = ctx.tool_registry {
            let mut reg = registry.lock().unwrap();
            reg.register(Arc::new(MyTool));
        }
        Ok(())
    }

    fn stop(&mut self) {}
}

// 导出 FFI 符号
#[no_mangle]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(MyPlugin)
}
```

在 `Cargo.toml` 中添加：
```toml
[lib]
crate-type = ["cdylib"]
```

## 插件加载机制

1. `PluginManager` 扫描插件目录（默认 `../target/debug/`）
2. 过滤 `.dll` / `.so` / `.dylib` 文件
3. 通过 `PLUGIN_LOAD` / `PLUGIN_SKIP` 环境变量过滤
4. 使用 `libloading` 加载动态库，查找 `plugin_create` FFI 符号
5. 调用 `plugin.start(ctx)` — 插件在此注册工具、创建 actor、订阅事件

## Skill 系统

Skill 插件的 Markdown 文件格式（放在 `data/skills/` 目录）：

```markdown
---
name: summarize_text
description: "对文本进行摘要总结"
parameters:
  type: object
  properties:
    text:
      type: string
      description: "要总结的文本内容"
  required: [text]
---
请对以下文本进行简明扼要的总结，提取关键信息，控制在 200 字以内：

{{text}}
```

每个 `.md` 文件注册为 `skill__{name}` 工具，LLM 调用时返回正文（支持 `{{param}}` 模板替换）。

## 项目结构

```
bn-agent/
├── plugin-interface/        # 共享契约：Plugin trait, ToolRegistry, EventBus
├── main-app/                # 主程序入口，HTTP API，Actor 系统
│   └── src/
│       ├── main.rs          # 启动入口 + HTTP 路由
│       ├── llm_actor.rs     # LLM API 封装 + 流式响应
│       ├── pipeline.rs      # 工具调用循环编排
│       ├── plugin_manager.rs# 插件加载管理
│       ├── chat_store.rs    # 对话历史 SQLite
│       ├── retry_actor.rs   # 重试+熔断
│       ├── token_usage_actor.rs  # Token 用量追踪
│       ├── rate_limit_actor.rs   # 频率限制
│       ├── metrics_actor.rs      # 结构化观测
│       └── cancellation_actor.rs # 请求取消
├── plugins/
│   ├── hello-plugin/        # 演示
│   ├── logger-plugin/       # 日志
│   ├── time-plugin/         # 时间注入
│   ├── tg-im-plugin/        # Telegram IM
│   ├── feishu-im-plugin/    # 飞书 IM
│   ├── wechat-claw-plugin/  # 微信 IM
│   ├── claude-bridge-plugin/ # Claude CLI 桥接
│   ├── asr-tts-plugin/      # 语音识别 + 合成
│   ├── audio-capture-plugin/ # 音频捕获
│   ├── webrtc-plugin/        # WebRTC P2P
│   ├── image-plugin/         # 图片处理
│   ├── image-gen-plugin/     # 本地 SD 生图 (ComfyUI)
│   ├── video-plugin/         # 视频分析
│   ├── tui-plugin/           # 终端 UI
│   ├── mcp-plugin/           # MCP 服务器桥接
│   ├── skill-plugin/         # Markdown Skill 工具
│   └── proactive-plugin/     # 主动消息推送
└── data/
    ├── skills/               # Skill .md 文件目录
    └── jailbreak_prompts.csv # 提示词注入
```
