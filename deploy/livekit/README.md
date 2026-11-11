# BN_agent LiveKit 通话后端

独立于 OpenIM 的 LiveKit 部署：LiveKit server（本目录）+ `call-livekit-plugin`（建房/token/dispatch）
+ 浏览器通话页（`/call`，PC / 手机通用）。

```
手机/PC 浏览器 ── HTTPS 18443 ──> main-app (0.0.0.0:18080 / 18443)
    │                                │  /api/call/start|end|health
    │                                ▼
    │                         call-livekit-plugin ── HTTP Twirp ──> LiveKit 127.0.0.1:7880
    │                                └── dispatch agent_name=yueyue
    │
    ├── wss 7443（Caddy TLS 终止）──> LiveKit 7880（信令）
    └── WebRTC 媒体：tcp 7881 / udp 7882-7892 ──> LiveKit
                                                    ▲
                                    call-agent (yueyue) ┘
```

## 1. 端口

| 端口 | 协议 | 用途 |
|---|---|---|
| 7880 | TCP | LiveKit HTTP + WebSocket 信令（服务端内部使用） |
| 7881 | TCP | WebRTC over TCP（UDP 不通时回退） |
| 7882-7892 | UDP | WebRTC 媒体 |
| 7443 | TCP | Caddy：wss/HTTPS 信令反代（浏览器必须用 wss） |
| 18080 | TCP | main-app HTTP：`/call` 页面 + `/api/call/*` |
| 18443 | TCP | main-app HTTPS：手机通话页（`getUserMedia` 需要安全上下文） |

本机局域网地址：**192.168.0.101**。换机器/网段时需要同步：
`livekit.yaml` 的 `rtc.node_ip`、`tls/` 证书 SAN（重新签发）、插件配置的 3 个 URL、
以及页面里 HTTPS 链接的端口（默认 18443）。

## 2. 部署 / 启停（Windows 原生，推荐）

前置：`bin/livekit-server.exe`（v1.9.10）与 `bin/caddy.exe`（v2.11.9）。
两者都从 GitHub Release 下载后解压到 `bin/`，`bin/` 已被 .gitignore 忽略。

```powershell
cd E:\BN_agent\deploy\livekit
copy livekit.example.yaml livekit.yaml   # 首次部署；填入 keys（与插件配置一致）
.\start.ps1    # 启动 livekit-server + Caddy，并做健康检查
.\status.ps1   # 进程、端口、HTTP/HTTPS 探活
.\stop.ps1     # 停止 Caddy + LiveKit
```

> `livekit.yaml` 含真实 key/secret，已被 `.gitignore` 忽略；仓库里只保留
> `livekit.example.yaml` 模板（`keys` 为 `CHANGE_ME`）。

> **为什么不用 Docker Desktop**：本机 Docker Desktop(WSL2) 的端口发布只在
> `127.0.0.1` 可用，局域网/手机访问不到；且 Docker/WSL 镜像网络会在 Windows 侧
> 预留 7880/7881，导致原生进程 bind 失败。当前实现使用 Windows 原生二进制。
> `docker-compose.yml` 仅作为其他环境的备选，且需要把 TLS 证书挂载进容器。

**端口被 WSL 预留的排查**：`netstat` 看不到监听但 bind 报
`Only one usage of each socket address` 时，先 `wsl --shutdown`（会停止所有 WSL 发行版，
包括 Docker Desktop 的 VM），再 `.\start.ps1`。Ubuntu 可用 `wsl -d Ubuntu-22.04 -e true` 重新拉起。

## 3. TLS / 证书

浏览器在非 localhost 的 HTTP 页面上没有 `navigator.mediaDevices`，手机必须走 HTTPS；
HTTPS 页面又只能连 `wss://`，而 LiveKit 7880 本身不提供 TLS，因此：

- **Caddy** 在 7443 终止 TLS，把 HTTP/WebSocket 反代到 `127.0.0.1:7880`（见 `Caddyfile`）。
- **main-app** 在 18443 提供同一个通话页（`HTTPS_BIND`）。
- 证书由 `tls/ca.crt` 本地 CA 签发，SAN 包含 `localhost / 127.0.0.1 / 192.168.0.101`。

文件（`tls/*.key`、`*.srl` 不进版本库）：

