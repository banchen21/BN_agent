package com.bnagent.shiyu.data

import com.bnagent.shiyu.data.local.ChatState
import com.bnagent.shiyu.data.seed.SeedData
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.domain.model.MessageSender
import com.bnagent.shiyu.domain.model.headerSubtitle
import com.bnagent.shiyu.domain.model.rolePrimary
import com.bnagent.shiyu.domain.model.toContact
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** 种子数据完整性：删掉模拟联系人后只保留真实 Agent「悦悦」。 */
class SeedDataTest {

    private val state = SeedData.state

    @Test
    fun `single real agent conversation`() {
        assertEquals(listOf("yueyue"), state.conversations.map { it.id })
        val agent = state.conversations.single()
        assertEquals("齐悦", agent.name)
        assertEquals("yueyue", agent.handle)
        assertEquals("AI Agent · 语音 / 视频", agent.role)
        assertEquals("agent_yueyue", agent.avatar)
        assertTrue(agent.online)
        assertTrue(!agent.isGroup)
        assertTrue(state.messages.isEmpty())
    }

    @Test
    fun `profile matches design mock`() {
        val profile = state.profile
        assertEquals("苏雨", profile.name)
        assertEquals("shiyu_2049", profile.handle)
        assertEquals("上海", profile.city)
        assertEquals("先把话说清楚，再把事做漂亮。", profile.signature)
        assertEquals("avatar_me", profile.avatar)
        assertEquals("拾语号 shiyu_2049 · 上海", profile.handleLine)
    }

    @Test
    fun `derived conversation display rules`() {
        val agent = state.conversations.single()
        assertEquals("在线", agent.headerSubtitle)
        assertEquals("AI Agent", agent.rolePrimary)
        assertEquals(listOf("yueyue"), state.conversations.map { it.toContact().conversationId })
    }

    @Test
    fun `all seeded assets resolve to drawables`() {
        val assetNames = buildSet {
            add(state.profile.avatar)
            state.conversations.forEach { add(it.avatar) }
        }
        assetNames.forEach { name ->
            assertNotNull("素材未入库：$name", StockMedia.drawableFor(name))
        }
        assertNotNull(StockMedia.drawableFor(StockMedia.AGENT_YUEYUE))
    }

    @Test
    fun `chat state json round trip preserves voice transcript`() {
        val withMessage = state.copy(
            messages = mapOf(
                "yueyue" to listOf(
                    Message.Voice(
                        id = "v1",
                        sender = MessageSender.THEM,
                        durationSeconds = 6,
                        filePath = "/tmp/reply.wav",
                        transcript = "在呢，一直都在。",
                        time = "09:30",
                    ),
                ),
            ),
        )
        val encoded = ShiYuJson.encodeToString(ChatState.serializer(), withMessage)
        val decoded = ShiYuJson.decodeFromString(ChatState.serializer(), encoded)
        assertEquals(withMessage, decoded)
        val voice = decoded.messages.getValue("yueyue").single() as Message.Voice
        assertEquals("在呢，一直都在。", voice.transcript)
        assertEquals(6, voice.durationSeconds)
    }

    @Test
    fun `conversation serializer round trip keeps defaults`() {
        val agent = state.conversations.single()
        val encoded = ShiYuJson.encodeToString(Conversation.serializer(), agent)
        assertEquals(agent, ShiYuJson.decodeFromString(Conversation.serializer(), encoded))
    }
}
