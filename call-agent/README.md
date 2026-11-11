# call-agent · 悦悦 LiveKit Agent（全双工语音 + 视频订阅 + 视觉理解）

`E:\BN_agent\call-agent` 是 BN_agent 通话链路的 Python sidecar：
以 `agent_name="yueyue"` 注册 LiveKit Agent worker，由 call-backend（main-app :18080）
通过 `AgentDispatchService.CreateDispatch` 显式派发入会。

- **语音**：用户麦克风 → Silero VAD 切句 → yueyue `:8877` 的 `/api/asr` → `/api/chat`(SSE)
  → `/api/tts_stream`(SSE) → 房间音频。
- **全双工/边听边说**：AI 说话/思考期间 VAD/ASR 常开；短附和 duck 音量不打断；
  真插话取消当前 TTS/LLM 并回复新内容。
- **视频/视觉**：订阅用户摄像头（LiveKit WebRTC 视频轨）并按 1fps 维护最新帧；
  LLM 通过工具 `look_at_user` / `snapshot_view`（或关键词兜底自动看图）按需截图，
  交给多模态模型理解后再用口语回复。AI 只出声音，不发布视频。
- **人设/记忆**：从 `E:\BN_agent\data\yueyue` 读取 `persona.md` 与 `memory/*.md`，
  不引用 `E:\yueyue` 原目录。

## 目录

```
call-agent/
  agent_main.py           Agent worker 入口（WorkerOptions(agent_name="yueyue")）
  config.json             非密钥配置（LiveKit 地址、yueyue 地址、data_dir、vision…）
  config.py               config.json + 环境变量；从 call-livekit 插件配置读 key/secret
  yueyue.py               8877 异步客户端（ASR / chat SSE / tts_stream SSE / interrupt）
  plugins.py              LiveKit STT / LLM / TTS 插件 + 视觉工具标签解析
  bargein.py              全双工打断控制器（duck / 短附和 / 真插话 / VAD 兜底）
  frames.py               用户摄像头最新帧缓存（AgentSession.video_sampler）
  vision.py               视觉理解客户端（magpie / openai / bn_agent 三种 provider）
  persona.py              人设与记忆加载
  scripts/selftest.py     无客户端自测（入会 / 问候 / 语音 / 视频 / 视觉 / 挂断）
  scripts/bargein_test.py 边听边说自测（短附和 / speaking 插话 / thinking 插话）
  start-agent.cmd         后台启动，日志写 logs\agent.out.log
  start-agent-console.cmd 前台启动（Ctrl+C 退出）
  requirements.txt
```

## 安装

```bat
cd E:\BN_agent\call-agent
python -m venv .venv
.venv\Scripts\python.exe -m pip install -r requirements.txt
```

已在 Windows Python 3.13 实测：`livekit-agents 1.8.11` / `livekit-api 1.2.2` /
`livekit-plugins-silero` / `livekit-rtc 1.2.0`，不需要 WSL。

## 前置服务

| 服务 | 地址 | 说明 |
| --- | --- | --- |
| LiveKit server | `ws://127.0.0.1:7880`（agent 用） | 浏览器侧走 Caddy `wss://192.168.0.101:7443` |
| yueyue | `http://127.0.0.1:8877` | ASR/LLM/TTS，健康检查 `llm/asr/tts` 全 true |
| Magpie 网关（视觉） | `http://127.0.0.1:3425/v1` | 默认视觉模型 `group/ds-v4.1`（支持图片输入） |
| main-app / call-backend | `http://127.0.0.1:18080` | 建房、dispatch、通话页 |

## 启动 / 停止

```bat
:: 后台启动（推荐，日志 logs\agent.out.log）
E:\BN_agent\call-agent\start-agent.cmd

:: 前台启动（排障）
E:\BN_agent\call-agent\start-agent-console.cmd
```

停止：结束对应 `python agent_main.py start` 进程即可（前台 Ctrl+C；后台
`Get-Process python | Where-Object {...} | Stop-Process` 或任务管理器）。