| 文件 | 用途 |
|---|---|
| `tls/ca.crt` / `tls/ca.key` | 本地 CA（给手机安装 `ca.crt`） |
| `tls/server.crt` / `tls/server.key` | 7443(wss) 与 18443(HTTPS) 共用 |

重新签发（IP 变更时）：

```bash
MSYS_NO_PATHCONV=1 openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
  -keyout tls/ca.key -out tls/ca.crt -subj "/CN=BN Agent Local CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
MSYS_NO_PATHCONV=1 openssl req -newkey rsa:2048 -sha256 -nodes \
  -keyout tls/server.key -out tls/server.csr -subj "/CN=192.168.0.101"
# 修改 tls/server.ext 的 SAN 后：
MSYS_NO_PATHCONV=1 openssl x509 -req -in tls/server.csr -CA tls/ca.crt -CAkey tls/ca.key \
  -CAcreateserial -out tls/server.crt -days 825 -sha256 -extfile tls/server.ext
```

安装 CA：

- **Windows（本机）**：已执行
  `certutil -user -addstore -f Root deploy\livekit\tls\ca.crt`（当前用户根证书）。
- **iOS**：Safari 打开 `http://192.168.0.101:18080/call/assets/ca.crt` 下载 →
  设置 → 通用 → VPN与设备管理 → 安装描述文件 → 通用 → 关于 → 证书信任设置 → 打开完全信任。
- **Android**：浏览器下载 `ca.crt` → 设置 → 安全 → 加密与凭据 → 安装证书 → CA 证书。
- 手机未装 CA 时，HTTPS/wss 会报证书错误，麦克风也不可用。

## 4. 配置

### 4.1 LiveKit server

`deploy/livekit/livekit.yaml`：

- `port: 7880`、`rtc.tcp_port: 7881`、`rtc.port_range_start/end: 7882/7892`
- `rtc.node_ip: "192.168.0.101"`（客户端看到并连接的媒体地址）
- `keys: { APIBnCall73: <secret> }`
- `room.empty_timeout: 300`（秒）、`departure_timeout: 20`
- TURN 默认关闭；公网/NAT 场景再启用 `turn` 并在插件 `ice_servers` 配置。

### 4.2 call-livekit-plugin

优先在 WebUI（`http://<host>:18080/ui/config`）配置；落盘位置
`data/plugin_configs/call-livekit-plugin.json`。字段：

| 字段 | 当前值 | 说明 |
|---|---|---|
| `livekit_url` | `wss://192.168.0.101:7443` | 返回给手机/浏览器的 `url` |
| `livekit_url_local` | `wss://127.0.0.1:7443` | 返回给浏览器 `urlLocal` |
| `api_url` | `http://127.0.0.1:7880` | 服务端 Twirp（本机直连，不走 Caddy） |
| `api_key` / `api_secret` | 与 livekit.yaml 一致 | LiveKit 鉴权 |
| `api_ca_cert` | 空 | 仅当 `api_url` 为 https 时填写 CA 路径 |
| `agent_name` | `yueyue` | start 时 dispatch 的 agent |
| `room_prefix` | `ai-call-` | 房间名前缀 |
| `default_media_type` | `audio` | `audio` / `video` |
| `ice_servers` | `[]` | 额外 STUN/TURN URL 列表 |
| `turn_username` / `turn_password` | 空 | TURN 凭据 |
| `empty_timeout_secs` | `300` | 房间无人自动关闭 |
| `token_ttl_secs` | `3600` | 参与方 token 有效期 |

环境变量 fallback：`LIVEKIT_URL`、`LIVEKIT_URL_LOCAL`、`LIVEKIT_API_URL`、
`LIVEKIT_API_KEY`、`LIVEKIT_API_SECRET`、`LIVEKIT_AGENT_NAME`、`LIVEKIT_API_CA_CERT`。

保存配置后 WebUI 自动重载插件，`start()` 重新读取即生效。

### 4.3 main-app

`main-app/.env`：

