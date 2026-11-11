package com.bnagent.shiyu.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType

/**
 * 通用 Appbar（设计规格 3.1）。
 *
 * 三种形态：
 * - [LargeTitleBar]：大标题栏（chats / contacts / me），右侧动作区。
 * - [SmallTitleBar]：小标题栏（bubbles / profile / 二级设置页），居中标题 + 返回。
 * - [ChatHeader]：聊天头部（chat）。
 *
 * 三者默认自带 `statusBarsPadding()`（edge-to-edge），页面不要再重复加。
 */

/** 大标题栏：标题 28sp/ExtraBold，右侧 44dp 图标按钮区。 */
@Composable
fun LargeTitleBar(
    title: String,
    modifier: Modifier = Modifier,
    actions: @Composable RowScope.() -> Unit = {},
) {
    val colors = ShiYuTheme.colors
    BoxWithConstraints(
        modifier = modifier
            .fillMaxWidth()
            .background(colors.bg)
            .statusBarsPadding(),
    ) {
        // design-spec 2.9：≤380dp 宽时左右内边距 12，否则 14
        val horizontalPadding = if (maxWidth <= 380.dp) 12.dp else 14.dp
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(
                    start = horizontalPadding,
                    end = horizontalPadding,
                    top = 6.dp,
                    bottom = 8.dp,
                ),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .defaultMinSize(minHeight = 48.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    text = title,
                    modifier = Modifier.weight(1f),
                    color = colors.ink,
                    style = ShiYuType.pageTitle,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(2.dp),
                    content = actions,
                )
            }
        }
    }
}

/**
 * 小标题栏：返回按钮 + 居中标题 + 右侧动作（默认 44dp 占位保证标题视觉居中）。
 */
@Composable
fun SmallTitleBar(
    title: String,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    trailing: @Composable RowScope.() -> Unit = { Spacer(Modifier.size(44.dp)) },
) {
    val colors = ShiYuTheme.colors
    BoxWithConstraints(
        modifier = modifier
            .fillMaxWidth()
            .background(colors.bg)
            .statusBarsPadding(),
    ) {
        val horizontalPadding = if (maxWidth <= 380.dp) 12.dp else 14.dp
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(
                    start = horizontalPadding,
                    end = horizontalPadding,
                    top = 6.dp,
                    bottom = 8.dp,
                )
                .heightIn(min = 48.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton44(
                icon = ShiYuIcons.Back,
                contentDescription = "返回",
                onClick = onBack,
            )
            Text(
                text = title,
                modifier = Modifier.weight(1f),
                color = colors.ink,
                style = ShiYuType.smallTitle,
                textAlign = TextAlign.Center,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(2.dp),
                content = trailing,
            )
        }
    }
}

/**
 * 聊天头部（设计规格 3.1-C）：返回 + 对方信息区（可点进个人资料）+ 电话/视频/更多。
 *
 * @param subtitle 「在线 / 离线 / 6 人 · 群聊 / 正在输入…」
 * @param typing 对方输入中：副标题变 accentText
 * @param isGroup 群聊：隐藏电话、视频按钮（设计规格：群聊不可发起通话）
 */
@Composable
fun ChatHeader(
    name: String,
    subtitle: String,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    avatarModel: Any? = null,
    online: Boolean = false,
    typing: Boolean = false,
    isGroup: Boolean = false,
    onOpenProfile: () -> Unit = {},
    onVoiceCall: () -> Unit = {},
    onVideoCall: () -> Unit = {},
    onMore: () -> Unit = {},
) {
    val colors = ShiYuTheme.colors
    Column(
        modifier = modifier
            .fillMaxWidth()
            .background(colors.surface)
            .statusBarsPadding(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(start = 8.dp, end = 8.dp, top = 4.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton44(
                icon = ShiYuIcons.Back,
                contentDescription = "返回",
                onClick = onBack,
            )
            Row(
                modifier = Modifier
                    .weight(1f)
                    .heightIn(min = 44.dp)
                    .clip(RoundedCornerShape(14.dp))
                    .clickable(onClick = onOpenProfile)
                    .padding(horizontal = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Avatar(
                    name = name,
                    size = 36.dp,
                    model = avatarModel,
                    online = online,
                )
                Column(verticalArrangement = Arrangement.spacedBy(1.dp)) {
                    Text(
                        text = name,
                        color = colors.ink,
                        style = ShiYuType.chatHeaderName,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        text = subtitle,
                        color = if (typing) colors.accentText else colors.ink3,
                        style = ShiYuType.meta,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                if (!isGroup) {
                    IconButton44(
                        icon = ShiYuIcons.Phone,
                        contentDescription = "语音通话",
                        onClick = onVoiceCall,
                    )
                    IconButton44(
                        icon = ShiYuIcons.Video,
                        contentDescription = "视频通话",
                        onClick = onVideoCall,
                    )
                }
                IconButton44(
                    icon = ShiYuIcons.More,
                    contentDescription = "更多",
                    onClick = onMore,
                )
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(colors.line))
    }
}

/**
 * 44×44 图标按钮（设计规格 3.5）：透明底、圆角 14、图标 22、按压底 surface-2。
 */
@Composable
fun IconButton44(
    icon: ImageVector,
    contentDescription: String?,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    tint: Color = ShiYuTheme.colors.ink,
    enabled: Boolean = true,
    iconSize: Dp = 22.dp,
) {
    val colors = ShiYuTheme.colors
    val interactionSource = remember { MutableInteractionSource() }
    val pressed by interactionSource.collectIsPressedAsState()
    Box(
        modifier = modifier
            .size(44.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(if (pressed && enabled) colors.surface2 else Color.Transparent)
            .clickable(
                interactionSource = interactionSource,
                indication = null,
                enabled = enabled,
                onClick = onClick,
            ),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = contentDescription,
            tint = if (enabled) tint else colors.ink3,
            modifier = Modifier.size(iconSize),
        )
    }
}

/** 文字按钮（设计规格 3.5「完成」）：高 44、左右 12、圆角 12、accentText 15sp/700。 */
@Composable
fun TextActionButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val colors = ShiYuTheme.colors
    val interactionSource = remember { MutableInteractionSource() }
    val pressed by interactionSource.collectIsPressedAsState()
    Box(
        modifier = modifier
            .height(44.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(if (pressed && enabled) colors.accentSoft else Color.Transparent)
            .clickable(
                interactionSource = interactionSource,
                indication = null,
                enabled = enabled,
                onClick = onClick,
            )
            .padding(horizontal = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            color = if (enabled) colors.accentText else colors.ink3,
            style = ShiYuType.button,
        )
    }
}
