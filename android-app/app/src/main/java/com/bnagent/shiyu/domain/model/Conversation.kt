package com.bnagent.shiyu.domain.model

import kotlinx.serialization.Serializable

/**
 * 会话（设计规格 6.2）。
 *
 * 字段与设计稿 mock 一一对应：
 * - `id/name/handle/city/role/sign/avatar`：资料；
 * - `online/isGroup/memberCount`：在线点、群聊标识；
 * - `pinned/muted/unread/timeLabel/order/preview`：列表排序与未读；
 * - `announcement/tags`：群聊资料页附加数据。
 *
 * 存储顺序即 [Conversation] 列表顺序（lin/team/chen/zhao/wu），
 * 消息列表展示顺序由仓库按「置顶优先、order 降序」计算。
 */
@Serializable
data class Conversation(
    val id: String,
    val name: String,
    val handle: String,
    val role: String,
    /** drawable 资源名（如 `avatar_lin`），用 StockMedia.drawableFor 映射。 */
    val avatar: String,
    val city: String? = null,
    val sign: String? = null,
    val online: Boolean = false,
    val isGroup: Boolean = false,
    val memberCount: Int? = null,
    val pinned: Boolean = false,
    val muted: Boolean = false,
    val unread: Int = 0,
    /** 会话行右侧时间文案（`14:32`、`昨天`），由数据提供，不做本地化重算。 */
    val timeLabel: String = "",
    /** 排序序号；打开 / 发送 / 收到回复都会 ++。 */
    val order: Long = 0L,
    val preview: String = EMPTY_PREVIEW,
    /** 群公告（仅群聊）。 */
    val announcement: String? = null,
    /** 资料页标签（仅群聊；单聊用 rolePrimary + 常用）。 */
    val tags: List<String> = emptyList(),
) {
    companion object {
        const val EMPTY_PREVIEW = "还没有消息"
    }
}

/**
 * 通讯录联系人：从非群聊会话派生（设计规格 4.4）。
 *
 * `conversationId` 用于从联系人跳转个人资料 / 聊天。
 */
@Serializable
data class Contact(
    val conversationId: String,
    val name: String,
    val handle: String,
    val role: String,
    val avatar: String,
    val city: String? = null,
    val sign: String? = null,
    val online: Boolean = false,
)

/** 非群聊会话 → 联系人。 */
fun Conversation.toContact(): Contact = Contact(
    conversationId = id,
    name = name,
    handle = handle,
    role = role,
    avatar = avatar,
    city = city,
    sign = sign,
    online = online,
)

// ─── 展示派生规则（与设计稿 3.1-C / 4.5 / 5.11 对齐，UI 直接复用） ─────────

/** 职能第一段：`设计 · 视觉组` → `设计`。 */
val Conversation.rolePrimary: String
    get() = role.substringBefore(" · ").trim().ifEmpty { role.trim() }

/** 个人资料拾语号行：有城市时 `拾语号 lin_xia · 上海`。 */
val Conversation.handleLine: String
    get() = if (city.isNullOrBlank()) "拾语号 $handle" else "拾语号 $handle · $city"

/** 聊天头部副标题：群聊 `6 人 · 群聊`；单聊 `在线` / `离线`。 */
val Conversation.headerSubtitle: String
    get() = when {
        isGroup -> "${memberCount ?: 0} 人 · 群聊"
        online -> "在线"
        else -> "离线"
    }

/** 新聊天 / 文件选择 sheet 右侧备注：群聊 `6 人群聊`，单聊职能。 */
val Conversation.pickNote: String
    get() = if (isGroup) "${memberCount ?: 0} 人群聊" else role

/** 个人资料「备注」行：`{name} · {role}`（群聊不展示）。 */
val Conversation.remark: String
    get() = "$name · $role"

/** 个人资料「标签」：群聊用 tags，单聊用 [职能第一段, 常用]。 */
val Conversation.profileTags: List<String>
    get() = if (isGroup) tags else listOf(rolePrimary, "常用")

/** 单聊资料的「共同群聊」：设计稿固定「设计组（6）」；群聊不展示。 */
val Conversation.commonGroupLabel: String?
    get() = if (isGroup) null else "设计组（6）"
