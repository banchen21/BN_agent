package com.bnagent.shiyu.data.local

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.emptyPreferences
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import com.bnagent.shiyu.data.ShiYuJson
import com.bnagent.shiyu.data.seed.SeedData
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.map
import kotlinx.serialization.json.Json
import java.io.IOException

// DataStore 单例约束：同一文件在进程内只能有一个 delegate 实例，故放顶层。
private val Context.chatDataStore: DataStore<Preferences> by preferencesDataStore(name = "shiyu_chat")

/**
 * 会话数据本地存储：DataStore Preferences + 单个 JSON 字符串（[ChatState]）。
 *
 * - [state]：无数据时先以种子内容兜底，避免首帧空白；
 * - [ensureSeeded]：只在首次启动（还没有 chat_state 键）时写入种子；
 * - [update]：原子读改写（DataStore.edit 内完成），发送消息等跨会话改动一次落盘。
 */
class ChatStore(
    context: Context,
    private val json: Json = ShiYuJson,
) {
    private val appContext = context.applicationContext
    private val stateKey = stringPreferencesKey(KEY_CHAT_STATE)

    private val preferences: Flow<Preferences> = appContext.chatDataStore.data
        .catch { e ->
            if (e is IOException) emit(emptyPreferences()) else throw e
        }

    val state: Flow<ChatState> = preferences.map { prefs ->
        prefs.decodeState() ?: SeedData.state
    }

    suspend fun ensureSeeded() {
        appContext.chatDataStore.edit { prefs ->
            if (prefs[stateKey] == null) {
                prefs[stateKey] = json.encodeToString(ChatState.serializer(), SeedData.state)
            }
        }
    }

    /**
     * 原子读改写。`transform` 只做纯计算，不要在里面做挂起操作。
     * 内容未变化时不写盘（DataStore 也不会重复发射）。
     */
    suspend fun update(transform: (ChatState) -> ChatState) {
        appContext.chatDataStore.edit { prefs ->
            val current = prefs.decodeState() ?: SeedData.state
            val next = transform(current)
            if (next != current) {
                prefs[stateKey] = json.encodeToString(ChatState.serializer(), next)
            }
        }
    }

    private fun Preferences.decodeState(): ChatState? {
        val raw = this[stateKey] ?: return null
        return runCatching { json.decodeFromString(ChatState.serializer(), raw) }.getOrNull()
    }

    private companion object {
        const val KEY_CHAT_STATE = "chat_state"
    }
}
