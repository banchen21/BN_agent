package com.bnagent.shiyu.ui.components

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.slideInVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.DialogWindowProvider
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType

/**
 * 底部 Sheet 容器（设计规格 3.10 / 2.12）。
 *
 * - 覆盖全屏（含状态栏），遮罩 `rgba(8,11,13,.5)` 淡入 200ms，面板 240ms 曲线上滑；
 * - 面板：surface 底、顶部圆角 24、padding 10/18/(16 + 底部安全区)、最大高 78%、内容可滚；
 * - 自带把手与底部安全区；调用方传入 [SheetTitle] / 内容 / [SheetCancelButton]。
 *
 * 用法：
 * ```
 * if (show) {
 *     ShiYuSheet(onDismissRequest = { show = false }) {
 *         SheetTitle("聊天设置")
 *         // ... 行
 *         SheetCancelButton(onClick = { show = false })
 *     }
 * }
 * ```
 */
@Composable
fun ShiYuSheet(
    onDismissRequest: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable ColumnScope.() -> Unit,
) {
    Dialog(
        onDismissRequest = onDismissRequest,
        properties = DialogProperties(
            usePlatformDefaultWidth = false,
            decorFitsSystemWindows = false,
        ),
    ) {
        // 关闭系统默认 dim，使用设计稿遮罩色
        val dialogWindowProvider = LocalView.current.parent as? DialogWindowProvider
        SideEffect { dialogWindowProvider?.window?.setDimAmount(0f) }

        val colors = ShiYuTheme.colors
        val screenHeight = LocalConfiguration.current.screenHeightDp.dp
        var visible by remember { mutableStateOf(false) }
        LaunchedEffect(Unit) { visible = true }

        Box(modifier = Modifier.fillMaxSize()) {
            Box(
                modifier = Modifier
                    .matchParentSize()
                    .background(SheetScrimColor)
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClick = onDismissRequest,
                    ),
            )
            AnimatedVisibility(
                visible = visible,
                modifier = Modifier.align(Alignment.BottomCenter),
                enter = slideInVertically(
                    initialOffsetY = { it },
                    animationSpec = tween(
                        durationMillis = 240,
                        easing = CubicBezierEasing(0.2f, 0.8f, 0.2f, 1f),
                    ),
                ) + fadeIn(tween(durationMillis = 200)),
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(max = screenHeight * 0.78f)
                        .clip(RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp))
                        .background(colors.surface)
                        .navigationBarsPadding()
                        .verticalScroll(rememberScrollState())
                        .padding(start = 18.dp, end = 18.dp, top = 10.dp, bottom = 16.dp),
                    content = content,
                )
            }
        }
    }
}

/** sheet 顶部把手：40×4，底 line 色，下边距 12。 */
@Composable
fun SheetHandle() {
    val colors = ShiYuTheme.colors
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Box(
            modifier = Modifier
                .size(width = 40.dp, height = 4.dp)
                .clip(CircleShape)
                .background(colors.line),
        )
    }
}

/** sheet 标题：17sp/700，下边距 12。 */
@Composable
fun SheetTitle(text: String) {
    Text(
        text = text,
        modifier = Modifier.padding(bottom = 12.dp),
        color = ShiYuTheme.colors.ink,
        style = ShiYuType.sheetTitle,
    )
}

/** sheet 说明文案：12.5sp ink-3，lh 1.5，下边距 12。 */
@Composable
fun SheetDescription(text: String) {
    Text(
        text = text,
        modifier = Modifier.padding(bottom = 12.dp),
        color = ShiYuTheme.colors.ink3,
        style = ShiYuType.caption,
    )
}

/** sheet 取消按钮：全宽高 48、圆角 14、surface-2、上边距 12。 */
@Composable
fun SheetCancelButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    text: String = "取消",
) {
    val colors = ShiYuTheme.colors
    Box(
        modifier = modifier
            .fillMaxWidth()
            .padding(top = 12.dp)
            .height(48.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(colors.surface2)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            color = colors.ink,
            style = ShiYuType.button,
        )
    }
}

/**
 * 确认弹窗（设计规格 3.10）：宽 min(320, 屏宽−48)、padding 22、圆角 22、surface 底；
 * 标题 17sp/720，正文 13sp ink-2，按钮行高 44、圆角 13。
 *
 * @param destructive true = 确认按钮用 danger 底（如「清空」）；false = accent 底
 */
@Composable
fun ShiYuConfirmDialog(
    title: String,
    message: String,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
    confirmLabel: String = "确定",
    dismissLabel: String = "取消",
    destructive: Boolean = true,
) {
    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(
            usePlatformDefaultWidth = false,
            decorFitsSystemWindows = false,
        ),
    ) {
        val dialogWindowProvider = LocalView.current.parent as? DialogWindowProvider
        SideEffect { dialogWindowProvider?.window?.setDimAmount(0f) }

        val colors = ShiYuTheme.colors
        Box(
            modifier = modifier
                .fillMaxSize()
                .background(DialogScrimColor)
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                    onClick = onDismiss,
                ),
            contentAlignment = Alignment.Center,
        ) {
            Column(
                modifier = Modifier
                    .padding(horizontal = 24.dp)
                    .fillMaxWidth()
                    .widthIn(max = 320.dp)
                    .clip(RoundedCornerShape(22.dp))
                    .background(colors.surface)
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClick = {},
                    )
                    .padding(22.dp),
            ) {
                Text(
                    text = title,
                    color = colors.ink,
                    style = ShiYuType.dialogTitle,
                )
                Text(
                    text = message,
                    modifier = Modifier.padding(top = 8.dp),
                    color = colors.ink2,
                    style = ShiYuType.dialogBody,
                )
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 20.dp),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    DialogButton(
                        text = dismissLabel,
                        onClick = onDismiss,
                        container = colors.surface2,
                        content = colors.ink,
                        modifier = Modifier.weight(1f),
                    )
                    DialogButton(
                        text = confirmLabel,
                        onClick = onConfirm,
                        container = if (destructive) colors.danger else colors.accent,
                        content = Color.White,
                        modifier = Modifier.weight(1f),
                    )
                }
            }
        }
    }
}

@Composable
private fun DialogButton(
    text: String,
    onClick: () -> Unit,
    container: Color,
    content: Color,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier
            .height(44.dp)
            .clip(RoundedCornerShape(13.dp))
            .background(container)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            color = content,
            style = ShiYuType.dialogButton,
            textAlign = TextAlign.Center,
        )
    }
}

/** 遮罩：rgba(8,11,13,.5) / 弹窗 rgba(8,11,13,.52)（设计规格 3.10）。 */
private val SheetScrimColor = Color(0x80080B0D)
private val DialogScrimColor = Color(0x85080B0D)
