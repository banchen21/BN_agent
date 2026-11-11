package com.bnagent.shiyu.data.repository

import android.content.Context
import com.bnagent.shiyu.data.local.ChatState
import com.bnagent.shiyu.data.local.ChatStore
import com.bnagent.shiyu.data.local.VoiceFileStore
import com.bnagent.shiyu.data.remote.AgentClient
import com.bnagent.shiyu.data.remote.AgentException
import com.bnagent.shiyu.data.remote.CallHistoryMessage
import com.bnagent.shiyu.data.remote.CallSession
import com.bnagent.shiyu.domain.model.Contact
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.domain.model.MessageSender
import com.bnagent.shiyu.domain.model.MessageStatus
import com.bnagent.shiyu.domain.model.UserProfile
import com.bnagent.shiyu.domain.model.conversationPreview
import com.bnagent.shiyu.domain.model.toContact
import com.bnagent.shiyu.domain.model.withStatus
import com.bnagent.shiyu.domain.model.withTime
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import java.time.LocalTime
import java.time.format.DateTimeFormatter
import java.util.UUID

/**
 * 会话仓库：UI 只读这里的状态；所有写操作（打开 / 发送 / 清空）都在这里原子落库。
 *
 * 数据流：
 * - [conversations]：已按「置顶优先、order 降序」排序的会话列表（消息页）；
 * - [allConversations]：存储顺序（通讯录、新聊天 sheet、文件选择等）；
 * - [contacts]：非群聊会话派生的联系人；
 * - [unreadTotal]：非静音会话未读总和（tabbar 徽标）；
 * - [messages]：单个会话的消息序列（含日期分隔）。
 *
 * 发送：先本地追加我方消息（已送达），再 POST /api/chat；
 * 成功 → 追加回复并把该会话我方消息全部置为已读；失败 → 我方消息标记「发送失败」并抛 [AgentException]。
 */
