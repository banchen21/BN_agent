# Memory System 设计

## 三存储

- **ChatStore**（不动）— 原始聊天记录，供人/插件/LLM调工具读取
- **MemorySystem**（新建）— 边表 + 权重 + 衰减，recall() 返回 [记忆] 片段
- **PluginContext**（现有）— snapshot() 始终注入

## 两工具（内置，不注册外部）

- `load_context(chat_id, memory_limit, history_turns)` — 第一轮调用，决定多少记忆和历史
- `get_recent_chat_history(chat_id, turns)` — 任何时候觉得不够用，主动调

## messages 最终结构

```
系统
  ↓
[记忆] 片段 ← MemorySystem.recall()，weight 降序，数量由 load_context 决定
  ↓
最新原文  ← 尚未压缩的最近 N 轮，数量由 load_context 决定
  ↓
插件上下文 ← snapshot()
  ↓
当前消息
```

## 核心规则

- 第一轮：没有任何记忆和历史，LLM 调用 `load_context` 自己决定数量
- 记忆取代历史：压成 [记忆] 的对话不再以原文出现
- 数量不写死：LLM 第一轮定，后续随时可调工具改
- recall() 不分"压缩记忆"和"被动联想"，统一返回
- 权重衰减就是遗忘
