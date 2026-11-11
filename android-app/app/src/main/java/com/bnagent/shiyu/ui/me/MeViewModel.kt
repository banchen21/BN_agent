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

/** 「我」页状态：本人资料、通知开关、当前服务器地址、本地缓存。 */
class MeViewModel(
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

    fun clearCache() {
        _cacheBytes.value = repository.clearCache()
    }

    /** 刷新缓存占用（进入「我」页时调用）。 */
    fun refreshCacheSize() {
        _cacheBytes.value = repository.cacheSizeBytes()
    }

    companion object {
        fun factory(
            repository: ConversationRepository,
            settingsRepository: SettingsRepository,
        ) = viewModelFactory {
            initializer { MeViewModel(repository, settingsRepository) }
        }
    }
}
