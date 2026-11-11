package com.bnagent.shiyu.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.material3.Text
import com.bnagent.shiyu.ui.theme.ShiYuTheme

/** 徽标数字展示规则：>99 显示 99+。 */
private fun badgeText(count: Int): String = if (count > 99) "99+" else count.toString()

/**
 * 会话行未读徽标（设计规格 3.4）：高 20、min 宽 20、pill，11.5sp/700。
 * muted = true 时为静音会话的灰色徽标（仍显示，但不计入 tabbar 总数）。
 */
@Composable
fun UnreadBadge(
    count: Int,
    modifier: Modifier = Modifier,
    muted: Boolean = false,
) {
    if (count <= 0) return
    val colors = ShiYuTheme.colors
    Box(
        modifier = modifier
            .height(20.dp)
            .defaultMinSize(minWidth = 20.dp)
            .clip(RoundedCornerShape(50))
            .background(if (muted) colors.badgeMutedBg else colors.accent)
            .padding(horizontal = 6.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = badgeText(count),
            color = if (muted) colors.badgeMutedInk else colors.onAccent,
            fontSize = 11.5.sp,
            fontWeight = FontWeight.Bold,
            lineHeight = 14.sp,
        )
    }
}

/**
 * tabbar 消息图标上的未读徽标（设计规格 3.2）：高 18、min 宽 18、外圈 2dp surface，
 * 10.5sp/700；总数 0 时不显示（调用方自行判断或直接传入 0）。
 */
@Composable
fun TabBadge(
    count: Int,
    modifier: Modifier = Modifier,
) {
    if (count <= 0) return
    val colors = ShiYuTheme.colors
    Box(
        modifier = modifier
            .height(18.dp)
            .defaultMinSize(minWidth = 18.dp)
            .clip(RoundedCornerShape(50))
            .background(colors.accent)
            // 2dp surface 外圈（设计规格 3.2）：以边框形式画在 18dp 内，避免额外占位
            .border(2.dp, colors.surface, RoundedCornerShape(50))
            .padding(horizontal = 5.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = badgeText(count),
            color = colors.onAccent,
            fontSize = 10.5.sp,
            fontWeight = FontWeight.Bold,
            lineHeight = 12.sp,
        )
    }
}
