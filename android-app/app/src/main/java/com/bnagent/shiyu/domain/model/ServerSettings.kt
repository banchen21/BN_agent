package com.bnagent.shiyu.domain.model

import kotlinx.serialization.Serializable

/**
 * BN_agent 后端连接设置。
 *
 * - `baseUrl`：如 `http://192.168.0.101:18080`，可带或不带结尾 `/`；
 * - `apiKey`：可空；非空时请求同时携带 `Authorization: Bearer <key>` 与 `X-API-Key`。
 *
 * 默认地址指向本机启动的 BN_agent（HTTP_BIND 默认 18080，手机走局域网 IP）。
 * 若换机器 / 换端口，在「服务器设置」里改即可。
 */
@Serializable
data class ServerSettings(
    val baseUrl: String = DEFAULT_BASE_URL,
    val apiKey: String = "",
) {
    /** 去掉首尾空白与结尾斜杠，供拼 URL 使用。 */
    val normalizedBaseUrl: String
        get() = baseUrl.trim().trimEnd('/')

    val hasApiKey: Boolean
        get() = apiKey.isNotBlank()

    companion object {
        const val DEFAULT_BASE_URL = "http://192.168.0.101:18080"
    }
}

/** 应用级设置（服务器 + 通知开关）。主题设置见 ui/theme/ThemeSettings。 */
@Serializable
data class AppSettings(
    val server: ServerSettings = ServerSettings(),
    /** 设计规格 4.6：聊天通知默认开（本地演示开关）。 */
    val notificationsEnabled: Boolean = true,
)
