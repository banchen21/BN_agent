# BN Agent — 项目状态与待办

## 当前阶段：功能完备 beta → 向成熟版 v1.1 推进 🟢

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
- [x] **proactive-plugin** — 主动消息推送（工具驱动调度 + 到期回调 LLM 实时生成）
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

## 稳定化路线（→ 成熟版 v1.1）

> 现状：架构定型、功能完备的活跃迭代期（beta）。要达到生产级成熟版，按下列优先级推进。

### P0 — 正确性与健壮性（阻塞成熟版）

- [x] **多 Peer 关系隔离** — `peer_id` 已贯穿 IM 插件 → pipeline → ChatRequest → chat_store → memory；按人隔离对话历史与记忆，首个互动者自动绑定为主人。详见下方「多 Peer 关系」设计
- [ ] **收敛 panic 面** — 关键路径 `.lock().unwrap()` 改用 `parking_lot::Mutex`（不中毒）或优雅降级，避免单线程 panic 级联崩溃
- [ ] **熔断状态持久化** — 重启后保持 open/half-open，避免雪崩后立即重试

### P1 — 测试体系（质量保障，详见上方「测试类」）

- [ ] 单元测试：PipelineActor / LlmActor / 工具注册-调用链
- [ ] 集成测试：mock LLM 端点跑端到端工具执行
- [ ] 插件测试框架：模拟 PluginContext
- [ ] CI 门禁：`cargo build` + `clippy -D warnings` + `test`

### P2 — 运维与打磨

- [ ] 工具调用 per-tool 超时控制
- [ ] 微信图片/语音真实环境端到端联调验证
- [ ] ASR 调用链整体超时（ffmpeg 管道 + API）
- [ ] 流式工具调用参数解析兼容非标准分块

---

## 多 Peer 关系（v1.1 核心设计）

> 目标：同一个人格的「她」，能同时与多个人各自维护独立的对话历史与记忆；对主人绝对忠诚，对陌生人克制、由她掌握关系主动权。

**已定决策**：
- 主人 = 首个互动者，自动绑定并持久化
- 对陌生人：克制维护，不延续与主人的私密 / NSFW，关系推进由她主导
- 深度：仅按 peer 隔离历史 + 记忆（暂不做亲密度数值演化）
- 贯穿标识：`peer_id = {source}:{平台内唯一id}`（如 `telegram:123`、`wechat:wxid_x`）

### 阶段 1 — MVP（核心隔离）
- [x] plugin-interface：`user.message` 事件 + `ChatRequest` 增加 `peer_id`
- [x] 各 IM 插件（tg / 微信 / 飞书）发 `user.message` 时携带稳定 `peer_id`
- [x] pipeline 提取 `peer_id` 并透传到 `ChatRequest` 与存储
- [x] chat_history 表加 `peer_id` 列；`FetchRecent` 按 `peer_id` 过滤
- [x] 主人绑定：首个 peer 自动设为 owner 并持久化
- [x] 系统提示按 owner / 非 owner 注入关系指引（忠诚 vs 克制）

### 阶段 2 — 记忆隔离
- [x] memory-plugin 的 buffer / facts 按 `peer_id` 分桶
- [x] 解决 `snapshot()` 感知「当前 peer」（新增 `snapshot_for_peer`，主流程按当前 peer 刷新快照）

### 阶段 3 — 关系深化（暂缓）
- [ ] 关系标签 / 称呼、亲密度演化

---

## 已知问题

- **流式工具调用** — 部分模型在流式模式下分 chunk 发送 function call 参数，非标准实现可能解析异常
- **熔断状态非持久化** — 重启后重置为 closed
- **ASR 偶发超时** — ffmpeg 管道 + API 调用链缺乏整体超时控制
- **ComfyUI 依赖外部启动** — 生图需要独立运行 ComfyUI 服务

## 已解决的问题

- **DeepSeek 思考模式与工具调用冲突** — `thinking: enabled` 会导致后续请求忽略工具。解决方案：默认 `LLM_THINKING=disabled`，在 system prompt 最前面插入工具感知提示
- **响应被拆成多条 Telegram 消息** — 模型输出含 `\n` 导致视觉换行。解决方案：tg-im-plugin 发送前 `.replace('\n', "")`
- **工具调用后对话历史断裂** — 工具调用结果未存储导致连续两条 user 消息。解决方案：`original_user_msg` 字段 + follow-up 回复关联原始消息存储
- **Persona 覆盖工具意识** — NSFW 角色扮演过强导致模型忽略工具。解决方案：system prompt 顺序调整为 persona → jailbreak → tool_hint（近因效应）
- **生图后 image_describe 竞态** — pipeline 在 EventBus 异步缓存前调用 image_describe。解决方案：ToolResult.metadata 同步传递 base64
- **回复忽冷忽热** — 采样温度每次请求随机 `gen_range(0.7..=1.2)`，破限词仅多模态时随机注入，人格漂移。解决方案：固定温度 `LLM_TEMPERATURE`（默认 0.8）+ 破限词固定索引 `JAILBREAK_INDEX`（默认 0，可设 random 恢复随机）
- **generate_image 默认发到 TG** — tg-im 硬编码订阅 `image.gen.complete` 自动发图且不校验来源，跨平台误发。解决方案：移除自动发图订阅，`generate_image` 返回本地 file_path，由 LLM 按当前平台调 `tg_send_photo`/`wechat_send_image` 发送
- **主动消息机制重构** — 旧机制靠 `[SCHEDULE:N]` 文本标签，易被 IM 发送工具剥离而不触发。解决方案：改为工具驱动（`proactive_schedule_once`/`recurring`）+ 到期发 `proactive.trigger` 回调 LLM 按当前上下文实时生成
- **主动插件扩展为自主主动系统** — 原先只能处理用户/LLM 安排后的定时任务。解决方案：proactive-plugin 记录 peer 最近互动，超过 `PROACTIVE_AUTONOMOUS_IDLE_SECS` 且满足冷却/最少消息条件后自主发布 `proactive.trigger(reason=autonomous_idle)`；Pipeline 使用自主主动提示词回调 LLM，允许返回内部跳过标记避免打扰
- **微信回复整段发送** — 不像真人。解决方案：按换行/句末标点分句逐条发送（与 tg-im 一致），段间延时防限频
- **多人接入共享历史/记忆** — chat_history 与 memory 曾全局单桶。解决方案：引入 `peer_id={source}:{平台内id}`，历史按 peer 读写；memory-plugin 的 buffer/facts/snapshot 按 peer 分桶；首个互动者持久化为主人并注入 owner/visitor 关系守则

---

## 构建状态

```bash
cargo build              # 编译全部
cargo build -p main-app  # 仅主应用
cargo build -p <plugin>  # 仅某个插件
```
