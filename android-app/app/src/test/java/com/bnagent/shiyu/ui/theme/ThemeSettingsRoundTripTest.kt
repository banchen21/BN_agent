package com.bnagent.shiyu.ui.theme

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

/**
 * 主题持久化 round-trip：DataStoreThemeController 以枚举 id 字符串 + 布尔落盘，
 * 这里把同一份存储表示编码成 JSON 再读回，验证 5 个字段都能无损还原。
 */
class ThemeSettingsRoundTripTest {

    @Test
    fun `persisted ids round trip through json`() {
        val original = ThemeSettings(
            palette = BubblePalette.Amber,
            bubbleRadius = BubbleRadius.Large,
            chatFontSize = ChatFontSize.Small,
            wallpaper = ChatWallpaper.Ink,
            darkTheme = true,
        )

        val stored = buildJsonObject {
            put("bubble_palette", original.palette.id)
            put("bubble_radius", original.bubbleRadius.id)
            put("chat_font_size", original.chatFontSize.id)
            put("chat_wallpaper", original.wallpaper.id)
            put("dark_theme", original.darkTheme)
        }
        val json = Json.parseToJsonElement(stored.toString()).jsonObject

        val restored = ThemeSettings(
            palette = BubblePalette.fromId(json["bubble_palette"]?.jsonPrimitive?.content),
            bubbleRadius = BubbleRadius.fromId(json["bubble_radius"]?.jsonPrimitive?.content),
            chatFontSize = ChatFontSize.fromId(json["chat_font_size"]?.jsonPrimitive?.content),
            wallpaper = ChatWallpaper.fromId(json["chat_wallpaper"]?.jsonPrimitive?.content),
            darkTheme = json["dark_theme"]?.jsonPrimitive?.boolean ?: false,
        )
        assertEquals(original, restored)
    }

    @Test
    fun `defaults follow design spec`() {
        val settings = ThemeSettings()
        assertEquals(BubblePalette.Coral, settings.palette)
        assertEquals(BubbleRadius.Medium, settings.bubbleRadius)
        assertEquals(ChatFontSize.Standard, settings.chatFontSize)
        assertEquals(ChatWallpaper.Cool, settings.wallpaper)
        assertFalse(settings.darkTheme)
        assertEquals("珊瑚 · 中圆角", settings.summary)
    }

    @Test
    fun `unknown or missing ids fall back to defaults`() {
        val restored = ThemeSettings(
            palette = BubblePalette.fromId("no-such-palette"),
            bubbleRadius = BubbleRadius.fromId(null),
            chatFontSize = ChatFontSize.fromId(""),
            wallpaper = ChatWallpaper.fromId("no-such-wall"),
            darkTheme = false,
        )
        assertEquals(ThemeSettings(), restored)
    }

    @Test
    fun `all enum ids are unique and resolvable`() {
        assertEquals(
            BubblePalette.entries.size,
            BubblePalette.entries.map { it.id }.toSet().size,
        )
        BubblePalette.entries.forEach { assertEquals(it, BubblePalette.fromId(it.id)) }
        BubbleRadius.entries.forEach { assertEquals(it, BubbleRadius.fromId(it.id)) }
        ChatFontSize.entries.forEach { assertEquals(it, ChatFontSize.fromId(it.id)) }
        ChatWallpaper.entries.forEach { assertEquals(it, ChatWallpaper.fromId(it.id)) }
    }
}
