package com.bnagent.shiyu.ui.chat

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.StartOffset
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType
import java.util.Locale
import kotlin.math.PI
import kotlin.math.sin

/**
 * 聊天输入区（设计规格 4.2 composer / 录音态）。
 *
 * - 主行：`+` 圆钮、圆角 22 输入框、麦克风（无文字）/ 发送（有文字）；
 * - 键盘 Enter 直接发送；发送后清空并保持焦点；
 * - 录音条：取消 / 红点计时 + 波形 / 发送；
 * - 输入区自带 `navigationBarsPadding().imePadding()`（键盘弹出时随之上移）。
 */
@Composable
fun Composer(
    text: String,
    onTextChange: (String) -> Unit,
    onSend: () -> Unit,
    recording: Boolean,
    recordSeconds: Int,
    onOpenAttach: () -> Unit,
    onStartRecording: () -> Unit,
    onCancelRecording: () -> Unit,
    onSendRecording: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = ShiYuTheme.colors
    Column(
        modifier = modifier
            .fillMaxWidth()
            .background(colors.surface)
            .navigationBarsPadding()
            .imePadding(),
    ) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(colors.line))
        if (recording) {
            RecordingBar(
                seconds = recordSeconds,
                onCancel = onCancelRecording,
                onSend = onSendRecording,
            )
        } else {
            ComposerMainRow(
                text = text,
                onTextChange = onTextChange,
                onSend = onSend,
                onOpenAttach = onOpenAttach,
                onStartRecording = onStartRecording,
            )
        }
    }
}

@Composable
private fun ComposerMainRow(
    text: String,
    onTextChange: (String) -> Unit,
    onSend: () -> Unit,
    onOpenAttach: () -> Unit,
    onStartRecording: () -> Unit,
) {
    val focusRequester = remember { FocusRequester() }
    val hasText = text.isNotBlank()
    val submit: () -> Unit = {
        if (hasText) {
            onSend()
            // 发送后保持焦点（键盘不收起），继续输入。
            focusRequester.requestFocus()
        }
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 12.dp, end = 12.dp, top = 9.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        RoundButton(
            icon = ShiYuIcons.Plus,
            contentDescription = "发送图片、文件或语音",
            onClick = onOpenAttach,
        )
        MessageInput(
            text = text,
            onTextChange = onTextChange,
            focusRequester = focusRequester,
            onSubmit = submit,
            modifier = Modifier.weight(1f),
        )
        if (hasText) {
            SendButton(onClick = submit)
        } else {
            RoundButton(
                icon = ShiYuIcons.Mic,
                contentDescription = "录音",
                onClick = onStartRecording,
            )
        }
    }
}

@Composable
private fun MessageInput(
    text: String,
    onTextChange: (String) -> Unit,
    focusRequester: FocusRequester,
    onSubmit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = ShiYuTheme.colors
    var focused by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(22.dp)
    Box(
        modifier = modifier
            .height(44.dp)
            .clip(shape)
            .background(colors.surface2)
            .then(
                if (focused) {
                    Modifier
                        .border(1.dp, colors.accent, shape)
                        .drawFocusGlow()
                } else {
                    Modifier.border(1.dp, colors.line, shape)
                },
            )
            .padding(horizontal = 14.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        BasicTextField(
            value = text,
            onValueChange = onTextChange,
            modifier = Modifier
                .fillMaxWidth()
                .focusRequester(focusRequester)
                .onFocusChanged { focused = it.isFocused },
            textStyle = ShiYuType.input.copy(color = colors.ink),
            cursorBrush = SolidColor(colors.accent),
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
            keyboardActions = KeyboardActions(onSend = { onSubmit() }),
            decorationBox = { innerTextField ->
                Box(contentAlignment = Alignment.CenterStart) {
                    if (text.isEmpty()) {
                        Text(
                            text = "发消息…",
                            color = colors.ink3,
                            style = ShiYuType.input,
                        )
                    }
                    innerTextField()
                }
            },
        )
    }
}

/** 输入框聚焦时 3dp `rgba(201,59,36,.14)` 光晕（设计规格 3.6，内描边近似）。 */
@Composable
private fun Modifier.drawFocusGlow(): Modifier {
    val glow = ShiYuTheme.colors.accent.copy(alpha = 0.14f)
    return this.drawBehind {
        drawRoundRect(
            color = glow,
            cornerRadius = CornerRadius(22.dp.toPx()),
            style = Stroke(width = 3.dp.toPx()),
        )
    }
}

@Composable
private fun RoundButton(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    val interaction = remember { MutableInteractionSource() }
    Box(
        modifier = Modifier
            .size(44.dp)
            .clip(CircleShape)
            .background(colors.surface2)
            .clickable(
                interactionSource = interaction,
                indication = null,
                onClick = onClick,
            ),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = contentDescription,
            tint = colors.ink,
            modifier = Modifier.size(23.dp),
        )
    }
}

