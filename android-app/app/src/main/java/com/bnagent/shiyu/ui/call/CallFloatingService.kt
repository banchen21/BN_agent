package com.bnagent.shiyu.ui.call

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.PixelFormat
import android.os.Build
import android.os.IBinder
import android.view.Gravity
import android.view.WindowManager
import androidx.compose.ui.platform.ComposeView
import androidx.core.app.NotificationCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner
import androidx.lifecycle.setViewTreeLifecycleOwner
import androidx.lifecycle.setViewTreeViewModelStoreOwner
import androidx.savedstate.SavedStateRegistry
import androidx.savedstate.SavedStateRegistryController
import androidx.savedstate.SavedStateRegistryOwner
import androidx.savedstate.setViewTreeSavedStateRegistryOwner
import com.bnagent.shiyu.MainActivity
import com.bnagent.shiyu.R
import com.bnagent.shiyu.ShiYuApplication
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.data.call.CallManager
import com.bnagent.shiyu.data.call.formatCallDuration
import com.bnagent.shiyu.data.repository.ConversationRepository
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.navigation.ShiYuRoutes
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.ViewCompositionStrategy
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.livekit.android.renderer.TextureViewRenderer
import io.livekit.android.room.Room
import io.livekit.android.room.track.LocalVideoTrack

/**
 * 通话浮窗的前台服务：把 LiveKit（麦克风/摄像头/音频）挂在一个前台通知上保活，
 * 并把通话状态同步给 [CallFloatingOverlay]（系统级小窗）。
 *
 * 入口：[launch] / [stop]。
 */
class CallFloatingService : Service() {

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        appContext = applicationContext
        isRunning = true
        startForegroundCompat()
        val app = (application as ShiYuApplication).container
        val manager = app.callManager
        CallFloatingOverlay.show(
            context = this,
            windowManager = getSystemService(Context.WINDOW_SERVICE) as WindowManager,
            manager = manager,
            conversation = app.conversationRepository,
        )
        manager.subscribeState { state ->
            if (state.isActive) {
                updateNotification(state.video, state.connected, state.elapsedSeconds)
            } else {
                stopSelf()
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        isRunning = false
        CallFloatingOverlay.hide()
        super.onDestroy()
    }

    private fun startForegroundCompat() {
        val notifications = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            notifications.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "通话中", NotificationManager.IMPORTANCE_LOW),
            )
        }
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification: Notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_launcher_foreground)
            .setContentTitle("通话进行中")
            .setContentText("正在和齐悦通话，点击回到通话")
            .setContentIntent(open)
            .setOngoing(true)
            .setColor(android.graphics.Color.parseColor("#C9482B"))
            .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun updateNotification(video: Boolean, connected: Boolean, elapsed: Int) {
        val text = when {
            !connected -> "通话已断开"
            video -> "视频通话中 · ${formatCallDuration(elapsed)}"
            else -> "语音通话中 · ${formatCallDuration(elapsed)}"
        }
        val notifications = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        notifications.notify(
            NOTIFICATION_ID,
            NotificationCompat.Builder(this, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_launcher_foreground)
                .setContentTitle("和齐悦通话中")
                .setContentText(text)
                .setContentIntent(
                    PendingIntent.getActivity(
                        this,
                        0,
                        Intent(this, MainActivity::class.java),
                        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                    ),
                )
                .setOngoing(true)
                .build(),
        )
    }

    companion object {
        /** 进程内是否正在跑（App 里据此决定显示内置悬浮条还是系统浮窗）。 */
        @Volatile
        var isRunning: Boolean = false

        /** 进程上下文：语音/视频通话页判断已授权时用。 */
        internal var appContext: Context? = null

        private const val CHANNEL_ID = "it_new"
        private const val NOTIFICATION_ID = 2001

        /** 开始前台服务（通话最小化到其他应用时）：挂起通话进程并显示浮窗。 */
        fun launch(context: Context) {
            val intent = Intent(context, CallFloatingService::class.java)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }

        fun stop(context: Context) {
            runCatching { context.stopService(Intent(context, CallFloatingService::class.java)) }
        }
    }
}

