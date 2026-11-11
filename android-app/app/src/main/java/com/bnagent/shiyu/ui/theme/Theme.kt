package com.bnagent.shiyu.ui.theme

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat

/**
 * 主题设置模型。由 ThemeController 持久化（DataStore），由 ShiYuTheme 下发。
 *
 * 默认值见 design-spec 6.5：珊瑚 / 中圆角 / 标准字号 / 冷白 / 浅色。
 * 深色模式是应用内手动开关，不跟随系统（team-lead 已拍板）。
 */
@Immutable
data class ThemeSettings(
    val palette: BubblePalette = BubblePalette.Coral,
    val bubbleRadius: BubbleRadius = BubbleRadius.Medium,
    val chatFontSize: ChatFontSize = ChatFontSize.Standard,
    val wallpaper: ChatWallpaper = ChatWallpaper.Cool,
    val darkTheme: Boolean = false,
) {
    /** 「我」页菜单里的主题摘要：如「珊瑚 · 中圆角」。 */
    val summary: String get() = "${palette.label} · ${bubbleRadius.label}圆角"
}

// ─── CompositionLocal：唯一真源 ────────────────────────────────────────

val LocalShiYuColors = staticCompositionLocalOf { resolveShiYuColors(ThemeSettings()) }
val LocalShiYuDimens = staticCompositionLocalOf { resolveDimens(ThemeSettings()) }
val LocalThemeSettings = staticCompositionLocalOf { ThemeSettings() }

/**
 * 主题控制器由 MainActivity 在根部提供。业务页面直接：
 * ```
 * val theme = LocalThemeController.current
 * theme.setPalette(BubblePalette.Mint)
 * ```
 * 预览 / 单测里可用 `InMemoryThemeController()` 提供默认实现。
 */
val LocalThemeController = staticCompositionLocalOf<ThemeController> {
    error("LocalThemeController 未提供：请在根部用 CompositionLocalProvider 注入 ThemeController")
}

// ─── 主题入口 ─────────────────────────────────────────────────────────

/**
 * 拾语主题。用法：
 * ```
 * val controller = remember { DataStoreThemeController(context) }
 * val settings by controller.settings.collectAsStateWithLifecycle()
 * CompositionLocalProvider(LocalThemeController provides controller) {
 *     ShiYuTheme(settings) { App() }
 * }
 * ```
 */
@Composable
fun ShiYuTheme(
    settings: ThemeSettings = ThemeSettings(),
    content: @Composable () -> Unit,
) {
    val colors = remember(settings) { resolveShiYuColors(settings) }
    val dimens = remember(settings) { resolveDimens(settings) }

    val materialColors = remember(settings) {
        if (settings.darkTheme) {
            darkColorScheme(
                primary = colors.accent,
                onPrimary = colors.onAccent,
                background = colors.bg,
                onBackground = colors.ink,
                surface = colors.surface,
                onSurface = colors.ink,
                surfaceVariant = colors.surface2,
                onSurfaceVariant = colors.ink2,
                outline = colors.line,
                error = colors.danger,
            )
        } else {
            lightColorScheme(
                primary = colors.accent,
                onPrimary = colors.onAccent,
                background = colors.bg,
                onBackground = colors.ink,
                surface = colors.surface,
                onSurface = colors.ink,
                surfaceVariant = colors.surface2,
                onSurfaceVariant = colors.ink2,
                outline = colors.line,
                error = colors.danger,
            )
        }
    }

    CompositionLocalProvider(
        LocalShiYuColors provides colors,
        LocalShiYuDimens provides dimens,
        LocalThemeSettings provides settings,
    ) {
        MaterialTheme(colorScheme = materialColors, content = content)
    }
}

/** 主题读取入口：`ShiYuTheme.colors` / `ShiYuTheme.dimens` / `ShiYuTheme.settings`。 */
object ShiYuTheme {
    val colors: ShiYuColors
        @Composable @ReadOnlyComposable get() = LocalShiYuColors.current

    val dimens: ShiYuDimens
        @Composable @ReadOnlyComposable get() = LocalShiYuDimens.current

    val settings: ThemeSettings
        @Composable @ReadOnlyComposable get() = LocalThemeSettings.current

    val controller: ThemeController
        @Composable @ReadOnlyComposable get() = LocalThemeController.current

    /**
     * 气泡形状。对方（左）尾角在左下，我方（右）尾角在右下；图片/文件用 tail=false。
     */
    @Composable
    fun bubbleShape(isMine: Boolean, tail: Boolean = true): Shape {
        val r = dimens.bubbleRadius
        val t = dimens.bubbleTailRadius
        return when {
            !tail -> RoundedCornerShape(r)
            isMine -> RoundedCornerShape(topStart = r, topEnd = r, bottomEnd = t, bottomStart = r)
            else -> RoundedCornerShape(topStart = r, topEnd = r, bottomEnd = r, bottomStart = t)
        }
    }
}

// ─── 系统栏明暗 ───────────────────────────────────────────────────────

/**
 * 按当前主题刷新状态栏/导航栏图标明暗。
 *
 * @param darkChrome true = 深色底（浅色图标）。普通页面传 `settings.darkTheme`；
 * 通话页（路由以 `call/` 开头）由 AppRoot 强制传 true（设计规格 1.1 / 4.7）。
 */
@Composable
fun ApplySystemBarAppearance(darkChrome: Boolean) {
    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val activity = view.context.findActivity() ?: return@SideEffect
            val window = activity.window
            val controller = WindowCompat.getInsetsController(window, view)
            controller.isAppearanceLightStatusBars = !darkChrome
            controller.isAppearanceLightNavigationBars = !darkChrome
        }
    }
}

private fun Context.findActivity(): Activity? {
    var ctx: Context? = this
    while (ctx is ContextWrapper) {
        if (ctx is Activity) return ctx
        ctx = ctx.baseContext
    }
    return null
}
