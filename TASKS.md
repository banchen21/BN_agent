# BN Agent — 项目状态与待办

## 当前阶段：功能完备 beta → 向成熟版 v1.1 推进 🟢

```
核心架构      ██████████ 完成
多 IM 接入    ██████████ 完成（Telegram / 飞书 / 微信）
多模态        ██████████ 完成（图片 / 视频 / ASR / TTS）
工具系统      ██████████ 完成（MCP 桥接 + Skill 系统）
Agent Loop    █████████░ 较完整（循环 + 持久化 + 暂停恢复 + 清理）
重试/熔断     ██████████ 完成
Token 用量    ██████████ 完成
流式响应      ██████████ 完成
频率限制      ██████████ 完成
请求取消      ██████████ 完成
结构化观测    ██████████ 完成（Prometheus 指标）
```

### 最新进展（2026-06-22）

- [x] **P0 健壮性收口** — parking_lot 迁移（消除跨 cdylib 共享锁中毒级联）、熔断状态持久化、Agent Loop 状态持久化三项 P0 全部落地
- [x] **Agent Loop MVP 已落地** — `8950b7d feat(agent-loop): add goal loop actor mvp`，新增目标循环 actor、HTTP 控制接口、step observation 与事件发布
- [x] **主动系统升级完成** — 已从定时提醒扩展到带 jitter / probability / daily cap / backoff 的自主主动触发
- [x] **HTTP API 可选鉴权** — `API_KEY` 中间件（`from_fn`），未设=放行；`/api/health` 免鉴权；支持 `Authorization: Bearer <key>` 与 `X-API-Key`；`check_api_key` 纯函数 5 测试
- [x] **Agent Loop 事件驱动启动** — 任意插件发 `agent.loop.start` 事件即可启动 loop（与 HTTP/消息入口共用 `start_loop_internal`）
- [x] **Agent Loop 资源护栏三件套** — 工具 denylist（`AGENT_LOOP_TOOL_DENY`）+ 墙钟时长上限（`AGENT_LOOP_MAX_DURATION_SECS`）+ 并发上限（`AGENT_LOOP_MAX_CONCURRENT`）
- [x] **结构化健康检查** — `GET /api/health` 返回 version/uptime_secs/plugins_loaded/agent_loops_total（免鉴权，便于探活/监控）
- [ ] **下一阶段重点** — Agent Loop 规划器（plan/task tree + 反思）、流式推送至 IM、会话管理（标题/摘要/回收）

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
- [x] **HTTP API 鉴权** — 可选 API key 中间件（`API_KEY`，未设=不鉴权；`/api/health` 免鉴权），支持 `Bearer`/`X-API-Key`，`check_api_key` 纯函数已测（以中间件实现，非独立插件）

### 架构类

- [x] **Claude CLI 后端** — `LLM_BACKEND=claude` 用 `--resume` 复用原生会话，工具提示词注入
- [x] **Agent Loop MVP** — `AgentLoopActor` 支持目标启动、observe/decide/act 循环、工具调用、step observation、状态查询与停止
- [ ] **流式推送至 IM** — 将 `llm.chunk` 事件转发到 Telegram/飞书/微信
- [x] **Agent Loop 持久化队列** — loop 状态落 SQLite，支持重启恢复、按 peer 归档（`AGENT_LOOP_DB_PATH`）；终态自动清理（`AGENT_LOOP_MAX_KEEP`，默认保留 200）
- [x] **Agent Loop 暂停/恢复** — 新增 `paused` 状态 + `pause/resume` 消息与 HTTP API；runner 在步骤边界暂停等待，心跳不覆盖外部状态
- [x] **Agent Loop 工具护栏** — `AGENT_LOOP_TOOL_DENY` 黑名单：denied 工具对 LLM 不可见 + 执行前拒绝（纵深防御），防止自主 loop 误调危险工具
- [x] **Agent Loop 墙钟时长上限** — `AGENT_LOOP_MAX_DURATION_SECS` 步间检查总耗时，超限标记 failed（与 max_steps 互补，防失控消耗），0=不限
- [x] **Agent Loop 并发上限** — `AGENT_LOOP_MAX_CONCURRENT` 限制同时运行的 loop 数，达上限拒绝启动（与工具护栏 / 时长上限构成资源护栏三件套），0=不限
- [ ] **Agent Loop 规划器** — 引入 plan/task tree、step reflection、失败自我修正策略
- [ ] **Agent Loop 与主动系统联动** — 事件驱动启动已落地（任何插件发 `agent.loop.start` 即可启动 loop）；proactive 侧实际触发待做
- [x] **LLM 重试持久化** — 熔断状态重启后保持（`CIRCUIT_BREAKER_DB_PATH`）
- [x] **Token 预算控制** — 滚动窗口（日 24h/周 7d/月 30d）token 上限（`TOKEN_BUDGET_DAILY/WEEKLY/MONTHLY`）；pipeline 前置拦截超限请求 + 提示；`GET /api/token-usage/budget` 查询
- [ ] **会话管理** — 对话标题、自动摘要（历史回收已落地：`CHAT_HISTORY_MAX_PER_PEER` 按 peer 保留上限，append 时清理最旧）
- [x] **工具调用超时** — per-tool 超时控制（`TOOL_TIMEOUT_SECS`，默认 180s）；工具改在 blocking 池执行，避免同步工具阻塞 actix arbiter
- [ ] **速率限制提升** — 支持 IP 级别限流 + 分布式（Redis）
- [ ] **插件沙箱** — Wasm / Lua 沙箱运行插件

### 测试类

