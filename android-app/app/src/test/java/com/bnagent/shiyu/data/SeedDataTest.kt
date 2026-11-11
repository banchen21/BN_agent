package com.bnagent.shiyu.data

import com.bnagent.shiyu.data.local.ChatState
import com.bnagent.shiyu.data.seed.SeedData
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.domain.model.MessageSender
import com.bnagent.shiyu.domain.model.MessageStatus
import com.bnagent.shiyu.domain.model.commonGroupLabel
import com.bnagent.shiyu.domain.model.conversationPreview
import com.bnagent.shiyu.domain.model.durationLabel
import com.bnagent.shiyu.domain.model.headerSubtitle
import com.bnagent.shiyu.domain.model.pickNote
import com.bnagent.shiyu.domain.model.previewText
import com.bnagent.shiyu.domain.model.profileTags
import com.bnagent.shiyu.domain.model.remark
import com.bnagent.shiyu.domain.model.rolePrimary
import com.bnagent.shiyu.domain.model.toContact
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** 种子数据完整性：逐项对照 design-spec 第 6 节。 */
class SeedDataTest {

    private val state = SeedData.state

    @Test
    fun `five conversations in storage order`() {
        assertEquals(
            listOf("lin", "team", "chen", "zhao", "wu"),
            state.conversations.map { it.id },
        )
        assertEquals(
            listOf("林夏", "设计组", "陈屿", "赵敏", "吴其"),
            state.conversations.map { it.name },
        )
    }

    @Test
    fun `conversation fields match design mock`() {
        val lin = state.conversations.first { it.id == "lin" }
        assertEquals("lin_xia", lin.handle)
        assertEquals("上海", lin.city)
        assertEquals("设计 · 视觉组", lin.role)
        assertEquals("把复杂的事做简单。", lin.sign)
        assertEquals("avatar_lin", lin.avatar)
        assertTrue(lin.online)
        assertTrue(lin.pinned)
        assertFalse(lin.muted)
        assertEquals(2, lin.unread)
        assertEquals("14:32", lin.timeLabel)
        assertEquals(5L, lin.order)
        assertEquals("下午三点对一下这两处圆角和字重", lin.preview)

        val team = state.conversations.first { it.id == "team" }
        assertTrue(team.isGroup)
        assertEquals(6, team.memberCount)
        assertTrue(team.muted)
        assertEquals(2, team.unread)
        assertEquals("赵敏：新气泡已经导进画板了", team.preview)
        assertEquals("周五评审改到下午三点，会议室 6 楼", team.announcement)
        assertEquals(listOf("设计", "内部"), team.tags)

        val chen = state.conversations.first { it.id == "chen" }
        assertEquals("文件 · 拾语视觉规范_v3.pdf", chen.preview)
        assertEquals(1, chen.unread)

        val zhao = state.conversations.first { it.id == "zhao" }
        assertEquals("语音 · 0:18", zhao.preview)
        assertEquals("11:20", zhao.timeLabel)

        val wu = state.conversations.first { it.id == "wu" }
        assertEquals("昨天", wu.timeLabel)
        assertEquals(0, wu.unread)
    }

    @Test
    fun `chat list sorts pinned first then order desc`() {
        assertEquals(
            listOf("lin", "team", "chen", "zhao", "wu"),
            state.sortedConversations().map { it.id },
        )
    }

    @Test
    fun `initial tab unread total is three excluding muted group`() {
        assertEquals(3, state.unreadTotal())
        assertEquals(5L, state.orderSeq)
    }

    @Test
    fun `message counts match mock`() {
        assertEquals(
            mapOf("lin" to 8, "team" to 4, "chen" to 3, "zhao" to 2, "wu" to 2),
            state.messages.mapValues { it.value.size },
        )
    }

