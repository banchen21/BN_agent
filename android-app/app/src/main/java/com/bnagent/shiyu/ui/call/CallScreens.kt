package com.bnagent.shiyu.ui.call

import android.Manifest
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import coil.compose.AsyncImage
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.data.call.CallManager
import com.bnagent.shiyu.data.call.formatCallDuration
import com.bnagent.shiyu.domain.model.UserProfile
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.LocalToast
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.theme.ShiYuCallColors
import com.bnagent.shiyu.ui.theme.ShiYuType
import io.livekit.android.renderer.SurfaceViewRenderer
import io.livekit.android.room.Room
import io.livekit.android.room.track.VideoTrack
import kotlinx.coroutines.delay
import java.io.File
import kotlin.math.roundToInt

/**
 * 语音通话（设计规格 4.7）：呼叫波纹、接通计时、静音 / 免提、挂断。
 *
 * 右上角为最小化：只退出通话页，通话交给进程级 CallManager 继续（悬浮条可返回）；
 * 挂断走底部红色按钮，结束后由导航固定回到该会话聊天页。
 */
@Composable
fun VoiceCallScreen(
    conversationId: String,
    onMinimize: () -> Unit,
    onEndToChat: () -> Unit,
) {
    val container = LocalAppContainer.current
    val callManager = container.callManager
    val state by callManager.uiState.collectAsStateWithLifecycle()
    val conversation by container.conversationRepository.conversation(conversationId)
        .collectAsStateWithLifecycle(initialValue = null)
    val localVideo by callManager.localVideoTrack.collectAsStateWithLifecycle()
    val room by callManager.room.collectAsStateWithLifecycle()
    val toast = LocalToast.current

    val endCall: () -> Unit = {
        val result = callManager.endCall()
        toast?.show(
            if (result.connected) {
                "通话结束 · ${formatCallDuration(result.elapsedSeconds)}"
            } else {
                "已取消通话"
            },
        )
        onEndToChat()
    }
    // 返回键与右上角一致：最小化，不挂断。
    BackHandler(onBack = onMinimize)

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { grants ->
        if (grants.values.all { it }) {
            // 已在通话中时 connect 幂等复用现有连接（从悬浮条返回）。
            callManager.connect(conversationId, video = false)
        } else {
            toast?.show("需要麦克风权限才能通话")
            endCall()
        }
    }
    LaunchedEffect(Unit) {
        if (hasPermission(Manifest.permission.RECORD_AUDIO)) {
            // 从悬浮条/浮窗返回且在通话中：直接续上，不再重复弹权限框。
            callManager.connect(conversationId, video = false)
        } else {
            permissionLauncher.launch(arrayOf(Manifest.permission.RECORD_AUDIO))
        }
    }
    LaunchedEffect(Unit) {
        callManager.events.collect { toast?.show(it) }
    }
    LaunchedEffect(state.error) {
        state.error?.let { toast?.show("通话连接失败：$it") }
    }
    var incomingPhoto by remember { mutableStateOf<CallManager.IncomingImage?>(null) }
    LaunchedEffect(Unit) {
        callManager.incomingImages.collect { incomingPhoto = it }
    }

    Box(Modifier.fillMaxSize().background(ShiYuCallColors.background)) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .navigationBarsPadding()
                .padding(start = 22.dp, end = 22.dp, top = 6.dp, bottom = 24.dp),
        ) {
            VoiceCallTopBar(onMinimize = onMinimize)
            Column(
                modifier = Modifier.weight(1f).fillMaxWidth(),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Box(Modifier.size(176.dp), contentAlignment = Alignment.Center) {
                    if (!state.connected) {
                        CallingRipple()
                    }
                    Avatar(
                        name = conversation?.name.orEmpty(),
                        size = 112.dp,
                        model = conversation?.let { StockMedia.drawableFor(it.avatar) },
                        circle = true,
                        modifier = Modifier.shadow(
                            elevation = 18.dp,
                            shape = CircleShape,
                            ambientColor = Color.Black,
                            spotColor = Color.Black,
                        ),
                    )
                }
                Spacer(Modifier.height(20.dp))
                Text(
                    text = conversation?.name.orEmpty(),
                    color = ShiYuCallColors.name,
                    style = ShiYuType.callName,
                )
                Spacer(Modifier.height(6.dp))
                Text(
                    text = when {
                        state.error != null -> "连接失败"
                        state.connected -> formatCallDuration(state.elapsedSeconds)
                        else -> "正在呼叫…"
                    },
                    color = ShiYuCallColors.status,
                    style = ShiYuType.callStatus.copy(fontFeatureSettings = "tnum"),
                )
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(34.dp, Alignment.CenterHorizontally),
                verticalAlignment = Alignment.Top,
            ) {
                VoiceControl(
                    icon = if (state.muted) ShiYuIcons.MicOff else ShiYuIcons.Mic,
                    label = if (state.muted) "已静音" else "静音",
                    active = state.muted,
                    onClick = callManager::toggleMute,
                )
                VoiceControl(
                    icon = if (state.speakerOn) ShiYuIcons.SpeakerOn else ShiYuIcons.Speaker,
                    label = if (state.speakerOn) "外放中" else "免提",
                    active = state.speakerOn,
                    onClick = callManager::toggleSpeaker,
                )
                VoiceControl(
                    icon = ShiYuIcons.Video,
                    label = if (state.cameraEnabled) "关摄像头" else "摄像头",
                    active = state.cameraEnabled,
                    onClick = callManager::toggleCamera,
                )
                HangupButton(size = 68.dp, iconSize = 28.dp, onClick = endCall)
            }
        }
        // 语音通话中开启摄像头后显示本地预览（Agent 同时能收到视频帧）。
        val localTrack = localVideo
        val activeRoom = room
        if (state.cameraEnabled) {
            Box(
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(top = 88.dp, end = 16.dp)
                    .size(width = 96.dp, height = 132.dp)
                    .clip(RoundedCornerShape(18.dp))
                    .background(ShiYuCallColors.videoLocalBg),
            ) {
                if (localTrack != null && activeRoom != null) {
                    LiveKitVideo(
                        room = activeRoom,
                        track = localTrack,
                        mirrored = true,
                        modifier = Modifier.fillMaxSize(),
                    )
                }
            }
        }
        incomingPhoto?.let { photo ->
            CallPhotoOverlay(photo = photo, onDismiss = { incomingPhoto = null })
        }
    }
}