/** 通话页路由（浮窗点击回到通话）。 */
internal fun callRouteFor(conversationId: String, video: Boolean): String =
    if (video) ShiYuRoutes.callVideo(conversationId) else ShiYuRoutes.callVoice(conversationId)

/**
 * 系统级通话小窗（需要「显示在其他应用上层」权限）。
 *
 * - 语音：头像 + 计时；视频：本地摄像头画面（小窗渲染）；计时在下方胶囊里；
 * - 可拖动，双击回到通话页（经通知跳到 MainActivity 后由 [PendingCallOpen] 打开通话页）；
 * - 点 X 直接挂断。
 */
object CallFloatingOverlay {
    private var view: ComposeView? = null
    private var wm: WindowManager? = null
    private var owner: OverlayLifecycleOwner? = null

    fun show(
        context: Context,
        windowManager: WindowManager,
        manager: CallManager,
        conversation: ConversationRepository,
    ) {
        val appContext = context.applicationContext
        if (!android.provider.Settings.canDrawOverlays(appContext)) {
            // 用户关了浮窗权限：收起浮窗，从通知点回 App 时再恢复通话页。
            hide()
            return
        }
        if (view != null) return
        val lifecycleOwner = OverlayLifecycleOwner()
        lifecycleOwner.attach()
        val compose = ComposeView(appContext).apply {
            setViewCompositionStrategy(ViewCompositionStrategy.DisposeOnDetachedFromWindow)
            setViewTreeLifecycleOwner(lifecycleOwner)
            setViewTreeSavedStateRegistryOwner(lifecycleOwner)
            setViewTreeViewModelStoreOwner(lifecycleOwner)
            setContent {
                FloatingContent(
                    manager = manager,
                    conversationId = manager.uiState.value.conversationId,
                    conversation = conversation,
                    onOpen = { call ->
                        PendingCallOpen.request(call.conversationId)
                        // 先收起浮窗，再回到 App，避免浮窗压在通话页上。
                        hide()
                        appContext.startActivity(
                            Intent(appContext, MainActivity::class.java)
                                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
                        )
                    },
                    onHangup = {
                        manager.endCall()
                        appContext.stopService(Intent(appContext, CallFloatingService::class.java))
                    },
                )
            }
        }
        val density = appContext.resources.displayMetrics.density
        val metrics = appContext.resources.displayMetrics
        val width = (density * FLOAT_WIDTH_DP).toInt()
        val height = (density * FLOAT_HEIGHT_DP).toInt()
        val layout = WindowManager.LayoutParams(
            width,
            height,
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
            } else {
                @Suppress("DEPRECATION")
                WindowManager.LayoutParams.TYPE_PHONE
            },
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = (metrics.widthPixels - width - (16 * density).toInt()).coerceAtLeast(0)
            y = (metrics.heightPixels * 0.18f).toInt()
        }
        val added = runCatching { windowManager.addView(compose, layout) }.isSuccess
        if (!added) {
            lifecycleOwner.detach()
            return
        }
        view = compose
        wm = windowManager
        owner = lifecycleOwner
    }

    fun hide() {
        view?.let { wm?.removeView(it) }
        view = null
        wm = null
        owner?.detach()
        owner = null
    }
}

private const val FLOAT_WIDTH_DP = 168f
private const val FLOAT_HEIGHT_DP = 210f

/** 浮窗点了「回到通话页」：MainActivity 启动后打开对应通话页。 */
object PendingCallOpen {
    @Volatile
    private var conversationId: String? = null

    fun request(id: String) {
        conversationId = id
    }

    fun consume(): String? = conversationId.also { conversationId = null }
}

