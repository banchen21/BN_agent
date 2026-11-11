package com.bnagent.shiyu.ui.me

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.bnagent.shiyu.data.repository.ConversationRepository
import com.bnagent.shiyu.data.repository.SettingsRepository
import com.bnagent.shiyu.domain.model.ServerSettings
import com.bnagent.shiyu.domain.model.UserProfile
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

/** 「我」页状态：本人资料、通知开关、当前服务器地址。 */
class MeViewModel(
    repository: ConversationRepository,
    private val settingsRepository: SettingsRepository,
) : ViewModel() {

    val profile: StateFlow<UserProfile> = repository.profile
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), UserProfile())

    val notificationsEnabled: StateFlow<Boolean> = settingsRepository.notificationsEnabled
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), true)

    val serverSettings: StateFlow<ServerSettings> = settingsRepository.serverSettings
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), ServerSettings())

    fun setNotificationsEnabled(enabled: Boolean) {
        viewModelScope.launch { settingsRepository.setNotificationsEnabled(enabled) }
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
