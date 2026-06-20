# 微信插件开发计划

> 状态：v0.2.0 | iLink Bot API（腾讯官方协议）| 插件内置私有实现
> 基座：`https://ilinkai.weixin.qq.com`

## ✅ 已完成

### 登录
- [x] QR 码登录流程（私有 `ILinkClient::builder().login()`）
- [x] 二维码 PNG 生成（终端/文件）
- [x] 自动刷新过期二维码（私有实现内部处理，最多 3 次）
- [x] token 持久化到 `data/wechat_session.json`，重启免扫码

### 文本消息
- [x] 长轮询收消息（35s 挂起）
- [x] 发送文本消息（含 context_token）
- [x] 自动管理 context_token（私有实现内置缓存）
- [x] 断线检测（errcode -14）→ 自动清除 token → 重新登录
- [x] `wechat_send_message` 工具

### 输入状态
- [x] `get_typing_ticket()` / `send_typing()`
- [x] 接收消息后自动显示"对方正在输入中"
- [x] 每 4s 保活，最长 30s，回复后自动停止

### 插件集成
- [x] `snapshot()` — 连接状态注入 LLM 上下文
- [x] 事件流：user.message → Pipeline → assistant.message → send_message
- [x] `wechat_qrcode` 工具
- [x] 后台登录/轮询线程

### iLink 实现
- [x] iLink 客户端内聚到插件内（类型安全 + 完整协议支持）
- [x] 删除 hand-rolled `protocol.rs`（~510 行）
- [x] 插件适配层 `client.rs`（`WeChatClient` 包装器）

## 🔧 待完善

### 图片消息
- [x] 基础设施：CDN 下载 + AES-128-ECB 解密 → base64（私有实现已集成）
- [x] 发送端：`wechat_send_image` 工具 + CDN 上传管线（私有实现已集成）
- [ ] **联调验证**：真实微信环境端到端测试
- [ ] 缩略图处理
- [ ] 大图/原图下载（hd_size 字段）

### 语音消息
- [x] 基础设施：CDN 下载 + 解密 → SILK → WAV（私有实现已集成，voice feature 可选）
- [ ] **联调验证**：真实微信环境端到端测试
- [ ] 启用 `voice` feature（需 libclang → silk-codec 编译）
- [ ] 与 asr-tts-plugin 深度集成（语音识别/合成 pipeline）
- [ ] 语音发送工具（WAV → SILK 编码 + CDN 上传）

### 视频消息
- [x] 基础设施：CDN 下载 + 解密 → base64（私有实现已集成）
- [x] 发送端：CDN 上传管线（私有实现已集成）
- [ ] **联调验证**：真实微信环境端到端测试
- [ ] 视频缩略图处理

### 文件消息
- [x] 基础设施：CDN 下载 + 解密 → base64 + file_name（SDK 已集成）
- [x] 发送端：`wechat_send_file` 工具 + 自动类型检测（SDK 已集成）
- [ ] **联调验证**：真实微信环境端到端测试

## 📋 待做

### 稳定性
- [ ] get_updates 超时优雅处理（客户端超时后自动重试，不丢消息）
- [ ] typing 循环异常保护（断线时优雅退出，避免资源泄漏）
- [ ] 日志分级（消息体脱敏，避免泄露用户内容）

### 架构优化
- [ ] 迁移到 SDK 的 `UpdatesStream`（Stream trait，内置退避/重连）
- [ ] 使用 SDK 的 `Store`（SQLite/Turso）替代 JSON 文件持久化
- [ ] 大文件 CDN 下载进度回调
