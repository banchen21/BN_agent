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
| **AgentLoopActor** | 显式目标驱动的 observe→decide→act 循环，支持预算、状态查询、停止和失败自我修正 |
| **LlmActor** | OpenAI 兼容 API 封装，支持流式 SSE、多模态、SQLite 历史和长期摘要压缩；流式片段通过 `llm.chunk` 带 `request_id/source/peer_id` 发到 EventBus |
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
| `memory-plugin` | 长期记忆提取（Engram 风格双时态） | —（通过 snapshot 注入） |
| `proactive-plugin` | 主动消息推送 | — |
| `toy-control-plugin` | 跳蛋远程控制 + Web 面板 | `toy_vibrate`, `toy_stop`, `toy_set_pattern`, `toy_status` |
| `comfyui-qwen-plugin` | Qwen-Image 2.1 文生图 / 参考图编辑（ComfyUI 8188） | `comfyui_generate_image`, `comfyui_edit_image` |
| `call-livekit-plugin` | LiveKit 实时通话后端：建房、participant token、agent dispatch | —（HTTP `/api/call/*`） |

> **语音/视频通话**：`call-livekit-plugin`（Rust，信令/建房/token）+
> `call-agent/`（Python sidecar，`agent_name=yueyue`，全双工语音、摄像头帧订阅与
> `look_at_user` 截图理解）。部署与联调见 `deploy/livekit/README.md` 和
> `call-agent/README.md`；人设/记忆数据放在 `data/yueyue/`（运行时数据，不入库）。
>
> **Qwen 参考图上限（本机实测）**：RTX 4070 Laptop 8GB 在 512²/8 步 LoRA 下
> 4 张参考图安全，8 张会触发 GPU TDR（驱动重置）。插件通过
> `max_reference_images`（默认 4，范围 1–16）在请求 ComfyUI 前强制拒绝超限参考图；
> 显存 ≥12GB 可在 WebUI 配置里调高；TDR 后需整机重启才能恢复 ComfyUI。

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
| `CHAT_HISTORY_CLEAR_ON_START` | `false` | 启动时是否清空短期聊天历史，默认保留 |
| `CHAT_HISTORY_MAX_PER_PEER` | `1000` | 每 peer 保留的历史记录上限，超出清理最旧，0=不限 |
| `IMMEDIATE_CONTEXT_MSGS` | `200` | 即时上下文消息数 |
| `LLM_LONG_SUMMARY_ENABLED` | `true` | 是否启用 LLM 级长期摘要压缩；旧对话会后台压缩后注入后续 system prompt |
| `LLM_LONG_SUMMARY_KEEP_RECENT` | `200` | 长期压缩时保留的最近消息窗口，窗口内仍走即时上下文 |
| `LLM_LONG_SUMMARY_BATCH_SIZE` | `80` | 每次后台压缩的旧消息批量上限 |
| `LLM_LONG_SUMMARY_MAX_CHARS` | `4000` | 长期摘要正文最大字符数 |
| `LLM_MAX_TOOL_ROUNDS` | `20` | 最大工具调用轮数 |
| `TOOL_TIMEOUT_SECS` | `180` | 单个工具执行超时(秒)，0=禁用；超时返回错误且不阻塞主流程。本机 `.env` 设为 `0`：Qwen 生图约 5–14 分钟、编辑 7–20+ 分钟，180s 必然误判超时（如需上限建议 ≥1800） |
| `ASR_TIMEOUT_SECS` | `60` | ASR 调用链整体超时(秒)，覆盖 ffmpeg 转码 + ASR API 请求/响应读取，0=禁用 |
| `TOKEN_BUDGET_DAILY` | (无) | Token 预算·近 24h 上限；超限拒绝新请求，0/未设=无限 |
| `TOKEN_BUDGET_WEEKLY` | (无) | Token 预算·近 7d 上限 |
| `TOKEN_BUDGET_MONTHLY` | (无) | Token 预算·近 30d 上限 |
| `AGENT_LOOP_MAX_SLEEP_SECS` | `60` | Agent Loop 单步 sleep 上限 |
| `AGENT_LOOP_DB_PATH` | `data/agent_loops.db` | Agent Loop 状态持久化路径（重启恢复历史） |
| `AGENT_LOOP_MAX_KEEP` | `200` | 终态 Agent Loop 保留上限，超出清理最旧（防内存/DB 膨胀） |
| `AGENT_LOOP_TOOL_DENY` | (空) | Agent Loop 工具黑名单（逗号分隔工具名）；自主 loop 不可见且执行前拒绝，用于禁用危险工具 |
| `AGENT_LOOP_MAX_DURATION_SECS` | `0` | Agent Loop 墙钟总时长上限(秒)，步间检查超限标记 failed，0=不限 |
| `AGENT_LOOP_MAX_CONCURRENT` | `0` | 同时运行的 Agent Loop 数量上限，达上限拒绝启动新 loop，0=不限 |
| `LLM_THINKING` | `disabled` | DeepSeek 思考模式 |
| `PROACTIVE_MODE` | `auto` | 主动追问模式 (auto/semi-auto) |
| `PROACTIVE_LOOP_INTERVAL` | `15` | 主动追问轮询间隔(秒) |
| `PROACTIVE_AGENT_LOOP_MODE` | `off` | 自主空闲触发 Agent Loop 模式：`off`/`mirror`/`replace` |
| `PROACTIVE_AGENT_LOOP_GOAL` | 内置目标 | Agent Loop 目标模板，支持 `{source}`/`{chat_id}`/`{peer_id}`/`{idle_secs}` |
| `PROACTIVE_AGENT_LOOP_MAX_STEPS` | `6` | proactive 启动 Agent Loop 时的最大步骤数 |
| `PROACTIVE_AGENT_LOOP_MAX_TOOL_ROUNDS` | `3` | proactive 启动 Agent Loop 时每步最大工具调用轮数，`0`=禁用工具轮 |
| `MEMORY_EXTRACT_EVERY` | `10` | 记忆提取触发消息数 |
| `MEMORY_DB_PATH` | `data/memories.db` | 记忆数据库路径 |
| `TOY_CONTROL_PORT` | `8090` | 跳蛋 Web 控制面板端口 |
| `PLUGIN_LOAD` | (全部) | 插件白名单，逗号分隔 |
| `PLUGIN_SKIP` | (空) | 插件黑名单 |
| `RETRY_MAX_ATTEMPTS` | `3` | 最大重试次数 |
| `RETRY_BASE_DELAY_MS` | `1000` | 初始重试延迟(ms) |
| `RETRY_MAX_DELAY_MS` | `30000` | 最大重试延迟(ms) |
| `CIRCUIT_BREAKER_THRESHOLD` | `5` | 熔断触发连续失败数 |
| `CIRCUIT_BREAKER_COOLDOWN_MS` | `60000` | 熔断冷却时间(ms) |
| `CIRCUIT_BREAKER_DB_PATH` | `data/circuit_breaker.db` | 熔断状态持久化路径（重启恢复 open/half-open） |
| `RATE_LIMIT_PER_MIN` | `30` | 每分钟每会话请求上限 |
| `RATE_LIMIT_BURST` | `5` | 令牌桶突发容量 |
| `API_KEY` | (无) | HTTP API 鉴权密钥；设置后除 `/api/health`、`/ui/config`、`/config` 外所有接口需带 `Authorization: Bearer <key>`（或 `X-API-Key: <key>`），未设=不鉴权 |
| `PERSONA_PATH` | `persona.md` | 人设文件路径；可指向 `data/yueyue/persona.md` 等自定义文件 |
| `PLUGIN_CONFIG_DIR` | `data/plugin_configs` | 插件配置文件目录（每个插件一个 `<plugin-name>.json`） |
| `HTTP_BIND` | `127.0.0.1:8080` | HTTP API 监听地址（端口冲突时可改；本机 `.env` 现为 `0.0.0.0:18080`，因 8080 被其它服务占用，且手机需局域网访问通话/配置页——暴露到局域网前务必设置 `API_KEY`） |
| `SKILL_DIR` | `data/skills/` | Skill 文件目录 |
| `COMFYUI_URL` | `http://127.0.0.1:8188` | ComfyUI API 地址（image-gen-plugin） |
| `COMFYUI_OUTPUT_DIR` | `output` | ComfyUI 输出目录（image-gen-plugin） |
| `IMAGE_GEN_OUTPUT_DIR` | `./temp_images` | 生图本地副本目录 |

