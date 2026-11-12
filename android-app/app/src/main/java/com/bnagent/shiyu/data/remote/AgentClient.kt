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

    /** 通讯录真实 Agent 列表（`GET /api/agents`）。 */
    suspend fun agents(settings: ServerSettings): List<AgentInfo> {
        val request = buildRequest(settings = settings, path = PATH_AGENTS)
        val body = execute(request, timeoutSeconds = 20)
        val dto = try {
            json.decodeFromString(AgentListResponseDto.serializer(), body)
        } catch (e: SerializationException) {
            throw AgentException.InvalidResponse("/api/agents 响应解析失败：${e.message}", e)
        }
        return dto.agents.map {
            AgentInfo(
                id = it.id,
                name = it.name,
                handle = it.handle,
                role = it.role,
                sign = it.sign,
                avatar = it.avatar,
                online = it.online,
            )
        }
    }

    /**
     * 语音消息：上传录音，后端走本地 ASR → LLM → 本地 TTS。
     * 返回转写文本、文字回复，以及可选的回语音频（WAV）。
     */
    suspend fun voiceChat(
        settings: ServerSettings,
        peerId: String,
        audio: ByteArray,
        mime: String,
        userName: String,
    ): VoiceChatReply {
        val payload = VoiceChatRequestDto(
            audioBase64 = java.util.Base64.getEncoder().encodeToString(audio),
            mime = mime,
            peerId = peerId,
            userName = userName,
            source = SOURCE,
        )
        val request = buildRequest(
            settings = settings,
            path = PATH_VOICE,
            body = json.encodeToString(VoiceChatRequestDto.serializer(), payload)
                .toRequestBody(JSON_MEDIA_TYPE),
        )
        val body = execute(request, timeoutSeconds = 240)
        val dto = try {
            json.decodeFromString(VoiceChatResponseDto.serializer(), body)
        } catch (e: SerializationException) {
            throw AgentException.InvalidResponse("/api/chat/voice 响应解析失败：${e.message}", e)
        }
        val reply = dto.reply.trim()
        if (reply.isEmpty()) {
            throw AgentException.InvalidResponse("/api/chat/voice 返回了空回复")
        }
        val audioBytes = dto.audioBase64.takeIf { it.isNotBlank() }?.let {
            runCatching { java.util.Base64.getDecoder().decode(it) }.getOrNull()
        }
        return VoiceChatReply(
            transcript = dto.transcript.trim(),
            reply = reply,
            audio = audioBytes,
            durationSeconds = dto.durationSeconds.coerceAtLeast(1),
            ttsError = dto.ttsError,
        )
    }

    /** 创建 LiveKit 房间并获取参与方 token（后端派发 yueyue agent）。 */
    suspend fun startCall(
        settings: ServerSettings,
        userId: String,
        mediaType: String,
        peerId: String,
        history: List<CallHistoryMessage> = emptyList(),
    ): CallSession {
        val payload = CallStartRequestDto(
            userID = userId,
            mediaType = mediaType,
            peerId = peerId,
            history = history.map { CallHistoryMessageDto(it.role, it.content) },
        )
        val request = buildRequest(
            settings = settings,
            path = PATH_CALL_START,
            body = json.encodeToString(CallStartRequestDto.serializer(), payload)
                .toRequestBody(JSON_MEDIA_TYPE),
        )
        val body = execute(request, timeoutSeconds = 30)
        val dto = try {
            json.decodeFromString(CallStartResponseDto.serializer(), body)
        } catch (e: SerializationException) {
            throw AgentException.InvalidResponse("/api/call/start 响应解析失败：${e.message}", e)
        }
        if (dto.token.isBlank() || dto.room.isBlank()) {
            throw AgentException.InvalidResponse("/api/call/start 缺少 token/room")
        }
        return CallSession(
            url = dto.url,
            urlLocal = dto.urlLocal,
            token = dto.token,
            room = dto.room,
            identity = dto.identity,
            mediaType = dto.mediaType,
            expiresInSecs = dto.expiresInSecs,
            peerId = peerId,
        )
    }

    /** 挂断后删除房间；失败不阻断本地挂断流程。 */
    suspend fun endCall(settings: ServerSettings, room: String) {
        val payload = json.encodeToString(
            CallEndRequestDto.serializer(),
            CallEndRequestDto(room = room),
        )
        val request = buildRequest(
            settings = settings,
            path = PATH_CALL_END,
            body = payload.toRequestBody(JSON_MEDIA_TYPE),
        )
        runCatching { execute(request, timeoutSeconds = 15) }
    }

    /**
     * 轮询 Agent 主动发起的来电；没有待接来电时返回 null。
     * 服务端返回即消费该邀请（接听后用返回的 token 直接入会）。
     */
    suspend fun incomingCall(settings: ServerSettings): CallSession? {
        val request = buildRequest(settings = settings, path = PATH_CALL_INCOMING)
        val body = execute(request, timeoutSeconds = 15)
        val dto = try {
            json.decodeFromString(IncomingCallResponseDto.serializer(), body)
        } catch (e: SerializationException) {
            throw AgentException.InvalidResponse("/api/call/incoming 响应解析失败：${e.message}", e)
        }
        val invite = dto.invite ?: return null
        if (invite.token.isBlank() || invite.room.isBlank()) return null
        return CallSession(
            url = invite.url,
            urlLocal = invite.urlLocal,
            token = invite.token,
            room = invite.room,
            identity = invite.identity,
            mediaType = invite.mediaType,
            expiresInSecs = invite.expiresInSecs,
            peerId = invite.peerId,
        )
    }

    /** 拒绝 Agent 来电：让后端删除房间；失败不阻断本地流程。 */
    suspend fun declineCall(settings: ServerSettings, room: String) {
        val payload = json.encodeToString(
            CallEndRequestDto.serializer(),
            CallEndRequestDto(room = room),
        )
        val request = buildRequest(
            settings = settings,
            path = PATH_CALL_DECLINE,
            body = payload.toRequestBody(JSON_MEDIA_TYPE),
        )
        runCatching { execute(request, timeoutSeconds = 15) }
    }

    /** 下载通话中 Agent 发来的图片（相对路径，如 `/api/call/image/x.png`）。 */
    suspend fun fetchImage(settings: ServerSettings, path: String): ByteArray {
        val request = buildRequest(settings = settings, path = path)
        return executeBytes(request, timeoutSeconds = 30)
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

    private suspend fun execute(request: Request, timeoutSeconds: Long = 60): String =
        suspendCancellableCoroutine { cont ->
            val call = httpClient.newCall(request)
            // OkHttp 客户端默认 60s 整体超时；语音链路（ASR+TTS）需要更长。
            call.timeout().timeout(timeoutSeconds, TimeUnit.SECONDS)
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

    /** 二进制响应（图片下载）；错误处理与 [execute] 一致。 */
    private suspend fun executeBytes(request: Request, timeoutSeconds: Long = 60): ByteArray =
        suspendCancellableCoroutine { cont ->
            val call = httpClient.newCall(request)
            call.timeout().timeout(timeoutSeconds, TimeUnit.SECONDS)
            cont.invokeOnCancellation { call.cancel() }
            call.enqueue(object : Callback {
                override fun onFailure(call: Call, e: IOException) {
                    if (!cont.isActive) return
                    runCatching { cont.resumeWithException(e.toAgentException()) }
                }

                override fun onResponse(call: Call, response: Response) {
                    response.use { resp ->
                        val bytes = try {
                            resp.body?.bytes()
                        } catch (e: IOException) {
                            if (cont.isActive) {
                                runCatching { cont.resumeWithException(AgentException.Network(e)) }
                            }
                            return
                        }
                        if (!cont.isActive) return
                        if (resp.isSuccessful) {
                            runCatching { cont.resume(bytes ?: ByteArray(0)) }
                        } else {
                            runCatching {
                                cont.resumeWithException(
                                    agentExceptionForHttp(
                                        resp.code,
                                        bytes?.toString(Charsets.UTF_8).orEmpty(),
                                    ),
                                )
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
        private const val PATH_VOICE = "/api/chat/voice"
        private const val PATH_HEALTH = "/api/health"
        private const val PATH_AGENTS = "/api/agents"
        private const val PATH_CALL_START = "/api/call/start"
        private const val PATH_CALL_END = "/api/call/end"
        private const val PATH_CALL_INCOMING = "/api/call/incoming"
        private const val PATH_CALL_DECLINE = "/api/call/decline"
        private val JSON_MEDIA_TYPE = "application/json; charset=utf-8".toMediaType()
    }
}

/** 可直接对话的 Agent（通讯录数据源）。 */
data class AgentInfo(
    val id: String,
    val name: String,
    val handle: String,
    val role: String,
    val sign: String,
    val avatar: String,
    val online: Boolean,
)

/** 后端回复。 */
data class ChatReply(
    val content: String,
    val model: String,
    val promptTokens: Long,
    val completionTokens: Long,
)

/** 语音消息后端结果：转写 + 文字回复 + 可选回语音频（WAV）。 */
data class VoiceChatReply(
    val transcript: String,
    val reply: String,
    val audio: ByteArray?,
    val durationSeconds: Int,
    val ttsError: String? = null,
) {
    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        if (other !is VoiceChatReply) return false
        return transcript == other.transcript &&
            reply == other.reply &&
            durationSeconds == other.durationSeconds &&
            ttsError == other.ttsError &&
            (audio?.contentEquals(other.audio) ?: (other.audio == null))
    }

    override fun hashCode(): Int {
        var result = transcript.hashCode()
        result = 31 * result + reply.hashCode()
        result = 31 * result + (audio?.contentHashCode() ?: 0)
        result = 31 * result + durationSeconds
        result = 31 * result + (ttsError?.hashCode() ?: 0)
        return result
    }
}

/** 通话前聊天记录（用于保持通话与文字聊天的主体一致性）。 */
data class CallHistoryMessage(
    val role: String,
    val content: String,
)

/** LiveKit 通话会话（/api/call/start 返回，或 Agent 来电邀请）。 */
data class CallSession(
    val url: String,
    val urlLocal: String,
    val token: String,
    val room: String,
    val identity: String,
    val mediaType: String,
    val expiresInSecs: Long,
    /** Agent 来电时带上的后端 peer_id（如 `android:yueyue`）。 */
    val peerId: String = "",
) {
    /**
     * 本机 LAN 联调用的信令地址：服务端返回 wss://host:7443（Caddy 自签证书），
     * Android 端走明文 ws://host:7880，避免自签证书信任问题。
     */
    val localWsUrl: String
        get() {
            val raw = url.ifBlank { urlLocal }
            return raw
                .replaceFirst("wss://", "ws://")
                .replaceFirst(":7443", ":7880")
        }
}

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

@Serializable
internal data class AgentListResponseDto(
    val agents: List<AgentDto> = emptyList(),
)

@Serializable
internal data class AgentDto(
    val id: String = "",
    val name: String = "",
    val handle: String = "",
    val role: String = "",
    val sign: String = "",
    val avatar: String = "",
    val online: Boolean = false,
    val kind: String = "agent",
)

@Serializable
internal data class VoiceChatRequestDto(
    @SerialName("audio_base64") val audioBase64: String,
    val mime: String,
    @SerialName("peer_id") val peerId: String,
    @SerialName("user_name") val userName: String,
    val source: String,
)

@Serializable
internal data class VoiceChatResponseDto(
    val transcript: String = "",
    val reply: String = "",
    @SerialName("audio_base64") val audioBase64: String = "",
    @SerialName("audio_mime") val audioMime: String = "audio/wav",
    @SerialName("duration_secs") val durationSeconds: Int = 1,
    @SerialName("tts_error") val ttsError: String? = null,
)

@Serializable
internal data class CallStartRequestDto(
    // 后端字段名就是 userID（大写 ID）
    @SerialName("userID") val userID: String,
    val mediaType: String,
    @SerialName("peer_id") val peerId: String = "",
    val history: List<CallHistoryMessageDto> = emptyList(),
)

@Serializable
internal data class CallHistoryMessageDto(
    val role: String,
    val content: String,
)

@Serializable
internal data class CallStartResponseDto(
    val url: String = "",
    @SerialName("urlLocal") val urlLocal: String = "",
    val token: String = "",
    val room: String = "",
    val identity: String = "",
    val mediaType: String = "audio",
    @SerialName("expiresInSecs") val expiresInSecs: Long = 0,
    // 仅 /api/call/incoming 的邀请里带（发起方 peer_id）
    val peerId: String = "",
)

@Serializable
internal data class IncomingCallResponseDto(
    val invite: CallStartResponseDto? = null,
)

@Serializable
internal data class CallEndRequestDto(
    val room: String,
)
