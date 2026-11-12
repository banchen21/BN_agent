# 拾语 · Android 客户端

「拾语」是 BN_agent 的 Android 原生 IM 客户端（Kotlin + Jetpack Compose + Material 3），
通过 BN_agent 的多会话 HTTP 接口收发消息。

- 包名 / applicationId：`com.bnagent.shiyu`
- 版本：`2.4.0`（versionCode `20400`）
- minSdk 26 / compileSdk 36 / targetSdk 36（Android 16，edge-to-edge）
- UI 唯一依据：[`docs/design-spec.md`](docs/design-spec.md)（设计稿 412×915，1 CSS px ≈ 1 dp）

## 功能一览

- **消息列表**：会话来自后端 `GET /api/agents`（当前真实 Agent「齐悦」；启动时同步，删除旧占位联系人），
  搜索（名字+预览）；右上角「+」暂为占位（toast），底栏未读总数不含静音会话。
- **聊天**：文本走真实后端（乐观插入 → 等待期间「正在输入」→ 回复 / 失败标记「发送失败」+ toast）；
  图片走系统 Photo Picker 或「+ → 拍照」（系统相机，FileProvider 写入 cache），支持全屏查看器；
  通话中拍的图/选的图还会经 LiveKit 数据通道（topic `shiyu.photo`）发给 Agent，进入通话对话上下文，Agent 可用视觉能力回应；文件（演示文件池，下载按钮提示「演示环境，暂不下载」）；
  语音为真实录音（MediaRecorder）→ 上传 `POST /api/chat/voice`（后端本地 SenseVoice ASR → LLM → 本地 GSV-TTS），
  返回转写文本与可播放的语音回复（MediaPlayer）；群聊显示发送者名；含日期分隔、已读/已送达状态、
  清空聊天确认弹窗；聊天内可进个人资料 / 气泡与主题 / 通话。
  键盘：停在最新时内容随键盘上推；已向上滑动时只把输入框顶起，键盘覆盖消息区。
- **通讯录 / 个人资料**：列表来自真实 Agent（`/api/agents`），在线点、搜索、资料卡、发消息 / 语音 / 视频入口。
- **我**：资料卡、编辑资料（昵称/签名/头像）、清理本地语音缓存、气泡与主题、深色模式、聊天通知、
  关于拾语、服务器设置入口。
- **气泡与主题**：5 套气泡色（珊瑚/松绿/靛蓝/石墨/芥黄）、3 档圆角（10/18/26dp + 尾角 6dp）、
  3 档聊天字号（16/17/19sp）、3 种聊天背景（冷白/雾灰/深墨，含点阵）、实时预览、应用内深色开关；
  修改即时全局生效并持久化（DataStore）。
- **服务器设置**：Base URL、API Key（可空，密码态）、测试连接（`GET /api/health`）、保存。
- **通话（真实 LiveKit）**：`POST /api/call/start` 建房取 token，App 连接 `ws://<PC>:7880` 加入房间，
  后端派发 yueyue agent；麦克风/摄像头为真实 WebRTC 轨，远端音频自动播放；静音 / 免提 / 摄像头翻转 /
  挂断均为真实操作，挂断后 `POST /api/call/end` 删除房间。视频通话会发布摄像头，Agent 的 vision 工具可看到画面。
  右上角为**最小化**（系统返回键同）：只退出通话页、不挂断，通话由进程级 `CallManager` 继续；
  App 内全应用顶部出现通话悬浮条，**退到桌面/其他应用后**显示系统级通话浮窗（语音=头像计时，视频=本地摄像头画面，
  点一下回到通话页，点红色按钮挂断），由前台服务保活、不影响音视频；需要用户允许「显示在其他应用上层」。
  挂断只在底部红色按钮 / 悬浮条 / 浮窗挂断按钮。
  语音通话中可随时开/关摄像头；视频画面支持拖动小窗（限制在屏幕内）、点击小窗交换大小屏；Agent 大屏画面使用悦悦自拍。