### HTTP API

> 🔐 设置 `API_KEY` 后，除 `/api/health`、`/ui/config`、`/config` 外所有接口需在请求头携带 `Authorization: Bearer <key>`（或 `X-API-Key: <key>`），否则返回 401；未设置则不鉴权。
> 本机（已绑 `0.0.0.0:18080`）的 key 存放在 `.secrets/api_key.txt`（已 gitignore，勿提交、勿打印到日志）。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | 健康检查，返回 version/uptime_secs/plugins_loaded/agent_loops_total（始终免鉴权，便于探活） |
| GET | `/api/plugins` | 插件列表 |
| POST | `/api/plugins/load` | 加载插件 |
| POST | `/api/plugins/unload/{name}` | 卸载插件 |
| POST | `/api/plugins/reload/{name}` | 重载插件 |
| POST | `/api/plugins/scan` | 扫描目录自动加载 |
| POST | `/api/events` | 发布事件 |
| POST | `/api/llm/chat` | 简单 LLM 调用 |
| POST | `/api/chat` | 带工具+历史的完整对话；body: `message`（必填）与可选的 `peer_id`、`user_name`、`source`（`peer_id` 归一化后默认 `web:default`） |
| GET | `/api/chat/history` | 按 `peer_id` 拉取历史，返回 JSON 数组（元素含 `role`/`content`/`peer_id`）；`limit` 默认 50、上限 500；省略 `peer_id` 时同 `/api/chat` 回退 `web:default` |
| GET | `/api/chat/sessions` | 列出会话元数据；支持 `peer_id`、`limit` 查询参数 |
| POST | `/api/chat/sessions/refresh` | 刷新指定 `peer_id` 的标题、摘要和计数 |
| GET | `/api/tools` | 列出所有已注册工具 |
| POST | `/api/tools/call` | 直接调用工具 |
| POST | `/api/agent-loop/start` | 启动目标驱动 Agent Loop |
| GET | `/api/agent-loop/list` | 列出 Agent Loop 任务 |
| GET | `/api/agent-loop/status/{id}` | 查询 Agent Loop 状态 |
| POST | `/api/agent-loop/stop/{id}` | 请求停止 Agent Loop |
| POST | `/api/agent-loop/pause/{id}` | 暂停 Agent Loop |
| POST | `/api/agent-loop/resume/{id}` | 恢复已暂停的 Agent Loop |
| GET | `/api/metrics` | Prometheus 格式指标 |
| GET | `/api/metrics/json` | JSON 格式指标 |
| GET | `/api/token-usage` | 全局 Token 用量 |
| GET | `/api/token-usage/{chat_id}` | 按会话 Token 用量 |
| GET | `/api/token-usage/budget` | Token 预算状态（是否超限/用量/上限） |
| POST | `/api/cancel/{chat_id}` | 取消进行中的请求 |
| GET | `/api/retry/state` | 熔断器状态 |
| GET | `/api/plugin-config` | 插件配置 schema 列表（无 schema 的插件返回 `editable=false`） |
| GET | `/api/plugin-config/{name}` | 读取单个插件配置（secret 字段返回 `******`） |
| PUT | `/api/plugin-config/{name}` | 保存配置并自动重载插件；reload 后会校验插件运行态与工具是否仍注册，失败则回滚配置、恢复旧实例并返回 500 |
| POST | `/api/plugin-config/{name}/reload` | 重载插件以应用已落盘的配置 |
| GET | `/ui/config` | 静态插件配置 WebUI（免鉴权；页面内填写 API Key 调用 API） |
| ANY | `/api/plugin/{name}/{path:.*}` | 代理到插件 API |
| POST | `/api/shutdown` | 优雅关闭 |