启动成功标志：日志出现 `registered worker ... agent_name=yueyue`。
worker 自带 HTTP 健康端口 **:8081**（`curl http://127.0.0.1:8081/`）。

端口占用：**8081**（worker 健康）、7880/7881/7882-7892（LiveKit）、8877（yueyue）、
3425（Magpie）、18080（main-app）。agent 不占用 9001/18443。

## 配置

`config.json`（可用环境变量覆盖，见 `config.py`）：

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `agent_name` | `yueyue` | 必须与 call-backend dispatch 的 `agent_name` 一致 |
| `livekit_url` | `ws://127.0.0.1:7880` | agent 连 LiveKit 的地址（同机明文；不要用 7443） |
| `livekit_plugin_config` | `E:/BN_agent/data/plugin_configs/call-livekit-plugin.json` | 从这里读 `api_key/api_secret/api_url` |
| `yueyue_url` | `http://127.0.0.1:8877` | ASR/LLM/TTS |
| `data_dir` | `E:/BN_agent/data/yueyue` | persona.md、memory/、refs/、album/、voice/ |
| `greeting` | `喂？` | 等真人入会后说第一句；置空则不打招呼 |
| `worker_http_port` | `8081` | worker 健康端口 |
| `turn.framework_interrupt_min_duration` | `3.0` | 框架 VAD 兜底打断阈值（避免 0.8s 误打断） |
| `bargein.duck_gain` | `0.45` | 用户开口时 TTS 音量 |
| `video.max_fps` | `1.0` | 最新帧采样频率 |
| `video.max_width` | `960` | 截图缩放宽度 |
| `vision.provider` | `magpie` | `magpie` / `openai` / `bn_agent` |
| `vision.model` | `group/ds-v4.1` | 多模态模型名 |
| `vision.auto_look` | `true` | 命中关键词时先自动看一眼（抵御本地小模型不输出标签） |

环境变量覆盖：`LIVEKIT_URL`、`LIVEKIT_KEY`、`LIVEKIT_SECRET`、`YUEYUE_URL`、
`DATA_DIR`、`AGENT_NAME`、`GREETING`、`WORKER_HTTP_PORT`、`VISION_PROVIDER`、
`VISION_MODEL`、`VISION_BASE_URL`、`VISION_API_KEY`、`CALL_AGENT_CONFIG`。

**密钥只在运行时读取**：默认从 `call-livekit-plugin.json` 取 LiveKit key/secret；
视觉 token 优先 `VISION_API_KEY`，否则读环境变量 `ANTHROPIC_AUTH_TOKEN` 或
`~/.claude/settings.json` 的 `env.ANTHROPIC_AUTH_TOKEN`。日志不打印密钥。

## 与 call-backend 的约定

- 房间名：`ai-call-<userID>-<毫秒时间戳>`（后端生成）。
- dispatch：`agent_name="yueyue"`，metadata JSON 字符串：
  `{"userID","identity","mediaType","room","source":"bn-call"}`。
  agent 会打印到日志（`Agent 入会 ... metadata=...`），可据此判断是否视频通话。
- 真人 identity：`user-<userID>`；agent identity 由 LiveKit 生成为 `agent-<jobid>`，
  页面按 `agent` 前缀显示“悦悦已入会”并自动播放音频、挂视频主画面。
- agent **只发布音频轨**（`roomio_audio`），不发布视频；视频仅订阅用户摄像头。
- 挂断：真人离开房间后 RoomIO 触发 session 关闭，agent 清理资源并退出 job；
  房间由 LiveKit `empty_timeout=300s` 回收（后端也会主动删房）。

## 全双工 / 智能打断