@Composable
private fun VoiceCallTopBar(onMinimize: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(
            modifier = Modifier
                .clip(RoundedCornerShape(50))
                .background(ShiYuCallColors.controlBg)
                .padding(horizontal = 12.dp, vertical = 7.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(
                imageVector = ShiYuIcons.Lock,
                contentDescription = null,
                tint = ShiYuCallColors.pillText,
                modifier = Modifier.size(13.dp),
            )
            Text(
                text = "语音通话 · 已加密",
                color = ShiYuCallColors.pillText,
                fontSize = 11.5.sp,
                fontWeight = FontWeight.SemiBold,
                lineHeight = 16.sp,
            )
        }
        Box(
            modifier = Modifier
                .size(44.dp)
                .clip(RoundedCornerShape(14.dp))
                .background(CallSubtleBg)
                .clickable(onClick = onMinimize),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = ShiYuIcons.Minimize,
                contentDescription = "最小化",
                tint = ShiYuCallColors.name,
                modifier = Modifier.size(24.dp),
            )
        }
    }
}

/** 呼叫波纹：两圈 1px 外扩描边，2.2s 循环、第二圈延迟 0.8s（设计规格 2.12 / 4.7）。 */
@Composable
private fun CallingRipple() {
    val transition = rememberInfiniteTransition(label = "call-ripple")
    val scale1 by transition.animateFloat(
        initialValue = 1f,
        targetValue = 1.28f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 2200, easing = LinearOutSlowInEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "ripple-scale-1",
    )
    val alpha1 by transition.animateFloat(
        initialValue = 0.8f,
        targetValue = 0f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 2200, easing = LinearOutSlowInEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "ripple-alpha-1",
    )
    val scale2 by transition.animateFloat(
        initialValue = 1f,
        targetValue = 1.28f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 2200, delayMillis = 800, easing = LinearOutSlowInEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "ripple-scale-2",
    )
    val alpha2 by transition.animateFloat(
        initialValue = 0.8f,
        targetValue = 0f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 2200, delayMillis = 800, easing = LinearOutSlowInEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "ripple-alpha-2",
    )
    RippleRing(scale = scale1, alpha = alpha1, color = RippleOuter)
    RippleRing(scale = scale2, alpha = alpha2, color = RippleInner)
}

