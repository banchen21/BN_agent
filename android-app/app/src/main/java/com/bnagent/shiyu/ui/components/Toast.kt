package com.bnagent.shiyu.ui.components

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay

/**
 * 轻提示（设计规格 3.10）：距底 104dp 居中、最大宽 82%、pill、
 * 底 rgba(18,22,25,.94)、白字 12.5sp/600，显示 2100ms；新 toast 重置计时。
 *
 * 接线方式（AppRoot 已提供）：
 * ```
 * LocalToast.current?.show("聊天记录已清空")
 * ```
 */
@Stable
class ToastHostState {
    internal var message by mutableStateOf<ToastMessage?>(null)
        private set

    internal var displayText by mutableStateOf("")
        private set

    private var nextId = 0

    fun show(text: String) {
        displayText = text
        message = ToastMessage(++nextId, text)
    }

    internal fun dismiss(id: Int) {
        if (message?.id == id) message = null
    }
}

internal data class ToastMessage(val id: Int, val text: String)

/** 根部注入；预览等场景可能为 null，调用方用 `?.show(...)`。 */
val LocalToast = staticCompositionLocalOf<ToastHostState?> { null }

@Composable
fun rememberToastHostState(): ToastHostState = remember { ToastHostState() }

/** Toast 宿主：铺满父容器、不拦截点击，只在底部 104dp 处绘制提示。 */
@Composable
fun ToastHost(
    state: ToastHostState,
    modifier: Modifier = Modifier,
) {
    val current = state.message
    LaunchedEffect(current?.id) {
        val msg = current ?: return@LaunchedEffect
        delay(2100)
        state.dismiss(msg.id)
    }
    Box(
        modifier = modifier
            .fillMaxSize()
            .imePadding(),
        contentAlignment = Alignment.BottomCenter,
    ) {
        AnimatedVisibility(
            visible = current != null,
            enter = fadeIn(tween(durationMillis = 180)) +
                slideInVertically(tween(durationMillis = 180)) { height -> height / 2 },
            exit = fadeOut(tween(durationMillis = 120)),
        ) {
            BoxWithConstraints(modifier = Modifier.padding(bottom = 104.dp)) {
                Box(
                    modifier = Modifier
                        .widthIn(max = maxWidth * 0.82f)
                        .clip(RoundedCornerShape(50))
                        .background(ToastBackground)
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        text = state.displayText,
                        color = Color.White,
                        fontSize = 12.5.sp,
                        fontWeight = FontWeight.SemiBold,
                        lineHeight = 16.875.sp,
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }
    }
}

private val ToastBackground = Color(0xF0121619)
