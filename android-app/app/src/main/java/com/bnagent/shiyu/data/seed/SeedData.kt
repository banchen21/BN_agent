package com.bnagent.shiyu.data.seed

import com.bnagent.shiyu.data.local.ChatState
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.domain.model.MessageSender
import com.bnagent.shiyu.domain.model.MessageStatus
import com.bnagent.shiyu.domain.model.UserProfile

/**
 * 首屏种子数据，逐字还原 design-spec 第 6 节：
 * - 5 个会话：lin（置顶，未读 2）→ team（群聊，静音，未读 2）→ chen（未读 1）→ zhao → wu；
 * - 消息序列含日期分隔、图片 photo_cat、文件 PDF、语音 12s/18s、群聊发送者 陈屿/赵敏；
 * - 我方资料：苏雨 / shiyu_2049 / 上海 / 签名。
 *
 * 只作为首次启动的首装数据；之后收发消息一律走后端（team-lead 决策）。
 */
object SeedData {

    val profile = UserProfile()

    /** 存储顺序 = 列表顺序（设计规格 6.2）。 */
    val conversations: List<Conversation> = listOf(
        Conversation(
            id = "lin",
            name = "林夏",
            handle = "lin_xia",
            city = "上海",
            role = "设计 · 视觉组",
            sign = "把复杂的事做简单。",
            avatar = "avatar_lin",
            online = true,
            pinned = true,
            unread = 2,
            timeLabel = "14:32",
            order = 5,
            preview = "下午三点对一下这两处圆角和字重",
        ),
        Conversation(
            id = "team",
            name = "设计组",
            handle = "design_team",
            role = "群聊",
            avatar = "avatar_team",
            isGroup = true,
            memberCount = 6,
            muted = true,
            unread = 2,
            timeLabel = "13:05",
            order = 4,
            preview = "赵敏：新气泡已经导进画板了",
            announcement = "周五评审改到下午三点，会议室 6 楼",
            tags = listOf("设计", "内部"),
        ),
        Conversation(
            id = "chen",
            name = "陈屿",
            handle = "chen_yu",
            city = "杭州",
            role = "产品 · 增长",
            sign = "先跑通，再跑快。",
            avatar = "avatar_chen",
            unread = 1,
            timeLabel = "11:47",
            order = 3,
            preview = "文件 · 拾语视觉规范_v3.pdf",
        ),
        Conversation(
            id = "zhao",
            name = "赵敏",
            handle = "zhao_min",
            city = "深圳",
            role = "设计 · 动效",
            sign = "让动效解释因果。",
            avatar = "avatar_zhao",
            online = true,
            timeLabel = "11:20",
            order = 2,
            preview = "语音 · 0:18",
        ),
        Conversation(
            id = "wu",
            name = "吴其",
            handle = "wu_qi",
            city = "北京",
            role = "研发 · 客户端",
            sign = "能自动化就别手动。",
            avatar = "avatar_wu",
            timeLabel = "昨天",
            order = 1,
            preview = "好的，明天见",
        ),
    )

    val messages: Map<String, List<Message>> = linkedMapOf(
        "lin" to listOf(
            Message.System(id = "lin-0", text = "今天 09:12"),
            Message.Text(
                id = "lin-1",
                sender = MessageSender.THEM,
                content = "早，昨晚那份改稿我看完了，整体方向没问题",
                time = "09:14",
            ),
            Message.Text(
                id = "lin-2",
                sender = MessageSender.ME,
                content = "那太好了，气泡那块我留了两版，等下发你",
                time = "09:15",
                status = MessageStatus.READ,
            ),
            Message.Image(
                id = "lin-3",
                sender = MessageSender.THEM,
                asset = "photo_cat",
                alt = "橙色猫咪躺在沙发上",
                time = "09:20",
            ),
            Message.Text(
                id = "lin-4",
                sender = MessageSender.ME,
                content = "哈哈它天天这样，别叫它了",
                time = "09:21",
                status = MessageStatus.READ,
            ),
            Message.Voice(
                id = "lin-5",
                sender = MessageSender.THEM,
                durationSeconds = 12,
                time = "09:24",
            ),
            Message.File(
                id = "lin-6",
                sender = MessageSender.THEM,
                fileName = "拾语视觉规范_v3.pdf",
                sizeLabel = "2.4 MB",
                time = "09:26",
            ),
            Message.Text(
                id = "lin-7",
                sender = MessageSender.THEM,
                content = "下午三点对一下这两处圆角和字重",
                time = "09:27",
            ),
        ),
        "team" to listOf(
            Message.System(id = "team-0", text = "今天 12:40"),
            Message.Text(
                id = "team-1",
                sender = MessageSender.THEM,
                content = "周五评审改到下午三点，会议室换到 6 楼",
                who = "陈屿",
                time = "13:02",
            ),
            Message.Text(
                id = "team-2",
                sender = MessageSender.ME,
                content = "收到，我把字体那块补完",
                time = "13:03",
                status = MessageStatus.READ,
            ),
            Message.Text(
                id = "team-3",
                sender = MessageSender.THEM,
                content = "新气泡已经导进画板了，等会儿发群里",
                who = "赵敏",
                time = "13:05",
            ),
        ),
        "chen" to listOf(
            Message.System(id = "chen-0", text = "今天 11:40"),
            Message.Text(
                id = "chen-1",
                sender = MessageSender.THEM,
                content = "规范 PDF 我更新了，圆角和字号都写在里面",
                time = "11:44",
            ),
            Message.File(
                id = "chen-2",
                sender = MessageSender.THEM,
                fileName = "拾语视觉规范_v3.pdf",
                sizeLabel = "2.4 MB",
                time = "11:47",
            ),
        ),
        "zhao" to listOf(
            Message.Text(
                id = "zhao-0",
                sender = MessageSender.THEM,
                content = "那段动效我拆成两版了，你听下语音",
                time = "11:18",
            ),
            Message.Voice(
                id = "zhao-1",
                sender = MessageSender.THEM,
                durationSeconds = 18,
                time = "11:20",
            ),
        ),
        "wu" to listOf(
            Message.Text(
                id = "wu-0",
                sender = MessageSender.ME,
                content = "那我先把包发你，明早一起看",
                time = "18:12",
                status = MessageStatus.READ,
            ),
            Message.Text(
                id = "wu-1",
                sender = MessageSender.THEM,
                content = "好的，明天见",
                time = "18:14",
            ),
        ),
    )

    val state: ChatState = ChatState(
        conversations = conversations,
        messages = messages,
        profile = profile,
        orderSeq = ChatState.INITIAL_ORDER_SEQ,
    )
}