@Composable
private fun RippleRing(scale: Float, alpha: Float, color: Color) {
    Box(
        modifier = Modifier
            .size(112.dp)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
                this.alpha = alpha
            }
            .border(1.dp, color, CircleShape),
    )
}

@Composable
private fun VoiceControl(
    icon: ImageVector,
    label: String,
    active: Boolean,
    onClick: () -> Unit,
) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(7.dp),
    ) {
        Box(
            modifier = Modifier
                .size(62.dp)
                .clip(CircleShape)
                .background(if (active) ShiYuCallColors.controlActiveBg else ShiYuCallColors.controlBg)
                .clickable(onClick = onClick),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = icon,
                contentDescription = label,
                tint = if (active) ShiYuCallColors.controlActiveInk else ShiYuCallColors.name,
                modifier = Modifier.size(25.dp),
            )
        }
        Text(
            text = label,
            color = ShiYuCallColors.controlLabel,
            style = ShiYuType.callControlLabel,
        )
    }
}

/**
 * 视频通话（设计规格 4.8）：远端头像铺满 + 渐晕、本地小窗（可镜像）、
 * 静音 / 免提 / 切换摄像头 / 挂断，接通后顶部显示计时。
 */
@Composable
fun VideoCallScreen(
    conversationId: String,
    onMinimize: () -> Unit,
    onEndToChat: () -> Unit,
) {
    val container = LocalAppContainer.current
    val callManager = container.callManager
    val state by callManager.uiState.collectAsStateWithLifecycle()
    val conversation by container.conversationRepository.conversation(conversationId)
        .collectAsStateWithLifecycle(initialValue = null)
    val me by container.conversationRepository.profile
        .collectAsStateWithLifecycle(initialValue = UserProfile())
    val localVideo by callManager.localVideoTrack.collectAsStateWithLifecycle()
    val remoteVideo by callManager.remoteVideoTrack.collectAsStateWithLifecycle()
    val room by callManager.room.collectAsStateWithLifecycle()
    val toast = LocalToast.current

    val endCall: () -> Unit = {
        val result = callManager.endCall()
        toast?.show(
            if (result.connected) {
                "通话结束 · ${formatCallDuration(result.elapsedSeconds)}"
            } else {
                "已取消通话"
            },
        )
        onEndToChat()
    }
    val onFlip: () -> Unit = {
        val flipped = callManager.flipCamera()
        toast?.show(if (flipped) "已切换到前置摄像头" else "已切换到后置摄像头")
    }
    // 返回键与右上角一致：最小化，不挂断。
    BackHandler(onBack = onMinimize)

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { grants ->
        if (grants.values.all { it }) {
            // 已在通话中时 connect 幂等复用现有连接（从悬浮条返回）。
            callManager.connect(conversationId, video = true)
        } else {
            toast?.show("需要麦克风与摄像头权限才能视频通话")
            endCall()
        }
    }
    LaunchedEffect(Unit) {
        val perms = arrayOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.CAMERA)
        if (perms.all(::hasPermission)) {
            // 从悬浮条/浮窗返回且在通话中：直接续上，不再重复弹权限框。
            callManager.connect(conversationId, video = true)
        } else {
            permissionLauncher.launch(perms)
        }
    }
    LaunchedEffect(Unit) {
        callManager.events.collect { toast?.show(it) }
    }
    LaunchedEffect(state.error) {
        state.error?.let { toast?.show("通话连接失败：$it") }
    }
    var incomingPhoto by remember { mutableStateOf<CallManager.IncomingImage?>(null) }
    LaunchedEffect(Unit) {
        callManager.incomingImages.collect { incomingPhoto = it }
    }

    var localLarge by remember { mutableStateOf(false) }
    var pipOffset by remember { mutableStateOf(Offset.Zero) }
    val remoteDrawable = conversation?.let { StockMedia.drawableFor(it.avatar) }
    val meDrawable = StockMedia.avatarModel(me.avatar)
    val activeRoom = room

    BoxWithConstraints(Modifier.fillMaxSize().background(ShiYuCallColors.background)) {
        // 小窗拖动边界：小窗必须完整留在屏幕内（对齐右下角为基准）。
        val density = LocalDensity.current
        val screenWidthPx = with(density) { maxWidth.toPx() }
        val screenHeightPx = with(density) { maxHeight.toPx() }
        val pipWidthPx = with(density) { 96.dp.toPx() }
        val pipHeightPx = with(density) { 132.dp.toPx() }
        val pipEndPx = with(density) { 16.dp.toPx() }
        val pipTopPx = with(density) { 128.dp.toPx() }
        val pipBaseLeft = screenWidthPx - pipEndPx - pipWidthPx
        val minOffsetX = -pipBaseLeft
        val maxOffsetX = screenWidthPx - pipWidthPx - pipBaseLeft
        val minOffsetY = -pipTopPx
        val maxOffsetY = screenHeightPx - pipHeightPx - pipTopPx
        val dragPip: (Offset) -> Unit = { delta ->
            pipOffset = Offset(
                (pipOffset.x + delta.x).coerceIn(minOffsetX, maxOffsetX),
                (pipOffset.y + delta.y).coerceIn(minOffsetY, maxOffsetY),
            )
        }
        // 大屏：默认远端（悦悦自拍 / 远端视频），点击后与本地画面互换。
        Box(
            modifier = Modifier
                .fillMaxSize()
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                ) { localLarge = !localLarge },
        ) {
            if (localLarge) {
                val localTrack = localVideo
                if (localTrack != null && activeRoom != null) {
                    LiveKitVideo(
                        room = activeRoom,
                        track = localTrack,
                        mirrored = state.cameraFlipped,
                        modifier = Modifier.fillMaxSize(),
                    )
                } else if (meDrawable != null) {
                    AsyncImage(
                        model = meDrawable,
                        contentDescription = "本地画面",
                        contentScale = ContentScale.Crop,
                        modifier = Modifier.fillMaxSize(),
                    )
                }
            } else {
                val remoteTrack = remoteVideo
                if (remoteTrack != null && activeRoom != null) {
                    LiveKitVideo(
                        room = activeRoom,
                        track = remoteTrack,
                        mirrored = false,
                        modifier = Modifier.fillMaxSize(),
                    )
                } else if (remoteDrawable != null) {
                    AsyncImage(
                        model = remoteDrawable,
                        contentDescription = "对方画面",
                        contentScale = ContentScale.Crop,
                        alignment = Alignment.TopCenter,
                        modifier = Modifier.fillMaxSize(),
                    )
                }
            }
        }

        // 渐晕层
        Box(Modifier.fillMaxSize().background(VideoScrim))

        // 顶部：名字 + 状态 / 计时 + 最小化 / 切换摄像头
        Row(
            modifier = Modifier
                .align(Alignment.TopCenter)
                .fillMaxWidth()
                .statusBarsPadding()
                .padding(start = 18.dp, end = 18.dp, top = 10.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.Top,
        ) {
            Column {
                Text(
                    text = conversation?.name.orEmpty(),
                    color = ShiYuCallColors.name,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.Bold,
                    lineHeight = 23.sp,
                )
                Text(
                    text = when {
                        state.error != null -> "连接失败"
                        state.connected -> formatCallDuration(state.elapsedSeconds)
                        else -> "正在呼叫…"
                    },
                    modifier = Modifier.padding(top = 2.dp),
                    color = VideoStatusColor,
                    style = ShiYuType.callStatus.copy(
                        fontSize = 12.sp,
                        lineHeight = 16.sp,
                        fontFeatureSettings = "tnum",
                    ),
                )
            }
            VideoTopButton(
                icon = ShiYuIcons.Minimize,
                label = "最小化",
                onClick = onMinimize,
            )
        }

        // 小窗：默认本地画面；点击小窗即可交换大小屏；可拖动。
        val smallIsLocal = !localLarge
        DraggablePip(
            offset = pipOffset,
            onDrag = dragPip,
            onClick = { localLarge = !localLarge },
            modifier = Modifier
                .align(Alignment.TopEnd)
                .padding(top = 128.dp, end = 16.dp)
                .size(width = 96.dp, height = 132.dp),
        ) {
            if (smallIsLocal) {
                val localTrack = localVideo
                if (localTrack != null && activeRoom != null) {
                    LiveKitVideo(
                        room = activeRoom,
                        track = localTrack,
                        mirrored = state.cameraFlipped,
                        modifier = Modifier.fillMaxSize(),
                    )
                } else if (meDrawable != null) {
                    AsyncImage(
                        model = meDrawable,
                        contentDescription = "本地画面",
                        contentScale = ContentScale.Crop,
                        modifier = Modifier
                            .fillMaxSize()
                            .graphicsLayer { scaleX = if (state.cameraFlipped) -1f else 1f },
                    )
                }
            } else {
                val remoteTrack = remoteVideo
                if (remoteTrack != null && activeRoom != null) {
                    LiveKitVideo(
                        room = activeRoom,
                        track = remoteTrack,
                        mirrored = false,
                        modifier = Modifier.fillMaxSize(),
                    )
                } else if (remoteDrawable != null) {
                    AsyncImage(
                        model = remoteDrawable,
                        contentDescription = "对方画面",
                        contentScale = ContentScale.Crop,
                        alignment = Alignment.TopCenter,
                        modifier = Modifier.fillMaxSize(),
                    )
                }
            }
        }

        // 底部控制条
        Row(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(start = 20.dp, end = 20.dp, bottom = 30.dp),
            horizontalArrangement = Arrangement.spacedBy(18.dp, Alignment.CenterHorizontally),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            VideoControl(
                icon = if (state.muted) ShiYuIcons.MicOff else ShiYuIcons.Mic,
                label = if (state.muted) "已静音" else "静音",
                active = state.muted,
                onClick = callManager::toggleMute,
            )
            VideoControl(
                icon = if (state.speakerOn) ShiYuIcons.SpeakerOn else ShiYuIcons.Speaker,
                label = if (state.speakerOn) "外放中" else "免提",
                active = state.speakerOn,
                onClick = callManager::toggleSpeaker,
            )
            VideoControl(
                icon = ShiYuIcons.Flip,
                label = "切换摄像头",
                active = state.cameraFlipped,
                onClick = onFlip,
            )
            HangupButton(
                size = 54.dp,
                iconSize = 24.dp,
                onClick = endCall,
                modifier = Modifier.padding(start = 6.dp),
            )
        }

        incomingPhoto?.let { photo ->
            CallPhotoOverlay(photo = photo, onDismiss = { incomingPhoto = null })
        }
    }
}

