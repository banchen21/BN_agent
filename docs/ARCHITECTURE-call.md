# 通话架构（定稿草案）

> 目标：Android「拾语」的文字聊天、语音消息、语音通话、视频通话，**都是同一个 agent**
> （同人设 `data/yueyue/persona.md`、同 `memories.db`、同 `/api/chat` 会话上下文）。
> 不再存在第二个「接话 Agent」进程。

## 0. 定稿状态（2026-11-12）

用户已确认：

| 项 | 定稿 |
|---|---|
| 代码位置 | `plugins/call-livekit-plugin`（LiveKit 房间生命周期全归该插件） |
| 客户端 | 官方 Rust SDK（crate `livekit`），插件自签 agent token 直接进房，不用 CreateDispatch / `/agent` |
| 第一阶段范围 | **语音问答 + 能看对方摄像头**（订阅对方视频帧 → 视觉模型 → 口语描述）；不做打断、不做通话中发图 |
| LLM / 人格 | 主后端 `/api/chat`（同人设、同记忆、同会话） |
| Python 试做 `backend-call-agent/` | 已删除，不参与运行 |
| 分支 | 工作区回到 `feat/livekit-voice-video-comfyui`；临时空分支 `fix/livekit-backend-agent` 已删除 |

## 1. 分层

```
┌─────────────────────────────────────────────────────────────┐
│ Android「拾语」                                              │
│  ChatScreen / VoiceCallScreen / VideoCallScreen               │
│  CallManager（进程级通话状态机：连接、计时、静音、摄像头、浮窗） │
└───────────────┬───────────────────────────────┬─────────────┘
                │ HTTP（/api/*）                 │ WebRTC（音视频）
                ▼                               ▼
┌───────────────────────────────┐   ┌──────────────────────────┐
│ main-app（Rust，:18080）      │   │ LiveKit server（本机）    │
│  /api/chat        文字聊天     │   │  7880 信令 / 7881 TCP     │
│  /api/chat/voice  语音消息     │   │  7882-7892 UDP 媒体       │
│  /api/chat/append 通话回写     │   └───────────▲──────────────┘
│  /api/call/*  → call 插件      │               │ WebRTC（音视频）
└───────────────┬───────────────┘               │
                │ 插件 API 调用                  │
                ▼                               │
┌───────────────────────────────────────────────┴─────────────┐
│ call-livekit-plugin（Rust，main-app 进程内）                  │
│  · 建房 / 签 token（RoomService、jwt.rs）                     │
│  · **以 agent 身份进房**（livekit crate：Room::connect）       │
│  · 收对方音频 → VAD 切句 → ASR → 主后端 /api/chat → TTS → 发回 │
│  · 订阅对方视频（先不发布我方视频）                            │
└───────────────┬─────────────────────────────────────────────┘
                │ HTTP（本机）
                ▼
     yueyue 8877（SenseVoice ASR / /api/chat / GSV-TTS）
```

## 2. 关键决策

| 决策 | 结论 | 理由 |
|---|---|---|
| 通话接话进程 | **不要独立 sidecar** | 用户明确要求「同一个人」；独立进程必然产生第二套上下文 |
| LiveKit 客户端 | **官方 Rust SDK（crate `livekit`）** | 不自己复刻 RTC 协议；原生 WebRTC |
| 通话代码位置 | **call-livekit-plugin** | LiveKit 就是「WebRTC 服务端 + 客户端 SDK」，房间全生命周期归该插件 |
| 派生 agent 的方式 | **插件自签 token 直接 `Room::connect`** | CreateDispatch / `/agent` 注册是给外部 worker 用的，正是「像另一个人」的来源 |
| LLM 与人格 | **主后端 `/api/chat`** | 人设、记忆、会话历史全在 main-app |
| ASR / TTS | yueyue 8877 | 与语音消息同一条链路（SenseVoice / GSV-TTS） |
| 视觉 | 阶段一：订阅对方视频帧 → 视觉模型（复用 `image_understand`）→ 口语回答 | 用户要求「能看对方摄像头」 |
| 悦悦的画面 | 不发我方视频轨，App 用悦悦自拍图铺大屏 | 实时虚拟人暂不做 |
| 开关 | 插件配置 `backend_agent_enabled`（默认 true） | 可退回「只建房、不接话」 |

## 3. 一次通话的时序

1. App `POST /api/call/start` → 插件建房 + 返回「用户 token」；插件同时签「agent token」。
2. App `Room.connect`；插件 `Room::connect`（agent 身份）进同一房间，发布一条音频轨（悦悦的声音）。
3. 用户说话 → 插件订阅到音频帧 → VAD 切句（静音 ~600ms 收句）。
4. 句子 → `POST 8877/api/asr` → 文本 → `POST /api/chat`（`peer_id=android:<会话>`，`source=android`）。
5. 回复 → `POST 8877/api/tts` → PCM → 通过 `NativeAudioSource` 发回房间。
6. 挂断（任一方）→ 插件断开、删除房间；把本通 ASR 文本 + 回复 `POST /api/chat/append`
   （只进上下文，**不在消息列表显示**）。
7. App 在本地写一条「通话记录」气泡（语音/视频 + 时长）。

## 4. 与文字聊天共用上下文

- 同一个 `peer_id`（`android:yueyue`）→ 同一份会话历史。
- 通话前无需再单独带 16 条历史（主后端自己就有）；Android 的 `history` 字段保留兼容即可。
- 通话结束后回写，下一次文字聊天自然衔接。

## 5. 待定 / 已知风险

- `livekit` crate 依赖 `webrtc-sys`，Windows 需 MSVC 构建工具；若编译不通需评估替代（如仅音频用其他方案）。
- 打断（用户随时插话）阶段一不做。
- 系统代理（如 `127.0.0.1:59992`）会拦截本机 WebSocket，相关请求需显式绕过代理。
- Agent 主动来电、回拨：沿用 `main-app/src/call_invite.rs` 的 invite/incoming/decline。
- 独立进程方案 `backend-call-agent/`（Python 试做）**已废弃**，仅作参考，不参与运行。
