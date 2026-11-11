package com.bnagent.shiyu.data.local

import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.domain.model.UserProfile
import kotlinx.serialization.Serializable

/**
 * 会话数据持久化聚合（DataStore 里只存这一个 JSON 字符串）。
 *
 * 演示数据量很小（5 会话、约 20 条消息），整体序列化换来实现简单、
 * 原子读改写与跨会话一致性；后续要接服务端会话时可拆表。
 *
 * - [conversations] 保持存储顺序（lin/team/chen/zhao/wu）；
 * - [messages] 按会话 id 存消息序列；
 * - [orderSeq] 设计稿 5.4 的全局排序序号，初值 5。
 */
@Serializable
data class ChatState(
    val conversations: List<Conversation> = emptyList(),
    val messages: Map<String, List<Message>> = emptyMap(),
    val profile: UserProfile = UserProfile(),
    val orderSeq: Long = 5L,
) {
    /** 消息列表展示顺序：置顶优先，同组按 order 降序（设计规格 4.1 / 5.4）。 */
    fun sortedConversations(): List<Conversation> =
        conversations.sortedWith(
            compareByDescending<Conversation> { it.pinned }.thenByDescending { it.order },
        )

    /** tabbar 未读总数：静音会话不计入（设计规格 3.2 / 5.4）。 */
    fun unreadTotal(): Int = conversations.filterNot { it.muted }.sumOf { it.unread }

    fun conversation(id: String): Conversation? = conversations.firstOrNull { it.id == id }

    companion object {
        const val INITIAL_ORDER_SEQ = 5L
    }
}
