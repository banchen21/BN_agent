package com.bnagent.shiyu.ui.chat

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.StartOffset
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.compose.AsyncImage
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.domain.model.MessageSender
import com.bnagent.shiyu.domain.model.MessageStatus
import com.bnagent.shiyu.domain.model.durationLabel
import com.bnagent.shiyu.domain.model.summary
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType
import java.io.File
import kotlin.math.PI
import kotlin.math.sin

/**
 * 聊天消息区（设计规格 4.2）：日期分隔、四种气泡、群聊发送者名、
 * 最后一条我方消息的已读/已送达/发送失败状态、正在输入三点动画。
 */

/** 图片消息资源：内置素材名走 StockMedia，Photo Picker 拷贝出来的本地路径走 File。 */
internal fun imageModelFor(asset: String): Any? {
    val path = asset.removePrefix("file://")
    return if (path.startsWith("/")) {
        File(path).takeIf { it.isFile }
    } else {
        StockMedia.drawableFor(asset)
    }
}

/** 聊天背景 + 点阵（设计规格 2.6；chatDot 为 Transparent 时不画点）。 */
@Composable
internal fun Modifier.chatWallpaper(): Modifier {
    val colors = ShiYuTheme.colors
    return this
        .background(colors.chatBg)
        .drawBehind {
            val dot = colors.chatDot
            if (dot == Color.Transparent) return@drawBehind
            val step = 16.dp.toPx()
            val radius = 1.dp.toPx()
            var y = step / 2f
            while (y < size.height + step) {
                var x = step / 2f
                while (x < size.width + step) {
                    drawCircle(dot, radius, androidx.compose.ui.geometry.Offset(x, y))
                    x += step
                }
                y += step
            }
        }
}

@Composable
internal fun MessageList(
    messages: List<Message>,
    conversation: Conversation,
    typing: Boolean,
    pendingReply: Boolean,
    playingVoiceId: String?,
    onImageClick: (Message.Image) -> Unit,
    onVoiceClick: (Message.Voice) -> Unit,
    onDownloadClick: () -> Unit,
    onAtBottomChange: (Boolean) -> Unit = {},
    modifier: Modifier = Modifier,
) {
    val listState = rememberLazyListState()
    var didInitialScroll by remember { mutableStateOf(false) }

    LaunchedEffect(listState) {
        snapshotFlow {
            val info = listState.layoutInfo
            val lastVisible = info.visibleItemsInfo.lastOrNull()?.index ?: -1
            lastVisible >= info.totalItemsCount - 1
        }.distinctUntilChanged().collect(onAtBottomChange)
    }

    LaunchedEffect(messages.size, typing) {
        val target = messages.size - 1 + if (typing) 1 else 0
        if (target < 0) return@LaunchedEffect
        if (!didInitialScroll) {
            listState.scrollToItem(target)
            didInitialScroll = true
        } else {
            listState.animateScrollToItem(target)
        }
    }

    BoxWithConstraints(modifier = modifier.fillMaxSize()) {
        // 设计规格 4.2：气泡栈最大宽度 = 行宽 − 86dp；窄屏（≤380dp）图片 208dp。
        val bubbleMaxWidth = maxWidth - 24.dp - 86.dp
        val imageWidth = if (maxWidth <= 380.dp) 208.dp else 232.dp
        // 设计规格 6.3：仅当会话最后一条消息是我方且带状态时才显示「已读 / 已送达」。
        val lastMineId = messages.lastOrNull()
            ?.takeIf { it.sender == MessageSender.ME }
            ?.id

        LazyColumn(
            state = listState,
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 14.dp, bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            items(messages, key = { it.id }) { message ->
                MessageRow(
                    message = message,
                    conversation = conversation,
                    isLastMine = message.id == lastMineId,
                    pendingReply = pendingReply,
                    imageWidth = imageWidth,
                    bubbleMaxWidth = bubbleMaxWidth,
                    playing = message is Message.Voice && playingVoiceId == message.id,
                    onImageClick = onImageClick,
                    onVoiceClick = onVoiceClick,
                    onDownloadClick = onDownloadClick,
                )
            }
            if (typing) {
                item(key = "typing-indicator") {
                    TypingRow(conversation)
                }
            }
        }
    }
}

