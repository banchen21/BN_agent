package com.bnagent.shiyu.ui.me

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.bnagent.shiyu.data.repository.ConversationRepository
import com.bnagent.shiyu.data.repository.SettingsRepository
import com.bnagent.shiyu.domain.model.ServerSettings
import com.bnagent.shiyu.domain.model.UserProfile
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/** 「我」页状态：本人资料、通知开关、当前服务器地址、本地缓存。 */
class MeViewModel(
    private val context: android.content.Context,
    private val repository: ConversationRepository,
    private val settingsRepository: SettingsRepository,
) : ViewModel() {

    val profile: StateFlow<UserProfile> = repository.profile
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), UserProfile())

    val notificationsEnabled: StateFlow<Boolean> = settingsRepository.notificationsEnabled
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), true)

    val serverSettings: StateFlow<ServerSettings> = settingsRepository.serverSettings
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), ServerSettings())

    private val _cacheBytes = MutableStateFlow(repository.cacheSizeBytes())
    val cacheBytes: StateFlow<Long> = _cacheBytes.asStateFlow()

    fun setNotificationsEnabled(enabled: Boolean) {
        viewModelScope.launch { settingsRepository.setNotificationsEnabled(enabled) }
    }

    fun updateProfile(name: String, signature: String, avatar: String) {
        viewModelScope.launch { repository.updateProfile(name, signature, avatar) }
    }

    /**
     * 自定义头像：把相册选中的图片拷进应用私有目录（旧的不再使用的自定义头像顺手删掉），
     * 返回 `file://` 路径；失败返回 null。
     */
    suspend fun saveCustomAvatar(uri: android.net.Uri, previous: String): String? =
        withContext(kotlinx.coroutines.Dispatchers.IO) {
            runCatching {
                val dir = File(context.filesDir, "profile").apply { mkdirs() }
                val file = File(dir, "avatar_${System.currentTimeMillis()}.jpg")
                context.contentResolver.openInputStream(uri)?.use { input ->
                    file.outputStream().use { output -> input.copyTo(output) }
                } ?: error("openInputStream null")
                check(file.length() > 0) { "empty image" }
                if (previous.startsWith("file://")) {
                    runCatching { File(android.net.Uri.parse(previous).path.orEmpty()).delete() }
                }
                "file://${file.absolutePath}"
            }.getOrNull()
        }

    fun clearCache() {
        _cacheBytes.value = repository.clearCache()
    }

    /** 刷新缓存占用（进入「我」页时调用）。 */
    fun refreshCacheSize() {
        _cacheBytes.value = repository.cacheSizeBytes()
    }

    companion object {
        fun factory(
            context: android.content.Context,
            repository: ConversationRepository,
            settingsRepository: SettingsRepository,
        ) = viewModelFactory {
            initializer { MeViewModel(context.applicationContext, repository, settingsRepository) }
        }
    }
}