## 插件配置（schema 驱动 + WebUI）

每个插件可以声明一份配置 schema，宿主据此提供读取、校验、WebUI 表单与保存后重载。

### 存储与合并

- 配置存放在 `data/plugin_configs/<plugin-name>.json`（`PLUGIN_CONFIG_DIR` 可覆盖），
  `<plugin-name>` 必须等于 `PluginInfo.name`。
- 文件内容为扁平 JSON 对象：`{"field": value, ...}`；文件不存在/损坏/字段非法都不会阻止插件加载，
  宿主会用 schema 默认值补齐并记录警告。
- 插件在 `start(ctx)` 中通过 `ctx.config`（`serde_json::Value`）读取已合并默认值的配置。
- 保存配置后宿主自动 reload，插件 `start()` 重新读取 `ctx.config` 即生效。
  宿主会校验「插件实例已运行 + 旧实例注册过的工具在新实例中仍注册」；任一不满足即视为失败：
  回滚配置文件、用旧配置重新拉起旧实例，并返回 500 + `config_rolled_back` / `plugin_restored`，
  不会出现「接口 200 但插件静默失效」的半加载状态。
- 插件 `start()` 应把内部初始化失败作为 `Err` 返回；宿主另有工具校验作为兜底。
- Windows 上立即卸载「启动失败」的 cdylib 可能触发访问冲突，宿主在失败路径会保留该库句柄
  （进程内泄漏一次，换取稳定）；因此插件启动/reload 失败后若要重新编译该插件 DLL，
  需重启 main-app 释放文件锁。

