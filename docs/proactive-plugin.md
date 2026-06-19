# 主动插件 — LLM 驱动的主动消息推送

## 核心原则

**全部由 LLM 根据对话上下文决定，无硬编码秒数。**

插件只做三件事：
1. 收集 `user.message` / `assistant.message` 对话历史
2. 发给 LLM 问"怎么办"
3. 按 LLM 返回的 JSON 执行

## 环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `PROACTIVE_MODE` | `auto` | `auto` 全时段 / `semi-auto` 仅时间窗口 |
| `PROACTIVE_TIME_WINDOWS` | `09:00-22:00` | semi-auto 模式的时间窗口 |
| `PROACTIVE_LOOP_INTERVAL` | `15` | 后台轮询间隔(秒) |
| `PROACTIVE_CHAT_ID` | (自动检测) | 目标会话 ID，留空自动从 user.message 提取 |
| `PROACTIVE_SOURCE` | (自动检测) | 消息来源通道，留空自动从 user.message 提取 |

## 工作流程

```
plugin.start():
  → 从 env 读 chat_id / source（留空自动检测）
  → 启动后台线程（默认每 15s 循环）
  → 加载 DB 历史

每次 user.message / assistant.message：
  → 自动检测 chat_id + source（若未设）
  → buffer 记录消息（最多 100 条）
  → 用户回复时取消已安排的追问

后台循环 tick():
  ├─ 模式是 semi-auto 且不在时间窗口？ → 跳过
  ├─ 有已安排的追问？ → 到时间就发送 → 再问 LLM 下一步
  ├─ 距上次 LLM 决策 < 15s？ → 跳过（冷却）
  └─ 否则 → 发全部历史给 LLM 决策
          ├─ paused=true → 不做任何事
          └─ paused=false + message + wait_seconds → 安排追问
```

## LLM prompt

LLM 收到对话记录 + 当前状态（谁最后发言、沉默多久、当前时间），返回 JSON：

```json
{
  "paused": true / false,
  "message": "追问内容",
  "wait_seconds": 60,
  "continue_": true,
  "reason": "决策理由"
}
```

## 消息路由

1. 追问消息通过 `proactive.message` 事件发布
2. `MessageRouter` 接收并转换为 `assistant.message`
3. `tg-im-plugin` 接收并发送
4. 同时写入 ChatStore 持久化

## 状态跟踪

```rust
history: HashMap<chat_id, Vec<(role, text, timestamp)>>  // 最多 100 条
scheduled: HashMap<chat_id, ScheduledAction>              // 已安排的追问
last_decision: HashMap<chat_id, Instant>                  // 防频繁 LLM 调用
```
