package com.bnagent.shiyu.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.compose.AsyncImage
import com.bnagent.shiyu.ui.theme.AvatarFallbackGradient
import com.bnagent.shiyu.ui.theme.AvatarFallbackInk
import com.bnagent.shiyu.ui.theme.ShiYuTheme

/**
 * 头像（设计规格 3.3）。
 *
 * - 有图：Coil `AsyncImage`，centerCrop，圆角 = 尺寸 × 0.32（[circle] = true 时正圆）。
 * - 无图：单字文字头像，渐变底 `#3A474E → #171C1F`，字色 `#F2F6F7`，字号 = 尺寸 × 0.36。
 * - 在线点：11dp 绿点 + 2dp surface 外圈，位于右下并外溢 2dp。
 *
 * @param model 交给 Coil 的图片模型（String url / Uri / drawable res id / File 等）；
 *              为 null 时显示首字头像。
 * @param online 是否显示在线绿点（消息头像场景传 false）。
 */
@Composable
fun Avatar(
    name: String,
    size: Dp,
    modifier: Modifier = Modifier,
    model: Any? = null,
    online: Boolean = false,
    circle: Boolean = false,
) {
    val colors = ShiYuTheme.colors
    val shape: Shape = if (circle) CircleShape else RoundedCornerShape(size * 0.32f)
    Box(modifier = modifier.size(size), contentAlignment = Alignment.Center) {
        if (model != null) {
            AsyncImage(
                model = model,
                contentDescription = name,
                contentScale = ContentScale.Crop,
                modifier = Modifier.matchParentSize().clip(shape),
            )
        } else {
            Box(
                modifier = Modifier
                    .matchParentSize()
                    .clip(shape)
                    .background(Brush.linearGradient(AvatarFallbackGradient)),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = name.trim().take(1).ifEmpty { "?" },
                    color = AvatarFallbackInk,
                    fontSize = (size.value * 0.36f).sp,
                    fontWeight = FontWeight.Bold,
                )
            }
        }
        if (online) {
            // 外圈 15dp（surface）+ 内点 11dp（presence）；offset 外溢 2dp，不影响布局
            Box(
                modifier = Modifier
                    .align(Alignment.BottomEnd)
                    .offset(x = 2.dp, y = 2.dp)
                    .size(15.dp)
                    .clip(CircleShape)
                    .background(colors.surface),
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    modifier = Modifier
                        .size(11.dp)
                        .clip(CircleShape)
                        .background(colors.presence),
                )
            }
        }
    }
}