@Composable
private fun VideoControl(
    icon: ImageVector,
    label: String,
    active: Boolean,
    onClick: () -> Unit,
) {
    Box(
        modifier = Modifier
            .size(54.dp)
            .clip(CircleShape)
            .background(if (active) ShiYuCallColors.controlActiveBg else ShiYuCallColors.videoControlBg)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = label,
            tint = if (active) ShiYuCallColors.controlActiveInk else Color.White,
            modifier = Modifier.size(23.dp),
        )
    }
}

@Composable
private fun VideoTopButton(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
) {
    Box(
        modifier = Modifier
            .size(44.dp)
            .clip(CircleShape)
            .background(VideoFlipBg)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = label,
            tint = Color.White,
            modifier = Modifier.size(21.dp),
        )
    }
}

@Composable
private fun HangupButton(
    size: Dp,
    iconSize: Dp,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier
            .size(size)
            .shadow(
                elevation = 12.dp,
                shape = CircleShape,
                ambientColor = ShiYuCallColors.hangup.copy(alpha = 0.4f),
                spotColor = ShiYuCallColors.hangup,
            )
            .clip(CircleShape)
            .background(ShiYuCallColors.hangup)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = ShiYuIcons.Hangup,
            contentDescription = "挂断",
            tint = Color.White,
            modifier = Modifier.size(iconSize),
        )
    }
}

