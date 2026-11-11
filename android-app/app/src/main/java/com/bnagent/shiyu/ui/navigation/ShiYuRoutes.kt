package com.bnagent.shiyu.ui.navigation

import android.net.Uri

/**
 * 全部路由（任务约定，业务成员不要改名字，只替换 composable 内的实现）。
 *
 * | 路由 | 设计稿屏幕 | 是否显示 tabbar |
 * |---|---|---|
 * | chats | 消息列表 | ✅ |
 * | chat/{id} | 聊天 | ❌ |
 * | contacts | 通讯录 | ✅ |
 * | profile/{id} | 个人资料 | ❌ |
 * | me | 我 | ✅ |
 * | settings/theme | 气泡与主题（设计稿叫 bubbles） | ❌ |
 * | settings/server | 服务器设置（新增） | ❌ |
 * | call/voice/{id} | 语音通话 | ❌（强制深色 chrome） |
 * | call/video/{id} | 视频通话 | ❌（强制深色 chrome） |
 */
object ShiYuRoutes {
    const val CHATS = "chats"
    const val CONTACTS = "contacts"
    const val ME = "me"

    const val CHAT = "chat/{id}"
    fun chat(id: String): String = "chat/${Uri.encode(id)}"

    const val PROFILE = "profile/{id}"
    fun profile(id: String): String = "profile/${Uri.encode(id)}"

    const val THEME_SETTINGS = "settings/theme"
    const val SERVER_SETTINGS = "settings/server"

    const val CALL_VOICE = "call/voice/{id}"
    fun callVoice(id: String): String = "call/voice/${Uri.encode(id)}"

    const val CALL_VIDEO = "call/video/{id}"
    fun callVideo(id: String): String = "call/video/${Uri.encode(id)}"

    /** 通话页统一前缀：AppRoot 用它强制深色系统栏图标。 */
    const val CALL_PREFIX = "call/"
}
