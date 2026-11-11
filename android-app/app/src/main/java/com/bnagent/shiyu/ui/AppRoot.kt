package com.bnagent.shiyu.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavHostController
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.ui.components.LocalToast
import com.bnagent.shiyu.ui.components.MainTab
import com.bnagent.shiyu.ui.components.ShiYuTabBar
import com.bnagent.shiyu.ui.components.ToastHost
import com.bnagent.shiyu.ui.components.rememberToastHostState
import com.bnagent.shiyu.ui.navigation.ShiYuNavHost
import com.bnagent.shiyu.ui.navigation.ShiYuRoutes
import com.bnagent.shiyu.ui.theme.ApplySystemBarAppearance
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

    val isCallRoute = route?.startsWith(ShiYuRoutes.CALL_PREFIX) == true
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