class ConversationRepository(
    private val store: ChatStore,
    private val settingsRepository: SettingsRepository,
    private val agentClient: AgentClient,
    private val appContext: Context,
    private val now: () -> LocalTime = { LocalTime.now() },
    private val newId: () -> String = { UUID.randomUUID().toString() },
) {

    val conversations: Flow<List<Conversation>> =
        store.state.map { it.sortedConversations() }.distinctUntilChanged()

    val allConversations: Flow<List<Conversation>> =
        store.state.map { it.conversations }.distinctUntilChanged()

    val contacts: Flow<List<Contact>> =
        store.state.map { state ->
            state.conversations.filterNot { it.isGroup }.map { it.toContact() }
        }.distinctUntilChanged()

    val profile: Flow<UserProfile> =
        store.state.map { it.profile }.distinctUntilChanged()

    val unreadTotal: Flow<Int> =
        store.state.map { it.unreadTotal() }.distinctUntilChanged()

    fun conversation(id: String): Flow<Conversation?> =
        store.state.map { it.conversation(id) }.distinctUntilChanged()

    fun messages(conversationId: String): Flow<List<Message>> =
        store.state.map { it.messages[conversationId].orEmpty() }.distinctUntilChanged()

    /** 进入聊天：未读清零 + order 递增（设计规格 5.1）。 */
    suspend fun openConversation(id: String) {
        store.update { state ->
            if (state.conversation(id) == null) return@update state
            val nextSeq = state.orderSeq + 1
            state.copy(
                orderSeq = nextSeq,
                conversations = state.conversations.map { conv ->
                    if (conv.id == id) conv.copy(unread = 0, order = nextSeq) else conv
                },
            )
        }
    }

    /**
     * 从后端 `GET /api/agents` 同步真实可对话 Agent：
     * - 已有会话保留消息与排序，只更新资料；
     * - 新 Agent 追加会话；
     * - 不在列表里的旧占位会话（含消息）删除。
     *
     * @return true 表示同步成功；网络失败时保留本地现状。
     */
    suspend fun syncAgents(): Boolean {
        val server = settingsRepository.serverSettings.first()
        val agents = runCatching { agentClient.agents(server) }.getOrNull().orEmpty()
        if (agents.isEmpty()) return false
        store.update { state ->
            val existing = state.conversations.associateBy { it.id }
            var seq = state.orderSeq
            val conversations = agents.map { agent ->
                val old = existing[agent.id]
                if (old != null) {
                    old.copy(
                        name = agent.name,
                        handle = agent.handle,
                        role = agent.role,
                        sign = agent.sign,
                        avatar = agent.avatar,
                        online = agent.online,
                    )
                } else {
                    seq += 1
                    Conversation(
                        id = agent.id,
                        name = agent.name,
                        handle = agent.handle,
                        role = agent.role,
                        sign = agent.sign,
                        avatar = agent.avatar,
                        online = agent.online,
                        timeLabel = "",
                        order = seq,
                        preview = Conversation.EMPTY_PREVIEW,
                    )
                }
            }
            val ids = agents.map { it.id }.toSet()
            state.copy(
                orderSeq = seq,
                conversations = conversations,
                messages = state.messages.filterKeys { it in ids },
            )
        }
        return true
    }

    /** 编辑本人资料（昵称 / 签名 / 头像资源名）。 */
    suspend fun updateProfile(name: String, signature: String, avatar: String) {
        store.update { state ->
            state.copy(
                profile = state.profile.copy(
                    name = name.trim().ifBlank { state.profile.name },
                    signature = signature.trim(),
                    avatar = avatar,
                ),
            )
        }
    }

    /** 本地语音缓存占用（字节）。 */
    fun cacheSizeBytes(): Long =
        VoiceFileStore.voiceDir(appContext).listFiles()?.sumOf { it.length() } ?: 0L

    /** 清理本地语音缓存；返回释放的字节数。 */
    fun clearCache(): Long {
        var freed = 0L
        VoiceFileStore.voiceDir(appContext).listFiles()?.forEach { file ->
            freed += file.length()
            file.delete()
        }
        return freed
    }

    /**
     * 发送文本到后端并落库回复。
     *
     * @return 后端回复消息。
     * @throws AgentException 网络 / 鉴权 / 服务端错误（此时我方消息保留并标记为「发送失败」）。
     */
    suspend fun sendText(conversationId: String, text: String): Message {
        val content = text.trim()
        require(content.isNotEmpty()) { "消息不能为空" }

        val snapshot = store.state.first()
        if (snapshot.conversation(conversationId) == null) {
            throw IllegalArgumentException("会话不存在：$conversationId")
        }
        val server = settingsRepository.serverSettings.first()
        val sentTime = now().format(TIME_FORMAT)
        val mine = Message.Text(
            id = newId(),
            sender = MessageSender.ME,
            content = content,
            time = sentTime,
            status = MessageStatus.SENT,
        )
        store.update { it.appendOutgoing(conversationId, mine, sentTime) }

        val reply = try {
            agentClient.chat(
                settings = server,
                peerId = AgentClient.peerIdFor(conversationId),
                message = content,
                userName = snapshot.profile.name,
            )
        } catch (error: AgentException) {
            store.update { it.markMessageFailed(conversationId, mine.id) }
            throw error
        }

        val replyTime = now().format(TIME_FORMAT)
        val replyMessage = Message.Text(
            id = newId(),
            sender = MessageSender.THEM,
            content = reply.content,
            time = replyTime,
        )
        store.update { it.appendAgentReply(conversationId, replyMessage, replyTime) }
        return replyMessage
    }

    /**
     * 发送语音消息：本地上传录音 → 后端 ASR → LLM → TTS。
     * 成功追加一条可播放的 Agent 语音回复（含转写），失败保留我方消息并置「发送失败」。
     */
    suspend fun sendVoice(
        conversationId: String,
        audioFile: java.io.File,
        durationSeconds: Int,
    ): Message {
        val snapshot = store.state.first()
        if (snapshot.conversation(conversationId) == null) {
            throw IllegalArgumentException("会话不存在：$conversationId")
        }
        val server = settingsRepository.serverSettings.first()
        val sentTime = now().format(TIME_FORMAT)
        val mine = Message.Voice(
            id = newId(),
            sender = MessageSender.ME,
            durationSeconds = durationSeconds,
            filePath = audioFile.absolutePath,
            time = sentTime,
            status = MessageStatus.SENT,
        )
        store.update { it.appendOutgoing(conversationId, mine, sentTime) }

        val reply = try {
            agentClient.voiceChat(
                settings = server,
                peerId = AgentClient.peerIdFor(conversationId),
                audio = audioFile.readBytes(),
                mime = "audio/mp4",
                userName = snapshot.profile.name,
            )
        } catch (error: AgentException) {
            store.update { it.markMessageFailed(conversationId, mine.id) }
            throw error
        } catch (error: Exception) {
            store.update { it.markMessageFailed(conversationId, mine.id) }
            throw error
        }

        // 把我方语音气泡补上 ASR 转写。
        store.update { it.setVoiceTranscript(conversationId, mine.id, reply.transcript) }

        val replyTime = now().format(TIME_FORMAT)
        val replyAudio = reply.audio?.let {
            runCatching { VoiceFileStore.saveAgentReply(appContext, conversationId, it) }.getOrNull()
        }
        val replyMessage: Message = if (replyAudio != null) {
            Message.Voice(
                id = newId(),
                sender = MessageSender.THEM,
                durationSeconds = reply.durationSeconds,
                filePath = replyAudio.absolutePath,
                transcript = reply.reply,
                time = replyTime,
            )
        } else {
            // TTS 不可用时至少保留文字回复，不让消息丢失。
            Message.Text(
                id = newId(),
                sender = MessageSender.THEM,
                content = reply.reply,
                time = replyTime,
            )
        }
        store.update { it.appendAgentReply(conversationId, replyMessage, replyTime) }
        return replyMessage
    }

    /**
     * 发送附件（图片 / 文件 / 语音）。附件只落本地，不触发后端请求；
     * 调用方负责构造 [Message.Image] / [Message.File] / [Message.Voice]（sender = ME）。
     */
    suspend fun sendAttachment(conversationId: String, message: Message) {
        require(message.sender == MessageSender.ME) { "只能发送我方附件" }
        if (store.state.first().conversation(conversationId) == null) {
            throw IllegalArgumentException("会话不存在：$conversationId")
        }
        val time = now().format(TIME_FORMAT)
        val timed = message.withTime(time)
        store.update { it.appendOutgoing(conversationId, timed, time) }
    }

    /** 创建 LiveKit 通话房间并取 token；把当前会话的近期聊天记录一起带给 Agent。 */
    suspend fun startCall(conversationId: String, mediaType: String): CallSession {
        val server = settingsRepository.serverSettings.first()
        val history = store.state.first().messages[conversationId].orEmpty()
            .mapNotNull { message ->
                when (message) {
                    is Message.Text -> CallHistoryMessage(
                        role = if (message.sender == MessageSender.ME) "user" else "assistant",
                        content = message.content,
                    )
                    is Message.Voice -> message.transcript
                        ?.takeIf { it.isNotBlank() }
                        ?.let {
                            CallHistoryMessage(
                                role = if (message.sender == MessageSender.ME) "user" else "assistant",
                                content = it,
                            )
                        }
                    else -> null
                }
            }
            .takeLast(16)
        return agentClient.startCall(
            settings = server,
            userId = "android",
            mediaType = mediaType,
            peerId = AgentClient.peerIdFor(conversationId),
            history = history,
        )
    }

    /** 挂断后通知后端删除房间；失败不阻断本地流程。 */
    suspend fun endCall(room: String) {
        if (room.isBlank()) return
        runCatching {
            agentClient.endCall(settingsRepository.serverSettings.first(), room)
        }
    }

    /** 清空单个会话记录（设计规格 5.8）：消息清空、预览回到「还没有消息」。 */    suspend fun clearMessages(conversationId: String) {
        store.update { state ->
            if (state.conversation(conversationId) == null) return@update state
            state.copy(
                messages = state.messages + (conversationId to emptyList()),
                conversations = state.conversations.map { conv ->
                    if (conv.id == conversationId) {
                        conv.copy(preview = Conversation.EMPTY_PREVIEW, timeLabel = "", unread = 0)
                    } else {
                        conv
                    }
                },
            )
        }
    }

    private companion object {
        val TIME_FORMAT: DateTimeFormatter = DateTimeFormatter.ofPattern("HH:mm")
    }
}

