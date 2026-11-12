package com.bnagent.shiyu.domain.model

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import java.util.Locale

/**
 * 消息方向。
 *
 * 与设计稿一致：`me` 我方（右侧气泡）、`them` 对方（左侧气泡）、`system` 日期分隔。
 */
@Serializable
enum class MessageSender {
    @SerialName("me")
    ME,

    @SerialName("them")
    THEM,

    @SerialName("system")
    SYSTEM,
}

/**
 * 我方消息状态。对方消息与系统分隔没有状态（null）。
 *
 * `SENT`（已送达）/ `READ`（已读）来自设计稿；`FAILED` 用于后端发送失败时
 * 保留本地乐观消息并给出明确状态（UI 可显示「发送失败」）。
 */
@Serializable
enum class MessageStatus(val label: String) {
    @SerialName("sent")
    SENT("已送达"),

    @SerialName("read")
    READ("已读"),

    @SerialName("failed")
    FAILED("发送失败"),
}

/**
 * 单条消息（设计规格 6.3）。
 *
 * 持久化：kotlinx.serialization 多态（sealed interface + `type` 判别字段）。
 * - 文本 / 图片 / 文件 / 语音 / 系统分隔 5 种；
 * - `who` 仅群聊对方消息用于显示发送者名，单聊为 null；
 * - `time` 为展示用时间字符串（`09:14`），系统分隔放在 [Message.System.text]；
 * - `status` 仅我方消息有值。
 */
@Serializable
sealed interface Message {
    val id: String
    val sender: MessageSender
    val time: String?
    val status: MessageStatus?

    /** 群聊发送者显示名；单聊与我方消息为 null。 */
    val who: String?

    @Serializable
    @SerialName("text")
    data class Text(
        override val id: String,
        override val sender: MessageSender,
        val content: String,
        override val time: String? = null,
        override val status: MessageStatus? = null,
        override val who: String? = null,
    ) : Message

    @Serializable
    @SerialName("image")
    data class Image(
        override val id: String,
        override val sender: MessageSender,
        /** 资源名（如 `photo_cat`），用 StockMedia.drawableFor 映射到 drawable。 */
        val asset: String,
        /** 查看器说明与预览用（如「橙色猫咪躺在沙发上」）。 */
        val alt: String? = null,
        override val time: String? = null,
        override val status: MessageStatus? = null,
        override val who: String? = null,
    ) : Message

    @Serializable
    @SerialName("file")
    data class File(
        override val id: String,
        override val sender: MessageSender,
        val fileName: String,
        /** 展示用大小（如 `2.4 MB`）。 */
        val sizeLabel: String,
        override val time: String? = null,
        override val status: MessageStatus? = null,
        override val who: String? = null,
    ) : Message

    @Serializable
    @SerialName("voice")
    data class Voice(
        override val id: String,
        override val sender: MessageSender,
        val durationSeconds: Int,
        /** 真实录音 / 回语音频文件路径（应用私有目录）；种子演示语音为 null。 */
        val filePath: String? = null,
        /** 语音转写文本（我方为 ASR 结果，对方为 TTS 原文）；种子/纯演示语音为 null。 */
        val transcript: String? = null,
        override val time: String? = null,
        override val status: MessageStatus? = null,
        override val who: String? = null,
    ) : Message

    /** 通话记录：语音 / 视频 + 接通时长（0 = 未接通）。 */
    @Serializable
    @SerialName("call")
    data class Call(
        override val id: String,
        override val sender: MessageSender,
        val video: Boolean,
        val durationSeconds: Int,
        override val time: String? = null,
        override val status: MessageStatus? = null,
        override val who: String? = null,
    ) : Message

    @Serializable
    @SerialName("system")
    data class System(
        override val id: String,
        /** 日期分隔文案，如「今天 09:12」。 */
        val text: String,
    ) : Message {
        override val sender: MessageSender get() = MessageSender.SYSTEM
        override val time: String? get() = null
        override val status: MessageStatus? get() = null
        override val who: String? get() = null
    }
}

/** 语音时长展示：`m:ss`（12 → `0:12`）。 */
val Message.Voice.durationLabel: String
    get() = String.format(Locale.ROOT, "%d:%02d", durationSeconds / 60, durationSeconds % 60)

/** 通话时长展示：`mm:ss`（与通话页计时一致）。 */
val Message.Call.durationLabel: String
    get() = String.format(Locale.ROOT, "%02d:%02d", durationSeconds / 60, durationSeconds % 60)

/** 通话记录气泡文案：`语音通话：03:53` / `视频通话：未接通`。 */
val Message.Call.summary: String
    get() {
        val kind = if (video) "视频通话" else "语音通话"
        return if (durationSeconds > 0) "$kind：$durationLabel" else "$kind：未接通"
    }

/** 设计规格 5.3 的 previewOf 规则（不含群聊「谁：」前缀）。 */
fun Message.previewText(): String = when (this) {
    is Message.Text -> content
    is Message.Image -> "图片 · ${alt?.takeIf { it.isNotBlank() } ?: "图片消息"}"
    is Message.File -> "文件 · $fileName"
    is Message.Voice -> "语音 · $durationLabel"
    is Message.Call -> summary
    is Message.System -> text
}

/** 会话列表预览：群聊里对方的消息加「发送者：」前缀；我方消息不加。 */
fun Message.conversationPreview(isGroup: Boolean): String {
    val base = previewText()
    return if (isGroup && sender == MessageSender.THEM && !who.isNullOrBlank()) "$who：$base" else base
}

/** 复制一份带指定状态的消息（系统消息原样返回）。 */
fun Message.withStatus(status: MessageStatus?): Message = when (this) {
    is Message.Text -> copy(status = status)
    is Message.Image -> copy(status = status)
    is Message.File -> copy(status = status)
    is Message.Voice -> copy(status = status)
    is Message.Call -> copy(status = status)
    is Message.System -> this
}

/** 复制一份带指定展示时间的消息（系统消息原样返回）。 */
fun Message.withTime(time: String?): Message = when (this) {
    is Message.Text -> copy(time = time)
    is Message.Image -> copy(time = time)
    is Message.File -> copy(time = time)
    is Message.Voice -> copy(time = time)
    is Message.Call -> copy(time = time)
    is Message.System -> this
}