### schema 约定 v1

```json
{
  "version": 1,
  "title": "ComfyUI Qwen 生图",
  "description": "本地 Qwen-Image 出图参数",
  "fields": {
    "server_url":  {"type": "string", "default": "http://127.0.0.1:8188", "description": "ComfyUI 地址"},
    "prompt_neg":  {"type": "text",   "default": "", "description": "默认负面提示词"},
    "enabled":     {"type": "bool",   "default": true},
    "timeout":     {"type": "int",    "default": 300, "min": 10, "max": 3600},
    "cfg":         {"type": "float",  "default": 1.0, "min": 0, "max": 30},
    "model":       {"type": "select", "default": "qwen-image", "options": ["qwen-image", "qwen-image-edit"]},
    "tags":        {"type": "list",   "default": []},
    "api_key":     {"type": "string", "default": "", "secret": true}
  }
}
```

| 字段 | 说明 |
|---|---|
| `type` | `string` / `text` / `bool` / `int` / `float` / `select` / `list`（字符串数组） |
| `default` | 默认值；缺省按类型生成（select 取第一个 option） |
| `description` | WebUI 说明文字 |
| `options` | `select` 必填候选值 |
| `secret` | true 时 GET 打码为 `******`；PUT 回传 `******` 或省略=保持原值，空串=清空 |
| `min` / `max` | `int` / `float` 范围校验 |
| `placeholder` | 输入框占位符 |

### 新增插件接入

```rust
use std::collections::BTreeMap;
use plugin_interface::*;

impl Plugin for MyPlugin {
    fn config_schema(&self) -> Option<PluginConfigSchema> {
        Some(PluginConfigSchema {
            title: "我的插件".into(),
            description: "示例配置".into(),
            fields: BTreeMap::from([
                ("url".into(), ConfigField {
                    field_type: ConfigFieldType::String,
                    default: serde_json::json!("http://127.0.0.1:9000"),
                    description: "服务地址".into(),
                    ..Default::default()
                }),
                ("api_key".into(), ConfigField {
                    field_type: ConfigFieldType::String,
                    secret: true,
                    ..Default::default()
                }),
            ]),
            ..Default::default()
        })
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let url = ctx.config.get("url").and_then(|v| v.as_str())
            .unwrap_or("http://127.0.0.1:9000");
        // 建议保留原环境变量作为 fallback，兼容旧部署
        Ok(())
    }
}
```

