# 主动插件 — 工具驱动的主动消息和提醒

## 核心原则

主动插件不直接调用 LLM，也不解析 `[SCHEDULE:N]` 文本标签。它只负责注册调度工具、保存定时任务，并在任务到期后发布触发事件。

LLM 仍然可以主动调用工具来安排未来消息：

- `proactive_schedule_once`：一次性主动消息或定时提醒
- `proactive_schedule_recurring`：循环主动消息

此外，插件也会记录会话活跃状态；开启自主主动后，用户沉默超过一段弹性时间且冷却结束时，插件会自己发布 `proactive.trigger`，让 Pipeline 回调 LLM 生成自然开场。自主主动不是固定秒表触发，会使用随机浮动、概率触发、每日上限和无人回应退避来减少机械感。

自主主动也可以接入 Agent Loop：默认不启用，开启后空闲触发会发布 `agent.loop.start`，由目标循环自主观察、规划、调用工具并决定是否联系用户。

## 环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `PROACTIVE_MODE` | `auto` | `auto` 全时段触发；`semi-auto` 仅时间窗口内触发 |
| `PROACTIVE_TIME_WINDOWS` | `09:00-22:00` | semi-auto 模式的时间窗口 |
| `PROACTIVE_LOOP_INTERVAL` | `5` | 后台轮询间隔，单位秒 |
| `PROACTIVE_CHAT_ID` | 自动检测 | 目标会话 ID，留空时从 `user.message` 提取 |
| `PROACTIVE_SOURCE` | 自动检测 | 消息来源通道，留空时从 `user.message` 提取 |
| `PROACTIVE_AUTONOMOUS_ENABLED` | `true` | 是否开启空闲后的自主主动触发 |
| `PROACTIVE_AUTONOMOUS_IDLE_SECS` | `1800` | 用户/助手最后互动后沉默多久才考虑主动开口 |
| `PROACTIVE_AUTONOMOUS_COOLDOWN_SECS` | `3600` | 同一会话两次自主主动之间的最小间隔 |
| `PROACTIVE_AUTONOMOUS_MIN_USER_MESSAGES` | `1` | 至少收到多少条用户消息后才允许自主主动 |
| `PROACTIVE_AUTONOMOUS_IDLE_JITTER_PCT` | `45` | 空闲阈值随机浮动百分比，默认约为 16.5 到 43.5 分钟 |
| `PROACTIVE_AUTONOMOUS_COOLDOWN_JITTER_PCT` | `35` | 冷却时间随机浮动百分比 |
| `PROACTIVE_AUTONOMOUS_CHANCE_PCT` | `65` | 到达下一次机会时真正触发的概率；未触发会稍后再试 |
| `PROACTIVE_AUTONOMOUS_DAILY_LIMIT` | `4` | 同一会话每天最多自主主动次数，`0` 表示不限 |
| `PROACTIVE_AUTONOMOUS_MAX_BACKOFF_MULTIPLIER` | `4` | 连续自主主动无人回应时的最大退避倍数 |
| `PROACTIVE_AGENT_LOOP_MODE` | `off` | 自主空闲触发 Agent Loop 的模式：`off` 只走原 `proactive.trigger`；`mirror` 同时发 `proactive.trigger` 与 `agent.loop.start`；`replace` 只发 `agent.loop.start` |
| `PROACTIVE_AGENT_LOOP_GOAL` | 内置目标 | Agent Loop 目标模板，支持 `{source}`、`{chat_id}`、`{peer_id}`、`{idle_secs}` 占位符 |
| `PROACTIVE_AGENT_LOOP_MAX_STEPS` | `6` | proactive 启动 Agent Loop 时的最大步骤数 |
| `PROACTIVE_AGENT_LOOP_MAX_TOOL_ROUNDS` | `3` | proactive 启动 Agent Loop 时每步最大工具调用轮数；`0` 表示不允许工具轮 |

## 工作流程

```text
用户消息进入 Pipeline
  -> LLM 根据请求决定是否调用 proactive_schedule_once / recurring
  -> proactive-plugin 保存 ScheduledTask
  -> 到期后发布 proactive.trigger
  -> PipelineActor 禁用工具并回调 LLM 生成主动回复文本
  -> route.message -> MessageRouter -> assistant.message -> IM 插件发送
```