/** 通话中收到的图片：居中浮层，点按关闭，8 秒自动消失。 */
@Composable
private fun CallPhotoOverlay(
    photo: CallManager.IncomingImage,
    onDismiss: () -> Unit,
) {
    LaunchedEffect(photo.path) {
        delay(8_000)
        onDismiss()
    }
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color(0xB306090B))
            .clickable(onClick = onDismiss),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(10.dp),
            modifier = Modifier.padding(horizontal = 24.dp),
        ) {
            AsyncImage(
                model = File(photo.path),
                contentDescription = photo.caption.ifBlank { "收到的图片" },
                contentScale = ContentScale.Fit,
                modifier = Modifier
                    .widthIn(max = 320.dp)
                    .heightIn(max = 440.dp)
                    .clip(RoundedCornerShape(18.dp))
                    .background(ShiYuCallColors.videoLocalBg),
            )
            Text(
                text = photo.caption.ifBlank { "齐悦发来一张图片 · 轻点关闭" },
                color = Color.White,
                fontSize = 12.5.sp,
                lineHeight = 17.sp,
            )
        }
    }
}

/** 可拖动 + 点击交换大小屏的小窗。 */
@Composable
private fun DraggablePip(
    offset: Offset,
    onDrag: (Offset) -> Unit,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable BoxScope.() -> Unit,
) {
    val shape = RoundedCornerShape(18.dp)
    Box(
        modifier = modifier
            .offset { IntOffset(offset.x.roundToInt(), offset.y.roundToInt()) }
            .shadow(
                elevation = 14.dp,
                shape = shape,
                ambientColor = Color.Black,
                spotColor = Color.Black,
            )
            .clip(shape)
            .background(ShiYuCallColors.videoLocalBg)
            .border(1.dp, LocalWindowStroke, shape)
            .pointerInput(Unit) {
                detectDragGestures { change, drag ->
                    change.consume()
                    onDrag(drag)
                }
            }
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                onClick = onClick,
            ),
        content = content,
    )
}

