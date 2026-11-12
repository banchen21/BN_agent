package com.bnagent.shiyu.ui.settings

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.LocalToast
import com.bnagent.shiyu.ui.components.SegmentOption
import com.bnagent.shiyu.ui.components.SegmentedControl
import com.bnagent.shiyu.ui.components.ShiYuSwitch
import com.bnagent.shiyu.ui.components.SmallTitleBar
import com.bnagent.shiyu.ui.components.TextActionButton
import com.bnagent.shiyu.ui.theme.BubblePalette
import com.bnagent.shiyu.ui.theme.BubbleRadius
import com.bnagent.shiyu.ui.theme.ChatFontSize
import com.bnagent.shiyu.ui.theme.ChatWallpaper
import com.bnagent.shiyu.ui.theme.ShiYuColors
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType
import com.bnagent.shiyu.ui.theme.ThemeSettings
import com.bnagent.shiyu.ui.theme.resolveShiYuColors

/**
 * 气泡与主题（设计规格 4.3）：实时预览 + 5 套气泡配色 + 3 档圆角 + 3 档字号
 * + 3 种聊天背景 + 深色模式开关。
 *
 * 所有修改通过 [ShiYuTheme.controller] 立即生效并持久化（DataStore）。
 */
@Composable
fun BubblesThemeScreen(
    onBack: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    val settings = ShiYuTheme.settings
    val controller = ShiYuTheme.controller

    Column(Modifier.fillMaxSize().background(colors.bg)) {
        SmallTitleBar(
            title = "气泡与主题",
            onBack = onBack,
            trailing = { TextActionButton(text = "完成", onClick = onBack) },
        )
        Column(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 26.dp),
            verticalArrangement = Arrangement.spacedBy(20.dp),
        ) {
            ThemePreviewCard()

            ThemeSection(title = "我的气泡配色") {
                PaletteGrid(
                    selected = settings.palette,
                    onSelect = controller::setPalette,
                )
            }
            ThemeSection(title = "气泡圆角") {
                SegmentedControl(
                    options = BubbleRadius.entries.map { SegmentOption(it, it.label) },
                    selected = settings.bubbleRadius,
                    onSelect = controller::setBubbleRadius,
                )
            }
            ThemeSection(title = "聊天字号") {
                SegmentedControl(
                    options = ChatFontSize.entries.map { SegmentOption(it, it.label) },
                    selected = settings.chatFontSize,
                    onSelect = controller::setChatFontSize,
                )
            }
            ThemeSection(title = "聊天背景") {
                WallpaperGrid(
                    settings = settings,
                    onSelect = controller::setWallpaper,
                )
            }
            ThemeSection(title = "外观") {
                DarkModeCard()
            }
        }
    }
}

@Composable
private fun ThemeSection(
    title: String,
    content: @Composable () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(
            text = title,
            color = ShiYuTheme.colors.ink3,
            style = ShiYuType.groupTitle,
        )
        content()
    }
}

// ─── 实时预览 ───────────────────────────────────────────────────────

@Composable
private fun ThemePreviewCard() {
    val colors = ShiYuTheme.colors
    val cardShape = RoundedCornerShape(22.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .shadow(10.dp, cardShape)
            .clip(cardShape)
            .background(colors.surface)
            .border(1.dp, colors.line, cardShape)
            .padding(12.dp),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(16.dp))
                .background(colors.chatBg),
        ) {
            ChatDots(color = colors.chatDot, spacing = 16.dp, modifier = Modifier.matchParentSize())
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 208.dp)
                    .padding(horizontal = 12.dp, vertical = 14.dp),
                verticalArrangement = Arrangement.Center,
            ) {
                PreviewDateChip()
                Spacer(Modifier.height(10.dp))
                PreviewMessageRow(isMine = false, text = "这个圆角是不是有点方？")
                Spacer(Modifier.height(10.dp))
                PreviewMessageRow(isMine = true, text = "我调了两版，你看看手感")
                Spacer(Modifier.height(10.dp))
                PreviewMessageRow(isMine = false, text = "第二版好看，就它吧")
            }
        }
        Text(
            text = "实时预览 · 设置会同步到所有聊天",
            modifier = Modifier.fillMaxWidth().padding(top = 9.dp),
            color = colors.ink3,
            fontSize = 11.5.sp,
            lineHeight = 16.sp,
            textAlign = TextAlign.Center,
        )
    }
}

