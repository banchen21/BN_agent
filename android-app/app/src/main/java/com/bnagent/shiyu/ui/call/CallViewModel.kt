package com.bnagent.shiyu.ui.call

import android.app.Application
import android.content.Context
import android.media.AudioManager
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.bnagent.shiyu.data.repository.ConversationRepository
import com.bnagent.shiyu.data.remote.CallSession
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.UserProfile
import io.livekit.android.LiveKit
import io.livekit.android.events.RoomEvent
import io.livekit.android.events.collect
import io.livekit.android.room.Room
import io.livekit.android.room.track.LocalVideoTrack
import io.livekit.android.room.track.Track
import io.livekit.android.room.track.VideoTrack
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.util.Locale

/**
 * 真实通话状态机（LiveKit）：
 * - `POST /api/call/start` 建房并取 token，后端派发 yueyue agent；
 * - 连接 `ws://host:7880`（LAN 明文，避开 Caddy 自签证书），发布麦克风；
 *   视频通话额外发布摄像头，Agent 通过 LiveKit 视频轨做人脸/画面理解；
 * - 远端音频由 LiveKit 自动播放；静音 / 免提 / 摄像头翻转 / 挂断为真实操作；
 * - 挂断时断开房间并 `POST /api/call/end` 删除房间。
 */