/** LiveKit 视频轨渲染（本地预览 / 远端画面共用）。 */
@Composable
private fun LiveKitVideo(
    room: Room,
    track: VideoTrack,
    mirrored: Boolean,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val renderer = remember { SurfaceViewRenderer(context) }
    DisposableEffect(room, renderer) {
        runCatching { room.initVideoRenderer(renderer) }
        onDispose { runCatching { renderer.release() } }
    }
    DisposableEffect(track, renderer) {
        runCatching { track.addRenderer(renderer) }
        onDispose { runCatching { track.removeRenderer(renderer) } }
    }
    AndroidView(
        factory = { renderer },
        modifier = modifier.graphicsLayer { scaleX = if (mirrored) -1f else 1f },
    )
}

/** 是否已授予权限（用于从悬浮条返回时跳过重复授权弹窗）。 */
private fun hasPermission(permission: String): Boolean {
    val context = com.bnagent.shiyu.ui.call.CallFloatingService.appContext
    return context != null &&
        androidx.core.content.ContextCompat.checkSelfPermission(context, permission) ==
        android.content.pm.PackageManager.PERMISSION_GRANTED
}

/** 通话页固定配色（不随主题变化）。 */
private val CallSubtleBg = Color(0x14FFFFFF)
private val RippleOuter = Color(0x2EFFFFFF)
private val RippleInner = Color(0x1AFFFFFF)
private val VideoStatusColor = Color(0xFFC9D3D7)
private val VideoFlipBg = Color(0x80101416)
private val LocalWindowStroke = Color(0x29FFFFFF)

/** 视频渐晕：上 72% → 12% → 透明 → 下 55% → 86%。 */
private val VideoScrim = Brush.verticalGradient(
    0f to Color(0xB806090B),
    0.24f to Color(0x1F06090B),
    0.46f to Color(0x0006090B),
    0.82f to Color(0x8C06090B),
    1f to Color(0xDB06090B),
)