- AI 说话/思考期间 `room_io` 不 mute 用户麦克风，VAD/ASR 常开。
- 用户开口且 agent 处于 `thinking|speaking`：先把 TTS 音量 duck 到 45%（对后续合成 PCM 降增益）。
- **短附和**（嗯/对/好/哦/嗯嗯/好的…）不打断，用户停后 ~0.6s 恢复音量。
- **真插话**（不在短附和白名单）：final transcript 到达时 `session.interrupt()`
  取消 TTS/LLM，随后按新一句回复。
- **框架层兜底**：`turn_handling.interruption.min_duration=3.0`，只处理超长连续说话；
  不再使用 0.8s 阈值抢先打断（旧实现漏洞）。
- 日志格式（QA 可 grep）：

```
bargein: user started while agent active (stage=speaking) -> duck=0.45
bargein: backchannel text='嗯。' chars=1 duration=0.6s duck=0.45
bargein: interrupt (stage=speaking) text='等下。' chars=2 duration=0.9s cancelled_tts='...'
bargein: interrupt (stage=thinking) ...
bargein: interrupt (stage=vad) ...   # 框架 VAD 已先打断、转写迟到时的兜底日志
bargein: restore tts volume
```

> 提示：真机外放时建议戴耳机，避免 AI 的声音被麦克风收回去造成自打断
> （客户端 AEC/NS 默认开启；agent 侧不做额外回声消除）。

## 视频与视觉理解

标准路径：LiveKit WebRTC 视频轨 → `RoomOptions(video_input=True)` →
`AgentSession(video_sampler=FrameStore)`。`FrameStore` 每帧只做时间门控
（默认 1fps）、保存最新一帧 RGBA，返回 `False` 表示不把视频帧注入 LLM 上下文。
截图是混合兜底：需要时把最新帧编码 JPEG 发给视觉模型，不持续占用上下文/显存。

LLM 工具（在 `YueyueAgent` 上注册，由 LLM 标签触发）：

- `look_at_user(question)`：看用户摄像头并回答关于画面的问题。
- `snapshot_view()`：截一帧并描述场景。

本地 8877 的 Qwen 不支持原生 function calling，因此 `plugins.py` 用标签协议桥接：
LLM 输出 `<look>问题</look>` / `<snapshot/>` / `<tool_call>{"name":...}</tool_call>`
时，插件把它转成框架的 `FunctionToolCall`，由框架执行工具后把视觉结果回填，
再请求 8877 生成自然语言回复。另有**关键词兜底**（`vision.auto_look_keywords`）：
用户说“看见/长什么样/穿的/画面/摄像头/房间…”时，在请求 LLM 前先自动看一眼，
避免小模型不按标签协议输出导致“睁眼说瞎话”。

视觉 provider：

| provider | 行为 |
| --- | --- |
| `magpie`（默认） | `POST http://127.0.0.1:3425/v1/chat/completions`，模型 `group/ds-v4.1`（支持图片），token 读 `ANTHROPIC_AUTH_TOKEN` |
| `openai` | 任意 OpenAI 兼容多模态端点（`vision.base_url/model/api_key`，可配 `auth_header`） |
| `bn_agent` | 调 main-app `POST /api/tools/call` → `image_understand` 工具 |

> `bn_agent` provider 依赖 main-app 的 MiMo 视觉后端；当前 `IMAGE_API_KEY`
> 调 `token-plan-cn.xiaomimimo.com` 返回 **401 Invalid API Key**，因此默认改用本地
> Magpie 的 `group/ds-v4.1`（实测可识图）。等 MiMo key 恢复后可切回。
> 视觉推理约 2–6s，属于“问到了才看”的按需路径，不影响普通对话延迟。

## 测试

先启动 worker，再执行（脚本会自建房间 + dispatch + 用户参与者入会）：

```bat
:: 全链路：入会/问候/语音问答/视频订阅/视觉理解/挂断
.venv\Scripts\python.exe scripts\selftest.py --media video

:: 只测语音
.venv\Scripts\python.exe scripts\selftest.py --media audio

:: 边听边说：短附和不打断 + speaking/thinking 阶段真插话
.venv\Scripts\python.exe scripts\bargein_test.py

:: 视觉 provider 单测（不需要 LiveKit / 通话，直接发一张图）
.venv\Scripts\python.exe scripts\vision_test.py
```

