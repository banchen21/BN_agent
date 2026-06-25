# Agent Loop MVP

Agent Loop 是显式目标驱动的后台循环，不替代普通聊天链路。普通聊天仍由 `PipelineActor` 处理；Agent Loop 由 HTTP API 创建一个目标任务，然后按预算执行 observe -> decide -> act。

## 能力边界

当前 MVP 已支持：

- 接收一个 goal 并创建独立 loop id
- 每步刷新当前 peer 的插件 snapshot
- 让 LLM 决策下一步，并可调用已注册工具
- 将工具结果作为 observation 喂回后续步骤
- 记录每一步的状态、LLM 消息、工具调用、工具结果和耗时
- 维护 plan/task tree（支持嵌套任务与 pending/in_progress/done/blocked/skipped 状态）
- 记录每一步的 step reflection，并反馈给后续步骤
- 记录每一步的 correction；检测到工具错误、预算耗尽或 failed observation 后，会在下一步提示中强制要求自我修正、更新计划并避免原样重复失败动作
- 支持最大 step 数和最大 tool round 数
- 支持查询、列表、停止、暂停与恢复
- 发布 `agent.loop.step` 与 `agent.loop.done` 事件
- loop 状态持久化到 SQLite，进程重启后恢复历史（运行中的 loop 标记为中断）
- 终态 loop 自动清理（内存与 DB 只保留最近 `AGENT_LOOP_MAX_KEEP` 个，默认 200）
- 事件驱动启动：任何插件发 `agent.loop.start` 事件（data: `{goal, peer_id?, max_steps?, max_tool_rounds?}`）即可启动 loop

当前 MVP 暂不支持：

- 多目标优先级队列
- LLM 级长期反思/摘要压缩

## 失败自我修正

Agent Loop 会扫描历史 observation 中的失败信号，例如工具返回 `错误：...`、`failed` 状态、timeout、预算耗尽或工具轮次达到上限。检测到失败后，下一步 prompt 会加入“失败自我修正要求”：

- 必须在 JSON 中填写 `correction`，说明失败原因与修正策略。
- 必须更新 `plan`，把失败分支标为 `blocked` / `skipped`，或写出替代路径。
- 不要原样重复刚失败的工具调用，除非 `correction` 明确说明改变了参数、前置条件或验证方式。
- 除非目标逻辑上不可能，否则优先 `continue` 或 `ask_user`，不要直接 `failed`。

`AgentLoopStep`、SQLite 快照和 `agent.loop.step` 事件都会携带 `correction` 字段，便于 UI 或日志展示修正轨迹。

## HTTP API

启动：

```bash
curl -X POST http://127.0.0.1:8080/api/agent-loop/start \
  -H "Content-Type: application/json" \
  -d '{"goal":"整理最近一次对话里提到的待办，并给出下一步建议","peer_id":"web:default","max_steps":6,"max_tool_rounds":5}'
```

列表：

```bash
curl http://127.0.0.1:8080/api/agent-loop/list
```

查询：

```bash
curl http://127.0.0.1:8080/api/agent-loop/status/{id}
```

停止：

```bash
curl -X POST http://127.0.0.1:8080/api/agent-loop/stop/{id}
```

暂停 / 恢复：

```bash
curl -X POST http://127.0.0.1:8080/api/agent-loop/pause/{id}
curl -X POST http://127.0.0.1:8080/api/agent-loop/resume/{id}
```

## 状态

- `running`：loop 正在执行
- `paused`：已暂停，runner 在步骤边界等待恢复（`resume` 后继续）
- `completed`：目标完成
- `waiting_for_user`：需要用户补充信息
- `stopping`：已收到停止请求，等待当前步结束
- `stopped`：已停止
- `failed`：LLM、工具调用链或预算耗尽导致失败

## 配置

- `max_steps`：单个 loop 的最大步骤数，默认 8，硬上限 50
- `max_tool_rounds`：单步最大工具调用轮数，默认 5，硬上限 20
- `AGENT_LOOP_MAX_SLEEP_SECS`：LLM 决策 sleep 时的单步上限，默认 60 秒
- `AGENT_LOOP_DB_PATH`：loop 状态持久化路径，默认 `data/agent_loops.db`；进程重启时残留的 `running`/`stopping`/`paused` 会被标记为 `failed`（`interrupted by process restart`）
- `AGENT_LOOP_MAX_KEEP`：终态 loop 保留上限，默认 200；每次新建 loop 后清理更旧的终态记录（仅 Completed/Stopped/Failed，活跃 loop 不受影响）

## 设计关系

`PipelineActor` 是一次用户消息内的工具调用循环；`AgentLoopActor` 是跨步骤的目标循环。它复用同一套 `RetryActor`、`ToolRegistry`、`PluginManager`、`TokenUsageActor` 和 `MetricsActor`，因此不需要复制 LLM 或工具基础设施。
