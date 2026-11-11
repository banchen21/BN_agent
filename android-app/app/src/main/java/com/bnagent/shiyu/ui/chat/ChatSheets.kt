package com.bnagent.shiyu.ui.chat

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import coil.compose.AsyncImage
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.ui.components.SheetCancelButton
import com.bnagent.shiyu.ui.components.SheetHandle
import com.bnagent.shiyu.ui.components.SheetTitle
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.components.ShiYuSheet
import com.bnagent.shiyu.ui.theme.ShiYuTheme

/**
 * 聊天页浮层（设计规格 3.10 / 5.9 / 5.10）：
 * 发送 sheet、选择文件 sheet、聊天设置 sheet、图片查看器。
 */

/** 发送 sheet：图片 / 文件 / 语音三列。 */
@Composable
fun AttachSheet(
    onDismiss: () -> Unit,
    onPickImage: () -> Unit,
    onPickFile: () -> Unit,
    onRecordVoice: () -> Unit,
) {
    ShiYuSheet(onDismissRequest = onDismiss) {
        SheetHandle()
        SheetTitle("发送")
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            AttachCell(
                icon = ShiYuIcons.Image,
                label = "图片",
                modifier = Modifier.weight(1f),
                onClick = onPickImage,
            )
            AttachCell(
                icon = ShiYuIcons.File,
                label = "文件",
                modifier = Modifier.weight(1f),
                onClick = onPickFile,
            )
            AttachCell(
                icon = ShiYuIcons.Mic,
                label = "语音",
                modifier = Modifier.weight(1f),
                onClick = onRecordVoice,
            )
        }
        SheetCancelButton(onClick = onDismiss)
    }
}

@Composable
private fun AttachCell(
    icon: ImageVector,
    label: String,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    val interaction = remember { MutableInteractionSource() }
    Column(
        modifier = modifier
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surface2)
            .clickable(
                interactionSource = interaction,
                indication = null,
                onClick = onClick,
            )
            .padding(vertical = 14.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(7.dp),
    ) {
        Box(
            modifier = Modifier
                .size(46.dp)
                .clip(RoundedCornerShape(15.dp))
                .background(colors.surface),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = icon,
                contentDescription = null,
                tint = colors.accentText,
                modifier = Modifier.size(23.dp),
            )
        }
        Text(
            text = label,
            color = colors.ink,
            fontSize = 12.5.sp,
            fontWeight = FontWeight.SemiBold,
            lineHeight = 17.sp,
        )
    }
}

/** 文件选择 sheet：3 个示例文件。 */
@Composable
fun FilePickerSheet(
    files: List<StockMedia.FileTemplate>,
    onDismiss: () -> Unit,
    onPick: (StockMedia.FileTemplate) -> Unit,
) {
    val colors = ShiYuTheme.colors
    ShiYuSheet(onDismissRequest = onDismiss) {
        SheetHandle()
        SheetTitle("选择文件")
        files.forEachIndexed { index, template ->
            val interaction = remember { MutableInteractionSource() }
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable(
                        interactionSource = interaction,
                        indication = null,
                        onClick = { onPick(template) },
                    )
                    .padding(horizontal = 2.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Box(
                    modifier = Modifier
                        .size(42.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(colors.accentSoft),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        imageVector = ShiYuIcons.File,
                        contentDescription = null,
                        tint = colors.accentText,
                        modifier = Modifier.size(21.dp),
                    )
                }
                Column(
                    modifier = Modifier.weight(1f),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    Text(
                        text = template.name,
                        color = colors.ink,
                        fontSize = 14.5.sp,
                        fontWeight = FontWeight.SemiBold,
                        lineHeight = 20.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        text = template.sizeLabel,
                        color = colors.ink3,
                        fontSize = 11.5.sp,
                        fontWeight = FontWeight.Normal,
                        lineHeight = 16.sp,
                    )
                }
                Icon(
                    imageVector = ShiYuIcons.Chevron,
                    contentDescription = null,
                    tint = colors.ink3,
                    modifier = Modifier.size(17.dp),
                )
            }
            if (index != files.lastIndex) {
                Box(Modifier.fillMaxWidth().height(1.dp).background(colors.line))
            }
        }
        SheetCancelButton(onClick = onDismiss)
    }
}