实测结果（2026-11-11 18:33，本机，最终代码）：

- `selftest --media video`（exit 0，room `ai-call-selftest-1794393227830`）：
  agent 入会 PASS、ASR→LLM→TTS PASS、视频帧订阅 PASS（355 帧/36 帧入库，640x360）、
  视觉理解+回复 PASS（`vision: look_at_user ok 5.15s`，回复“淡黄色长袖+浅蓝短裙…”）、
  挂断退出 PASS。
  问候首句 `喂？` 已生成（`TTS 首包 0.25s`）；测试客户端偶发漏听首句
  （订阅竞态，见排障），不影响问答音频链路，因此脚本标 INFO 而非 FAIL。
- `bargein_test`（全 PASS）：
  `bargein: backchannel text='嗯。' ... duck=0.45`（不打断）、
  `bargein: interrupt (stage=speaking) text='等下。' ...`、
  `bargein: interrupt (stage=thinking) text='等下。' ...`、
  `bargein: interrupt (stage=vad) duration=...`（>=3s 长语音兜底）；
  两种真插话后均有新回复。
- 与 call-backend 联调：`POST /api/call/start` 返回 `agent.dispatched=true`，
  worker 日志出现 `Agent 入会 ... metadata={"identity":"user-...","source":"bn-call"}`，
  0.8s 内 `Agent ready`；`/api/call/end` 删房后 `Agent 退出`。
  注意 LiveKit `ListRooms.num_participants` 不计入 AGENT 类型参与者（显示 0），
  用 `ListParticipants` 可看到 `agent-<jobid>`（kind=4）。

测试脚本输出目录：worker 日志 `logs\agent.out.log`；脚本按日志 offset
只读取本次通话新增内容。

## 排障

- **worker 未注册 / dispatch 一直 pending**：看日志有没有 `registered worker`；
  `curl http://127.0.0.1:8081/` 是否 200；`curl http://127.0.0.1:7880` 是否可达。
- **`400 Invalid response status` 连接失败**：系统 HTTP 代理劫持了
  `ws://127.0.0.1:7880`。`config.py` 已在进程内清空 `HTTP(S)_PROXY/ALL_PROXY`；
  从其他脚本启动 worker 时也要清掉。
- **能听到声音但没有回复**：8877 健康检查 `llm/asr/tts` 是否全 true；
  yueyue 模型加载约 30–60s；看日志 `ASR:` / `LLM 输入:` / `TTS 首包:` 三段。
- **问候听不到**：agent 会等真人入会、且本端订阅到对方麦克风轨道后再等 0.8s 才说；
  测试脚本用“双方几乎同时入会 + 立即推麦克风”的极端时序，偶发漏听首句（订阅竞态）。
  浏览器通话页是“agent 先入会、用户后入会”，一般不会漏听；若真机也漏听，
  可调大 `_greet_when_ready` 的 `await asyncio.sleep(0.8)`。
- **有画面但视觉说看不到**：确认日志 `video: 收到用户首帧`、`vision: tool-call/auto-look`、
  `vision: look_at_user ok`；若视觉 HTTP 失败会打印 `vision: ... 失败` 与原因。
- **视觉空回复**：推理模型会把 token 花在 reasoning 上；`vision.py` 已对空内容
  自动加倍 `max_tokens` 重试一次（默认 1200）。
- **日志不实时**：worker 重定向到文件时已启用行缓冲（`PYTHONUNBUFFERED=1` +
  `sys.stdout.reconfigure(line_buffering=True)`）。

## 边界

本目录只负责 Agent worker；不改 OpenIM（已删除）、`E:\yueyue` 原目录、ComfyUI、webui。
人设/记忆只读 `E:\BN_agent\data\yueyue`，不写入。
