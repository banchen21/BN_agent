# BN Agent — 项目状态与待办

## 当前阶段：基础功能完备 🟢

```
核心架构      ██████████ 完成
多 IM 接入    ██████████ 完成（Telegram / 飞书 / 微信）
多模态        ██████████ 完成（图片 / 视频 / ASR / TTS）
工具系统      ██████████ 完成（MCP 桥接 + Skill 系统）
重试/熔断     ██████████ 完成
Token 用量    ██████████ 完成
流式响应      ██████████ 完成
频率限制      ██████████ 完成
请求取消      ██████████ 完成
结构化观测    ██████████ 完成（Prometheus 指标）
```

---

## 已完成功能

### 基础设施（10/10）

- [x] **重试+熔断** — 指数退避重试 + Circuit Breaker（closed/open/half-open）
- [x] **Token 用量追踪** — SQLite 持久化 per-chat/per-model 用量
- [x] **流式响应** — SSE 流式输出，逐 chunk 发送到 EventBus
- [x] **请求取消** — 按 chat_id 取消进行中的 LLM 请求
- [x] **频率限制** — 令牌桶 per-chat_id 限流
- [x] **结构化观测** — Prometheus 格式指标（延迟/成功率/调用计数）
- [x] **LLM 多模态路由** — 图片/视频自动切换专用模型
- [x] **DeepSeek 思考模式开关** — LLM_THINKING 环境变量控制 thinking type
- [x] **可配置 max_tokens** — LLM_MAX_TOKENS 环境变量（默认 384000）
- [x] **工具调用稳定性** — tool_choice:auto + system prompt 工具感知提示，解决 persona 覆盖工具意识

### 插件（19/19）

- [x] **hello-plugin** — 演示
- [x] **logger-plugin** — 日志
- [x] **time-plugin** — 时间注入
- [x] **tg-im-plugin** — Telegram Bot
- [x] **feishu-im-plugin** — 飞书 Bot
- [x] **wechat-claw-plugin** — 微信 Bot
- [x] **claude-bridge-plugin** — Claude CLI 桥接
- [x] **asr-tts-plugin** — 语音识别 + 合成
- [x] **audio-capture-plugin** — 麦克风音频捕获
- [x] **webrtc-plugin** — WebRTC P2P 通信
- [x] **image-plugin** — 图片处理 / 理解
- [x] **image-gen-plugin** — 本地 SD 生图 (ComfyUI)
- [x] **video-plugin** — 视频分析
- [x] **tui-plugin** — 终端聊天界面
- [x] **mcp-plugin** — MCP 服务器桥接
- [x] **skill-plugin** — Markdown Skill 工具
- [x] **proactive-plugin** — 主动消息推送
- [x] **memory-plugin** — 长期记忆提取（Engram 风格双时态 + 时间分桶）
- [x] **toy-control-plugin** — 跳蛋远程控制 + 内置 Web 面板

---

## 下一步计划

### 插件类

- [ ] **web-search-plugin** — 搜索引擎（DuckDuckGo / SearXNG）
- [ ] **web-fetch-plugin** — URL 内容抓取
- [ ] **exec-plugin** — 安全沙箱代码执行（Python/JS）
- [ ] **fs-plugin** — 文件系统读写
- [x] **memory-plugin** — 长期记忆（Engram 双时态 + 时间分桶 + 矛盾追踪）
- [ ] **db-plugin** — 数据库查询（SQLite / PostgreSQL）
- [ ] **weather-plugin** — 天气查询
- [ ] **auth-plugin** — HTTP API 鉴权

### 架构类

- [x] **Claude CLI 后端** — `LLM_BACKEND=claude` 用 `--resume` 复用原生会话，工具提示词注入
- [ ] **流式推送至 IM** — 将 `llm.chunk` 事件转发到 Telegram/飞书/微信
- [ ] **LLM 重试持久化** — 熔断状态重启后保持
- [ ] **Token 预算控制** — 按天/周/月设置 token 上限
- [ ] **会话管理** — 对话标题、自动摘要、长时间未活动会话回收
- [ ] **工具调用超时** — per-tool 超时控制
- [ ] **速率限制提升** — 支持 IP 级别限流 + 分布式（Redis）
- [ ] **插件沙箱** — Wasm / Lua 沙箱运行插件

### 测试类

- [ ] **单元测试** — 每个 actor 的核心逻辑
- [ ] **集成测试** — 端到端 LLM 调用 + 工具执行
- [ ] **插件测试框架** — 模拟 PluginContext 测试插件

---

## 已知问题

- **流式工具调用** — 部分模型在流式模式下分 chunk 发送 function call 参数，非标准实现可能解析异常
- **熔断状态非持久化** — 重启后重置为 closed
- **ASR 偶发超时** — ffmpeg 管道 + API 调用链缺乏整体超时控制
- **chat_history 全局混存** — 不区分 chat_id，多会话可能混入历史
- **ComfyUI 依赖外部启动** — 生图需要独立运行 ComfyUI 服务

## 已解决的问题

- **DeepSeek 思考模式与工具调用冲突** — `thinking: enabled` 会导致后续请求忽略工具。解决方案：默认 `LLM_THINKING=disabled`，在 system prompt 最前面插入工具感知提示
- **响应被拆成多条 Telegram 消息** — 模型输出含 `\n` 导致视觉换行。解决方案：tg-im-plugin 发送前 `.replace('\n', "")`
- **工具调用后对话历史断裂** — 工具调用结果未存储导致连续两条 user 消息。解决方案：`original_user_msg` 字段 + follow-up 回复关联原始消息存储
- **Persona 覆盖工具意识** — NSFW 角色扮演过强导致模型忽略工具。解决方案：system prompt 顺序调整为 persona → jailbreak → tool_hint（近因效应）
- **生图后 image_describe 竞态** — pipeline 在 EventBus 异步缓存前调用 image_describe。解决方案：ToolResult.metadata 同步传递 base64

---

## 构建状态

```bash
cargo build              # 编译全部
cargo build -p main-app  # 仅主应用
cargo build -p <plugin>  # 仅某个插件
```