```ini
HTTP_BIND=0.0.0.0:18080          # 8080 已被 backend-73Info 占用
HTTPS_BIND=0.0.0.0:18443
TLS_CERT=E:/BN_agent/deploy/livekit/tls/server.crt
TLS_KEY=E:/BN_agent/deploy/livekit/tls/server.key
PLUGIN_CONFIG_DIR=E:/BN_agent/data/plugin_configs
CALL_DEPLOY_DIR=E:/BN_agent/deploy/livekit
API_KEY=<E:\BN_agent\.secrets\api_key.txt>
# 必须包含 call 插件，否则 /api/call/* 会 404
PLUGIN_LOAD=call-livekit-plugin,...
```

`API_KEY` 启用后：`/api/call/start`、`/api/call/end` 需要 `X-API-Key: <key>`
或 `Authorization: Bearer <key>`；`/api/call/health`、`/call`、`/call/assets/*` 免鉴权
（页面内请求仍带 key）；`/api/call/*` 的 CORS 预检（OPTIONS）免鉴权。

## 5. 接口

统一返回 JSON。除 health 外均要求 API Key（若服务端启用）。

### POST /api/call/start

```bash
curl -X POST http://127.0.0.1:18080/api/call/start \
  -H "Content-Type: application/json" \
  -H "X-API-Key: $(cat E:/BN_agent/.secrets/api_key.txt)" \
  -d '{"userID":"alice","mediaType":"audio"}'
```

```json
{
  "url": "wss://192.168.0.101:7443",
  "urlLocal": "wss://127.0.0.1:7443",
  "token": "<participant JWT>",
  "room": "ai-call-alice-1762850000123",
  "identity": "user-alice",
  "mediaType": "audio",
  "agent": { "name": "yueyue", "dispatched": true, "dispatchId": "..." },
  "iceServers": [],
  "expiresInSecs": 3600
}
```

- 房间名：`<room_prefix><userID>-<毫秒时间戳>` → `ai-call-<userID>-<ts>`
- 参与方 identity：默认 `user-<userID>`；可传 `identity` 覆盖
- 同时调用 `AgentDispatchService/CreateDispatch`，`agent_name=<agent_name>`
- agent worker 未上线时接口仍返回 200，`agent.dispatched=false` 且带 `agent.warning`
- 同名房间已存在不报错（时间戳保证唯一）

### POST /api/call/end

```bash
curl -X POST http://127.0.0.1:18080/api/call/end \
  -H "Content-Type: application/json" \
  -H "X-API-Key: <key>" \
  -d '{"room":"ai-call-alice-1762850000123"}'
```

返回 `{"deleted":true,...}`；房间已不存在时 `{"deleted":false}`（幂等）。删除会强制关闭房间。

### GET /api/call/health

```bash
curl http://127.0.0.1:18080/api/call/health
```

```json
{"status":"ok","livekit":true,"apiUrl":"http://127.0.0.1:7880",
 "url":"wss://192.168.0.101:7443","agentName":"yueyue","configured":true,"rooms":0}
```

LiveKit 不可达时返回 503。

### GET /api/call/rooms

当前房间列表：`{"rooms":[{"name","sid","numParticipants","creationTime"}]}`。

> 注意：LiveKit `ListRooms` 的 `numParticipants` **不统计 AGENT 类型参与者**，agent 入会后
> 这里仍可能显示 0。通话页按 participant kind/identity（`agent` 前缀）判断“悦悦已入会”，
> 不依赖该字段；需要精确人数时用 `ListParticipants` 并过滤 kind。

### 插件原始入口

`/api/plugin/call-livekit-plugin/{start,end,health,rooms}`（同一套能力，无额外 CORS）。

## 6. 浏览器 / 手机通话页

- **手机（推荐）**：`https://192.168.0.101:18443/call?key=<API_KEY>`
  （先按第 3 节安装 `ca.crt`）
- **本机 PC**：`https://127.0.0.1:18443/call` 或 `http://127.0.0.1:18080/call`
  （localhost 属于安全上下文；wss 仍需本机信任 CA）
- 局域网 HTTP（`http://192.168.0.101:18080/call`）会显示黄色提示并引导到 HTTPS；
  该页面无法使用麦克风/摄像头。

功能：

- 输入用户 ID、选择语音/视频 → 开始呼叫
- 请求麦克风/摄像头，发布音视频；agent 音频自动播放
- 静音、开关摄像头、挂断；连接状态（连接中/已连接/重连/断开）
- agent 入会提示；API Key 保存在浏览器 localStorage，也可用 `?key=<key>` 带入一次
- 页面关闭时用 `fetch keepalive` 调用 `/api/call/end`（带 API Key）