@Composable
private fun MessageRow(
    message: Message,
    conversation: Conversation,
    isLastMine: Boolean,
    pendingReply: Boolean,
    imageWidth: Dp,
    bubbleMaxWidth: Dp,
    playing: Boolean,
    onImageClick: (Message.Image) -> Unit,
    onVoiceClick: (Message.Voice) -> Unit,
    onDownloadClick: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    if (message is Message.System) {
        DateDivider(message.text)
        return
    }
    val isMine = message.sender == MessageSender.ME
    val avatarModel = if (isMine) {
        StockMedia.drawableFor(StockMedia.AVATAR_ME)
    } else {
        StockMedia.drawableFor(conversation.avatar)
    }

    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = if (isMine) Arrangement.End else Arrangement.Start,
        verticalAlignment = Alignment.Bottom,
    ) {
        if (!isMine) {
            Avatar(
                name = conversation.name,
                size = 34.dp,
                model = avatarModel,
                modifier = Modifier.padding(bottom = 2.dp),
            )
            Spacer(Modifier.width(8.dp))
        }
        Column(
            modifier = Modifier.widthIn(max = bubbleMaxWidth),
            horizontalAlignment = if (isMine) Alignment.End else Alignment.Start,
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            // 群聊对方消息在气泡上方显示发送者名（我方不显示）。
            val senderName = message.who?.takeIf { it.isNotBlank() }
            if (!isMine && conversation.isGroup && senderName != null) {
                Text(
                    text = senderName,
                    modifier = Modifier.padding(start = 2.dp),
                    color = colors.chatMeta,
                    fontSize = 11.5.sp,
                    fontWeight = FontWeight.SemiBold,
                    lineHeight = 16.sp,
                )
            }
            when (message) {
                is Message.Text -> TextBubble(message, isMine)
                is Message.Image -> ImageBubble(message, isMine, imageWidth, onImageClick)
                is Message.File -> FileBubble(message, onDownloadClick)
                is Message.Voice -> VoiceBubble(message, isMine, playing, onVoiceClick)
                is Message.Call -> CallBubble(message, isMine)
                is Message.System -> Unit
            }
            val status = message.status
            if (isMine && isLastMine && status != null) {
                MessageStatusRow(status = status, pending = pendingReply && status == MessageStatus.SENT)
            }
        }
        if (isMine) {
            Spacer(Modifier.width(8.dp))
            Avatar(
                name = "我",
                size = 34.dp,
                model = avatarModel,
                modifier = Modifier.padding(bottom = 2.dp),
            )
        }
    }
}

// ─── 日期分隔 ─────────────────────────────────────────────────────────

@Composable
private fun DateDivider(text: String) {
    val colors = ShiYuTheme.colors
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Box(
            modifier = Modifier
                .clip(RoundedCornerShape(50))
                .background(colors.dateChipBg)
                .padding(horizontal = 10.dp, vertical = 4.dp),
        ) {
            Text(
                text = text,
                color = colors.chatMeta,
                fontSize = 11.5.sp,
                fontWeight = FontWeight.SemiBold,
                lineHeight = 16.sp,
            )
        }
    }
}

// ─── 文本气泡 ─────────────────────────────────────────────────────────

@Composable
private fun TextBubble(message: Message.Text, isMine: Boolean) {
    val colors = ShiYuTheme.colors
    val dimens = ShiYuTheme.dimens
    val shape = ShiYuTheme.bubbleShape(isMine = isMine, tail = true)
    Box(
        modifier = Modifier
            .widthIn(min = 42.dp)
            .clip(shape)
            .background(if (isMine) colors.bubbleOut else colors.bubbleIn)
            .then(if (isMine) Modifier else Modifier.border(1.dp, colors.line, shape))
            .padding(horizontal = 13.dp, vertical = 10.dp),
    ) {
        Text(
            text = message.content,
            color = if (isMine) colors.bubbleOutInk else colors.bubbleInInk,
            fontSize = dimens.chatFontSize,
            lineHeight = dimens.chatLineHeight,
        )
    }
}

// ─── 通话记录气泡 ─────────────────────────────────────────────────────

