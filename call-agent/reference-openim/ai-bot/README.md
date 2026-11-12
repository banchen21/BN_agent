# 悦悦 AI 消息 Bot（OpenIM Companion · Phase 2）

FastAPI 服务（默认 `0.0.0.0:9000`），通过 OpenIM Webhook 接收发给 `ai_yueyue` 的单聊消息，
调用本地 AI 能力后以文字 / 图片 / 语音消息回给对方。

```
OpenIM 容器 ──webhook──▶ ai-bot :9000
                            ├─ 8877 /api/chat      对话（SSE）
                            ├─ 8877 /api/asr       入站语音转文字
                            ├─ 8877 /api/tts       出站语音合成（WAV）
                            ├─ 8190 /generate      ComfyUI Qwen-Image 生图
                            └─ MinIO :10005        图片 / 语音对象存储
```

## 启动顺序

1. **OpenIM 基础设施**（infra 负责，ai-bot 不碰 compose）
   ```bat
   cd E:\openim-companion\openim-docker
   docker compose up -d
   ```
2. **语音服务 8877**（先 9880 TTS 引擎，再 8877 server）
   ```bat
   E:\yueyue\call\start-tts.cmd
   D:\Python\Python313\python.exe E:\yueyue\call\server.py
   ```
3. **生图服务 8190**（依赖 ComfyUI 8188）
   ```bat
   D:\Python\Python313\python.exe E:\yueyue\tools\qwen_api.py --port 8190
   ```
4. **ai-bot**
   ```bat
   cd E:\openim-companion\ai-bot
   python -m venv .venv
   .venv\Scripts\python -m pip install -r requirements.txt
   start-bot.cmd
   ```
   或：
   ```bat
   .venv\Scripts\python.exe -m uvicorn main:app --host 0.0.0.0 --port 9000
   ```

启动时会自动：

- 确保 MinIO bucket `openim` 存在并设置公开读；
- 注册 `ai_yueyue`（已存在则跳过）；
- 给 `BOT_FRIENDS` 里的账号和 `ai_yueyue` 建立双向好友。

## 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `AI_BOT_HOST` / `AI_BOT_PORT` | `0.0.0.0` / `9000` | 监听地址 |
| `OPENIM_API_BASE` | `http://127.0.0.1:10002` | OpenIM API |
| `OPENIM_SECRET` | infra 提供的 secret | 管理员 token 用 |
| `OPENIM_ADMIN_USER` | `imAdmin` | 管理员 userID |
| `BOT_USER_ID` / `BOT_NICKNAME` | `ai_yueyue` / `悦悦` | Bot 账号 |
| `BOT_PASSWORD_MD5` | `md5("yueyue123")` | 注册字段；App 登录用明文 `yueyue123` |
| `BOT_FRIENDS` | `3370971303,4460069020` | 自动建立好友的账号，逗号分隔 |
| `YUEYUE_API_BASE` | `http://127.0.0.1:8877` | 对话 / ASR / TTS |
| `QWEN_IMAGE_API` | `http://127.0.0.1:8190` | 生图 API |
| `MINIO_ENDPOINT` | `127.0.0.1:10005` | MinIO（本机直传） |
| `MINIO_ROOT_USER` / `MINIO_ROOT_PASSWORD` | `root` / infra 密码 | MinIO 凭据 |
| `MINIO_BUCKET` | `openim` | 对象 bucket |
| `MINIO_PUBLIC_BASE` | `http://192.168.0.101:10005` | 对外媒体 URL 前缀 |
| `MINIO_INTERNAL_BASE` | `http://127.0.0.1:10005` | 入站媒体下载时替换用的内网地址 |
| `IMAGE_TIMEOUT_SECONDS` | `1200` | 单次生图超时 |
| `IMAGE_WAIT_CALL_SECONDS` | `60` | 通话中排队生图的最长等待 |

## HTTP 接口

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| POST | `/openim/webhook` | OpenIM 回调；立即返回 `{"actionCode":0,...}`，后台处理 |
| GET | `/health` | OpenIM / 8877 / 8190 / GPU 状态 |
| POST | `/ai/gpu/begin_call` | 通话开始，标记 GPU 占用（Phase 3 ai-agent 调用） |
| POST | `/ai/gpu/end_call` | 通话结束 |
| GET | `/ai/gpu/status` | `{in_call, image_running}` |
| POST | `/debug/send_text` | `{recvID, text}`，让 bot 直接发一条文字 |
| POST | `/debug/say` | `{recvID, text}`，绕过 webhook 跑完整对话/生图/语音链路 |

Webhook 只处理 `sessionType=1`、`recvID=ai_yueyue`、`sendID != ai_yueyue` 的消息；
用 `clientMsgID/msgID` 去重，重复回调不会回复两次。

## 消息能力

- **文字**：入站 `contentType=101` → `8877 /api/chat`（SSE 汇总）→ `contentType=101` 发回。
- **生图**：文本含「画 / 绘制 / 生成图 / 出图」等 → `8190 /generate` → 下载 PNG → MinIO →
  `contentType=102`（`PictureElem`，source/big/snapshot 三份齐全）。
- **语音**：文本含「说句话 / 语音 / 念一下」等 → 先对话再 `8877 /api/tts` → MinIO →
  `contentType=103`（`SoundElem`，带 bytes 与 duration）。
- **入站语音**：`contentType=103` → 下载音频 → `8877 /api/asr` → 当文字处理。
- **入站图片/文件/视频**：回执一句文字，不做进一步处理。

## GPU 协调

`gpu.py` 提供进程内 `GpuCoordinator`：

