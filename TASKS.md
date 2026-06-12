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

### 基础设施（7/7）

- [x] **重试+熔断** — 指数退避重试 + Circuit Breaker（closed/open/half-open）
- [x] **Token 用量追踪** — SQLite 持久化 per-chat/per-model 用量
- [x] **流式响应** — SSE 流式输出，逐 chunk 发送到 EventBus
- [x] **请求取消** — 按 chat_id 取消进行中的 LLM 请求
- [x] **频率限制** — 令牌桶 per-chat_id 限流
- [x] **结构化观测** — Prometheus 格式指标（延迟/成功率/调用计数）
- [x] **LLM 多模态路由** — 图片/视频自动切换专用模型

### 插件（15/15）

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
- [x] **image-plugin** — 图片处理
- [x] **video-plugin** — 视频分析
- [x] **tui-plugin** — 终端聊天界面
- [x] **mcp-plugin** — MCP 服务器桥接
- [x] **skill-plugin** — Markdown Skill 工具

---

## 下一步计划

### 插件类

- [ ] **web-search-plugin** — 搜索引擎（DuckDuckGo / SearXNG）
- [ ] **web-fetch-plugin** — URL 内容抓取
- [ ] **exec-plugin** — 安全沙箱代码执行（Python/JS）
- [ ] **fs-plugin** — 文件系统读写
- [ ] **memory-plugin** — 长期记忆 / RAG（向量存储 + 语义搜索）
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

- **流式工具调用（streaming tool_calls）** — 部分模型在流式模式下会分 chunk 发送 function call 参数，当前实现依赖 OpenAI 格式，非标准实现可能解析异常
- **熔断状态非持久化** — 重启后重置为 closed，可能被上游 API 持续故障触发频繁重试
- **图片/视频模型专用配置** — 需要独立配置 `IMAGE_MODEL` / `VIDEO_MODEL`，增加了部署复杂度
- **链式工具调用** — 当前只支持一轮工具调用 + 结果回送，不支持多轮链式调用

---

## 构建状态

```bash
cargo build              # 编译全部
cargo build -p main-app  # 仅主应用
cargo build -p <plugin>  # 仅某个插件
```
