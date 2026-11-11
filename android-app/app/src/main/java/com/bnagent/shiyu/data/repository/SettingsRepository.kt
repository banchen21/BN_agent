package com.bnagent.shiyu.data.repository

import com.bnagent.shiyu.data.local.SettingsStore
import com.bnagent.shiyu.domain.model.AppSettings
import com.bnagent.shiyu.domain.model.ServerSettings
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

/**
 * 设置仓库：服务器地址 / API Key / 聊天通知开关。
 *
 * 主题设置请继续使用 `LocalThemeController`（architect 的 DataStoreThemeController），
 * 这里不重复持久化。
 */
class SettingsRepository(
    private val store: SettingsStore,
) {
    val settings: Flow<AppSettings> = store.settings

    val serverSettings: Flow<ServerSettings> =
        store.settings.map { it.server }.distinctUntilChanged()

    val notificationsEnabled: Flow<Boolean> =
        store.settings.map { it.notificationsEnabled }.distinctUntilChanged()

    suspend fun currentServerSettings(): ServerSettings = serverSettings.first()

    suspend fun updateServerSettings(server: ServerSettings) {
        store.update { it.copy(server = server) }
    }

    suspend fun setNotificationsEnabled(enabled: Boolean) {
        store.update { it.copy(notificationsEnabled = enabled) }
    }
}