- 生图互斥（串行），避免两个 ComfyUI 任务同时抢显存；
- 通话期间生图先排队，最多等 `IMAGE_WAIT_CALL_SECONDS`，超时回复「正在打电话，晚点画」；
- Phase 3 的 `ai-agent` 在入会/离会时调用 `/ai/gpu/begin_call`、`/ai/gpu/end_call`，
  建房间前可先等 `wait_image_idle`（HTTP 侧无直接对应接口，Agent 进程内可直接 import）。

### 8GB 显存的硬限制（务必遵守）

RTX 4070 Laptop 只有 8GB 显存，**Qwen3-8B（llama）+ GSV-TTS + Qwen-Image 三者不能同时常驻**：

- 三者同时在场时，ComfyUI 会退化成疯狂换页：实测一张 768×768 / 8 步的图从平时 36~92 秒
  退化到 **10 分钟以上仍未出图**，同时把系统提交内存吃满，连 Gradle 构建都会被
  `WinError 1455（页面文件太小）` 杀掉。
- 因此 **生图与聊天/语音必须串行**：
  - 日常聊天/语音：只跑 8877 + 9880；
  - 生图：先停 8877/9880，让 ComfyUI 独占显存，出图后再按需恢复；
  - 生图结束后建议 `POST http://127.0.0.1:8188/free {"unload_models":true,"free_memory":true}`
    主动卸载模型，把显存和内存还回去。
- 本服务的 `gpu_lock` 只解决「并发」问题，**不能**解决「三套模型同时驻留」的显存容量问题，
  真正串行要靠启动/停止 8877、9880 与 ComfyUI 的运维顺序。

### 实测记录（2026-11-11，13800000003 ↔ ai_yueyue）

| 能力 | 条件 | 耗时 | 结果 |
| --- | --- | --- | --- |
| 文字 101 | 8877 已预热 | 13.4s | serverMsgID `8d0ae070…`，contentType=101 |
| 语音 103（TTS-only） | 仅 9880 | 合成 5.5s + 上传 0.13s + 发送 0.05s | 245,538B / 3.84s WAV，serverMsgID `f5ad9f5f…` |
| 图片 102 | ComfyUI 冷启动、768×768、8 步 | **136.2s**（含模型加载） | 1,038,384B PNG，serverMsgID `2645defe…` |

生图与 LLM+TTS 同时驻留时，首张曾跑 10 分钟仍未出图；ComfyUI 独占显存后恢复到 136s（冷）/ 正常热启动 36–92s。
生图结束建议 `POST http://127.0.0.1:8188/free {"unload_models":true,"free_memory":true}` 释放显存。

> **延迟口径**：上表都是宿主单侧时钟测量。AI 通话的 `active → agent audio published`：
> 首通（worker 重启后）冷启动约 **2.72s**，来自服务端同一份 LiveKit 日志（同钟真实间隔）；
> 最后一通 Android 热调用约 **0.387s**。**跨宿主/容器日志不可直接相减**（曾有 3.1s 的跨钟差值，已废弃）。
> LLM 首 token（1.88–2.25s）与 TTS 首包（0.23–0.51s）为 agent 单侧时钟，仍然有效。

## 测试

```bat
cd E:\openim-companion\ai-bot
.venv\Scripts\python.exe -m pytest -q
```

覆盖：指令识别、PictureElem/SoundElem 结构、WAV 时长、图片尺寸、webhook 回执/去重/解析、
Bot 自消息过滤、GPU 互斥与通话排队。

手工验证：

```bat
curl --noproxy * http://127.0.0.1:9000/health
curl --noproxy * -X POST http://127.0.0.1:9000/debug/send_text ^
  -H "Content-Type: application/json" -d "{\"recvID\":\"3370971303\",\"text\":\"你好\"}"

:: 绕过 webhook 直测三条链路
curl --noproxy * -X POST http://127.0.0.1:9000/debug/say ^
  -H "Content-Type: application/json" -d "{\"recvID\":\"3370971303\",\"text\":\"你好\"}"
curl --noproxy * -X POST http://127.0.0.1:9000/debug/say ^
  -H "Content-Type: application/json" -d "{\"recvID\":\"3370971303\",\"text\":\"画一只猫\"}"
curl --noproxy * -X POST http://127.0.0.1:9000/debug/say ^
  -H "Content-Type: application/json" -d "{\"recvID\":\"3370971303\",\"text\":\"说句话\"}"
```

验收（手机 / Electron 上操作）：

1. 用 `13800000003` 给 `ai_yueyue` 发「你好」→ 收到文字回复；
2. 发「画一只猫」→ 收到图片消息；
3. 发「说句话」→ 收到语音消息；
4. ai-bot 日志出现 `webhook callback=... sendID=...` 与 `发送文字/图片/语音 ...`。

## Media 上传策略

优先直传 MinIO（bucket 公开读，URL `http://192.168.0.101:10005/openim/ai-bot/<uuid>`）。
直传失败时自动回落到 OpenIM 对象通道（`/object/initiate_form_data` → 表单上传 → `complete_form_data`），
因此即使 MinIO 凭据或端口暂时不通，图片 / 语音仍能发出。

## 给 ai-agent（Phase 3）的可复用接口

- `GET /health`：`ai.image`、`ai.yueyue` 可判断底层是否就绪；
- `POST /ai/gpu/begin_call` / `end_call` / `GET /ai/gpu/status`：和消息生图共享同一把 GPU 锁；
- 8877 的 `/api/asr`、`/api/chat`（SSE）、`/api/tts_stream`（SSE PCM）可直接复用，
  本服务不占用它们的独占资源；
- `openim.py` 的 `OpenIMClient.send_msg` 可用于 Agent 侧发系统提示。