没有实现 `config_schema()`（返回默认 `None`）的插件完全兼容：宿主注入空配置/原样透传
`data/plugin_configs/<name>.json`，WebUI 中显示为不可编辑，加载与重载不受影响。

### API 示例

```bash
# 列出可配置插件
curl -H "X-API-Key: $API_KEY" http://127.0.0.1:8080/api/plugin-config
# 读取（secret 已打码）
curl -H "X-API-Key: $API_KEY" http://127.0.0.1:8080/api/plugin-config/comfyui-qwen-plugin
# 保存并重载（****** 表示保持原 secret）
curl -X PUT -H "X-API-Key: $API_KEY" -H 'Content-Type: application/json' \
  -d '{"values":{"server_url":"http://127.0.0.1:8188","timeout_secs":300}}' \
  http://127.0.0.1:8080/api/plugin-config/comfyui-qwen-plugin
# 手动重载
curl -X POST -H "X-API-Key: $API_KEY" \
  http://127.0.0.1:8080/api/plugin-config/comfyui-qwen-plugin/reload
```

浏览器打开 `http://127.0.0.1:18080/ui/config`（别名 `/config`）：输入 `API_KEY` → 选择插件 →
schema 驱动表单 → 保存并重载。页面本身不包含任何 secret；接口仍受 `API_KEY` 保护，
调用方式为 `X-API-Key: <key>` 或 `Authorization: Bearer <key>`，本机 key 见 `.secrets/api_key.txt`。
> 本机 `.env` 中 `HTTP_BIND=0.0.0.0:18080`（8080 被其它服务占用，且手机需局域网访问），
> 因此实际地址为 `http://127.0.0.1:18080/ui/config`；已启用 `API_KEY`，局域网访问需携带同一 key。

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

## Android 客户端（拾语）

`android-app/` 是「拾语」Android 16 原生客户端（Kotlin + Jetpack Compose + Material 3），
通过 `POST /api/chat` 的多会话契约（`peer_id = android:<convId>`）连接本仓库的后端。

- 构建 / 安装、服务器与 API Key 配置、adb reverse 联调、架构与功能清单：见 [`android-app/README.md`](android-app/README.md)
- UI 设计依据：[`android-app/docs/design-spec.md`](android-app/docs/design-spec.md)

## 项目结构

```
bn-agent/
├── plugin-interface/        # 共享契约：Plugin trait, ToolRegistry, EventBus
├── main-app/                # 主程序入口，HTTP API，Actor 系统
│   ├── src/
│   │   ├── main.rs          # 启动入口 + HTTP 路由
│   │   ├── llm_actor.rs     # LLM API 封装 + 流式响应
│   │   ├── pipeline.rs      # 工具调用循环编排
│   │   ├── plugin_manager.rs# 插件加载管理
│   │   ├── plugin_config.rs # 插件配置服务（校验/打码/回滚）
│   │   ├── chat_store.rs    # 对话历史 SQLite
│   │   ├── retry_actor.rs   # 重试+熔断
│   │   ├── token_usage_actor.rs  # Token 用量追踪
│   │   ├── rate_limit_actor.rs   # 频率限制
│   │   ├── metrics_actor.rs      # 结构化观测
│   │   └── cancellation_actor.rs # 请求取消
│   └── static/config.html   # 插件配置 WebUI（单文件，免构建）
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
│   ├── proactive-plugin/     # 主动消息推送
│   ├── memory-plugin/        # 长期记忆提取（Engram 风格）
│   └── toy-control-plugin/   # 跳蛋远程控制 + Web 面板
└── data/
    ├── skills/               # Skill .md 文件目录
    ├── plugin_configs/       # 插件配置 JSON（每个插件一个文件）
    └── jailbreak_prompts.csv # 提示词注入（已 gitignore）
```