- **通话中发图**：call-agent 根据 LLM 的 `<send_photo>` 标签调用后端 `comfyui_generate_image` 生成图片，
  经 LiveKit 数据通道（topic `shiyu.image`）把下载路径发给客户端；App 用 `GET /api/call/image/{name}` 下载后
  追加一条对方图片消息，并在通话页弹出可点掉的图片浮层（约 1-3 分钟出图）。
- **通话记录与 Agent 来电**：通话结束后在消息列表写入通话记录气泡（「语音通话：mm:ss」/「视频通话：mm:ss」，
  我发起在右侧、Agent 来电在左侧），会话预览同步；Agent 可由 call-agent 通过 `POST /api/call/invite`
  登记来电，App 在前台每 4s 轮询 `GET /api/call/incoming`，弹出全屏来电页（接听 / 拒绝，
  拒绝走 `POST /api/call/decline`）；接听后直接用邀请里的 token 入会。通话中 Agent 主动离开
  （LLM 输出 `<end_call/>` 或 `<call_back/>`）时，App 自动结束通话、提示「对方已挂断」并回到聊天页；
  `<call_back/>` 会在 Agent 挂断后登记一条新的来电，实现「挂断后回拨」。
  通话会把当前会话最近 16 条聊天记录随 `/api/call/start` 带给 Agent，保持通话与文字聊天的主体一致性；
  通话结束后 call-agent 会把本通电话的 ASR 文本与回复回写 `/api/chat/append`，
  仅用于下一次聊天/通话的上下文，不在消息列表里生成文字气泡（避免干扰）。

## 环境要求

| 项 | 要求 |
|---|---|
| JDK | **JDK 17**（AGP/Gradle 要求）。Windows `PATH` 上的 `java` 可能是 8，务必设置 `JAVA_HOME=C:\Program Files\Microsoft\jdk-17.0.18.8-hotspot` 后用 `gradlew.bat` 构建。 |
| Android SDK | `platforms;android-36`、`build-tools;36.0.0`；SDK 路径写在 `local.properties` 的 `sdk.dir`（该文件不入库）。 |
| Gradle | 工程自带 Gradle wrapper 8.11.1；`distributionUrl` 指向腾讯云镜像（当前网络直连 `services.gradle.org` 会超时，海外环境可改回官方源）。无需本机预装 Gradle。 |
| 网络 | 首次构建需联网下载 AGP / Kotlin / Compose / OkHttp 等依赖。 |

## 构建、安装与检查

```bash
cd E:\BN_agent\android-app

# 1) 编译 Debug APK
gradlew.bat :app:assembleDebug
# 产物：app/build/outputs/apk/debug/app-debug.apk（约 20 MB）

# 2) 单元测试（29 个用例：种子数据 / 错误解析 / peer_id / 主题序列化）
gradlew.bat :app:testDebugUnitTest

# 3) Lint（当前 0 error / 6 warning）
gradlew.bat :app:lintDebug

# 4) 安装并启动（设备已授权 USB/无线调试）
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n com.bnagent.shiyu/.MainActivity
```

## 服务器与联调

### 1. 启动后端

在仓库根目录启动 BN_agent（HTTP 默认端口 18080，本机 `.env` 里 `HTTP_BIND=0.0.0.0:18080`）：

```bash
cargo run -p main-app
```

- `API_KEY` 未设置时后端不鉴权；本机实际 key 在 `.secrets/api_key.txt`（勿提交 / 勿打印到日志）。
- 手机与 PC 需在同一局域网；PC 本机地址为 `192.168.0.101`。

### 2. App 内配置

进入 **我 → 服务器**：

- Base URL：默认 `http://192.168.0.101:18080`（可在 `ServerSettings.DEFAULT_BASE_URL` 或 App 内修改）。
- API Key：可选。填写后请求同时携带 `Authorization: Bearer <key>` 与 `X-API-Key: <key>`。
- debug 包预置：在 `android-app/local.properties`（不入库）写 `shiyu.apiKey=<key>`，
  构建时会注入 `BuildConfig.DEFAULT_API_KEY`，首次安装即带鉴权；release 包恒为空。