页面静态依赖：`deploy/livekit/static/livekit-client.umd.js`
（livekit-client 2.23.0，本地加载失败时回退 unpkg CDN）。

## 7. 与 call-agent 的约定

- **room**：`ai-call-<userID>-<毫秒时间戳>`（前缀可通过 `room_prefix` 改）
- **participant identity**：默认 `user-<userID>`；agent 侧把 `user-` 开头且非
  AGENT kind 的参与者视为真人
- **dispatch**：`CreateDispatch { room, agent_name: "yueyue", metadata }`；
  agent worker 必须以 `agent_name="yueyue"` 注册（`WorkerOptions(agent_name="yueyue")`）
- **agent 连接地址**：同机直接用 `ws://127.0.0.1:7880`（无需 TLS/CA）；不要用 7443
- **metadata**（JSON 字符串）：
  `{"userID","identity","mediaType","room","source":"bn-call"}`
- agent 音频轨发布后页面自动订阅播放；视频轨自动挂到主画面
- agent 加入后以 `agent` 前缀或 AGENT kind 出现，页面显示“悦悦已入会”

## 8. 测试 / 排障

```powershell
cd E:\BN_agent\deploy\livekit
.\status.ps1

# 后端
curl http://127.0.0.1:18080/api/call/health
curl -X POST http://127.0.0.1:18080/api/call/start -H "Content-Type: application/json" -H "X-API-Key: <key>" -d '{"userID":"qa1","mediaType":"audio"}'
# 记下返回的 room
curl http://127.0.0.1:18080/api/call/rooms -H "X-API-Key: <key>"
curl -X POST http://127.0.0.1:18080/api/call/end -H "Content-Type: application/json" -H "X-API-Key: <key>" -d '{"room":"<room>"}'
curl http://127.0.0.1:18080/api/call/rooms -H "X-API-Key: <key>"   # 期望 []

# 插件
cargo test -p call-livekit-plugin
cargo check -p call-livekit-plugin --all-targets
cargo check -p main-app --all-targets
```

常见问题：

- **health 503 / 建房失败**：LiveKit 未启动；`api_key/api_secret` 与 `livekit.yaml`
  不一致；`api_url` 不通。
- **bind 7880/7881 失败（WSL 预留）**：`wsl --shutdown` 后重新 `.\start.ps1`。
- **手机页面提示不安全 / 无麦克风**：用了 HTTP；改 HTTPS 18443 并安装 CA。
- **HTTPS 证书错误**：CA 未安装；或 IP 变更后证书 SAN 不匹配，需重新签发。
- **手机能打开页面但连不上媒体**：确认同网段，UDP 7882-7892 未被防火墙拦截
  （本机防火墙当前关闭；如启用需放行 TCP 7443/7880/7881/18080/18443 与 UDP 7882-7892）。
- **agent 未入会**：call-agent 未启动或未以 `agent_name=yueyue` 注册；
  `start` 的 `agent.warning` 会给出原因，真人音视频仍可用。
- **401**：页面 API Key 未填 / 与 `E:\BN_agent\.secrets\api_key.txt` 不一致。
- **main-app 启动即段错误**：`plugin-interface` 变更后旧插件 DLL 的 vtable 不兼容。
  执行 `cargo build -p <plugin>` 重建插件；`target/debug/hello_plugin.dll` 是无源码的
  陈旧产物，建议删除或保持 `PLUGIN_SKIP=hello-plugin`。当前 `.env` 用 `PLUGIN_LOAD` 白名单规避。
- **/api/call/* 404**：`PLUGIN_LOAD` 未包含 `call-livekit-plugin`，或 DLL 未构建。

## 9. 安全说明

- `livekit.yaml`、`data/plugin_configs/call-livekit-plugin.json`、`tls/*.key`
  仅用于本地开发；对外暴露前更换 key/secret、限制访问。
- `/call` 页面本身不含 secret；API Key 存在使用者浏览器 localStorage。
- 8080/18080 与 18443 监听 0.0.0.0；如不需要手机访问，可改回 127.0.0.1。
- 生产建议：反向代理启用公网可信证书、启用 TURN、限制 keys 权限与房间配额。
