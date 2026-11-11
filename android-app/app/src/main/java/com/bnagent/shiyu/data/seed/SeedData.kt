package com.bnagent.shiyu.data.seed

import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.data.local.ChatState
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.domain.model.UserProfile

/**
 * 首装种子数据。
 *
 * 团队决策：删掉设计稿的 5 个模拟联系人，只保留真实可对话的 Agent「悦悦」。
 * 启动后由 [com.bnagent.shiyu.data.repository.ConversationRepository.syncAgents]
 * 调用后端 `GET /api/agents` 覆盖/扩展列表；离线时使用这里的兜底数据。
 */
object SeedData {

    val profile = UserProfile()

    val conversations: List<Conversation> = listOf(
        Conversation(
            id = "yueyue",
            name = "齐悦",
            handle = "yueyue",
            role = "AI Agent · 语音 / 视频",
            sign = "叫我悦悦就行。",
            avatar = StockMedia.AGENT_YUEYUE,
            online = true,
            timeLabel = "",
            order = 1,
            preview = Conversation.EMPTY_PREVIEW,
        ),
    )

    val messages: Map<String, List<Message>> = emptyMap()

    /** 首装状态（ChatStore 首次启动写入）。 */
    val state: ChatState = ChatState(
        conversations = conversations,
        messages = messages,
        profile = profile,
        orderSeq = ChatState.INITIAL_ORDER_SEQ,
    )
}