// ─── ChatState 原子写操作 ─────────────────────────────────────────────

private fun ChatState.appendOutgoing(
    conversationId: String,
    message: Message,
    timeLabel: String,
): ChatState {
    val conv = conversation(conversationId) ?: return this
    val nextSeq = orderSeq + 1
    return copy(
        orderSeq = nextSeq,
        conversations = conversations.map {
            if (it.id == conversationId) {
                it.copy(
                    preview = message.conversationPreview(conv.isGroup),
                    timeLabel = timeLabel,
                    order = nextSeq,
                    unread = 0,
                )
            } else {
                it
            }
        },
        messages = messages + (conversationId to (messages[conversationId].orEmpty() + message)),
    )
}

private fun ChatState.appendAgentReply(
    conversationId: String,
    reply: Message,
    timeLabel: String,
): ChatState {
    val conv = conversation(conversationId) ?: return this
    val nextSeq = orderSeq + 1
    val updatedMessages = (messages[conversationId].orEmpty() + reply).map { message ->
        if (message.sender == MessageSender.ME && message.status == MessageStatus.SENT) {
            message.withStatus(MessageStatus.READ)
        } else {
            message
        }
    }
    return copy(
        orderSeq = nextSeq,
        conversations = conversations.map {
            if (it.id == conversationId) {
                it.copy(
                    preview = reply.conversationPreview(conv.isGroup),
                    timeLabel = timeLabel,
                    order = nextSeq,
                    unread = 0,
                )
            } else {
                it
            }
        },
        messages = messages + (conversationId to updatedMessages),
    )
}

private fun ChatState.markMessageFailed(conversationId: String, messageId: String): ChatState {
    val list = messages[conversationId] ?: return this
    val updated = list.map { message ->
        if (message.id == messageId) message.withStatus(MessageStatus.FAILED) else message
    }
    return copy(messages = messages + (conversationId to updated))
}

/** 语音消息成功后补写 ASR 转写（只改指定消息，不动预览）。 */
private fun ChatState.setVoiceTranscript(
    conversationId: String,
    messageId: String,
    transcript: String,
): ChatState {
    if (transcript.isBlank()) return this
    val list = messages[conversationId] ?: return this
    val updated = list.map { message ->
        if (message is Message.Voice && message.id == messageId) {
            message.copy(transcript = transcript)
        } else {
            message
        }
    }
    return copy(messages = messages + (conversationId to updated))
}
