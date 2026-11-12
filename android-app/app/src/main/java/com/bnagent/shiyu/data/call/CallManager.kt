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
import kotlinx.coroutines.flow.updateAndGet
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.util.Locale
import java.util.concurrent.CopyOnWriteArrayList

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

    private val stateObservers = CopyOnWriteArrayList<(CallUiState) -> Unit>()

    /** 非 Compose 的观察者（通话浮窗服务）订阅状态。 */
    fun subscribeState(observer: (CallUiState) -> Unit): () -> Unit {
        stateObservers += observer
        return { stateObservers -= observer }
    }

    /** 所有状态写入统一走这里，顺带通知浮窗等非 Compose 观察者。 */
    private inline fun updateState(transform: (CallUiState) -> CallUiState) {
        val next = _uiState.updateAndGet(transform)
        stateObservers.forEach { observer -> runCatching { observer(next) } }
    }

    private val _events = MutableSharedFlow<String>(extraBufferCapacity = 4)
    val events: SharedFlow<String> = _events.asSharedFlow()

    /** 对方（Agent）主动结束通话；UI 负责回聊天页并提示。 */
    private val _endedByPeer = MutableSharedFlow<CallUiState>(extraBufferCapacity = 1)
    val endedByPeer: SharedFlow<CallUiState> = _endedByPeer.asSharedFlow()

    /** 当前通话的 LiveKit Room；未在通话中为 null（渲染器据此初始化）。 */
    private val _room = MutableStateFlow<Room?>(null)
    val room: StateFlow<Room?> = _room.asStateFlow()

    private val _localVideoTrack = MutableStateFlow<LocalVideoTrack?>(null)
    val localVideoTrack: StateFlow<LocalVideoTrack?> = _localVideoTrack.asStateFlow()

    private val _remoteVideoTrack = MutableStateFlow<VideoTrack?>(null)
    val remoteVideoTrack: StateFlow<VideoTrack?> = _remoteVideoTrack.asStateFlow()

    private var session: CallSession? = null
    private var heartbeatJob: Job? = null
    private var timerJob: Job? = null
    private var connectJob: Job? = null
    private var roomEventsJob: Job? = null

    /** 已接听的 Agent 来电：通话页拿到权限后用它直接入会（不再新建房间）。 */
    private var pendingInvite: CallSession? = null

    /** 用户点了「接听」：暂存邀请，等通话页完成权限申请后由 [connect] 消费。 */
    fun acceptInvite(invite: CallSession) {
        pendingInvite = invite
    }

    /**
     * 建立通话（幂等）：同一会话已在通话中时直接复用现有连接。
     * 从最小化悬浮条返回通话页时会重复调用，这里必须无副作用。
     * 若存在已接听的来电邀请（[acceptInvite]），优先用它入会。
     */
    fun connect(conversationId: String, video: Boolean) {
        val current = _uiState.value
        if (current.isActive) {
            if (current.conversationId == conversationId) return
            endCall() // 同一时间只保留一路通话
        }
        if (connectJob?.isActive == true) return
        val invite = pendingInvite
        pendingInvite = null
        val effectiveVideo = invite?.mediaType?.equals("video", ignoreCase = true) ?: video
        connectJob = scope.launch {
            updateState {
                CallUiState(
                    conversationId = conversationId,
                    video = effectiveVideo,
                    incoming = invite != null,
                    connecting = true,
                )
            }
            try {
                val call = invite
                    ?: repository.startCall(conversationId, if (video) "video" else "audio")
                session = call
                val room = LiveKit.create(application)
                _room.value = room
                observeRoom(room)
                room.connect(call.localWsUrl, call.token)
                room.localParticipant.setMicrophoneEnabled(true)
                if (effectiveVideo) {
                    room.localParticipant.setCameraEnabled(true)
                    updateState { it.copy(cameraEnabled = true) }
                    refreshLocalVideoTrack()
                }
                enterAudioMode()
                updateState { it.copy(connecting = false, connected = true) }
                startTimer()
                startHeartbeat(call.room)
            } catch (e: Exception) {
                val message = e.message?.takeIf { it.isNotBlank() } ?: "无法建立通话连接"
                updateState { it.copy(connecting = false, connected = false, error = message) }
                _events.tryEmit("通话连接失败：$message")
                releaseRoom()
                endSessionOnServer()
            }
        }
    }

    fun toggleMute() {
        val next = !_uiState.value.muted
        updateState { it.copy(muted = next) }
        scope.launch {
            runCatching {
                _room.value?.localParticipant?.setMicrophoneEnabled(!next)
            }.onFailure { _events.tryEmit("麦克风切换失败") }
        }
    }

    fun toggleSpeaker() {
        val next = !_uiState.value.speakerOn
        updateState { it.copy(speakerOn = next) }
        runCatching { audioManager.isSpeakerphoneOn = next }
    }

    /** 语音通话中开/关摄像头（开启后 Agent 可通过视频帧看到画面）。 */
    fun toggleCamera() {
        if (_uiState.value.conversationId.isEmpty()) return
        val next = !_uiState.value.cameraEnabled
        updateState { it.copy(cameraEnabled = next) }
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
        updateState { it.copy(cameraFlipped = next) }
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
        pendingInvite = null
        val snapshot = _uiState.value
        updateState { CallUiState() }
        releaseRoom()
        restoreAudioMode()
        endSessionOnServer()
        appendRecord(snapshot)
        return CallResult(
            connected = snapshot.connected,
            elapsedSeconds = snapshot.elapsedSeconds,
            conversationId = snapshot.conversationId,
            video = snapshot.video,
        )
    }

    /** 对方（Agent）主动挂断：与本地挂断同样清理，并通知 UI 回聊天页。 */
    private fun endByPeer() {
        val snapshot = _uiState.value
        if (!snapshot.isActive) return
        connectJob?.cancel()
        connectJob = null
        timerJob?.cancel()
        timerJob = null
        updateState { CallUiState() }
        releaseRoom()
        restoreAudioMode()
        endSessionOnServer()
        appendRecord(snapshot)
        scope.launch { _endedByPeer.emit(snapshot) }
    }

    /** 通话结束写一条记录到消息列表（只在真正接通过后记录）。 */
    private fun appendRecord(snapshot: CallUiState) {
        if (!snapshot.connected || snapshot.conversationId.isBlank()) return
        scope.launch {
            repository.appendCallRecord(
                conversationId = snapshot.conversationId,
                video = snapshot.video,
                durationSeconds = snapshot.elapsedSeconds,
                mine = !snapshot.incoming,
            )
        }
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
                    is RoomEvent.ParticipantDisconnected -> {
                        // 1:1 通话里对方（Agent）离开即视为对方挂断。
                        if (event.participant.identity != room.localParticipant.identity) {
                            endByPeer()
                        }
                    }
                    is RoomEvent.Disconnected -> {
                        if (_uiState.value.connected) {
                            updateState { it.copy(connected = false) }
                        }
                    }
                    else -> Unit
                }
            }
        }
    }

    private fun releaseRoom() {
        roomEventsJob?.cancel()
        roomEventsJob = null
        heartbeatJob?.cancel()
        heartbeatJob = null
        _room.value?.let { room -> runCatching { room.disconnect() } }
        _room.value = null
        _localVideoTrack.value = null
        _remoteVideoTrack.value = null
    }

    /**
     * 通话心跳：每 4s 告诉后端房间还在用；App 被杀/退到后台后心跳停止，
     * 后端看门狗会删除房间，避免 Agent 留在空房间里。
     */
    private fun startHeartbeat(room: String) {
        heartbeatJob?.cancel()
        heartbeatJob = scope.launch {
            while (isActive) {
                repository.callHeartbeat(room)
                delay(HEARTBEAT_MS)
            }
        }
    }

    private fun startTimer() {
        timerJob?.cancel()
        timerJob = scope.launch {
            while (isActive) {
                delay(TICK_MS)
                updateState { it.copy(elapsedSeconds = it.elapsedSeconds + 1) }
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
        /** true = Agent 来电（记录气泡显示在对方一侧）。 */
        val incoming: Boolean = false,
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
        private const val HEARTBEAT_MS = 4_000L
    }
}

/** 通话计时 `mm:ss`。 */
fun formatCallDuration(seconds: Int): String =
    String.format(Locale.ROOT, "%02d:%02d", seconds / 60, seconds % 60)