@Composable
private fun SendButton(onClick: () -> Unit) {
    val colors = ShiYuTheme.colors
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(
        targetValue = if (pressed) 0.94f else 1f,
        label = "send-scale",
    )
    Box(
        modifier = Modifier
            .size(44.dp)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .clip(CircleShape)
            .background(colors.bubbleOut)
            .clickable(
                interactionSource = interaction,
                indication = null,
                onClick = onClick,
            ),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = ShiYuIcons.Send,
            contentDescription = "发送",
            tint = colors.bubbleOutInk,
            modifier = Modifier.size(22.dp),
        )
    }
}

// ─── 录音条 ───────────────────────────────────────────────────────────

@Composable
private fun RecordingBar(
    seconds: Int,
    onCancel: () -> Unit,
    onSend: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 12.dp, end = 12.dp, top = 9.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        RecordActionButton(
            text = "取消",
            container = colors.surface2,
            content = colors.ink2,
            horizontalPadding = 14.dp,
            fontWeight = FontWeight.SemiBold,
            onClick = onCancel,
        )
        Row(
            modifier = Modifier
                .weight(1f)
                .height(44.dp)
                .clip(RoundedCornerShape(22.dp))
                .background(colors.accentSoft)
                .padding(horizontal = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(9.dp),
        ) {
            RecordingDot()
            Text(
                text = formatClock(seconds),
                color = colors.accentText,
                style = TextStyle(
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Bold,
                    lineHeight = 20.sp,
                    fontFeatureSettings = "tnum",
                ),
                maxLines = 1,
            )
            RecordingWaveform(
                color = colors.accentText,
                modifier = Modifier.weight(1f),
            )
        }
        RecordActionButton(
            text = "发送",
            container = colors.bubbleOut,
            content = colors.bubbleOutInk,
            horizontalPadding = 18.dp,
            fontWeight = FontWeight.Bold,
            onClick = onSend,
        )
    }
}

@Composable
private fun RecordActionButton(
    text: String,
    container: Color,
    content: Color,
    horizontalPadding: Dp,
    fontWeight: FontWeight,
    onClick: () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    Box(
        modifier = Modifier
            .height(44.dp)
            .clip(RoundedCornerShape(22.dp))
            .background(container)
            .clickable(
                interactionSource = interaction,
                indication = null,
                onClick = onClick,
            )
            .padding(horizontal = horizontalPadding),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            color = content,
            fontSize = 14.sp,
            fontWeight = fontWeight,
            lineHeight = 20.sp,
            maxLines = 1,
        )
    }
}

/** 录音红点：scale 1→.75、opacity 1→.6，1.2s 无限。 */
@Composable
private fun RecordingDot() {
    val colors = ShiYuTheme.colors
    val transition = rememberInfiniteTransition(label = "rec-dot")
    val scale by transition.animateFloat(
        initialValue = 1f,
        targetValue = 0.75f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 600, easing = LinearEasing),
            repeatMode = RepeatMode.Reverse,
            initialStartOffset = StartOffset(0),
        ),
        label = "rec-dot-scale",
    )
    val alpha by transition.animateFloat(
        initialValue = 1f,
        targetValue = 0.6f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 600, easing = LinearEasing),
            repeatMode = RepeatMode.Reverse,
            initialStartOffset = StartOffset(0),
        ),
        label = "rec-dot-alpha",
    )
    Box(
        modifier = Modifier
            .size(9.dp)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
                this.alpha = alpha
            }
            .background(colors.accent, CircleShape),
    )
}

/** 录音波形：14 根柱，scaleY .55→1.6，780ms 交替。 */
@Composable
private fun RecordingWaveform(color: Color, modifier: Modifier = Modifier) {
    val heights = remember { listOf(7, 13, 9, 16, 11, 6, 14, 18, 8, 12, 16, 7, 10, 15) }
    val transition = rememberInfiniteTransition(label = "rec-wave")
    val progress by transition.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 780, easing = LinearEasing),
            repeatMode = RepeatMode.Restart,
            initialStartOffset = StartOffset(0),
        ),
        label = "rec-wave-progress",
    )
    Row(
        modifier = modifier.height(20.dp),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        heights.forEachIndexed { index, height ->
            val phase = (index % 4) * 0.1f
            val scale = 1.075f + 0.525f * sin((progress + phase) * 2f * PI.toFloat())
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

private fun formatClock(seconds: Int): String =
    String.format(Locale.ROOT, "%02d:%02d", seconds / 60, seconds % 60)