    @Test
    fun `lin messages carry divider image voice file and text`() {
        val lin = state.messages.getValue("lin")
        assertEquals(8, lin.size)
        assertTrue(lin[0] is Message.System)
        assertEquals("今天 09:12", (lin[0] as Message.System).text)

        assertEquals(
            "早，昨晚那份改稿我看完了，整体方向没问题",
            (lin[1] as Message.Text).content,
        )
        assertEquals(MessageStatus.READ, (lin[2] as Message.Text).status)

        val image = lin[3] as Message.Image
        assertEquals("photo_cat", image.asset)
        assertEquals("橙色猫咪躺在沙发上", image.alt)
        assertEquals("图片 · 橙色猫咪躺在沙发上", image.previewText())

        val voice = lin[5] as Message.Voice
        assertEquals(12, voice.durationSeconds)
        assertEquals("0:12", voice.durationLabel)

        val file = lin[6] as Message.File
        assertEquals("拾语视觉规范_v3.pdf", file.fileName)
        assertEquals("2.4 MB", file.sizeLabel)

        assertEquals("下午三点对一下这两处圆角和字重", (lin[7] as Message.Text).content)
        assertEquals(MessageSender.THEM, lin[7].sender)
    }

    @Test
    fun `team messages expose group senders`() {
        val team = state.messages.getValue("team")
        assertEquals("今天 12:40", (team[0] as Message.System).text)
        assertEquals("陈屿", (team[1] as Message.Text).who)
        assertEquals(MessageSender.ME, team[2].sender)
        assertEquals(MessageStatus.READ, team[2].status)
        assertEquals("赵敏", (team[3] as Message.Text).who)
        assertEquals(
            "赵敏：新气泡已经导进画板了，等会儿发群里",
            (team[3] as Message.Text).conversationPreview(isGroup = true),
        )
    }

    @Test
    fun `zhao has no date divider and an eighteen second voice`() {
        val zhao = state.messages.getValue("zhao")
        assertTrue(zhao.none { it is Message.System })
        val voice = zhao[1] as Message.Voice
        assertEquals(18, voice.durationSeconds)
        assertEquals("0:18", voice.durationLabel)
    }

    @Test
    fun `only my messages carry status`() {
        state.messages.values.flatten().forEach { message ->
            when (message.sender) {
                MessageSender.ME -> assertNotNull(message.status)
                else -> assertNull(message.status)
            }
        }
        assertEquals(MessageStatus.READ, (state.messages.getValue("wu")[0] as Message.Text).status)
    }

    @Test
    fun `my profile matches design mock`() {
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
        val lin = state.conversations.first { it.id == "lin" }
        assertEquals("在线", lin.headerSubtitle)
        assertEquals("设计", lin.rolePrimary)
        assertEquals("林夏 · 设计 · 视觉组", lin.remark)
        assertEquals(listOf("设计", "常用"), lin.profileTags)
        assertEquals("设计组（6）", lin.commonGroupLabel)

        val team = state.conversations.first { it.id == "team" }
        assertEquals("6 人 · 群聊", team.headerSubtitle)
        assertEquals("6 人群聊", team.pickNote)
        assertNull(team.commonGroupLabel)

        val contacts = state.conversations.filterNot { it.isGroup }.map { it.toContact() }
        assertEquals(listOf("lin", "chen", "zhao", "wu"), contacts.map { it.conversationId })
    }

    @Test
    fun `all seeded assets resolve to drawables`() {
        val assetNames = buildSet {
            add(state.profile.avatar)
            state.conversations.forEach { add(it.avatar) }
            state.messages.values.flatten().forEach { message ->
                if (message is Message.Image) add(message.asset)
            }
        }
        assetNames.forEach { name ->
            assertNotNull("素材未入库：$name", StockMedia.drawableFor(name))
        }
        assertEquals(
            listOf("photo_cat", "photo_coffee", "photo_desk"),
            StockMedia.photos.map { it.asset },
        )
    }

    @Test
    fun `chat state json round trip preserves every message type`() {
        val encoded = ShiYuJson.encodeToString(ChatState.serializer(), state)
        val decoded = ShiYuJson.decodeFromString(ChatState.serializer(), encoded)
        assertEquals(state, decoded)

        val lin = decoded.messages.getValue("lin")
        assertTrue(lin[0] is Message.System)
        assertTrue(lin[1] is Message.Text)
        assertTrue(lin[3] is Message.Image)
        assertTrue(lin[5] is Message.Voice)
        assertTrue(lin[6] is Message.File)
        assertEquals(MessageSender.SYSTEM, lin[0].sender)
    }

    @Test
    fun `conversation serializer round trip keeps defaults`() {
        val team = state.conversations.first { it.id == "team" }
        val encoded = ShiYuJson.encodeToString(Conversation.serializer(), team)
        assertEquals(team, ShiYuJson.decodeFromString(Conversation.serializer(), encoded))
    }
}
