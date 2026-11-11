package com.bnagent.shiyu.ui.theme

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.emptyPreferences
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.launch
import java.io.IOException

/**
 * 主题控制器：读取/修改主题设置。业务页面通过 `LocalThemeController.current` 获取。
 * 修改立即生效（乐观更新），异步写入 DataStore。
 */
interface ThemeController {
    val settings: StateFlow<ThemeSettings>

    /** 统一更新入口；同一帧内返回相同值会被忽略。 */
    fun update(transform: (ThemeSettings) -> ThemeSettings)

    fun setPalette(palette: BubblePalette) = update { it.copy(palette = palette) }

    fun setBubbleRadius(radius: BubbleRadius) = update { it.copy(bubbleRadius = radius) }

    fun setChatFontSize(size: ChatFontSize) = update { it.copy(chatFontSize = size) }

    fun setWallpaper(wallpaper: ChatWallpaper) = update { it.copy(wallpaper = wallpaper) }

    fun setDarkTheme(dark: Boolean) = update { it.copy(darkTheme = dark) }

    fun toggleDarkTheme() = update { it.copy(darkTheme = !it.darkTheme) }
}

/** 内存实现：用于 @Preview、单测与未注入 DataStore 的场景。 */
class InMemoryThemeController(initial: ThemeSettings = ThemeSettings()) : ThemeController {
    private val _settings = MutableStateFlow(initial)
    override val settings: StateFlow<ThemeSettings> = _settings.asStateFlow()

    override fun update(transform: (ThemeSettings) -> ThemeSettings) {
        _settings.value = transform(_settings.value)
    }
}

private val Context.themeDataStore: DataStore<Preferences> by preferencesDataStore(name = "shiyu_theme")

/** DataStore Preferences 实现；应用进程内只应在根节点创建一个实例。 */
class DataStoreThemeController(context: Context) : ThemeController {
    private val appContext = context.applicationContext
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val _settings = MutableStateFlow(ThemeSettings())
    override val settings: StateFlow<ThemeSettings> = _settings.asStateFlow()

    init {
        scope.launch {
            appContext.themeDataStore.data
                .catch { e ->
                    if (e is IOException) emit(emptyPreferences()) else throw e
                }
                .collect { prefs -> _settings.value = prefs.toThemeSettings() }
        }
    }

    override fun update(transform: (ThemeSettings) -> ThemeSettings) {
        val next = transform(_settings.value)
        if (next == _settings.value) return
        _settings.value = next
        scope.launch {
            appContext.themeDataStore.edit { prefs -> prefs.write(next) }
        }
    }
}

private object ThemeKeys {
    val palette = stringPreferencesKey("bubble_palette")
    val bubbleRadius = stringPreferencesKey("bubble_radius")
    val chatFontSize = stringPreferencesKey("chat_font_size")
    val wallpaper = stringPreferencesKey("chat_wallpaper")
    val darkTheme = booleanPreferencesKey("dark_theme")
}

private fun Preferences.toThemeSettings(): ThemeSettings = ThemeSettings(
    palette = BubblePalette.fromId(this[ThemeKeys.palette]),
    bubbleRadius = BubbleRadius.fromId(this[ThemeKeys.bubbleRadius]),
    chatFontSize = ChatFontSize.fromId(this[ThemeKeys.chatFontSize]),
    wallpaper = ChatWallpaper.fromId(this[ThemeKeys.wallpaper]),
    darkTheme = this[ThemeKeys.darkTheme] ?: false,
)

private fun androidx.datastore.preferences.core.MutablePreferences.write(settings: ThemeSettings) {
    this[ThemeKeys.palette] = settings.palette.id
    this[ThemeKeys.bubbleRadius] = settings.bubbleRadius.id
    this[ThemeKeys.chatFontSize] = settings.chatFontSize.id
    this[ThemeKeys.wallpaper] = settings.wallpaper.id
    this[ThemeKeys.darkTheme] = settings.darkTheme
}
