package com.bnagent.shiyu.ui.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * 拾语设计令牌（颜色 / 圆角 / 字号 / 背景）。
 *
 * 来源：android-app/docs/design-spec.md 第 2 节。所有颜色只在这里定义，
 * 业务页面通过 `ShiYuTheme.colors` 读取，不要散落硬编码 hex。
 */

// ─── 我的气泡配色（5 套，独立于深/浅色） ────────────────────────────────

enum class BubblePalette(
    val id: String,
    val label: String,
    val bubbleOut: Color,
    val bubbleOutInk: Color,
) {
    Coral("coral", "珊瑚", Color(0xFFC93B24), Color(0xFFFFFFFF)),
    Mint("mint", "松绿", Color(0xFF0F6B57), Color(0xFFFFFFFF)),
    Indigo("indigo", "靛蓝", Color(0xFF22409A), Color(0xFFFFFFFF)),
    Graphite("graphite", "石墨", Color(0xFF2A2F33), Color(0xFFFFFFFF)),
    Amber("amber", "芥黄", Color(0xFFF0C24A), Color(0xFF221A06)),
    ;

    companion object {
        fun fromId(id: String?): BubblePalette = entries.firstOrNull { it.id == id } ?: Coral
    }
}

// ─── 气泡圆角（3 档）+ 尾角 ────────────────────────────────────────────

enum class BubbleRadius(val id: String, val label: String, val radius: Dp) {
    Small("sm", "小", 10.dp),
    Medium("std", "中", 18.dp),
    Large("lg", "大", 26.dp),
    ;

    companion object {
        fun fromId(id: String?): BubbleRadius = entries.firstOrNull { it.id == id } ?: Medium
    }
}

/** 气泡尾部小圆角固定 6dp。 */
val BubbleTailRadius: Dp = 6.dp

// ─── 聊天字号（3 档） ─────────────────────────────────────────────────

enum class ChatFontSize(val id: String, val label: String, val size: TextUnit) {
    Small("sm", "小", 16.sp),
    Standard("std", "标准", 17.sp),
    Large("lg", "大", 19.sp),
    ;

    companion object {
        fun fromId(id: String?): ChatFontSize = entries.firstOrNull { it.id == id } ?: Standard
    }
}

// ─── 聊天背景（3 种） ─────────────────────────────────────────────────

enum class ChatWallpaper(val id: String, val label: String) {
    Cool("cool", "冷白"),
    Mist("mist", "雾灰"),
    Ink("ink", "深墨"),
    ;

    companion object {
        fun fromId(id: String?): ChatWallpaper = entries.firstOrNull { it.id == id } ?: Cool
    }
}

// ─── 解析后的颜色集合 ─────────────────────────────────────────────────

/**
 * 应用内唯一颜色入口。通过 `ShiYuTheme.colors` 获取。
 *
 * 聊天相关字段（chatBg / chatDot / chatMeta / dateChipBg）已经按
 * 「主题深浅 × 聊天背景」组合算好，业务侧不需要再判断。
 */
@Immutable
data class ShiYuColors(
    // 基础
    val bg: Color,
    val surface: Color,
    val surface2: Color,
    val ink: Color,
    val ink2: Color,
    val ink3: Color,
    val line: Color,
    val accent: Color,
    val accentPress: Color,
    val accentText: Color,
    val accentSoft: Color,
    val onAccent: Color,
    val danger: Color,
    val presence: Color,
    val switchOff: Color,
    val screenBg: Color,
    val badgeMutedBg: Color,
    val badgeMutedInk: Color,
    // 气泡
    val bubbleOut: Color,
    val bubbleOutInk: Color,
    val bubbleIn: Color,
    val bubbleInInk: Color,
    // 聊天背景
    val chatBg: Color,
    /** 点阵颜色；Color.Transparent 表示不画点阵。 */
    val chatDot: Color,
    val chatMeta: Color,
    val dateChipBg: Color,
    // 状态
    val isDark: Boolean,
)

