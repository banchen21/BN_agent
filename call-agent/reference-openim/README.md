# ai-call · AI 通话服务 + LiveKit Agent（Phase 3）

悦悦 AI 通话的独立服务：HTTP 入口 `:9001` + LiveKit Agent worker（`agent_name="yueyue"`），
AI 能力复用 `E:\yueyue\call` 的 8877（SenseVoice ASR / Qwen3-8B 对话 / GSV-TTS 流式合成）。

```
Android / Win11 Electron
   │  POST /ai/call/start  {userID, mediaType}
   ▼
ai-call :9001 (FastAPI)
   ├─ LiveKit RoomService.CreateRoom   房间 ai-call-<userID>-<timestamp>
   ├─ 签发用户 JWT（可发布音视频）
   └─ AgentDispatchService.CreateDispatch(agent_name="yueyue")
        │
        ▼
LiveKit :7880/7881/7882-7892
        │  显式派发 job
        ▼
agent_main.py（worker）
   VAD 切句 → 8877/api/asr → 8877/api/chat(SSE) → 8877/api/tts_stream(SSE) → 房间音频
```

- 客户端以**普通 LiveKit 参与者**入会：`audio` 只发麦克风，`video` 额外发摄像头。
- AI **只出声音**，不发布视频；视频通话时只订阅用户视频。
- 用户开口时 VAD 触发打断：取消当前 LLM/TTS 流、清空播放队列，并 `POST 8877/api/interrupt`。

## 目录

```
ai-call/
  main.py            FastAPI :9001（/ai/call/start | end、/health）
  livekit_service.py 建房 / 签 token / dispatch / 删房
  yueyue.py          8877 异步客户端（ASR、SSE 对话、SSE TTS、打断）
  plugins.py         LiveKit Agents 自定义 STT / LLM / TTS 插件
  agent_main.py      Agent worker（python agent_main.py start）
  config.py          配置（全部可用环境变量覆盖）
  requirements.txt
  start-api.cmd      启动 :9001
  start-agent.cmd    启动 worker
```

## 安装（Windows Python 3.13，实测可用）

```bat
cd E:\openim-companion\ai-call
python -m venv .venv
.venv\Scripts\python.exe -m pip install -r requirements.txt
```

`livekit-agents 1.8.11` / `livekit-api 1.2.2` / `livekit-plugins-silero` 在 Windows Python 3.13
原生可用，**不需要 WSL**。

## 启动顺序

1. **GSV-TTS 引擎 :9880**
   ```bat
   E:\yueyue\call\start-tts.cmd
   ```
2. **yueyue 主服务 :8877**（ASR + LLM + TTS 转发，首次加载模型约 30~60 秒）
   ```bat
   E:\yueyue\call\start-call.cmd
   ```
   确认 `curl http://127.0.0.1:8877/api/health` 里 `llm/asr/tts` 都为 `true`。
3. **ai-call HTTP :9001**
   ```bat
   E:\openim-companion\ai-call\start-api.cmd
   ```
   或 `.venv\Scripts\python.exe -m uvicorn main:app --host 0.0.0.0 --port 9001`
4. **Agent worker（一次性启动，常驻；必须先于客户端发起通话）**
   ```bat
   E:\openim-companion\ai-call\start-agent.cmd
   ```
   日志出现 `registered worker ... agent_name=yueyue` 才算就绪。worker 的 HTTP 健康端口是 `8081`。

> LiveKit 已由 infra 部署：`ws://192.168.0.101:7880`（局域网 / 手机），
> `ws://127.0.0.1:7880`（Windows 宿主直连）。key/secret 见 `..\deploy\livekit.yaml`。
> worker 与 9001 的服务端 API 都走 `ws://127.0.0.1:7880`。

## 接口

### POST /ai/call/start

```bash
curl -X POST http://127.0.0.1:9001/ai/call/start \
  -H 'Content-Type: application/json' \
  -d '{"userID":"<OpenIM userID>","mediaType":"audio"}'
```