class CallViewModel(
    application: Application,
    private val repository: ConversationRepository,
    conversationId: String,
    private val video: Boolean,
) : AndroidViewModel(application) {

    val conversation: StateFlow<Conversation?> = repository.conversation(conversationId)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    val me: StateFlow<UserProfile> = repository.profile
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), UserProfile())

    private val audioManager =
        application.getSystemService(Context.AUDIO_SERVICE) as AudioManager

    private val _uiState = MutableStateFlow(CallUiState())
    val uiState: StateFlow<CallUiState> = _uiState.asStateFlow()

    private val _events = MutableSharedFlow<String>(extraBufferCapacity = 4)
    val events: SharedFlow<String> = _events.asSharedFlow()

    /** LiveKit Room：屏幕用它初始化视频渲染器。 */
    val room: Room = LiveKit.create(application)

    private val _localVideoTrack = MutableStateFlow<LocalVideoTrack?>(null)
    val localVideoTrack: StateFlow<LocalVideoTrack?> = _localVideoTrack.asStateFlow()

    private val _remoteVideoTrack = MutableStateFlow<VideoTrack?>(null)
    val remoteVideoTrack: StateFlow<VideoTrack?> = _remoteVideoTrack.asStateFlow()

    private var session: CallSession? = null
    private var timerJob: Job? = null
    private var connectJob: Job? = null

    init {
        viewModelScope.launch {
            room.events.collect { event ->
                when (event) {
                    is RoomEvent.TrackSubscribed -> {
                        val track = event.track
                        if (track is VideoTrack &&
                            event.participant.identity != room.localParticipant.identity
                        ) {
                            _remoteVideoTrack.value = track
                        }
                    }
                    is RoomEvent.TrackUnsubscribed -> {
                        if (event.track is VideoTrack) _remoteVideoTrack.value = null
                    }
                    is RoomEvent.Disconnected -> {
                        _uiState.update { it.copy(connected = false) }
                    }
                    else -> Unit
                }
            }
        }
    }

    /** 权限授予后调用：建房 → 连接 → 发布音视频 → 开始计时。 */
    fun connect() {
        if (connectJob?.isActive == true || _uiState.value.connected) return
        connectJob = viewModelScope.launch {
            _uiState.update { it.copy(connecting = true, error = null) }
            try {
                val call = repository.startCall(if (video) "video" else "audio")
                session = call
                room.connect(call.localWsUrl, call.token)
                room.localParticipant.setMicrophoneEnabled(true)
                if (video) {
                    room.localParticipant.setCameraEnabled(true)
                    refreshLocalVideoTrack()
                }
                enterAudioMode()
                _uiState.update { it.copy(connecting = false, connected = true) }
                startTimer()
            } catch (e: Exception) {
                val message = e.message?.takeIf { it.isNotBlank() } ?: "无法建立通话连接"
                _uiState.update { it.copy(connecting = false, connected = false, error = message) }
                _events.tryEmit("通话连接失败：$message")
                runCatching { room.disconnect() }
                endSessionOnServer()
            }
        }
    }

    fun toggleMute() {
        val next = !_uiState.value.muted
        _uiState.update { it.copy(muted = next) }
        viewModelScope.launch {
            runCatching { room.localParticipant.setMicrophoneEnabled(!next) }
                .onFailure { _events.tryEmit("麦克风切换失败") }
        }
    }

    fun toggleSpeaker() {
        val next = !_uiState.value.speakerOn
        _uiState.update { it.copy(speakerOn = next) }
        runCatching { audioManager.isSpeakerphoneOn = next }
    }

    /** 翻转摄像头并返回翻转后的状态（true = 前置 / 镜像）。 */
    fun flipCamera(): Boolean {
        val next = !_uiState.value.cameraFlipped
        _uiState.update { it.copy(cameraFlipped = next) }
        runCatching { _localVideoTrack.value?.switchCamera() }
            .onFailure { _events.tryEmit("切换摄像头失败") }
        return next
    }

    /** 挂断：断开 LiveKit、删除房间、恢复音频；返回结果供 toast 使用。 */
    fun endCall(): CallResult {
        timerJob?.cancel()
        timerJob = null
        val snapshot = _uiState.value
        _uiState.update { it.copy(connected = false) }
        runCatching { room.disconnect() }
        restoreAudioMode()
        endSessionOnServer()
        return CallResult(
            connected = snapshot.connected,
            elapsedSeconds = snapshot.elapsedSeconds,
        )
    }

    private fun startTimer() {
        timerJob?.cancel()
        timerJob = viewModelScope.launch {
            while (isActive) {
                delay(TICK_MS)
                _uiState.update { it.copy(elapsedSeconds = it.elapsedSeconds + 1) }
            }
        }
    }

    private fun refreshLocalVideoTrack() {
        _localVideoTrack.value =
            room.localParticipant.getTrackPublication(Track.Source.CAMERA)?.track as? LocalVideoTrack
    }

    private fun endSessionOnServer() {
        val roomName = session?.room.orEmpty()
        session = null
        if (roomName.isNotBlank()) {
            viewModelScope.launch { repository.endCall(roomName) }
        }
    }

    private fun enterAudioMode() {
        runCatching {
            audioManager.mode = AudioManager.MODE_IN_COMMUNICATION
            audioManager.isSpeakerphoneOn = _uiState.value.speakerOn
        }
    }

    private fun restoreAudioMode() {
        runCatching {
            audioManager.isSpeakerphoneOn = false
            audioManager.mode = AudioManager.MODE_NORMAL
        }
    }

    override fun onCleared() {
        timerJob?.cancel()
        runCatching { room.disconnect() }
        restoreAudioMode()
        endSessionOnServer()
        super.onCleared()
    }

    data class CallUiState(
        val connecting: Boolean = false,
        val connected: Boolean = false,
        val elapsedSeconds: Int = 0,
        val muted: Boolean = false,
        val speakerOn: Boolean = false,
        val cameraFlipped: Boolean = false,
        val error: String? = null,
    )

    data class CallResult(
        val connected: Boolean,
        val elapsedSeconds: Int,
    )

    companion object {
        private const val TICK_MS = 1_000L

        fun factory(
            application: Application,
            repository: ConversationRepository,
            conversationId: String,
            video: Boolean,
        ) = viewModelFactory {
            initializer { CallViewModel(application, repository, conversationId, video) }
        }
    }
}

/** 通话计时 `mm:ss`。 */
fun formatCallDuration(seconds: Int): String =
    String.format(Locale.ROOT, "%02d:%02d", seconds / 60, seconds % 60)
