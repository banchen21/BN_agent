package com.bnagent.shiyu.data.local

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.emptyPreferences
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import com.bnagent.shiyu.data.ShiYuJson
import com.bnagent.shiyu.domain.model.AppSettings
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.map
import kotlinx.serialization.json.Json
import java.io.IOException

// 与 shiyu_theme / shiyu_chat 分开的文件，互不干扰。
private val Context.settingsDataStore: DataStore<Preferences> by preferencesDataStore(name = "shiyu_settings")

/**
 * 应用设置本地存储（服务器地址 / API Key / 通知开关）。
 *
 * 主题设置由 ui/theme/DataStoreThemeController 单独持久化（architect 已实现），
 * 这里不重复建。
 */
class SettingsStore(
    context: Context,
    private val json: Json = ShiYuJson,
) {
    private val appContext = context.applicationContext
    private val settingsKey = stringPreferencesKey(KEY_APP_SETTINGS)

    val settings: Flow<AppSettings> = appContext.settingsDataStore.data
        .catch { e ->
            if (e is IOException) emit(emptyPreferences()) else throw e
        }
        .map { prefs -> prefs.decode() ?: AppSettings() }

    suspend fun update(transform: (AppSettings) -> AppSettings) {
        appContext.settingsDataStore.edit { prefs ->
            val current = prefs.decode() ?: AppSettings()
            val next = transform(current)
            if (next != current) {
                prefs[settingsKey] = json.encodeToString(AppSettings.serializer(), next)
            }
        }
    }

    private fun Preferences.decode(): AppSettings? {
        val raw = this[settingsKey] ?: return null
        return runCatching { json.decodeFromString(AppSettings.serializer(), raw) }.getOrNull()
    }

    private companion object {
        const val KEY_APP_SETTINGS = "app_settings"
    }
}