```json
{
  "ok": true,
  "url": "ws://192.168.0.101:7880",
  "urlLocal": "ws://127.0.0.1:7880",
  "token": "<livekit jwt>",
  "room": "ai-call-<userID>-<timestamp>",
  "identity": "<userID>",
  "mediaType": "audio",
  "agentDispatchId": "AD_xxx"
}
```

- 手机用 `url`；Windows 宿主用 `urlLocal`（宿主访问 192.168.0.101 会受 hairpin 限制）。
- `mediaType`: `"audio"` | `"video"`。
- 错误：`400 {"ok":false,"code":"BAD_REQUEST"}`；LiveKit 异常时
  `503 {"ok":false,"code":"LIVEKIT_UNAVAILABLE"}`。
- 已开 CORS（`allow_origins=["*"]`，Electron 渲染进程 / Android 均可直接 POST）。
- 成功后会在后台调用 ai-bot 的 `/ai/gpu/begin_call`（best-effort，不阻塞响应）。

### POST /ai/call/end

```bash
curl -X POST http://127.0.0.1:9001/ai/call/end \
  -H 'Content-Type: application/json' \
  -d '{"room":"ai-call-xxx-123"}'
```

只会删除 `ai-call-*` 前缀的房间；客户端 best-effort 调用，不阻塞挂断。
后台调用 ai-bot 的 `/ai/gpu/end_call`。

### GET /health

```json
{"ok":true,"livekit":true,"yueyue":true,"aiBot":true,"agent":"yueyue",
 "url":"ws://192.168.0.101:7880","urlLocal":"ws://127.0.0.1:7880"}
```

## 测试步骤

1. `curl http://127.0.0.1:9001/health` → `livekit` / `yueyue` 为 true。
2. `curl` 上面的 `/ai/call/start`，记下 `room`；同时看 worker 日志出现
   `Agent 入会 room=...`，LiveKit 日志出现 `assigned job to worker`。
3. 客户端用返回的 `url`（手机）/ `urlLocal`（Windows）+ `token` 入会。
4. 对麦克风说一句「你在干嘛」：agent 日志应依次出现 `ASR: ...` → `LLM 输入: ...` →
   `TTS 首包: ...`，房间内听到悦悦回应，且客户端看到 agent participant。
5. 悦悦说话途中再次开口：当前回复应立即停止（打断），随后按新一句话回应。
6. 视频通话：客户端发摄像头，AI 只出声音、不发布视频轨道。
7. 挂断后调 `/ai/call/end`（或房间空置 120s 自动关闭）。
8. 无客户端自测：
   ```bat
   .venv\Scripts\python.exe scripts\selftest.py
   ```
   自建房间 → Python 参与者入会 → 检查 agent 入会/问候 → 推一段 TTS 语音触发
   ASR→LLM→TTS → 说话途中插话验证打断，最后自动删房。

## 边听边说 / 智能打断

- AI 说话期间 VAD/ASR 常开，不 mute 用户麦克风。
- 用户开口时先把 TTS 音量 duck 到 45%；短附和（嗯/对/好/哦/嗯嗯/好的…）**不打断**，
  用户停止后约 0.6s 恢复音量。
- 真正的插话（不在短附和词表，或语音时长 ≥0.8s）取消当前 TTS 并回复新内容：
  - VAD 层：`turn_handling.interruption.min_duration = 0.8`；
  - 转写层：短插话（如「等一下」）在 final transcript 到达时补一次 `session.interrupt()`。
- 日志：`bargein: backchannel text=... chars=... duration=... duck=0.45` /
  `bargein: interrupt (stage=thinking|speaking) text=... chars=... duration=... cancelled_tts=...`；
  框架 VAD 先打断、转写未及时到达时，补记 `bargein: interrupt (vad) ...`。
  打断判定按 `agent_active = thinking | speaking` 跟踪，不依赖瞬时 `agent_speaking`。