- 「测试连接」调用 `GET /api/health`，显示 `连接成功 · v<version>` / 401 / 超时 / 不可达等状态。

### 3. adb reverse（手机不在同一网段时的备选）

```bash
# 设备 127.0.0.1:8080 → PC 127.0.0.1:18080
adb reverse tcp:8080 tcp:18080
# 然后在 App 服务器设置里填 http://127.0.0.1:8080

# 或者直接同端口映射，填 http://127.0.0.1:18080（T7 验收即此方式）
adb reverse tcp:18080 tcp:18080

# 清理
adb reverse --remove-all
```

### 4. 接口契约（多会话）

- `POST /api/chat`，body：`{ "message": "...", "peer_id": "android:<convId>", "user_name": "苏雨", "source": "android" }`；
  返回 `{ content, model, prompt_tokens, completion_tokens, ... }`。
- 每个会话固定 `peer_id = android:<会话 id>`（如 `android:lin`），后端据此维护**每会话独立的 agent 上下文**。
- `GET /api/health`：健康检查（免鉴权）。
- `GET /api/chat/history?peer_id=...`：按会话拉取历史（源码已实现；见下方旧实例说明）。
- `POST /api/call/start` / `POST /api/call/end`：建房 / 删房（通话页）。
- `POST /api/call/invite`：Agent 侧登记来电（call-agent 在请求回拨时调用）。
- `GET /api/call/incoming`：取走待接来电（`{"invite": {...}}` 或 `{"invite": null}`）。
- `POST /api/call/decline`：拒接来电，删除房间。
- 客户端超时：连接 15s，读写与整体调用 60s；错误统一映射为中文 toast。

### 5. ⚠ 当前 18080 运行的是改动前旧二进制

本机 18080 进程启动于后端多会话改造之前（`main.rs` 17:24 才修改），因此：

- `GET /api/chat/history` 返回 404，`peer_id` 新契约未在运行实例上生效；
- **源码已实现并通过** `cargo check --workspace --all-targets` 与 `cargo test -p main-app`（111 个测试）；
- 需要重启（`cargo run -p main-app`）或重新部署后，Android 端才能用上新的历史 / 多会话能力。

### 6. LLM Key 与错误表现

- 后端 `main-app/.env` 的 `LLM_API_KEY` 失效时，`POST /api/chat` 返回 **502**，App 会把服务端错误文案以 toast 展示，我方消息标记「发送失败」。
- 更换 `LLM_API_KEY` 后重启后端即可；API Key（HTTP 鉴权）与 LLM Key 是两个不同的配置。

### 7. 明文 HTTP 说明

`AndroidManifest.xml` 中 `android:usesCleartextTraffic="true"` 仅用于本地局域网 HTTP 联调（否则 Android 9+ 会直接拒绝明文请求）。
生产环境应改用 HTTPS，或使用 `network-security-config` 限定明文域名。

## 已验证环境与验收证据

| 设备 | 系统 | 结果 |
|---|---|---|
| `192.168.0.100:5555` | Xiaomi Android 16（SDK 36） | T7 全量验收：构建 / 单测 / lint / 冷启动 / 逐页截图 / adb reverse 联调 |
| `192.168.0.103:5555` | Android 14 | 安装与启动冒烟 |

- 截图证据（74 张，本机构建产物，不入库）：`android-app/build/verification/`
- T7 结论：`assembleDebug` PASS、`testDebugUnitTest` 29/29 PASS、`lintDebug` 0 error / 6 warning、
  冷启动全程无 FATAL / ANR、`adb reverse` + 后端三态（200 / 401 / 不可达）联调 PASS。
- 注意：验收期间在设备上留下的测试消息（hello-verify、toast-test、1:15 语音等）不是首装种子数据；
  卸载重装可恢复设计稿种子。

## 架构说明