@Composable
private fun PreviewDateChip() {
    val colors = ShiYuTheme.colors
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Box(
            modifier = Modifier
                .clip(RoundedCornerShape(50))
                .background(colors.dateChipBg)
                .padding(horizontal = 10.dp, vertical = 4.dp),
        ) {
            Text(
                text = "今天 09:24",
                color = colors.chatMeta,
                fontSize = 11.sp,
                fontWeight = FontWeight.SemiBold,
                lineHeight = 15.sp,
            )
        }
    }
}

@Composable
private fun PreviewMessageRow(
    isMine: Boolean,
    text: String,
) {
    val colors = ShiYuTheme.colors
    val dimens = ShiYuTheme.dimens
    val bubbleShape = ShiYuTheme.bubbleShape(isMine)
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = if (isMine) Arrangement.End else Arrangement.Start,
        verticalAlignment = Alignment.Bottom,
    ) {
        if (!isMine) {
            Avatar(
                name = "林夏",
                size = 26.dp,
                model = StockMedia.drawableFor(StockMedia.AVATAR_LIN),
            )
            Spacer(Modifier.width(8.dp))
        }
        Box(
            modifier = Modifier
                .widthIn(max = 230.dp)
                .clip(bubbleShape)
                .background(if (isMine) colors.bubbleOut else colors.bubbleIn)
                .then(if (isMine) Modifier else Modifier.border(1.dp, colors.line, bubbleShape))
                .padding(horizontal = 11.dp, vertical = 8.dp),
        ) {
            Text(
                text = text,
                color = if (isMine) colors.bubbleOutInk else colors.bubbleInInk,
                fontSize = dimens.chatFontSize,
                lineHeight = dimens.chatLineHeight,
            )
        }
        if (isMine) {
            Spacer(Modifier.width(8.dp))
            Avatar(
                name = "苏雨",
                size = 26.dp,
                model = StockMedia.drawableFor(StockMedia.AVATAR_ME),
            )
        }
    }
}

// ─── 气泡配色 ───────────────────────────────────────────────────────

@Composable
private fun PaletteGrid(
    selected: BubblePalette,
    onSelect: (BubblePalette) -> Unit,
) {
    val colors = ShiYuTheme.colors
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        BubblePalette.entries.forEach { palette ->
            val isSelected = palette == selected
            val cellShape = RoundedCornerShape(15.dp)
            Column(
                modifier = Modifier
                    .weight(1f)
                    .clip(cellShape)
                    .background(if (isSelected) colors.accentSoft else Color.Transparent)
                    .border(1.dp, if (isSelected) colors.accent else Color.Transparent, cellShape)
                    .clickable { onSelect(palette) }
                    .padding(horizontal = 4.dp, vertical = 9.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Box(
                    modifier = Modifier
                        .size(30.dp)
                        .clip(CircleShape)
                        .background(palette.bubbleOut)
                        .border(1.dp, SwatchStroke, CircleShape),
                )
                Spacer(Modifier.height(7.dp))
                Text(
                    text = palette.label,
                    color = if (isSelected) colors.accentText else colors.ink2,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                    lineHeight = 16.sp,
                )
            }
        }
    }
}

// ─── 聊天背景 ───────────────────────────────────────────────────────