/** ComposeView 需要的 Lifecycle/ViewModelStore/SavedState 兼容实现。 */
private class OverlayLifecycleOwner : LifecycleOwner, ViewModelStoreOwner, SavedStateRegistryOwner {
    private val registry = LifecycleRegistry(this)
    private var savedState: SavedStateRegistryController? = null
    override val lifecycle: Lifecycle get() = registry
    override val viewModelStore: ViewModelStore = ViewModelStore()
    override val savedStateRegistry: SavedStateRegistry
        get() {
            val controller = savedState
                ?: SavedStateRegistryController.create(this).also { savedState = it }
            return controller.savedStateRegistry
        }

    fun attach() {
        savedStateRegistry // 先创建，再恢复
        savedState?.performRestore(null)
        registry.currentState = Lifecycle.State.STARTED
    }

    fun detach() {
        registry.currentState = Lifecycle.State.DESTROYED
        viewModelStore.clear()
    }
}

@Composable
private fun FloatingContent(
    manager: CallManager,
    conversationId: String,
    conversation: ConversationRepository,
    onOpen: (CallManager.CallUiState) -> Unit,
    onHangup: () -> Unit,
) {
    val activeRoom by manager.room.collectAsState()
    val state = rememberActiveState(manager)
    val conv by remember(conversationId) {
        conversation.conversation(conversationId)
    }.collectAsState(initial = null)
    val video = state?.video == true
    val track by manager.localVideoTrack.collectAsState()
    val room = activeRoom
    val name = conv?.name.orEmpty().ifBlank { "齐悦" }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .clip(RoundedCornerShape(18.dp))
            .background(FloatingBackground)
            // 单击回到通话页（双击会让第一次点击丢失感知，返回时容易连续跳转）。
            .clickable(
                interactionSource = remember { androidx.compose.foundation.interaction.MutableInteractionSource() },
                indication = null,
            ) { state?.let(onOpen) },
    ) {
        if (video && room != null && track != null) {
            FloatTrackView(room = room, track = track!!, modifier = Modifier.fillMaxSize())
        } else {
            val model = StockMedia.drawableFor(conv?.avatar.orEmpty())
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Avatar(name = name, size = 78.dp, model = model, circle = true)
            }
        }
        Row(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .padding(8.dp)
                .clip(RoundedCornerShape(50))
                .background(FloatingPill)
                .padding(start = 10.dp, end = 4.dp, top = 3.dp, bottom = 3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Column {
                Text(name, color = Color.White, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
                Text(
                    text = state?.let {
                        if (it.connected) formatCallDuration(it.elapsedSeconds) else "正在接通…"
                    } ?: "",
                    color = FloatingMeta,
                    fontSize = 10.sp,
                )
            }
            Box(
                modifier = Modifier
                    .size(26.dp)
                    .clip(CircleShape)
                    .background(FloatingHangup)
                    .clickable(onClick = onHangup),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    imageVector = ShiYuIcons.Hangup,
                    contentDescription = "挂断",
                    tint = Color.White,
                    modifier = Modifier.size(14.dp),
                )
            }
        }
    }
}

@Composable
private fun rememberActiveState(manager: CallManager): CallManager.CallUiState? {
    val state by manager.uiState.collectAsState()
    return if (state.isActive) state else null
}

@Composable
private fun FloatTrackView(room: Room, track: LocalVideoTrack, modifier: Modifier = Modifier) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val renderer = remember { TextureViewRenderer(context) }
    androidx.compose.runtime.DisposableEffect(room, renderer) {
        runCatching { room.initVideoRenderer(renderer) }
        onDispose { runCatching { renderer.release() } }
    }
    androidx.compose.runtime.DisposableEffect(track, renderer) {
        runCatching { track.addRenderer(renderer) }
        onDispose { runCatching { track.removeRenderer(renderer) } }
    }
    androidx.compose.ui.viewinterop.AndroidView(factory = { renderer }, modifier = modifier)
}

private val FloatingBackground = Color(0xF0101416)
private val FloatingPill = Color(0xB806090B)
private val FloatingMeta = Color(0xFFC9D3D7)
private val FloatingHangup = Color(0xFFE5484D)
