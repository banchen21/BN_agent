package com.bnagent.shiyu.ui.components

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import com.bnagent.shiyu.ui.theme.ShiYuTheme

/**
 * 拾语开关（设计规格 3.5）：52×32 pill，滑块 26，关闭底 #C9D1D5（深 #384247），
 * 打开底 accent，180ms 位移 + 变色。
 *
 * 用法与 Material Switch 一致：`ShiYuSwitch(checked, onCheckedChange)`。
 */
@Composable
fun ShiYuSwitch(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val colors = ShiYuTheme.colors
    val trackColor by animateColorAsState(
        targetValue = if (checked) colors.accent else colors.switchOff,
        animationSpec = tween(durationMillis = 180),
        label = "switch-track",
    )
    val thumbOffset by animateDpAsState(
        targetValue = if (checked) 20.dp else 0.dp,
        animationSpec = tween(durationMillis = 180),
        label = "switch-thumb",
    )
    Box(
        modifier = modifier
            .size(width = 52.dp, height = 32.dp)
            .clip(CircleShape)
            .background(trackColor)
            .toggleable(
                value = checked,
                enabled = enabled,
                role = Role.Switch,
                onValueChange = onCheckedChange,
            )
            .padding(3.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        Box(
            modifier = Modifier
                .offset(x = thumbOffset)
                .size(26.dp)
                .shadow(elevation = 2.dp, shape = CircleShape)
                .clip(CircleShape)
                .background(Color.White),
        )
    }
}
