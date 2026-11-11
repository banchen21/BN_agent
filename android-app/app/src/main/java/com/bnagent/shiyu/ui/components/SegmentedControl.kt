package com.bnagent.shiyu.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.bnagent.shiyu.ui.theme.ShiYuTheme

/** 分段控件选项。 */
data class SegmentOption<T>(val value: T, val label: String)

/**
 * 三段（可扩展 N 段）分段控件（设计规格 3.8）：
 * 外框 padding 4、圆角 14、1px line、底 surface-2；段高 44、圆角 10；
 * 选中底 surface + 阴影、字 ink，未选中字 ink-2。
 *
 * 用于「气泡圆角 / 聊天字号」等 3 选 1 设置。
 */
@Composable
fun <T> SegmentedControl(
    options: List<SegmentOption<T>>,
    selected: T,
    onSelect: (T) -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = ShiYuTheme.colors
    val shape = RoundedCornerShape(14.dp)
    Row(
        modifier = modifier
            .clip(shape)
            .background(colors.surface2)
            .border(1.dp, colors.line, shape)
            .padding(4.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        options.forEach { option ->
            val isSelected = option.value == selected
            val segmentShape = RoundedCornerShape(10.dp)
            Box(
                modifier = Modifier
                    .weight(1f)
                    .height(44.dp)
                    .clip(segmentShape)
                    .then(
                        if (isSelected) {
                            Modifier
                                .shadow(elevation = 2.dp, shape = segmentShape)
                                .background(colors.surface)
                        } else {
                            Modifier
                        },
                    )
                    .clickable { onSelect(option.value) },
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = option.label,
                    color = if (isSelected) colors.ink else colors.ink2,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
    }
}