/** 设计规格 2.1 / 2.2：基础色，不含气泡与聊天背景。 */
private fun baseColors(dark: Boolean): ShiYuColors {
    val palette = BubblePalette.Coral
    return if (!dark) {
        ShiYuColors(
            bg = Color(0xFFF2F4F5),
            surface = Color(0xFFFFFFFF),
            surface2 = Color(0xFFF5F7F8),
            ink = Color(0xFF14181B),
            ink2 = Color(0xFF5B666C),
            ink3 = Color(0xFF657076),
            line = Color(0xFFE2E7E9),
            accent = Color(0xFFC93B24),
            accentPress = Color(0xFFB1321D),
            accentText = Color(0xFFB5321D),
            accentSoft = Color(0xFFFBEAE5),
            onAccent = Color(0xFFFFFFFF),
            danger = Color(0xFFC0392B),
            presence = Color(0xFF2E9E68),
            switchOff = Color(0xFFC9D1D5),
            screenBg = Color(0xFFF2F4F5),
            badgeMutedBg = Color(0xFFDDE3E5),
            badgeMutedInk = Color(0xFF39434A),
            bubbleOut = palette.bubbleOut,
            bubbleOutInk = palette.bubbleOutInk,
            bubbleIn = Color(0xFFFFFFFF),
            bubbleInInk = Color(0xFF14181B),
            chatBg = Color(0xFFF2F4F5),
            chatDot = Color.Transparent,
            chatMeta = Color(0xFF5F6A70),
            dateChipBg = Color(0x0E14181B),
            isDark = false,
        )
    } else {
        ShiYuColors(
            bg = Color(0xFF0E1113),
            surface = Color(0xFF171C1F),
            surface2 = Color(0xFF1D2428),
            ink = Color(0xFFE9EEF0),
            ink2 = Color(0xFFA6B1B7),
            ink3 = Color(0xFF93A0A6),
            line = Color(0xFF262E32),
            accent = Color(0xFFC93B24),
            accentPress = Color(0xFFB1321D),
            accentText = Color(0xFFFF8F76),
            accentSoft = Color(0xFF37201A),
            onAccent = Color(0xFFFFFFFF),
            danger = Color(0xFFC0392B),
            presence = Color(0xFF2E9E68),
            switchOff = Color(0xFF384247),
            screenBg = Color(0xFF0B0E10),
            badgeMutedBg = Color(0xFF2A3338),
            badgeMutedInk = Color(0xFFC9D3D7),
            bubbleOut = palette.bubbleOut,
            bubbleOutInk = palette.bubbleOutInk,
            bubbleIn = Color(0xFF212A2E),
            bubbleInInk = Color(0xFFE9EEF0),
            chatBg = Color(0xFF0D1113),
            chatDot = Color.Transparent,
            chatMeta = Color(0xFF93A0A6),
            dateChipBg = Color(0x12FFFFFF),
            isDark = true,
        )
    }
}

/**
 * 设计规格 2.6：聊天背景优先级（必须按 CSS 真实结果实现）。
 * - 深墨 + 浅色主题：深底 + 白点阵 + 浅灰 meta；
 * - 深墨 + 深色主题：#080C0E 纯色无点阵；
 * - 深色 + 冷白：#0D1113 纯色。
 */
internal fun resolveShiYuColors(settings: ThemeSettings): ShiYuColors {
    val dark = settings.darkTheme
    val base = baseColors(dark)
    val (chatBg, chatDot, chatMeta) = when (settings.wallpaper) {
        ChatWallpaper.Cool -> Triple(
            if (dark) Color(0xFF0D1113) else Color(0xFFF2F4F5),
            Color.Transparent,
            if (dark) Color(0xFF93A0A6) else Color(0xFF5F6A70),
        )
        ChatWallpaper.Mist -> Triple(
            if (dark) Color(0xFF121A1E) else Color(0xFFE8ECEE),
            if (dark) Color(0x0EFFFFFF) else Color(0x1314181B),
            if (dark) Color(0xFF93A0A6) else Color(0xFF5F6A70),
        )
        ChatWallpaper.Ink -> Triple(
            if (dark) Color(0xFF080C0E) else Color(0xFF0F1418),
            if (dark) Color.Transparent else Color(0x0EFFFFFF),
            if (dark) Color(0xFF93A0A6) else Color(0xFFA8B3B8),
        )
    }
    val dateChip = when {
        dark -> Color(0x12FFFFFF)
        settings.wallpaper == ChatWallpaper.Ink -> Color(0x14FFFFFF)
        else -> Color(0x0E14181B)
    }
    return base.copy(
        bubbleOut = settings.palette.bubbleOut,
        bubbleOutInk = settings.palette.bubbleOutInk,
        chatBg = chatBg,
        chatDot = chatDot,
        chatMeta = chatMeta,
        dateChipBg = dateChip,
    )
}

// ─── 尺寸令牌 ─────────────────────────────────────────────────────────

/** 随主题变化的尺寸：气泡圆角 + 聊天正文排版。通过 `ShiYuTheme.dimens` 获取。 */
@Immutable
data class ShiYuDimens(
    val bubbleRadius: Dp,
    val bubbleTailRadius: Dp,
    val chatFontSize: TextUnit,
    val chatLineHeight: TextUnit,
)

internal fun resolveDimens(settings: ThemeSettings): ShiYuDimens = ShiYuDimens(
    bubbleRadius = settings.bubbleRadius.radius,
    bubbleTailRadius = BubbleTailRadius,
    chatFontSize = settings.chatFontSize.size,
    chatLineHeight = (settings.chatFontSize.size.value * 1.5f).sp,
)

// ─── 通话页固定深色（设计规格 4.7 / 4.8） ─────────────────────────────

/**
 * 通话页强制深色 chrome，配色不随主题变化。
 * 通话页（android-ui-chat 负责实现）可直接使用这些常量。
 */
object ShiYuCallColors {
    val background = Color(0xFF0A0D0F)
    val name = Color(0xFFF4F7F8)
    val status = Color(0xFFAEB9BE)
    val pillText = Color(0xFFDDE5E8)
    val controlLabel = Color(0xFFD5DDE0)
    val hangup = Color(0xFFD13425)
    val controlBg = Color(0x24FFFFFF)
    val controlActiveBg = Color(0xFFF2F5F6)
    val controlActiveInk = Color(0xFF14181B)
    val videoLocalBg = Color(0xFF1B2226)
    val videoControlBg = Color(0xB81A2024)
}

// ─── 头像与通用渐变 ──────────────────────────────────────────────────

internal val AvatarFallbackGradient = listOf(Color(0xFF3A474E), Color(0xFF171C1F))
internal val AvatarFallbackInk = Color(0xFFF2F6F7)
