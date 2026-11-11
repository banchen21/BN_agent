package com.bnagent.shiyu.ui.call

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.bnagent.shiyu.data.repository.ConversationRepository
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.UserProfile
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.util.Locale

/**
 * 通话模拟状态机（设计规格 5.6）：
 * - 进入即「正在呼叫…」；
 * - [CONNECT_DELAY_MS] 后接通并开始每秒计时；
 * - 静音 / 免提 / 摄像头翻转均为本地状态，无真实音视频。
 */
class CallViewModel(
    repository: ConversationRepository,
    conversationId: String,
) : ViewModel() {

    val conversation: StateFlow<Conversation?> = repository.conversation(conversationId)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    val me: StateFlow<UserProfile> = repository.profile
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), UserProfile())

    private val _uiState = MutableStateFlow(CallUiState())
    val uiState: StateFlow<CallUiState> = _uiState.asStateFlow()

    private val sessionJob: Job = viewModelScope.launch {
        delay(CONNECT_DELAY_MS)
        _uiState.update { it.copy(connected = true) }
        while (isActive) {
            delay(TICK_MS)
            _uiState.update { it.copy(elapsedSeconds = it.elapsedSeconds + 1) }
        }
    }

    fun toggleMute() = _uiState.update { it.copy(muted = !it.muted) }

    fun toggleSpeaker() = _uiState.update { it.copy(speakerOn = !it.speakerOn) }

    /** 翻转本地画面；返回翻转后的状态（true = 前置 / 镜像）。 */
    fun flipCamera(): Boolean {
        val next = !_uiState.value.cameraFlipped
        _uiState.update { it.copy(cameraFlipped = next) }
        return next
    }

    /** 挂断并冻结计时；返回结果供 toast 使用。 */
    fun endCall(): CallResult {
        sessionJob.cancel()
        val snapshot = _uiState.value
        return CallResult(
            connected = snapshot.connected,
            elapsedSeconds = snapshot.elapsedSeconds,
        )
    }

    data class CallUiState(
        val connected: Boolean = false,
        val elapsedSeconds: Int = 0,
        val muted: Boolean = false,
        val speakerOn: Boolean = false,
        val cameraFlipped: Boolean = false,
    )

    data class CallResult(
        val connected: Boolean,
        val elapsedSeconds: Int,
    )

    companion object {
        const val CONNECT_DELAY_MS = 2_400L
        private const val TICK_MS = 1_000L

        fun factory(
            repository: ConversationRepository,
            conversationId: String,
        ) = viewModelFactory {
            initializer { CallViewModel(repository, conversationId) }
        }
    }
}

/** 通话计时 `mm:ss`。 */
fun formatCallDuration(seconds: Int): String =
    String.format(Locale.ROOT, "%02d:%02d", seconds / 60, seconds % 60)
