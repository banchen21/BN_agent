# 微信插件开发计划

> 状态：v2.0.0 | 自包含 WeChat Web 协议

## 一、微信绑定

**目标：插件自包含，零外部依赖。启动 → QR → 扫码 → 收发消息。**

### 协议层（protocol.rs）
- [x] `fetch_uuid()` — 从 login.weixin.qq.com 获取 UUID
- [x] `poll_login()` — 轮询扫码状态（窗口码 201/200/408）
- [x] `get_session()` — 扫码后获取 session（skey, wxsid, wxuin, pass_ticket）
- [x] `webwxinit()` — 初始化联系人 + 获取 SyncKey
- [x] `synccheck()` — 轮询新消息
- [x] `webwxsync()` — 拉取新消息
- [x] `send_msg()` — 发送文本消息
- [x] 二维码 PNG 生成（qrcode + image crate）

### 插件层（lib.rs）
- [x] 启动时自动获取 UUID + 生成二维码
- [x] 终端输出二维码图片路径 + URL
- [x] `wechat_qrcode` 工具 — 获取当前二维码信息
- [x] `wechat_send_message` 工具 — 发送消息
- [x] `snapshot()` — 连接状态注入 LLM 上下文
- [x] 后台轮询扫码 + 自动登录
- [x] 登录后消息轮询循环

### 待做
- [ ] 绑定成功后持久化凭证，断线重连
- [ ] 凭证过期 / 解绑的检测与提示
- [ ] 音频（语音接收 / 发送）

## 二、音频

### 接收（微信 → Bot）
- [x] 语音消息识别（WeChat Web `voice_item.text`）
- [x] 转发为 `user.message`（标记来源 `wechat`）

### 发送（Bot → 微信）
- [ ] ~~TTS + iLink send_voice~~ 当前 iLink 返回 `{"ret":-3}`，发送失败
- [ ] 排查 `ret:-3` 原因（音频格式？MIME？大小限制？）
- [ ] 改用 iLink `sendmessage` 发送 audio，或换其他方式
- [ ] "正在输入" 状态（iLink sendtyping 需要 `typing_ticket`）

## 三、视频

### 接收（微信 → Bot）
- [ ] 视频消息的 item 结构调研（`video_item` 字段）
- [ ] 视频文件下载 / CDN 解密
- [ ] 转发为 `user.message` 含 `video_base64`

### 发送（Bot → 微信）
- [ ] `wechat_send_video` 工具
- [ ] 视频格式转换支持

## 四、文件

### 接收（微信 → Bot）
- [ ] 文件消息的 item 结构调研（`file_item` 字段）
- [ ] 文件下载 / CDN 解密
- [ ] 转发为 `user.message` 含 `file_base64`

### 发送（Bot → 微信）
- [ ] `wechat_send_file` 工具
- [ ] 文件大小限制处理

## 五、架构优化

- [ ] 清理 dead code（`extract_media`、`tool_registry` 参数）
- [ ] `process_message` 用 `route.message` 走 MessageRouter 统一路由
- [ ] 统一媒体处理流程（CDN 下载 → 解密 → base64）