/** 通话记录：与文本气泡同形状，图标 + 「语音通话：mm:ss」/「视频通话：mm:ss」。 */
@Composable
private fun CallBubble(message: Message.Call, isMine: Boolean) {
    val colors = ShiYuTheme.colors
    val dimens = ShiYuTheme.dimens
    val shape = ShiYuTheme.bubbleShape(isMine = isMine, tail = true)
    Row(
        modifier = Modifier
            .clip(shape)
            .background(if (isMine) colors.bubbleOut else colors.bubbleIn)
            .then(if (isMine) Modifier else Modifier.border(1.dp, colors.line, shape))
            .padding(horizontal = 13.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(
            imageVector = if (message.video) ShiYuIcons.Video else ShiYuIcons.Phone,
            contentDescription = null,
            tint = if (isMine) colors.bubbleOutInk else colors.bubbleInInk,
            modifier = Modifier.size(17.dp),
        )
        Text(
            text = message.summary,
            color = if (isMine) colors.bubbleOutInk else colors.bubbleInInk,
            fontSize = dimens.chatFontSize,
            lineHeight = dimens.chatLineHeight,
        )
    }
}

// ─── 图片气泡 ─────────────────────────────────────────────────────────

@Composable
private fun ImageBubble(
    message: Message.Image,
    isMine: Boolean,
    imageWidth: Dp,
    onClick: (Message.Image) -> Unit,
) {
    val colors = ShiYuTheme.colors
    val shape = ShiYuTheme.bubbleShape(isMine = isMine, tail = false)
    val interaction = remember { MutableInteractionSource() }
    Box(
        modifier = Modifier
            .width(imageWidth)
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, colors.line, shape)
            .clickable(
                interactionSource = interaction,
                indication = null,
                onClick = { onClick(message) },
            ),
    ) {
        AsyncImage(
            model = imageModelFor(message.asset),
            contentDescription = message.alt ?: "图片消息",
            contentScale = ContentScale.Crop,
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 120.dp),
        )
    }
}

// ─── 文件气泡 ─────────────────────────────────────────────────────────

@Composable
private fun FileBubble(message: Message.File, onDownloadClick: () -> Unit) {
    val colors = ShiYuTheme.colors
    val shape = ShiYuTheme.bubbleShape(isMine = false, tail = false)
    val interaction = remember { MutableInteractionSource() }
    Row(
        modifier = Modifier
            .width(254.dp)
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, colors.line, shape)
            .padding(horizontal = 11.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(
            modifier = Modifier
                .size(38.dp)
                .clip(RoundedCornerShape(12.dp))
                .background(colors.accentSoft),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = ShiYuIcons.File,
                contentDescription = null,
                tint = colors.accentText,
                modifier = Modifier.size(20.dp),
            )
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                text = message.fileName,
                color = colors.ink,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                lineHeight = 19.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = message.sizeLabel,
                color = colors.ink3,
                fontSize = 11.5.sp,
                fontWeight = FontWeight.Normal,
                lineHeight = 16.sp,
            )
        }
        Box(
            modifier = Modifier
                .size(44.dp)
                .clip(RoundedCornerShape(13.dp))
                .background(colors.surface2)
                .clickable(
                    interactionSource = interaction,
                    indication = null,
                    onClick = onDownloadClick,
                ),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = ShiYuIcons.Download,
                contentDescription = "下载文件",
                tint = colors.ink2,
                modifier = Modifier.size(18.dp),
            )
        }
    }
}

// ─── 语音气泡 ─────────────────────────────────────────────────────────