/** 聊天设置 sheet：气泡与主题 / 清空聊天记录。 */
@Composable
fun ChatSettingsSheet(
    onDismiss: () -> Unit,
    onOpenTheme: () -> Unit,
    onClearClick: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    ShiYuSheet(onDismissRequest = onDismiss) {
        SheetHandle()
        SheetTitle("聊天设置")
        MenuRow(
            icon = ShiYuIcons.Palette,
            title = "气泡与主题",
            subtitle = "配色、圆角、字号与背景",
            onClick = onOpenTheme,
        )
        Box(Modifier.fillMaxWidth().height(1.dp).background(colors.line))
        MenuRow(
            icon = ShiYuIcons.Close,
            title = "清空聊天记录",
            subtitle = "仅清空当前会话",
            onClick = onClearClick,
        )
        SheetCancelButton(onClick = onDismiss)
    }
}

@Composable
private fun MenuRow(
    icon: ImageVector,
    title: String,
    subtitle: String,
    onClick: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    val interaction = remember { MutableInteractionSource() }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(
                interactionSource = interaction,
                indication = null,
                onClick = onClick,
            )
            .padding(horizontal = 4.dp, vertical = 15.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = colors.ink2,
            modifier = Modifier.size(20.dp),
        )
        Column(
            modifier = Modifier.weight(1f),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text(
                text = title,
                color = colors.ink,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                lineHeight = 21.sp,
            )
            Text(
                text = subtitle,
                color = colors.ink3,
                fontSize = 11.5.sp,
                fontWeight = FontWeight.Normal,
                lineHeight = 16.sp,
            )
        }
        Icon(
            imageVector = ShiYuIcons.Chevron,
            contentDescription = null,
            tint = colors.ink3,
            modifier = Modifier.size(17.dp),
        )
    }
}

/** 全屏图片查看器（设计规格 3.10）：点背景 / 右上 × 关闭。 */
@Composable
fun ImageViewer(
    message: Message.Image,
    onDismiss: () -> Unit,
) {
    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(
            usePlatformDefaultWidth = false,
            decorFitsSystemWindows = false,
        ),
    ) {
        val caption = buildString {
            append(message.alt?.takeIf { it.isNotBlank() } ?: "图片消息")
            message.time?.takeIf { it.isNotBlank() }?.let { append(" · ").append(it) }
        }
        val backgroundInteraction = remember { MutableInteractionSource() }
        val imageInteraction = remember { MutableInteractionSource() }
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(Color(0xFF07090B))
                .clickable(
                    interactionSource = backgroundInteraction,
                    indication = null,
                    onClick = onDismiss,
                )
                .statusBarsPadding()
                .navigationBarsPadding()
                .padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 18.dp),
        ) {
            Column(
                modifier = Modifier.fillMaxSize(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.End,
                ) {
                    Box(
                        modifier = Modifier
                            .size(44.dp)
                            .clip(CircleShape)
                            .background(Color(0x1FFFFFFF))
                            .clickable(
                                interactionSource = remember { MutableInteractionSource() },
                                indication = null,
                                onClick = onDismiss,
                            ),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(
                            imageVector = ShiYuIcons.Close,
                            contentDescription = "关闭",
                            tint = Color.White,
                            modifier = Modifier.size(21.dp),
                        )
                    }
                }
                Box(
                    modifier = Modifier
                        .weight(1f)
                        .fillMaxWidth(),
                    contentAlignment = Alignment.Center,
                ) {
                    AsyncImage(
                        model = imageModelFor(message.asset),
                        contentDescription = message.alt ?: "图片消息",
                        contentScale = ContentScale.Fit,
                        modifier = Modifier
                            .fillMaxSize()
                            .clip(RoundedCornerShape(12.dp))
                            // 图片区域消费点击，避免点图误关查看器
                            .clickable(
                                interactionSource = imageInteraction,
                                indication = null,
                                onClick = {},
                            ),
                    )
                }
                Spacer(Modifier.height(0.dp))
                Text(
                    text = caption,
                    color = Color(0xFFB9C4C8),
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Normal,
                    lineHeight = 17.sp,
                )
            }
        }
    }
}
