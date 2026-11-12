package com.bnagent.shiyu.ui.navigation

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavHostController
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.navArgument
import com.bnagent.shiyu.ui.call.VideoCallScreen
import com.bnagent.shiyu.ui.call.VoiceCallScreen
import com.bnagent.shiyu.ui.contacts.ContactsScreen
import com.bnagent.shiyu.ui.me.MeScreen
import com.bnagent.shiyu.ui.profile.ProfileScreen
import com.bnagent.shiyu.ui.settings.BubblesThemeScreen
import com.bnagent.shiyu.ui.settings.ServerSettingsScreen

/**
 * 全局导航图（起点 chats）。
 *
 * 各路由当前挂的是 [Placeholders.kt] 里的占位页；业务成员实现真实页面后，
 * 只改本文件里对应 composable 的内容即可，路由名与参数保持不变。
 */
@Composable
fun ShiYuNavHost(
    navController: NavHostController,
    modifier: Modifier = Modifier,
) {
    NavHost(
        navController = navController,
        startDestination = ShiYuRoutes.CHATS,
        modifier = modifier,
    ) {
        // 消息列表 —— android-ui-chat
        composable(ShiYuRoutes.CHATS) {
            com.bnagent.shiyu.ui.chats.ChatsScreen(
                onOpenChat = { id -> navController.navigate(ShiYuRoutes.chat(id)) },
            )
        }

        // 聊天 —— android-ui-chat
        composable(
            route = ShiYuRoutes.CHAT,
            arguments = listOf(navArgument("id") { type = NavType.StringType }),
        ) { entry ->
            val id = entry.arguments?.getString("id").orEmpty()
            com.bnagent.shiyu.ui.chat.ChatScreen(
                id = id,
                onBack = { navController.popBackStack() },
                onOpenProfile = { navController.navigate(ShiYuRoutes.profile(id)) },
                onOpenTheme = { navController.navigate(ShiYuRoutes.THEME_SETTINGS) },
                onVoiceCall = { navController.navigate(ShiYuRoutes.callVoice(id)) },
                onVideoCall = { navController.navigate(ShiYuRoutes.callVideo(id)) },
            )
        }

        // 通讯录 —— android-ui-shell
        composable(ShiYuRoutes.CONTACTS) {
            ContactsScreen(
                onOpenProfile = { id -> navController.navigate(ShiYuRoutes.profile(id)) },
            )
        }

        // 个人资料 —— android-ui-shell
        composable(
            route = ShiYuRoutes.PROFILE,
            arguments = listOf(navArgument("id") { type = NavType.StringType }),
        ) { entry ->
            val id = entry.arguments?.getString("id").orEmpty()
            ProfileScreen(
                conversationId = id,
                onBack = { navController.popBackStack() },
                onOpenMessage = { navController.navigate(ShiYuRoutes.chat(id)) },
                onVoiceCall = { navController.navigate(ShiYuRoutes.callVoice(id)) },
                onVideoCall = { navController.navigate(ShiYuRoutes.callVideo(id)) },
            )
        }

        // 我 —— android-ui-shell
        composable(ShiYuRoutes.ME) {
            MeScreen(
                onOpenTheme = { navController.navigate(ShiYuRoutes.THEME_SETTINGS) },
                onOpenServerSettings = { navController.navigate(ShiYuRoutes.SERVER_SETTINGS) },
            )
        }

        // 气泡与主题 —— android-ui-shell（设计稿屏幕名 bubbles）
        composable(ShiYuRoutes.THEME_SETTINGS) {
            BubblesThemeScreen(onBack = { navController.popBackStack() })
        }

        // 服务器设置 —— android-ui-shell（页面本体归 T6）
        composable(ShiYuRoutes.SERVER_SETTINGS) {
            ServerSettingsScreen(onBack = { navController.popBackStack() })
        }

        // 语音通话 —— android-ui-shell（T6）
        composable(
            route = ShiYuRoutes.CALL_VOICE,
            arguments = listOf(navArgument("id") { type = NavType.StringType }),
        ) { entry ->
            val id = entry.arguments?.getString("id").orEmpty()
            VoiceCallScreen(
                conversationId = id,
                onMinimize = { navController.minimizeCall(id) },
                onEndToChat = { navController.returnToChatAfterCall(id) },
            )
        }

        // 视频通话 —— android-ui-shell（T6）
        composable(
            route = ShiYuRoutes.CALL_VIDEO,
            arguments = listOf(navArgument("id") { type = NavType.StringType }),
        ) { entry ->
            val id = entry.arguments?.getString("id").orEmpty()
            VideoCallScreen(
                conversationId = id,
                onMinimize = { navController.minimizeCall(id) },
                onEndToChat = { navController.returnToChatAfterCall(id) },
            )
        }
    }
}

/**
 * 最小化通话：只退出通话页，通话本身由进程级 CallManager 继续；
 * 没有可回退的页面时（异常栈）回到该会话聊天页。
 */
private fun NavHostController.minimizeCall(conversationId: String) {
    if (!popBackStack()) {
        navigate(ShiYuRoutes.chat(conversationId)) { launchSingleTop = true }
    }
}

/**
 * 通话结束固定回到该会话聊天页（设计规格 5.6 / team-lead 决策）：
 * 清掉通话页与来源页，只保留聊天页。
 */
private fun NavHostController.returnToChatAfterCall(conversationId: String) {
    navigate(ShiYuRoutes.chat(conversationId)) {
        popUpTo(graph.findStartDestination().id) { inclusive = false }
        launchSingleTop = true
    }
}
