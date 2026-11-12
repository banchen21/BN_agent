package com.bnagent.shiyu.data.call

import android.app.Application
import android.content.Context
import android.media.AudioManager
import com.bnagent.shiyu.data.remote.CallSession
import com.bnagent.shiyu.data.repository.ConversationRepository
import io.livekit.android.LiveKit
import io.livekit.android.events.RoomEvent
import io.livekit.android.events.collect
import io.livekit.android.room.Room
import io.livekit.android.room.track.LocalVideoTrack
import io.livekit.android.room.track.Track
import io.livekit.android.room.track.VideoTrack
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.util.Locale

/**
 * 进程级通话状态机（LiveKit），由 [com.bnagent.shiyu.data.AppContainer] 持有。
 *
 * 与页面解耦：通话页只是它的一个观察者，最小化（离开通话页）不会中断通话；
 * 静音 / 免提 / 摄像头 / 计时都保存在这里，重新进入通话页直接续上。
 *
 * - `POST /api/call/start` 建房并取 token，后端派发 yueyue agent；
 * - 连接 `ws://host:7880`（LAN 明文，避开 Caddy 自签证书），发布麦克风；
 *   视频通话额外发布摄像头，Agent 通过 LiveKit 视频轨做人脸/画面理解；
 * - 挂断时断开房间并 `POST /api/call/end` 删除房间。
 */
class CallManager(
    private val application: Application,
    private val repository: ConversationRepository,
) {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    private val audioManager =
        application.getSystemService(Context.AUDIO_SERVICE) as AudioManager

    private val _uiState = MutableStateFlow(CallUiState())
    val uiState: StateFlow<CallUiState> = _uiState.asStateFlow()

    private val _events = MutableSharedFlow<String>(extraBufferCapacity = 4)
    val events: SharedFlow<String> = _events.asSharedFlow()

    /** 当前通话的 LiveKit Room；未在通话中为 null（渲染器据此初始化）。 */
    private val _room = MutableStateFlow<Room?>(null)
    val room: StateFlow<Room?> = _room.asStateFlow()

    private val _localVideoTrack = MutableStateFlow<LocalVideoTrack?>(null)
    val localVideoTrack: StateFlow<LocalVideoTrack?> = _localVideoTrack.asStateFlow()

    private val _remoteVideoTrack = MutableStateFlow<VideoTrack?>(null)
    val remoteVideoTrack: StateFlow<VideoTrack?> = _remoteVideoTrack.asStateFlow()

    private var session: CallSession? = null
    private var timerJob: Job? = null
    private var connectJob: Job? = null
    private var roomEventsJob: Job? = null

    /**
     * 建立通话（幂等）：同一会话已在通话中时直接复用现有连接。
     * 从最小化悬浮条返回通话页时会重复调用，这里必须无副作用。
     */
    fun connect(conversationId: String, video: Boolean) {
        val current = _uiState.value
        if (current.isActive) {
            if (current.conversationId == conversationId) return
            endCall() // 同一时间只保留一路通话
        }
        if (connectJob?.isActive == true) return
        connectJob = scope.launch {
            _uiState.value = CallUiState(
                conversationId = conversationId,
                video = video,
                connecting = true,
            )
            try {
                val call = repository.startCall(conversationId, if (video) "video" else "audio")
                session = call
                val room = LiveKit.create(application)
                _room.value = room
                observeRoom(room)
                room.connect(call.localWsUrl, call.token)
                room.localParticipant.setMicrophoneEnabled(true)
                if (video) {
                    room.localParticipant.setCameraEnabled(true)
                    _uiState.update { it.copy(cameraEnabled = true) }
                    refreshLocalVideoTrack()
                }
                enterAudioMode()
                _uiState.update { it.copy(connecting = false, connected = true) }
                startTimer()
            } catch (e: Exception) {
                val message = e.message?.takeIf { it.isNotBlank() } ?: "无法建立通话连接"
                _uiState.update { it.copy(connecting = false, connected = false, error = message) }
                _events.tryEmit("通话连接失败：$message")
                releaseRoom()
                endSessionOnServer()
            }
        }
    }

    fun toggleMute() {
        val next = !_uiState.value.muted
        _uiState.update { it.copy(muted = next) }
        scope.launch {
            runCatching {
                _room.value?.localParticipant?.setMicrophoneEnabled(!next)
            }.onFailure { _events.tryEmit("麦克风切换失败") }
        }
    }

    fun toggleSpeaker() {
        val next = !_uiState.value.speakerOn
        _uiState.update { it.copy(speakerOn = next) }
        runCatching { audioManager.isSpeakerphoneOn = next }
    }

    /** 语音通话中开/关摄像头（开启后 Agent 可通过视频帧看到画面）。 */
    fun toggleCamera() {
        if (_uiState.value.conversationId.isEmpty()) return
        val next = !_uiState.value.cameraEnabled
        _uiState.update { it.copy(cameraEnabled = next) }
        scope.launch {
            runCatching {
                _room.value?.localParticipant?.setCameraEnabled(next)
            }.onSuccess {
                if (next) refreshLocalVideoTrack() else _localVideoTrack.value = null
            }.onFailure { _events.tryEmit("摄像头切换失败") }
        }
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
        connectJob?.cancel()
        connectJob = null
        timerJob?.cancel()
        timerJob = null
        val snapshot = _uiState.value
        _uiState.value = CallUiState()
        releaseRoom()
        restoreAudioMode()
        endSessionOnServer()
        return CallResult(
            connected = snapshot.connected,
            elapsedSeconds = snapshot.elapsedSeconds,
            conversationId = snapshot.conversationId,
            video = snapshot.video,
        )
    }

    private fun observeRoom(room: Room) {
        roomEventsJob?.cancel()
        roomEventsJob = scope.launch {
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

    private fun releaseRoom() {
        roomEventsJob?.cancel()
        roomEventsJob = null
        _room.value?.let { room -> runCatching { room.disconnect() } }
        _room.value = null
        _localVideoTrack.value = null
        _remoteVideoTrack.value = null
    }

    private fun startTimer() {
        timerJob?.cancel()
        timerJob = scope.launch {
            while (isActive) {
                delay(TICK_MS)
                _uiState.update { it.copy(elapsedSeconds = it.elapsedSeconds + 1) }
            }
        }
    }

    private fun refreshLocalVideoTrack() {
        _localVideoTrack.value =
            _room.value?.localParticipant?.getTrackPublication(Track.Source.CAMERA)?.track as? LocalVideoTrack
    }

    private fun endSessionOnServer() {
        val roomName = session?.room.orEmpty()
        session = null
        if (roomName.isNotBlank()) {
            scope.launch { repository.endCall(roomName) }
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

    data class CallUiState(
        val conversationId: String = "",
        val video: Boolean = false,
        val connecting: Boolean = false,
        val connected: Boolean = false,
        val elapsedSeconds: Int = 0,
        val muted: Boolean = false,
        val speakerOn: Boolean = false,
        val cameraFlipped: Boolean = false,
        val cameraEnabled: Boolean = false,
        val error: String? = null,
    ) {
        /** 通话是否在进行（含接通前）；最小化后的悬浮条据此显示。 */
        val isActive: Boolean get() = connecting || connected
    }

    data class CallResult(
        val connected: Boolean,
        val elapsedSeconds: Int,
        val conversationId: String = "",
        val video: Boolean = false,
    )

    private companion object {
        private const val TICK_MS = 1_000L
    }
}

/** 通话计时 `mm:ss`。 */
fun formatCallDuration(seconds: Int): String =
    String.format(Locale.ROOT, "%02d:%02d", seconds / 60, seconds % 60)
