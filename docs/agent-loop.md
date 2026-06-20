# Agent Loop MVP

Agent Loop 是显式目标驱动的后台循环，不替代普通聊天链路。普通聊天仍由 `PipelineActor` 处理；Agent Loop 由 HTTP API 创建一个目标任务，然后按预算执行 observe -> decide -> act。

## 能力边界

当前 MVP 已支持：

- 接收一个 goal 并创建独立 loop id
- 每步刷新当前 peer 的插件 snapshot
- 让 LLM 决策下一步，并可调用已注册工具
- 将工具结果作为 observation 喂回后续步骤
- 记录每一步的状态、LLM 消息、工具调用、工具结果和耗时
- 支持最大 step 数和最大 tool round 数
- 支持查询、列表和停止
- 发布 `agent.loop.step` 与 `agent.loop.done` 事件

当前 MVP 暂不支持：

- loop 状态持久化和重启恢复
- 暂停/恢复
- 多目标优先级队列
- plan/task tree
- 长期反思压缩

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

## 状态

- `running`：loop 正在执行
- `completed`：目标完成
- `waiting_for_user`：需要用户补充信息
- `stopping`：已收到停止请求，等待当前步结束
- `stopped`：已停止
- `failed`：LLM、工具调用链或预算耗尽导致失败

## 配置

- `max_steps`：单个 loop 的最大步骤数，默认 8，硬上限 50
- `max_tool_rounds`：单步最大工具调用轮数，默认 5，硬上限 20
- `AGENT_LOOP_MAX_SLEEP_SECS`：LLM 决策 sleep 时的单步上限，默认 60 秒

## 设计关系

`PipelineActor` 是一次用户消息内的工具调用循环；`AgentLoopActor` 是跨步骤的目标循环。它复用同一套 `RetryActor`、`ToolRegistry`、`PluginManager`、`TokenUsageActor` 和 `MetricsActor`，因此不需要复制 LLM 或工具基础设施。