@Composable
private fun VoiceBubble(
    message: Message.Voice,
    isMine: Boolean,
    playing: Boolean,
    onVoiceClick: (Message.Voice) -> Unit,
) {
    val colors = ShiYuTheme.colors
    val shape = ShiYuTheme.bubbleShape(isMine = isMine, tail = true)
    val ink = if (isMine) colors.bubbleOutInk else colors.bubbleInInk
    val interaction = remember { MutableInteractionSource() }
    Column(
        horizontalAlignment = if (isMine) Alignment.End else Alignment.Start,
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(
            modifier = Modifier
                .widthIn(min = 156.dp)
                .clip(shape)
                .background(if (isMine) colors.bubbleOut else colors.bubbleIn)
                .then(if (isMine) Modifier else Modifier.border(1.dp, colors.line, shape))
                .clickable(
                    interactionSource = interaction,
                    indication = null,
                    onClick = { onVoiceClick(message) },
                )
                .padding(horizontal = 14.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(9.dp),
        ) {
            Icon(
                imageVector = ShiYuIcons.Play,
                contentDescription = if (playing) "停止播放" else "播放语音",
                tint = ink.copy(alpha = if (playing) 0.7f else 1f),
                modifier = Modifier.size(15.dp),
            )
            VoiceWaveform(playing = playing, color = ink)
            Text(
                text = message.durationLabel,
                color = ink,
                fontSize = 11.5.sp,
                fontWeight = FontWeight.SemiBold,
                lineHeight = 16.sp,
                maxLines = 1,
                softWrap = false,
                modifier = Modifier.padding(start = 2.dp),
            )
        }
        // 真实语音消息：气泡下方显示 ASR / TTS 转写（种子演示语音没有转写）。
        val transcript = message.transcript?.takeIf { it.isNotBlank() }
        if (transcript != null) {
            Text(
                text = transcript,
                color = colors.chatMeta,
                fontSize = 11.5.sp,
                lineHeight = 16.sp,
                modifier = Modifier.padding(horizontal = 6.dp),
            )
        }
    }
}

@Composable
private fun VoiceWaveform(playing: Boolean, color: Color) {
    val heights = remember { listOf(5, 16, 14, 12, 10, 8, 6, 17, 15, 13, 11, 9, 7, 5, 16, 14, 12, 10, 8, 6) }
    val progress = if (playing) {
        val transition = rememberInfiniteTransition(label = "voice-wave")
        val value by transition.animateFloat(
            initialValue = 0f,
            targetValue = 1f,
            animationSpec = infiniteRepeatable(
                animation = tween(durationMillis = 850, easing = LinearEasing),
                repeatMode = RepeatMode.Restart,
            ),
            label = "voice-wave-progress",
        )
        value
    } else {
        1f
    }
    Row(
        modifier = Modifier.height(20.dp),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        heights.forEachIndexed { index, height ->
            val phase = when (index % 3) {
                0 -> 0.12f
                1 -> 0.24f
                else -> 0f
            }
            val scale = if (playing) {
                1.075f + 0.525f * sin((progress + phase) * 2f * PI.toFloat())
            } else {
                1f
            }
            Box(
                modifier = Modifier
                    .width(2.5.dp)
                    .height(height.dp)
                    .graphicsLayer {
                        scaleY = scale
                    }
                    .background(color.copy(alpha = 0.9f), RoundedCornerShape(2.dp)),
            )
        }
    }
}

// ─── 状态 / 输入中 / 空态 ─────────────────────────────────────────────

@Composable
private fun MessageStatusRow(status: MessageStatus, pending: Boolean) {
    val colors = ShiYuTheme.colors
    val label = if (pending && status == MessageStatus.SENT) "发送中…" else status.label
    val color = if (status == MessageStatus.FAILED) colors.danger else colors.chatMeta
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        if (status != MessageStatus.FAILED && !pending) {
            Icon(
                imageVector = ShiYuIcons.Check,
                contentDescription = null,
                tint = colors.chatMeta,
                modifier = Modifier.size(12.dp),
            )
        }
        Text(
            text = label,
            color = color,
            fontSize = 10.5.sp,
            fontWeight = FontWeight.Normal,
            lineHeight = 14.sp,
        )
    }
}

@Composable
private fun TypingRow(conversation: Conversation) {
    val colors = ShiYuTheme.colors
    val shape = ShiYuTheme.bubbleShape(isMine = false, tail = true)
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.Start,
        verticalAlignment = Alignment.Bottom,
    ) {
        Avatar(
            name = conversation.name,
            size = 34.dp,
            model = StockMedia.drawableFor(conversation.avatar),
            modifier = Modifier.padding(bottom = 2.dp),
        )
        Spacer(Modifier.width(8.dp))
        Box(
            modifier = Modifier
                .clip(shape)
                .background(colors.bubbleIn)
                .border(1.dp, colors.line, shape)
                .padding(14.dp),
        ) {
            TypingDots()
        }
    }
}

@Composable
private fun TypingDots() {
    val colors = ShiYuTheme.colors
    val transition = rememberInfiniteTransition(label = "typing-dots")
    val progress by transition.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 1100, easing = LinearEasing),
            repeatMode = RepeatMode.Restart,
            initialStartOffset = StartOffset(0),
        ),
        label = "typing-progress",
    )
    Row(
        modifier = Modifier.height(6.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        repeat(3) { index ->
            val phase = (progress + index * 0.15f) % 1f
            val lift = if (phase < 0.5f) phase * 2f else (1f - phase) * 2f
            Box(
                modifier = Modifier
                    .size(6.dp)
                    .graphicsLayer {
                        translationY = (-3f * lift) * density
                    }
                    .background(colors.ink3.copy(alpha = 0.45f + 0.55f * lift), CircleShape),
            )
        }
    }
}

@Composable
internal fun EmptyChatState(name: String) {
    val colors = ShiYuTheme.colors
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(horizontal = 20.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Icon(
            imageVector = ShiYuIcons.Message,
            contentDescription = null,
            tint = colors.chatMeta,
            modifier = Modifier.size(34.dp),
        )
        Spacer(Modifier.height(8.dp))
        Text(
            text = "还没有消息",
            color = colors.ink2,
            fontSize = 15.5.sp,
            fontWeight = FontWeight.SemiBold,
            lineHeight = 21.sp,
        )
        Spacer(Modifier.height(6.dp))
        Text(
            text = "发一条消息，开始和${name.ifBlank { "对方" }}聊天",
            color = colors.ink3,
            style = ShiYuType.caption,
            textAlign = TextAlign.Center,
        )
    }
}
