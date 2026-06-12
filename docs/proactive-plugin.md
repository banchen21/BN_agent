# 主动插件方案 v3 — 纯 LLM 决策

## 核心原则

**没有任何硬编码的秒数或轮数。全部由 LLM 根据对话上下文决定。**

插件只做三件事：
1. 收集对话历史
2. 发给 LLM 问"怎么办"
3. 按 LLM 说的执行

---

## 环境变量演变

```
v1（固定数值）          v2（半 LLM 决策）        v3（纯 LLM 决策）
─────────────────     ─────────────────     ─────────────────
PROACTIVE_CHAT_ID      PROACTIVE_CHAT_ID     PROACTIVE_CHAT_ID
PROACTIVE_SOURCE       PROACTIVE_SOURCE      PROACTIVE_SOURCE
PROACTIVE_MODE         PROACTIVE_MODE        PROACTIVE_MODE
PROACTIVE_TIME_WINDOWS PROACTIVE_TIME_WINDOWS PROACTIVE_TIME_WINDOWS
PROACTIVE_IDLE_TIMEOUT  ──删除──              ──删除──
PROACTIVE_COOLDOWN      ──删除──              ──删除──
PROACTIVE_PROMPT        ──删除──              ──删除──
                       PROACTIVE_PAUSE_SECS   ──删除──
                       PROACTIVE_MAX_CONTEXT  ──删除──
                       PROACTIVE_DECISION_MODEL ──删除──
```

只剩 4 个配置：CHAT_ID / SOURCE / MODE / TIME_WINDOWS

---

## 工作流程

```
plugin.on_event(): 持续收集 user/assistant 消息到 history[chat_id]
                          │
                  15s 循环检查
                          │
                 有 scheduled action？──── 是 → 检查是否到发送时间
                          │                        │
                         否                   用户回复了？── 是 → 取消
                          │                        │
                 距上次 LLM 决策 < 15s？── 否   到时间 → 发消息
                          │                        │
                         是（跳过本轮）         LLM 决策下一轮
                          │
             把全部历史发给 LLM
                          │
           ┌── LLM 返回 JSON ──┐
           │                    │
      paused=false         paused=true
           │                    │
       跳过本轮           { message, wait_seconds, continue_ }
                                │
                         schedule: send_at = now + wait_seconds
                                │
                        15s 循环检测到 → 发送
```

## LLM prompt

```text
【对话记录】
user: 帮我推荐一本书
assistant: 你喜欢什么类型？
user: 科幻的吧
assistant: 推荐《三体》，硬科幻经典

【状态】
- 最后一条消息来自: assistant
- 距离最后一条消息: 2 分钟前
- 当前时间: 14:30

【任务】
分析以上对话。请以 JSON 格式输出以下决策：
{
  "paused": true,          ← 对话是否已暂停？
  "message": "要不要试试《三体》？它格局宏大，适合入坑。",
                           ← 追问内容
  "wait_seconds": 300,     ← 等多少秒再发
  "continue_": true,       ← 发完后是否继续主动循环
  "reason": "用户问了推荐但未回应具体推荐，等5分钟给一个具体选项"
}
```

## 状态跟踪

插件内部只维护最简单的状态：

```rust
// 完整的对话历史（最多保留 100 条）
history: HashMap<chat_id, Vec<(role, text, timestamp)>>

// 已安排的主动消息
scheduled: HashMap<chat_id, ScheduledAction>
  - message: String        // 要发送的内容
  - send_at: Instant       // 何时发送
  - decided_at: Instant    // 何时做出的决策（用于判断用户是否在此期间回复）
  - should_continue: bool  // 发完后是否继续

// 防频繁调用 LLM
last_llm_call: HashMap<chat_id, Instant>
```

## LLM 判断的维度

| 维度 | LLM 如何判断 | 对应旧 env var |
|---|---|---|
| 对话是否暂停 | 看最后一条消息是谁发的 + 沉默时长 | `PROACTIVE_PAUSE_SECS` |
| 多久后追问 | 话题热度、用户投入程度 | `PROACTIVE_COOLDOWN` |
| 追问什么 | 对话上下文、最近话题 | `PROACTIVE_PROMPT` |
| 是否继续循环 | 话题是否还有延续空间 | 隐含在 cooldown 中 |
| 需要多少上下文 | 自己看全部历史，忽略无关部分 | `PROACTIVE_MAX_CONTEXT_ROUNDS` |

LLM 比任何固定数值都更适合这些判断——它真正"理解"对话。
