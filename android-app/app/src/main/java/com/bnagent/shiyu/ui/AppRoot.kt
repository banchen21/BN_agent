package com.bnagent.shiyu.ui

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import android.view.WindowManager
import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavHostController
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.data.call.CallManager
import com.bnagent.shiyu.data.call.formatCallDuration
import com.bnagent.shiyu.ui.call.CallFloatingOverlay
import com.bnagent.shiyu.ui.call.CallFloatingService
import com.bnagent.shiyu.ui.call.PendingCallOpen
import com.bnagent.shiyu.ui.call.callRouteFor
import com.bnagent.shiyu.data.remote.CallSession
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
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.isActive as coroutineActive
import kotlinx.coroutines.launch

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

    // Agent 主动来电：本地没有推送通道，前台每 4s 轮询一次（通话中/来电弹窗期间暂停）。
    val lifecycleOwner = LocalLifecycleOwner.current
    val scope = rememberCoroutineScope()
    var incomingInvite by remember { mutableStateOf<CallSession?>(null) }
    val inviteConversation by remember(incomingInvite?.peerId) {
        container.conversationRepository.conversation(
            incomingInvite?.peerId.orEmpty().removePrefix("android:"),
        )
    }.collectAsStateWithLifecycle(initialValue = null)

    LaunchedEffect(callManager, lifecycleOwner) {
        lifecycleOwner.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            while (coroutineActive) {
                if (incomingInvite == null && !callManager.uiState.value.isActive) {
                    val invite = runCatching {
                        val server = container.settingsRepository.serverSettings.first()
                        container.agentClient.incomingCall(server)
                    }.getOrNull()
                    if (invite != null) incomingInvite = invite
                }
                delay(INCOMING_POLL_MS)
            }
        }
    }

    val isCallRoute = route?.startsWith(ShiYuRoutes.CALL_PREFIX) == true
    // 悬浮条只在「通话已接通且不在通话页」时出现：呼出/接通中不应该弹出来。
    val showMiniCall = callState.connected && !isCallRoute

    // App 在前台时用内置悬浮条；离开 App（退到桌面/其他应用）时改由系统级浮窗承接通话。
    val overlayContext = LocalContext.current.applicationContext
    val appLifecycle = lifecycleOwner.lifecycle
    // 只有真正回到前台（RESUMED）才算：WINDOWING 模式下退到桌面时 Activity 仍会停在 STARTED。
    var appActive by remember { mutableStateOf(appLifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) }
    DisposableEffect(appLifecycle) {
        val observer = androidx.lifecycle.LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_RESUME -> appActive = true
                Lifecycle.Event.ON_PAUSE -> appActive = false
                else -> Unit
            }
        }
        appLifecycle.addObserver(observer)
        onDispose { appLifecycle.removeObserver(observer) }
    }
    var pendingOverlayGrant by remember { mutableStateOf(false) }
    val showOverlay: () -> Unit = {
        if (Settings.canDrawOverlays(overlayContext)) {
            CallFloatingService.launch(overlayContext)
            CallFloatingService.isRunning = true
        } else {
            Toast.makeText(overlayContext, "请允许拾语显示在其他应用上层", Toast.LENGTH_LONG).show()
            pendingOverlayGrant = true
            runCatching {
                overlayContext.startActivity(
                    Intent(
                        Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                        Uri.parse("package:${overlayContext.packageName}"),
                    ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
            }
        }
    }
    val hideOverlay: () -> Unit = {
        CallFloatingService.stop(overlayContext)
        CallFloatingService.isRunning = false
    }
    DisposableEffect(callState.isActive) {
        if (callState.isActive) showOverlay() else hideOverlay()
        onDispose { }
    }
    LaunchedEffect(callState.isActive, appActive) {
        if (!callState.isActive) return@LaunchedEffect
        if (appActive) {
            // 从后台回到 App（或从授权页返回）：收起系统浮窗，用内置悬浮条。
            CallFloatingOverlay.hide()
        } else {
            // 退到桌面/其他应用：显示系统浮窗（未授权时提示一次）。
            if (Settings.canDrawOverlays(overlayContext)) {
                CallFloatingOverlay.show(
                    context = overlayContext,
                    windowManager = overlayContext.getSystemService(Context.WINDOW_SERVICE) as WindowManager,
                    manager = callManager,
                    conversation = container.conversationRepository,
                )
            } else if (pendingOverlayGrant) {
                pendingOverlayGrant = false
            }
        }
    }
    // 从浮窗/通知回来：直接跳到通话页。
    LaunchedEffect(Unit) {
        PendingCallOpen.consume()?.takeIf { it.isNotBlank() }?.let { id ->
            val video = callManager.uiState.value.video
            navController.navigate(callRouteFor(id, video)) { launchSingleTop = true }
        }
    }
    // 通话进行中回到 App 时统一落在通话页：避免通话页与聊天页同时在栈上，互相误触发。
    val inCallConversation = if (callState.isActive) callState.conversationId else null
    LaunchedEffect(inCallConversation, route, appActive) {
        val id = inCallConversation ?: return@LaunchedEffect
        if (!appActive || route.isNullOrEmpty()) return@LaunchedEffect
        if (route.startsWith(ShiYuRoutes.CALL_PREFIX)) return@LaunchedEffect
        navController.navigate(callRouteFor(id, callState.video)) {
            launchSingleTop = true
        }
    }
    // 兜底：App 在前台时，系统浮窗一律收起（避免和 App 内小窗叠在一起）。
    LaunchedEffect(appActive, route, callState.video, callState.connected) {
        if (appActive) CallFloatingOverlay.hideIfShown()
    }
    // 从授权页回来：若用户已允许浮窗，再补开服务。
    LaunchedEffect(appActive) {
        if (appActive && pendingOverlayGrant && Settings.canDrawOverlays(overlayContext)) {
            if (callState.isActive) showOverlay()
            pendingOverlayGrant = false
        }
    }
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

    // 对方（Agent）主动挂断：提示；若正停在通话页，退回该会话聊天页。
    LaunchedEffect(callManager) {
        callManager.endedByPeer.collect { ended ->
            toastState.show("对方已挂断")
            val current = navController.currentBackStackEntry?.destination?.route
            if (current?.startsWith(ShiYuRoutes.CALL_PREFIX) == true) {
                navController.navigate(ShiYuRoutes.chat(ended.conversationId)) {
                    popUpTo(navController.graph.findStartDestination().id) { inclusive = false }
                    launchSingleTop = true
                }
            }
        }
    }

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
            incomingInvite?.let { invite ->
                IncomingCallScreen(
                    invite = invite,
                    name = inviteConversation?.name.orEmpty().ifBlank { "齐悦" },
                    avatar = inviteConversation?.avatar.orEmpty(),
                    onAccept = {
                        callManager.acceptInvite(invite)
                        incomingInvite = null
                        val convId = invite.peerId.removePrefix("android:").ifBlank { "yueyue" }
                        val route = if (invite.mediaType.equals("video", ignoreCase = true)) {
                            ShiYuRoutes.callVideo(convId)
                        } else {
                            ShiYuRoutes.callVoice(convId)
                        }
                        navController.navigate(route) { launchSingleTop = true }
                    },
                    onDecline = {
                        incomingInvite = null
                        scope.launch {
                            runCatching {
                                val server = container.settingsRepository.serverSettings.first()
                                container.agentClient.declineCall(server, invite.room)
                            }
                        }
                        toastState.show("已拒绝来电")
                    },
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

private const val INCOMING_POLL_MS = 4_000L

/** Agent 来电全屏页：接听 / 拒绝。 */
@Composable
private fun IncomingCallScreen(
    invite: CallSession,
    name: String,
    avatar: String,
    onAccept: () -> Unit,
    onDecline: () -> Unit,
) {
    val isVideo = invite.mediaType.equals("video", ignoreCase = true)
    BackHandler(onBack = onDecline)
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(ShiYuCallColors.background),
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .navigationBarsPadding()
                .padding(horizontal = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Spacer(Modifier.height(64.dp))
            Avatar(
                name = name,
                size = 112.dp,
                model = StockMedia.drawableFor(avatar),
                circle = true,
            )
            Spacer(Modifier.height(20.dp))
            Text(
                text = name,
                color = ShiYuCallColors.name,
                fontSize = 22.sp,
                fontWeight = FontWeight.Bold,
                lineHeight = 30.sp,
            )
            Spacer(Modifier.height(8.dp))
            Text(
                text = "邀请你${if (isVideo) "视频通话" else "语音通话"}…",
                color = ShiYuCallColors.status,
                fontSize = 14.sp,
                lineHeight = 20.sp,
            )
            Spacer(Modifier.weight(1f))
            Row(horizontalArrangement = Arrangement.spacedBy(56.dp)) {
                IncomingCallButton(
                    icon = ShiYuIcons.Hangup,
                    label = "拒绝",
                    container = ShiYuCallColors.hangup,
                    onClick = onDecline,
                )
                IncomingCallButton(
                    icon = if (isVideo) ShiYuIcons.Video else ShiYuIcons.Phone,
                    label = "接听",
                    container = AnswerGreen,
                    onClick = onAccept,
                )
            }
            Spacer(Modifier.height(36.dp))
        }
    }
}

@Composable
private fun IncomingCallButton(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    container: Color,
    onClick: () -> Unit,
) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            modifier = Modifier
                .size(64.dp)
                .clip(CircleShape)
                .background(container)
                .clickable(onClick = onClick),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = icon,
                contentDescription = label,
                tint = Color.White,
                modifier = Modifier.size(27.dp),
            )
        }
        Text(
            text = label,
            color = ShiYuCallColors.controlLabel,
            fontSize = 12.sp,
            lineHeight = 16.sp,
        )
    }
}

private val AnswerGreen = Color(0xFF2FBE6E)