@Composable
private fun WallpaperGrid(
    settings: ThemeSettings,
    onSelect: (ChatWallpaper) -> Unit,
) {
    val colors = ShiYuTheme.colors
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        ChatWallpaper.entries.forEach { wallpaper ->
            val isSelected = settings.wallpaper == wallpaper
            val thumbColors = resolveShiYuColors(settings.copy(wallpaper = wallpaper))
            val cellShape = RoundedCornerShape(15.dp)
            Column(
                modifier = Modifier
                    .weight(1f)
                    .clip(cellShape)
                    .background(colors.surface)
                    .border(1.dp, if (isSelected) colors.accent else colors.line, cellShape)
                    .clickable { onSelect(wallpaper) }
                    .padding(start = 7.dp, end = 7.dp, top = 7.dp, bottom = 9.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                WallpaperThumbnail(wallpaper = wallpaper, colors = thumbColors)
                Spacer(Modifier.height(8.dp))
                Text(
                    text = wallpaper.label,
                    color = if (isSelected) colors.accentText else colors.ink2,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                    lineHeight = 16.sp,
                )
            }
        }
    }
}

@Composable
private fun WallpaperThumbnail(
    wallpaper: ChatWallpaper,
    colors: ShiYuColors,
) {
    val base = ShiYuTheme.colors
    BoxWithConstraints(
        modifier = Modifier
            .fillMaxWidth()
            .aspectRatio(1.6f)
            .clip(RoundedCornerShape(9.dp))
            .background(colors.chatBg),
    ) {
        ChatDots(color = colors.chatDot, spacing = 7.dp, modifier = Modifier.matchParentSize())
        val thumbWidth = maxWidth
        // 左下白色小条：深墨缩略图为纯白
        val barColor = if (wallpaper == ChatWallpaper.Ink) Color.White else colors.bubbleIn
        Box(
            modifier = Modifier
                .align(Alignment.BottomStart)
                .padding(start = 8.dp, bottom = 9.dp)
                .size(width = thumbWidth * 0.4f, height = 12.dp)
                .clip(RoundedCornerShape(6.dp))
                .background(barColor)
                .border(1.dp, BarStroke, RoundedCornerShape(6.dp)),
        )
        // 右上偏下的彩色小条
        Box(
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .padding(end = 8.dp, bottom = 25.dp)
                .size(width = thumbWidth * 0.34f, height = 12.dp)
                .clip(RoundedCornerShape(6.dp))
                .background(base.bubbleOut),
        )
    }
}

// ─── 外观 ───────────────────────────────────────────────────────────

@Composable
private fun DarkModeCard() {
    val colors = ShiYuTheme.colors
    val settings = ShiYuTheme.settings
    val controller = ShiYuTheme.controller
    val toast = LocalToast.current
    val shape = RoundedCornerShape(18.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, colors.line, shape)
            .padding(horizontal = 16.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text(text = "深色模式", color = colors.ink, style = ShiYuType.settingTitle)
                Text(
                    text = "单独适配通话与聊天界面的深色配色",
                    modifier = Modifier.padding(top = 2.dp),
                    color = colors.ink3,
                    style = ShiYuType.settingSubtitle,
                )
            }
            ShiYuSwitch(
                checked = settings.darkTheme,
                onCheckedChange = { dark ->
                    controller.setDarkTheme(dark)
                    toast?.show(if (dark) "已切换到深色模式" else "已切换到浅色模式")
                },
            )
        }
    }
}

// ─── 点阵背景 ───────────────────────────────────────────────────────

/** 聊天区点阵：间距 16dp；缩略图 7dp（设计规格 2.6）。 */
@Composable
private fun ChatDots(
    color: Color,
    spacing: Dp,
    modifier: Modifier = Modifier,
) {
    if (color == Color.Transparent) return
    Canvas(modifier = modifier) {
        val step = spacing.toPx()
        val radius = 1.dp.toPx()
        var y = step / 2f
        while (y < size.height) {
            var x = step / 2f
            while (x < size.width) {
                drawCircle(color = color, radius = radius, center = Offset(x, y))
                x += step
            }
            y += step
        }
    }
}

/** 色板圆点内描边 rgba(0,0,0,.1)。 */
private val SwatchStroke = Color(0x1A000000)

/** 墙纸缩略图白色小条内描边 rgba(20,24,27,.08)。 */
private val BarStroke = Color(0x1414181B)