- 自测：重启 worker 后运行 `.venv\Scripts\python.exe scripts\bargein_test.py`。
- 客户端 AEC/NS：Android / Electron Demo 均使用 LiveKit SDK 默认音频处理
  （echoCancellation / noiseSuppression / autoGainControl 未在代码中关闭）。
  真机测试建议戴耳机，避免外放声音被麦克风收回去造成自打断。

## 延迟口径（验收实测，务必按此解读）

- **agent 侧（单侧时钟，可比）**：ASR 终句 → LLM 首 token **1.88–1.98s**（Magpie 模式）；
  每句 TTS 首包 **0.23–0.65s**；问候 TTS 首包 0.27–0.42s。
- **LiveKit 侧（服务端同钟事件，可比）**：
  - `assigned → participant active` ≈ 0.1s，`dispatch → assigned` ≈ 0.2s；
  - 热调用 `participant active → agent audio track published` ≈ **0.39s**；
  - worker 刚重启后的**首通冷启动**（job runner / HTTP 客户端 / RoomIO 初始化）
    同钟约 **2.7s**，属真实冷启动开销，不是时钟问题；统计时把首通单独标注，勿平均。
- **跨侧（host 日志 vs 容器日志）不可直接相减**：宿主与 Docker 容器时钟存在持续漂移
  （实测 15s 内可漂 1.2s，量级 1.3–2.9s）。跨侧对齐请以 room 名里的 epoch 为锚，或统一换算到同一时钟。

## 环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `AI_CALL_HOST` / `AI_CALL_PORT` | `0.0.0.0` / `9001` | 服务监听 |
| `LIVEKIT_URL` | `ws://127.0.0.1:7880` | 服务端 API / worker 连接地址 |
| `LIVEKIT_PUBLIC_URL` | `ws://192.168.0.101:7880` | 返回给手机的 `url` |
| `LIVEKIT_LOCAL_URL` | `ws://127.0.0.1:7880` | 返回给 Windows 的 `urlLocal` |
| `LIVEKIT_KEY` / `LIVEKIT_SECRET` | 取自 deploy/livekit.yaml | LiveKit 凭据 |
| `AI_AGENT_NAME` | `yueyue` | 必须与 worker `agent_name` 一致 |
| `YUEYUE_API` | `http://127.0.0.1:8877` | ASR/LLM/TTS |
| `AI_BOT_API` | `http://127.0.0.1:9000` | GPU 协调（可关） |
| `AI_CALL_GPU_COORDINATION` | `1` | 是否调用 begin/end_call |
| `AI_CALL_GREETING` | `喂？` | 入会第一句话；置空则不打招呼 |
| `AI_WORKER_HTTP_PORT` | `8081` | worker 健康端口 |
| `AI_CALL_TOKEN_TTL` | `21600` | 客户端 JWT 有效期（秒） |

## 排障

- **worker 日志 `failed to connect ... /agent 400 Invalid response status`**
  aiohttp 把 `ws://127.0.0.1` 的握手交给了系统 HTTP 代理。`config.py` 启动时会清空
  `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY`，若从其他脚本启动 worker，请同样清掉这些变量。
- **客户端一直停在「等待悦悦加入」**：worker 没注册。先看 worker 日志有没有
  `registered worker`，再看 `GET /health` 的 `livekit`；dispatch 在 worker 注册成功后才会落到 job。
- **能听到声音但没有回复**：`GET /health` 的 `yueyue` 为 false，或 8877 模型还在加载；
  `curl 127.0.0.1:8877/api/health` 应三项全 true。
- **8GB 显存**：通话（LLM+TTS）与生图互斥；ai-bot 通过 `/ai/gpu/begin_call` 让生图排队。
  若 OOM，降低 `E:\yueyue\call\config.json` 的 `llm.n_gpu_layers`。
- **TTS 采样率**：`/api/tts_stream` 返回 `sr=32000`，插件按 32000 声明；若引擎换采样率，
  日志会打 warning（当前不退避，仍按声明播放）。
- **端口**：9001（本服务）、8081（worker 健康）、7880/7881/7882-7892（LiveKit）、
  8877（yueyue）、9880（GSV-TTS）、9000（ai-bot）。
