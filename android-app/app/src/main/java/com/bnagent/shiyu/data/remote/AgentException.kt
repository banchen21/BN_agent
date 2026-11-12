package com.bnagent.shiyu.data.remote

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import java.io.IOException

/**
 * 网络层明确错误类型。
 *
 * [userMessage] 是可以直接展示给用户的中文文案；UI 捕获后 toast / 错误行即可。
 */
sealed class AgentException(
    message: String,
    val userMessage: String,
    cause: Throwable? = null,
) : Exception(message, cause) {

    /** 服务器地址 / API Key 等配置问题（未发出请求）。 */
    class InvalidConfig(userMessage: String) :
        AgentException("invalid config: $userMessage", userMessage)

    /** DNS / 连接失败 / 连接被重置等。 */
    class Network(cause: IOException) :
        AgentException("network error: ${cause.message}", "无法连接服务器，请检查网络与服务器地址", cause)

    /** 60 秒超时（连接超时、读超时、call timeout）。 */
    class Timeout(cause: IOException) :
        AgentException("timeout: ${cause.message}", "服务器响应超时（60 秒），请稍后重试", cause)

    /** 401 / 403：API Key 缺失或错误。 */
    class Unauthorized(val code: Int) :
        AgentException(
            "unauthorized (HTTP $code)",
            "API Key 无效或缺失，请在服务器设置中检查",
        )

    /** 其他 4xx。 */
    class ClientError(val code: Int, val serverMessage: String) :
        AgentException(
            "client error (HTTP $code): $serverMessage",
            "请求失败（HTTP $code）：$serverMessage",
        )

    /** 5xx（含后端 502「LLM not configured」）。 */
    class ServerError(val code: Int, val serverMessage: String) :
        AgentException(
            "server error (HTTP $code): $serverMessage",
            "服务器开小差了（HTTP $code）：$serverMessage",
        )

    /** 2xx 但响应体不是预期 JSON / 内容为空。 */
    class InvalidResponse(val detail: String, cause: Throwable? = null) :
        AgentException("invalid response: $detail", "服务器返回了无法识别的内容", cause)
}

// ─── 错误响应解析（后端真实错误体：`{"error": "..."}`，401 为纯文本） ──────

private val ErrorJson = Json {
    ignoreUnknownKeys = true
    isLenient = true
}

private const val MAX_ERROR_CHARS = 200

/**
 * 从错误响应体里提取可读信息：
 * 1. `{"error": "..."}` / `{"error": {"message": "..."}}`；
 * 2. `{"detail": "..."}` / `{"message": "..."}` / `{"msg": "..."}`；
 * 3. 纯文本（如 Actix 的 `missing or invalid API key`）取第一行；
 * 4. HTML / 空体 → 按状态码给中文兜底。
 */
internal fun parseErrorMessage(body: String?, code: Int): String {
    val raw = body?.trim().orEmpty()
    if (raw.isNotEmpty()) {
        extractJsonMessage(raw)?.let { return it.take(MAX_ERROR_CHARS) }
        if (!raw.startsWith("<")) {
            val firstLine = raw.lineSequence().firstOrNull()?.trim().orEmpty()
            if (firstLine.isNotEmpty()) return firstLine.take(MAX_ERROR_CHARS)
        }
    }
    return defaultMessageFor(code)
}

private fun extractJsonMessage(raw: String): String? {
    val obj = runCatching { ErrorJson.parseToJsonElement(raw) }.getOrNull() as? JsonObject
        ?: return null
    val error = obj["error"]
    if (error is JsonPrimitive) {
        error.contentOrNull?.takeIf { it.isNotBlank() }?.let { return it }
    }
    if (error is JsonObject) {
        (error["message"] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }?.let { return it }
    }
    for (key in listOf("detail", "message", "msg", "error_message")) {
        (obj[key] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }?.let { return it }
    }
    return null
}

private fun defaultMessageFor(code: Int): String = when (code) {
    400 -> "请求格式不正确"
    401 -> "API Key 无效或缺失"
    403 -> "没有访问权限"
    404 -> "接口不存在，请检查服务器地址与版本"
    409 -> "请求冲突，请稍后重试"
    413 -> "请求内容过大"
    429 -> "请求过于频繁，请稍后重试"
    in 500..599 -> "服务器内部错误"
    else -> "请求失败"
}

/** HTTP 状态码 + 响应体 → 明确错误类型。 */
internal fun agentExceptionForHttp(code: Int, body: String?): AgentException {
    val parsed = parseErrorMessage(body, code)
    return when {
        code == 401 || code == 403 -> AgentException.Unauthorized(code)
        code in 400..499 -> AgentException.ClientError(code, parsed)
        else -> AgentException.ServerError(code, parsed)
    }
}