- [x] **单元测试** — 核心纯逻辑 + AgentLoopActor handler 级（mock 依赖注入）+ MessageRouter 路由解析 + 工具超时 + token 预算 + loop 清理 ～72 个测试全绿；PipelineActor / LlmActor handler 级待补
- [x] **集成测试** — mock LLM 驱动 Agent Loop 端到端（observe→decide→act 跑完整 loop 至 Completed）
- [x] **插件测试框架** — `PluginContext::for_test` 提供最小可注入上下文（plugin-interface）

---

## 稳定化路线（→ 成熟版 v1.1）

> 现状：架构定型、功能完备的活跃迭代期（beta）。要达到生产级成熟版，按下列优先级推进。

### P0 — 正确性与健壮性（阻塞成熟版）

- [x] **多 Peer 关系隔离** — `peer_id` 已贯穿 IM 插件 → pipeline → ChatRequest → chat_store → memory；按人隔离对话历史与记忆，首个互动者自动绑定为主人。详见下方「多 Peer 关系」设计
- [x] **Agent Loop 状态持久化** — `AgentLoopActor` 状态落 SQLite（`data/agent_loops.db`，整快照 JSON），重启恢复历史；残留 running/stopping 标记为中断
- [x] **收敛 panic 面** — main-app 全部 + plugin-interface 共享 `ToolRegistry` 锁迁至 `parking_lot::Mutex`（不中毒）；插件私有 std 锁（局部不级联）与 `token_usage_actor` 优雅降级保留
- [x] **熔断状态持久化** — `RetryActor` 熔断状态落 SQLite（`data/circuit_breaker.db`），重启保持 open/half-open；冷却已过的 open 恢复为 half-open

### P1 — 测试体系（质量保障，详见上方「测试类」）

- [x] Agent Loop 单测：纯逻辑（决策/状态/格式化/持久化）+ actor 级（clamp/默认值、停止请求终止、get/list、空 goal 拒绝）
- [x] `ToolRegistry` 注册-调用链单测（plugin-interface）；PipelineActor / LlmActor actor 级单测待补
- [x] 集成测试：mock LLM 驱动 AgentLoopActor 端到端跑完整 loop（`loop_completes_with_mock_llm`）
- [x] 插件测试框架：`PluginContext::for_test`（plugin-interface）+ 示例插件工具注册测试
- [x] CI 门禁：`.github/workflows/ci.yml`（windows runner）build + test 硬门禁；clippy 信息性（待历史 warning 清零后升级 `-D warnings`）

### P2 — 运维与打磨

- [x] 工具调用 per-tool 超时控制（`TOOL_TIMEOUT_SECS`，spawn_blocking + timeout，不阻塞 arbiter）
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
- **主动插件扩展为自主主动系统** — 原先只能处理用户/LLM 安排后的定时任务。解决方案：proactive-plugin 记录 peer 最近互动，为每个 peer 计算带 jitter/probability/daily limit/backoff 的下一次自主主动机会，到点后自主发布 `proactive.trigger(reason=autonomous_idle)`；Pipeline 使用自主主动提示词回调 LLM，允许返回内部跳过标记避免打扰
- **缺少完整 Agent Loop 模式** — 原先只有 Pipeline 单次对话工具循环和 proactive tick。解决方案：新增 `AgentLoopActor`，通过 `/api/agent-loop/start` 接收目标，执行 bounded observe→decide→act 循环，记录每步 observation，并提供 list/status/stop 控制接口
- **重启后短期聊天历史丢失** — 主程序曾在启动时无条件清空 `chat_history`。解决方案：默认保留历史，仅当 `CHAT_HISTORY_CLEAR_ON_START=true` 时才执行清空
- **微信回复整段发送** — 不像真人。解决方案：按换行/句末标点分句逐条发送（与 tg-im 一致），段间延时防限频
- **多人接入共享历史/记忆** — chat_history 与 memory 曾全局单桶。解决方案：引入 `peer_id={source}:{平台内id}`，历史按 peer 读写；memory-plugin 的 buffer/facts/snapshot 按 peer 分桶；首个互动者持久化为主人并注入 owner/visitor 关系守则
- **锁中毒级联 panic 风险** — 满屏 `std::sync::Mutex` + `.lock().unwrap()`，单线程 panic 会毒化锁级联崩溃。解决方案：plugin-interface `pub use parking_lot::{Mutex, RwLock}` 统一跨 cdylib 共享锁；main-app 全量迁移；插件共享 `ctx.tool_registry` 访问改 `.lock()`；插件私有 std 锁（局部不级联）与 `token_usage_actor` 优雅降级保留
- **熔断 / Agent Loop 状态重启丢失** — 二者原先仅存内存。解决方案：分别持久化到 `data/circuit_breaker.db` 与 `data/agent_loops.db`，启动恢复；熔断冷却已过的 open 恢复为 half-open；Agent Loop 残留 running/stopping 标记为中断（interrupted by process restart）
- **同步工具阻塞 / 无超时** — 工具 `execute()` 同步执行会阻塞 actix arbiter 线程，且卡死的工具无限挂起。解决方案：新增 `tool_exec::execute_with_timeout`，经 `spawn_blocking` 移到 blocking 池执行（工具已设计为线程无关、内部自建 runtime）+ `TOOL_TIMEOUT_SECS` 超时（默认 180s，0 禁用）；pipeline 与 agent loop 工具执行均接入

---

## 构建状态

```bash
cargo build              # 编译全部
cargo build -p main-app  # 仅主应用
cargo build -p <plugin>  # 仅某个插件
```
