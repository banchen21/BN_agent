# 微信插件开发计划

> 状态：v0.2.0 | iLink Bot API（腾讯官方协议）
> 基座：`https://ilinkai.weixin.qq.com`

## ✅ 已完成

### 登录
- [x] `fetch_qrcode()` — 获取二维码 + QR 内容
- [x] `poll_qrcode()` — 轮询扫码状态（wait → scaned → confirmed / expired）
- [x] 二维码 PNG 生成（qrcode + image crate）
- [x] 自动刷新过期二维码（最多 3 次）
- [x] token 持久化到 `data/wechat_session.json`，重启免扫码

### 消息收发
- [x] `get_updates()` — 长轮询收消息（35s 挂起）
- [x] `send_message()` — 发送文本消息（含 context_token）
- [x] 自动管理 context_token（按用户缓存）
- [x] 断线检测（errcode -14）→ 自动清除 token → 重新登录
- [x] `wechat_send_message` 工具
- [x] `wechat_qrcode` 工具

### 输入状态
- [x] `get_typing_ticket()` — 获取 typing 凭证（POST /getconfig）
- [x] `send_typing()` — 发送输入状态（POST /sendtyping）
- [x] 接收消息后自动显示"对方正在输入中"
- [x] 每 5 秒保活，最长 30 秒
- [x] 回复发送后自动停止
- [x] typing_ticket 按用户缓存（≈24h）

### 插件集成
- [x] `snapshot()` — 连接状态注入 LLM 上下文
- [x] 事件流：user.message → Pipeline → assistant.message → send_message
- [x] 2 个注册工具
- [x] 后台登录/轮询线程

## 📋 待做

### 消息接收增强
- [ ] 图片消息（CDN 下载 + AES-128-ECB 解密 → base64 → user.message）
- [ ] 语音消息（CDN 下载 + SILK 转码 → user.message）
- [ ] 视频消息（CDN 下载 → user.message）
- [ ] 文件消息（CDN 下载 → user.message）

### 消息发送增强
- [ ] `wechat_send_image` 工具（CDN 上传 + AES 加密）
- [ ] `wechat_send_voice` 工具
- [ ] `wechat_send_file` 工具
- [ ] `wechat_send_video` 工具

### 媒体 CDN
- [ ] `get_upload_url()` — 获取 CDN 上传参数
- [ ] AES-128-ECB 加密/解密（PKCS7 填充）
- [ ] CDN 文件上传/下载

### 稳定性
- [ ] get_updates 空响应超时处理（客户端超时后自动重试）
- [ ] typing 循环异常保护（断线时优雅退出）
- [ ] 日志分级（消息体脱敏）

### 架构
- [ ] 统一媒体处理流程（CDN → 解密 → 转码 → base64）
- [ ] 与 asr-tts-plugin 集成（语音识别/合成）
