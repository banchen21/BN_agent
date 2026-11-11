package com.bnagent.shiyu.ui.call

import android.Manifest
import android.app.Application
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
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import coil.compose.AsyncImage
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.LocalToast
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.theme.ShiYuCallColors
import com.bnagent.shiyu.ui.theme.ShiYuType
import io.livekit.android.renderer.SurfaceViewRenderer
import io.livekit.android.room.Room
import io.livekit.android.room.track.VideoTrack

/**
 * 语音通话（设计规格 4.7）：呼叫波纹、接通计时、静音 / 免提、挂断。
 *
 * 挂断与系统返回键等价；结束后由导航固定回到该会话聊天页。
 */
@Composable
fun VoiceCallScreen(
    conversationId: String,
    onEndToChat: () -> Unit,
) {
    val container = LocalAppContainer.current
    val app = LocalContext.current.applicationContext as Application
    val viewModel: CallViewModel = viewModel(
        factory = CallViewModel.factory(app, container.conversationRepository, conversationId, video = false),
    )
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    val conversation by viewModel.conversation.collectAsStateWithLifecycle()
    val toast = LocalToast.current

    val endCall: () -> Unit = {
        val result = viewModel.endCall()
        toast?.show(
            if (result.connected) {
                "通话结束 · ${formatCallDuration(result.elapsedSeconds)}"
            } else {
                "已取消通话"
            },
        )
        onEndToChat()
    }
    BackHandler(onBack = endCall)

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { grants ->
        if (grants.values.all { it }) {
            viewModel.connect()
        } else {
            toast?.show("需要麦克风权限才能通话")
            endCall()
        }
    }
    LaunchedEffect(Unit) {
        permissionLauncher.launch(arrayOf(Manifest.permission.RECORD_AUDIO))
    }
    LaunchedEffect(Unit) {
        viewModel.events.collect { toast?.show(it) }
    }
    LaunchedEffect(state.error) {
        state.error?.let { toast?.show("通话连接失败：$it") }
    }

    Box(Modifier.fillMaxSize().background(ShiYuCallColors.background)) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .navigationBarsPadding()
                .padding(start = 22.dp, end = 22.dp, top = 6.dp, bottom = 24.dp),
        ) {
            VoiceCallTopBar(onClose = endCall)
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
                    icon = ShiYuIcons.Mic,
                    label = "静音",
                    active = state.muted,
                    onClick = viewModel::toggleMute,
                )
                VoiceControl(
                    icon = ShiYuIcons.Speaker,
                    label = "免提",
                    active = state.speakerOn,
                    onClick = viewModel::toggleSpeaker,
                )
                HangupButton(size = 68.dp, iconSize = 28.dp, onClick = endCall)
            }
        }
    }
}

@Composable
private fun VoiceCallTopBar(onClose: () -> Unit) {
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
                .clickable(onClick = onClose),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = ShiYuIcons.Close,
                contentDescription = "挂断",
                tint = ShiYuCallColors.name,
                modifier = Modifier.size(21.dp),
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
    onEndToChat: () -> Unit,
) {
    val container = LocalAppContainer.current
    val app = LocalContext.current.applicationContext as Application
    val viewModel: CallViewModel = viewModel(
        factory = CallViewModel.factory(app, container.conversationRepository, conversationId, video = true),
    )
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    val conversation by viewModel.conversation.collectAsStateWithLifecycle()
    val me by viewModel.me.collectAsStateWithLifecycle()
    val localVideo by viewModel.localVideoTrack.collectAsStateWithLifecycle()
    val remoteVideo by viewModel.remoteVideoTrack.collectAsStateWithLifecycle()
    val toast = LocalToast.current

    val endCall: () -> Unit = {
        val result = viewModel.endCall()
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
        val flipped = viewModel.flipCamera()
        toast?.show(if (flipped) "已切换到前置摄像头" else "已切换到后置摄像头")
    }
    BackHandler(onBack = endCall)

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { grants ->
        if (grants.values.all { it }) {
            viewModel.connect()
        } else {
            toast?.show("需要麦克风与摄像头权限才能视频通话")
            endCall()
        }
    }
    LaunchedEffect(Unit) {
        permissionLauncher.launch(
            arrayOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.CAMERA),
        )
    }
    LaunchedEffect(Unit) {
        viewModel.events.collect { toast?.show(it) }
    }
    LaunchedEffect(state.error) {
        state.error?.let { toast?.show("通话连接失败：$it") }
    }

    Box(Modifier.fillMaxSize().background(ShiYuCallColors.background)) {
        // 远端画面：对方头像 cover，`object-position center 30%` 近似顶部对齐
        val remote = conversation?.let { StockMedia.drawableFor(it.avatar) }
        if (remote != null) {
            AsyncImage(
                model = remote,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                alignment = Alignment.TopCenter,
                modifier = Modifier.fillMaxSize(),
            )
        }
        // 对端发布视频时覆盖在头像上（悦悦只发音频，通常为空）。
        remoteVideo?.let { track ->
            LiveKitVideo(
                room = viewModel.room,
                track = track,
                mirrored = false,
                modifier = Modifier.fillMaxSize(),
            )
        }
        // 渐晕层
        Box(Modifier.fillMaxSize().background(VideoScrim))

        // 顶部：名字 + 状态 / 计时 + 切换摄像头
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
            Box(
                modifier = Modifier
                    .size(44.dp)
                    .clip(CircleShape)
                    .background(VideoFlipBg)
                    .clickable(onClick = onFlip),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    imageVector = ShiYuIcons.Flip,
                    contentDescription = "切换摄像头",
                    tint = Color.White,
                    modifier = Modifier.size(21.dp),
                )
            }
        }

        // 本地小窗：96×132，top 74 / right 16，翻转时镜像
        val localWindowShape = RoundedCornerShape(18.dp)
        Box(
            modifier = Modifier
                .align(Alignment.TopEnd)
                .padding(top = 74.dp, end = 16.dp)
                .size(width = 96.dp, height = 132.dp)
                .shadow(
                    elevation = 14.dp,
                    shape = localWindowShape,
                    ambientColor = Color.Black,
                    spotColor = Color.Black,
                )
                .clip(localWindowShape)
                .background(ShiYuCallColors.videoLocalBg)
                .border(1.dp, LocalWindowStroke, localWindowShape),
        ) {
            val local = StockMedia.drawableFor(me.avatar)
            val localTrack = localVideo
            if (localTrack != null) {
                LiveKitVideo(
                    room = viewModel.room,
                    track = localTrack,
                    mirrored = state.cameraFlipped,
                    modifier = Modifier.fillMaxSize(),
                )
            } else if (local != null) {
                AsyncImage(
                    model = local,
                    contentDescription = "本地画面",
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .fillMaxSize()
                        .graphicsLayer { scaleX = if (state.cameraFlipped) -1f else 1f },
                )
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
                icon = ShiYuIcons.Mic,
                label = "静音",
                active = state.muted,
                onClick = viewModel::toggleMute,
            )
            VideoControl(
                icon = ShiYuIcons.Speaker,
                label = "免提",
                active = state.speakerOn,
                onClick = viewModel::toggleSpeaker,
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