```
app/src/main/java/com/bnagent/shiyu/
├── MainActivity.kt              # 单 Activity；注入主题控制器与 AppContainer
├── ShiYuApplication.kt          # 创建进程内唯一 AppContainer，首装种子写入
├── data/
│   ├── AppContainer.kt          # 手写 DI：Store / Repository / AgentClient
│   ├── ShiYuJson.kt             # kotlinx.serialization Json 配置
│   ├── StockMedia.kt            # 内置素材与发送池映射
│   ├── local/                   # DataStore：ChatStore / SettingsStore / VoiceFileStore
│   ├── remote/                  # OkHttp AgentClient + AgentException（中文错误文案）
│   ├── repository/              # ConversationRepository / SettingsRepository
│   └── seed/SeedData.kt         # 首装种子（仅首次启动写入）
├── domain/model/                # Conversation / Message / Contact / UserProfile / ServerSettings
└── ui/
    ├── AppRoot.kt               # 根部：NavHost + tabbar 显隐 + Toast 宿主 + 系统栏 chrome
    ├── theme/                   # ShiYuTheme：颜色 / 字号 / 圆角 / 背景 / 深色 / 持久化
    ├── components/              # Avatar / AppBar / TabBar / Badge / Switch / SegmentedControl / Sheet / Toast / Icons
    ├── navigation/              # ShiYuRoutes + NavHost（chats、chat/{id}、contacts、profile/{id}、me、
    │                            #   settings/theme、settings/server、call/voice|video/{id}）
    ├── chats/ chat/             # 消息列表与聊天（ViewModel + Compose）
    ├── contacts/ profile/ me/   # 通讯录 / 资料 / 我
    ├── settings/                # 气泡与主题、服务器设置
    └── call/                    # LiveKit 语音 / 视频通话
```

- **UI**：单 Activity + Compose Navigation；tabbar 只在 `chats / contacts / me` 显示；通话页强制深色 chrome。
- **状态**：`ConversationRepository` 为唯一写入口，UI 通过 Flow 只读；会话与消息以 JSON 存在 DataStore Preferences
  （`shiyu_chat`），应用设置存 `shiyu_settings`，主题存 `shiyu_theme`。
- **网络**：OkHttp + kotlinx.serialization；`peer_id` 在 `AgentClient.peerIdFor()` 统一生成。
- **主题**：设计令牌集中在 `ui/theme/Color.kt` / `Type.kt`，`ShiYuTheme.colors / dimens / settings` 下发；
  深色为应用内手动开关（不跟随系统）。
- **媒体**：Coil 加载头像与图片（`res/drawable-nodpi`）、Photo Picker 选图、MediaRecorder/MediaPlayer 录放音。

## 已知限制

- **通话浮窗需要「显示在其他应用上层」权限**：未授权时退到桌面不会显示浮窗（有一次性提示），通话仍由前台服务保活。
  浮窗位置固定在右上区域（暂不支持拖动浮窗本身）。
- **通话依赖本机 LiveKit**：7880/tcp 信令、7881/tcp 与 7882-7892/udp 媒体、call-agent 8081；
  LAN 联调使用 `ws://<PC>:7880`（绕过 Caddy 自签证书），生产环境应改为 wss + 可信证书。
- **语音消息依赖本机 yueyue 8877**：`/api/asr`（SenseVoice）+ `/api/tts`（GSV-TTS），后端用 ffmpeg 把 m4a 转 16k wav。
- **LLM Key**：`LLM_API_KEY` 失效会导致 `/api/chat` 与语音回复 502；需更换 key 并重启后端。
- **群聊回复无 sender 名**：后端只返回文本内容，群聊自动回复不携带发送者，UI 展示为对方消息。
- **文件下载为演示**：点击下载仅 toast「演示环境，暂不下载」。
- **已读状态口径**：仅当会话最后一条是「我方」消息时显示状态（design-spec 6.3 的歧义项），
  后端回复到达后先前气泡的「已读」会隐藏，待产品确认。
- **Release 未验收**：release 构建未开启混淆 / 签名，当前只交付 debug 验收包。
- Toast 已加 IME padding；极端键盘/悬浮窗场景仍可能被系统浮层遮挡。
