package com.bnagent.shiyu.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavHostController
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.data.call.CallManager
import com.bnagent.shiyu.data.call.formatCallDuration
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.LocalToast
import com.bnagent.shiyu.ui.components.MainTab
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.components.ShiYuTabBar
import com.bnagent.shiyu.ui.components.ToastHost
import com.bnagent.shiyu.ui.components.rememberToastHostState
import com.bnagent.shiyu.ui.navigation.ShiYuNavHost
import com.bnagent.shiyu.ui.navigation.ShiYuRoutes
import com.bnagent.shiyu.ui.theme.ApplySystemBarAppearance
import com.bnagent.shiyu.ui.theme.ShiYuCallColors
import com.bnagent.shiyu.ui.theme.ShiYuTheme

/**
 * 应用根组件：
 * - 持有 NavHost 与 TabBar 的显示规则（只有 chats / contacts / me 显示 tabbar）；
 * - 在根部提供 Toast 宿主（`LocalToast`）；
 * - 统一控制系统栏图标明暗：深色主题或 call 路由（call/voice、call/video）时用浅色图标。
 *
 * 三块根内容各自处理自己的系统栏 padding（AppBar 自带 statusBarsPadding，
 * TabBar 自带 navigationBarsPadding）；这里不额外加 insets。
 */
@Composable
fun ShiYuApp() {
    val navController = rememberNavController()
    val backStackEntry by navController.currentBackStackEntryAsState()
    val route = backStackEntry?.destination?.route
    val settings = ShiYuTheme.settings

    // tabbar 未读总数 = 所有非静音会话未读之和（设计规格 3.2 / 5.4）。
    val container = LocalAppContainer.current
    val unreadCount by container.conversationRepository.unreadTotal
        .collectAsStateWithLifecycle(initialValue = 0)

    // 最小化后的通话：离开通话页仍在进行，用顶部悬浮条提示并可一键返回。
    val callManager = container.callManager
    val callState by callManager.uiState.collectAsStateWithLifecycle()
    val callConversation by remember(callState.conversationId) {
        container.conversationRepository.conversation(callState.conversationId)
    }.collectAsStateWithLifecycle(initialValue = null)

    val isCallRoute = route?.startsWith(ShiYuRoutes.CALL_PREFIX) == true
    val showMiniCall = callState.isActive && !isCallRoute
    val showTabBar = route == ShiYuRoutes.CHATS ||
        route == ShiYuRoutes.CONTACTS ||
        route == ShiYuRoutes.ME
    val selectedTab = when (route) {
        ShiYuRoutes.CONTACTS -> MainTab.Contacts
        ShiYuRoutes.ME -> MainTab.Me
        else -> MainTab.Chats
    }

    // 通话页强制深色 chrome；其余页面跟随应用内深色开关（不跟随系统）
    ApplySystemBarAppearance(darkChrome = settings.darkTheme || isCallRoute)

    val toastState = rememberToastHostState()

    CompositionLocalProvider(LocalToast provides toastState) {
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(ShiYuTheme.colors.bg),
        ) {
            Column(modifier = Modifier.fillMaxSize()) {
                Box(modifier = Modifier.weight(1f)) {
                    ShiYuNavHost(navController = navController)
                }
                if (showTabBar) {
                    ShiYuTabBar(
                        selected = selectedTab,
                        onSelect = { tab -> navController.navigateToTab(tab) },
                        // 非静音会话未读总和；0 时徽标自动隐藏，>99 显示 99+（由 TabBadge 处理）。
                        unreadCount = unreadCount,
                    )
                }
            }
            if (showMiniCall) {
                MiniCallBar(
                    state = callState,
                    name = callConversation?.name.orEmpty(),
                    avatar = callConversation?.avatar.orEmpty(),
                    onOpen = {
                        val route = if (callState.video) {
                            ShiYuRoutes.callVideo(callState.conversationId)
                        } else {
                            ShiYuRoutes.callVoice(callState.conversationId)
                        }
                        navController.navigate(route) { launchSingleTop = true }
                    },
                    onHangup = {
                        val result = callManager.endCall()
                        toastState.show(
                            if (result.connected) {
                                "通话结束 · ${formatCallDuration(result.elapsedSeconds)}"
                            } else {
                                "已取消通话"
                            },
                        )
                    },
                    modifier = Modifier.align(Alignment.TopCenter),
                )
            }
            ToastHost(state = toastState)
        }
    }
}

private fun NavHostController.navigateToTab(tab: MainTab) {
    navigate(tab.route) {
        popUpTo(graph.findStartDestination().id) { saveState = true }
        launchSingleTop = true
        restoreState = true
    }
}

/**
 * 最小化通话悬浮条：通话进行中且不在通话页时显示在顶部；
 * 整条可点（返回通话页），右侧红色按钮直接挂断。
 */
@Composable
private fun MiniCallBar(
    state: CallManager.CallUiState,
    name: String,
    avatar: String,
    onOpen: () -> Unit,
    onHangup: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier
            .statusBarsPadding()
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .shadow(10.dp, RoundedCornerShape(50), ambientColor = Color.Black, spotColor = Color.Black)
            .clip(RoundedCornerShape(50))
            .background(MiniCallBarBg)
            .clickable(onClick = onOpen)
            .padding(start = 8.dp, end = 7.dp, top = 7.dp, bottom = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(9.dp),
    ) {
        Avatar(
            name = name,
            size = 28.dp,
            model = StockMedia.drawableFor(avatar),
            circle = true,
        )
        Column {
            Text(
                text = name.ifBlank { "通话中" },
                color = Color.White,
                fontSize = 12.5.sp,
                fontWeight = FontWeight.SemiBold,
                lineHeight = 16.sp,
            )
            Text(
                text = if (state.connected) {
                    "${if (state.video) "视频通话中" else "语音通话中"} · ${formatCallDuration(state.elapsedSeconds)}"
                } else {
                    "正在接通…"
                },
                color = MiniCallStatusColor,
                fontSize = 10.5.sp,
                lineHeight = 14.sp,
            )
        }
        Spacer(Modifier.width(2.dp))
        Box(
            modifier = Modifier
                .size(30.dp)
                .clip(CircleShape)
                .background(ShiYuCallColors.hangup)
                .clickable(onClick = onHangup),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = ShiYuIcons.Hangup,
                contentDescription = "挂断",
                tint = Color.White,
                modifier = Modifier.size(16.dp),
            )
        }
    }
}

private val MiniCallBarBg = Color(0xF2101416)
private val MiniCallStatusColor = Color(0xFFC9D3D7)
