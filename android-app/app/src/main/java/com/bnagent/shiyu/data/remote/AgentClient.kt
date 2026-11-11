package com.bnagent.shiyu.data.remote

import com.bnagent.shiyu.data.ShiYuJson
import com.bnagent.shiyu.domain.model.ServerSettings
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import java.io.IOException
import java.io.InterruptedIOException
import java.net.SocketTimeoutException
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * BN_agent 多会话后端客户端。
 *
 * 契约（backend-multisession 已验收）：
 * - `POST {base}/api/chat`，body `{message, peer_id, user_name, source}`，
 *   返回 `{content, model, prompt_tokens, completion_tokens, ...}`；
 * - `GET {base}/api/health` → `{status:"ok", version, uptime_secs, ...}`；
 * - API Key 可选：非空时同时带 `Authorization: Bearer <key>` 与 `X-API-Key`。
 *
 * 每个会话固定 `peer_id = "android:<convId>"`（见 [peerIdFor]）。
 * 超时：连接 15s，读/写/call 60s。所有错误统一抛 [AgentException]。
 */
class AgentClient(
    private val httpClient: OkHttpClient = defaultHttpClient(),
    private val json: Json = ShiYuJson,
) {

    /** 发送一条文本，返回后端回复；失败抛 [AgentException]。 */
    suspend fun chat(
        settings: ServerSettings,
        peerId: String,
        message: String,
        userName: String,
    ): ChatReply {
        val payload = ChatRequestDto(
            message = message,
            peerId = peerId,
            userName = userName,
            source = SOURCE,
        )
        val request = buildRequest(
            settings = settings,
            path = PATH_CHAT,
            body = json.encodeToString(ChatRequestDto.serializer(), payload)
                .toRequestBody(JSON_MEDIA_TYPE),
        )
        val body = execute(request)
        val dto = try {
            json.decodeFromString(ChatResponseDto.serializer(), body)
        } catch (e: SerializationException) {
            throw AgentException.InvalidResponse("/api/chat 响应解析失败：${e.message}", e)
        }
        val content = dto.content.trim()
        if (content.isEmpty()) {
            throw AgentException.InvalidResponse("/api/chat 返回了空消息")
        }
        return ChatReply(
            content = content,
            model = dto.model,
            promptTokens = dto.promptTokens,
            completionTokens = dto.completionTokens,
        )
    }

    /** 健康检查（连接测试）。非 2xx 抛 [AgentException]；2xx 解析 status。 */
    suspend fun health(settings: ServerSettings): HealthInfo {
        val request = buildRequest(settings = settings, path = PATH_HEALTH)
        val body = execute(request)
        val dto = try {
            json.decodeFromString(HealthResponseDto.serializer(), body)
        } catch (e: SerializationException) {
            throw AgentException.InvalidResponse("/api/health 响应解析失败：${e.message}", e)
        }
        return HealthInfo(
            ok = dto.status.equals("ok", ignoreCase = true),
            status = dto.status,
            version = dto.version,
        )
    }

    // ─── 内部实现 ─────────────────────────────────────────────────────

    private fun buildRequest(
        settings: ServerSettings,
        path: String,
        body: okhttp3.RequestBody? = null,
    ): Request {
        val base = settings.normalizedBaseUrl
        if (base.isEmpty()) {
            throw AgentException.InvalidConfig("请先填写服务器地址")
        }
        if (!base.startsWith("http://", ignoreCase = true) &&
            !base.startsWith("https://", ignoreCase = true)
        ) {
            throw AgentException.InvalidConfig("服务器地址需要以 http:// 或 https:// 开头")
        }
        val url = try {
            "$base$path".toHttpUrl()
        } catch (e: IllegalArgumentException) {
            throw AgentException.InvalidConfig("服务器地址格式不正确：$base")
        }
        return Request.Builder()
            .url(url)
            .header("Accept", "application/json")
            .apply {
                val key = settings.apiKey.trim()
                if (key.isNotEmpty()) {
                    header("Authorization", "Bearer $key")
                    header("X-API-Key", key)
                }
                if (body != null) post(body)
            }
            .build()
    }

    private suspend fun execute(request: Request): String =
        suspendCancellableCoroutine { cont ->
            val call = httpClient.newCall(request)
            cont.invokeOnCancellation { call.cancel() }
            call.enqueue(object : Callback {
                override fun onFailure(call: Call, e: IOException) {
                    if (!cont.isActive) return
                    runCatching { cont.resumeWithException(e.toAgentException()) }
                }

                override fun onResponse(call: Call, response: Response) {
                    response.use { resp ->
                        val body = try {
                            resp.body?.string().orEmpty()
                        } catch (e: IOException) {
                            if (cont.isActive) {
                                runCatching { cont.resumeWithException(AgentException.Network(e)) }
                            }
                            return
                        }
                        if (!cont.isActive) return
                        if (resp.isSuccessful) {
                            runCatching { cont.resume(body) }
                        } else {
                            runCatching {
                                cont.resumeWithException(agentExceptionForHttp(resp.code, body))
                            }
                        }
                    }
                }
            })
        }

    private fun IOException.toAgentException(): AgentException = when (this) {
        is SocketTimeoutException -> AgentException.Timeout(this)
        is InterruptedIOException -> AgentException.Timeout(this)
        else -> AgentException.Network(this)
    }

    companion object {
        /** 每个会话固定的后端 peer_id。 */
        fun peerIdFor(conversationId: String): String = "android:$conversationId"

        /** 60 秒口径：连接 15s，读写与整体调用 60s。 */
        fun defaultHttpClient(): OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(60, TimeUnit.SECONDS)
            .writeTimeout(60, TimeUnit.SECONDS)
            .callTimeout(60, TimeUnit.SECONDS)
            .retryOnConnectionFailure(true)
            .build()

        private const val SOURCE = "android"
        private const val PATH_CHAT = "/api/chat"
        private const val PATH_HEALTH = "/api/health"
        private val JSON_MEDIA_TYPE = "application/json; charset=utf-8".toMediaType()
    }
}

/** 后端回复。 */
data class ChatReply(
    val content: String,
    val model: String,
    val promptTokens: Long,
    val completionTokens: Long,
)

/** 健康检查结果。 */
data class HealthInfo(
    val ok: Boolean,
    val status: String,
    val version: String?,
) {
    /** 服务器设置页可直接展示的摘要。 */
    val summary: String
        get() = when {
            ok && !version.isNullOrBlank() -> "连接成功 · v$version"
            ok -> "连接成功"
            status.isBlank() -> "连接失败"
            else -> "连接异常（status=$status）"
        }
}

// ─── 线上 DTO（字段与 backend-multisession 契约一致） ────────────────────

@Serializable
internal data class ChatRequestDto(
    val message: String,
    @SerialName("peer_id") val peerId: String,
    @SerialName("user_name") val userName: String,
    val source: String,
)

@Serializable
internal data class ChatResponseDto(
    val content: String = "",
    val model: String = "",
    @SerialName("prompt_tokens") val promptTokens: Long = 0,
    @SerialName("completion_tokens") val completionTokens: Long = 0,
)

@Serializable
internal data class HealthResponseDto(
    val status: String = "",
    val version: String? = null,
    @SerialName("uptime_secs") val uptimeSecs: Long? = null,
)