## 自主主动流程

```text
user.message / assistant.message 经过 EventBus
  -> proactive-plugin 记录 peer 的最后互动时间
  -> 为该 peer 计算下一次自主主动机会（idle/cooldown + jitter + backoff）
  -> 后台 tick 到点后检查 daily_limit/min_user_messages/未完成定时任务/chance
  -> 满足条件时默认发布 proactive.trigger(reason=autonomous_idle)
  -> PipelineActor 使用自主主动提示词回调 LLM
  -> LLM 判断不适合打扰时返回内部跳过标记，不发送也不写入历史
  -> route.message -> MessageRouter -> assistant.message -> IM 插件发送
```

自主主动是对原有定时功能的扩展，不会替代 `proactive_schedule_once` / `proactive_schedule_recurring`。定时任务和自主触发共用 `proactive.trigger` 路由，但通过 `reason` 区分语义：

- `scheduled`：由工具安排的定时任务，到期后完成提醒或主动消息。
- `autonomous_idle`：无人安排，插件根据会话空闲状态自主触发；LLM 可判断此刻不适合打扰并跳过发送。

如果用户一直不回应自主主动，插件会逐步拉长下一次主动的等待时间，直到 `PROACTIVE_AUTONOMOUS_MAX_BACKOFF_MULTIPLIER`。用户再次发消息后，退避会重置。

## Agent Loop 联动

`PROACTIVE_AGENT_LOOP_MODE` 只影响 `autonomous_idle`，不会改变工具安排的 `scheduled` 提醒语义：

- `off`：默认值，只发布 `proactive.trigger`，保持单次 LLM 回调路径。
- `mirror`：发布 `proactive.trigger` 的同时发布 `agent.loop.start`，适合灰度观察 Agent Loop 行为。
- `replace`：不再发布 `proactive.trigger`，改由 Agent Loop 负责判断、规划和工具发送，避免同一次空闲机会产生两条主动消息。

Agent Loop 事件数据包含 `goal`、`peer_id`、`max_steps`、`max_tool_rounds`。默认目标会把当前 `source`、`chat_id`、`peer_id` 与空闲秒数写入目标文本，并要求 loop 只在合适时使用当前平台的发送工具；不合适时完成并说明跳过。

```text
autonomous_idle 到点
  -> PROACTIVE_AGENT_LOOP_MODE=mirror: proactive.trigger + agent.loop.start
  -> PROACTIVE_AGENT_LOOP_MODE=replace: agent.loop.start
  -> AgentLoopActor 读取 peer 历史快照，执行 bounded observe -> decide -> act
```

用户一旦回复当前会话，proactive-plugin 会取消该会话全部已安排任务，避免旧提醒在用户已经回来后继续触发。

## 提醒备注

调度工具的 `note` 是到期时要完成的提醒内容或主动联系目的，不是新的延迟安排。例如用户说“三秒后叫我”，可以写成“三秒到了，叫用户”。

到期事件带有 `note` 时，Pipeline 会按定时提醒处理：只完成这条提醒，用一句自然简短的话告诉用户时间到了或完成备注里的要求，不延伸新话题，也不问“叫我干嘛”。

没有 `note` 时，Pipeline 会按普通主动消息处理：延续之前的话题或自然开启新话题。

## 消息路由

1. proactive-plugin 发布 `proactive.trigger`
2. `PipelineActor` 构造临时系统消息并回调 LLM（这一轮不开放工具）
3. LLM 生成最终主动文本
4. Pipeline 发布 `route.message`
5. `MessageRouter` 补全路由并转换为 `assistant.message`
6. 对应 IM 插件发送消息

## 注意事项

- 旧 `[SCHEDULE:N]` 标签机制已废弃。
- proactive-plugin 不保存预写回复文本；最终话术由到期时的 LLM 调用生成。
- 如果修改 proactive-plugin 的工具定义，需要重新 `cargo build`，确保 `target/debug` 下的插件 DLL 被刷新。
