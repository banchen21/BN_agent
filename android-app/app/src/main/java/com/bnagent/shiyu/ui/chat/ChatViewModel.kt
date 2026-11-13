package com.bnagent.shiyu.ui.chat

import android.content.Context
import android.media.MediaPlayer
import android.media.MediaRecorder
import android.net.Uri
import android.os.Build
import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.data.local.VoiceFileStore
import com.bnagent.shiyu.data.remote.AgentException
import com.bnagent.shiyu.data.repository.ConversationRepository
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.domain.model.MessageSender
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.util.UUID
import kotlin.math.max

/**
 * 聊天页 ViewModel（android-ui-chat 私有）。
 *
 * 职责：
 * - 从 [ConversationRepository] 读会话 / 消息 Flow，进入时 [ConversationRepository.openConversation]；
 * - 文本发送走后端（乐观插入 → 等待回复期间 typing=true → 成功追加回复 / 失败置 FAILED 并 toast）；
 * - 附件（图片 / 文件 / 语音）走 [ConversationRepository.sendAttachment]（仅落本地）；
 * - MediaRecorder 真实录音（AAC/m4a，VoiceFileStore 生成文件），MediaPlayer 播放真实录音；
 *   种子演示语音（filePath=null）只跑波形动画，按消息时长自动停；
 * - 所有错误通过 [events] 以中文文案抛给 UI 层 toast，不静默吞掉。
 */
class ChatViewModel(
    private val repository: ConversationRepository,
    private val context: Context,
    private val conversationId: String,
) : ViewModel() {

    val conversation: StateFlow<Conversation?> = repository.conversation(conversationId)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    val messages: StateFlow<List<Message>> = repository.messages(conversationId)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyList())

    /** UI toast 事件（网络错误 / 录音失败 / 清空完成等）。 */
    private val _events = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val events: SharedFlow<String> = _events

    /** 等待后端回复中：头部「正在输入…」+ 最后一条我方消息显示「发送中…」。 */
    var typing by mutableStateOf(false)
        private set

    var recording by mutableStateOf(false)
        private set

    var recordSeconds by mutableIntStateOf(0)
        private set

    /** 正在播放的语音消息 id；null 表示没有播放。 */
    var playingVoiceId by mutableStateOf<String?>(null)
        private set

    private var pendingSends = 0
    private var recorder: MediaRecorder? = null
    private var recordingFile: File? = null
    private var recordStartedAt = 0L
    private var lastRecordedSeconds = 0
    private var timerJob: Job? = null
    private var mediaPlayer: MediaPlayer? = null
    private var simulatedPlayback: Job? = null

    init {
        viewModelScope.launch {
            runCatching { repository.openConversation(conversationId) }
        }
    }

    // ─── 文本发送 ─────────────────────────────────────────────────────

    /**
     * 发送文本到 Agent（peer_id = android:<convId>，仓库内部处理）。
     * 本地乐观消息由仓库立即插入；等待期间 typing=true，失败抛 AgentException 并保留 FAILED 状态。
     */
    fun sendText(raw: String) {
        val content = raw.trim()
        if (content.isEmpty()) return
        // 「要语音 / 发条语音」这类消息：让后端把回复合成语音发回来。
        val voiceReply = WANTS_VOICE.any { content.contains(it) }
        viewModelScope.launch {
            pendingSends++
            typing = true
            try {
                repository.sendText(conversationId, content, voiceReply)
            } catch (e: AgentException) {
                _events.tryEmit(e.userMessage)
            } catch (e: Exception) {
                _events.tryEmit(e.message?.takeIf { it.isNotBlank() } ?: "发送失败，请稍后重试")
            } finally {
                pendingSends = (pendingSends - 1).coerceAtLeast(0)
                typing = pendingSends > 0
            }
        }
    }

    // ─── 附件发送 ─────────────────────────────────────────────────────

    /** 文件选择 sheet 的三个示例文件。 */
    fun sendFile(template: StockMedia.FileTemplate) {
        viewModelScope.launch {
            runCatching {
                repository.sendAttachment(
                    conversationId,
                    Message.File(
                        id = UUID.randomUUID().toString(),
                        sender = MessageSender.ME,
                        fileName = template.name,
                        sizeLabel = template.sizeLabel,
                    ),
                )
            }.onFailure { _events.tryEmit("文件发送失败，请稍后重试") }
        }
    }

    /**
     * Photo Picker 选中的图片：拷贝到应用私有目录（避免 picker 临时授权过期），
     * 然后作为我方图片消息插入。asset 存文件绝对路径，UI 侧按路径加载。
     */
    fun sendPickedImage(uri: Uri) {
        viewModelScope.launch {
            val path = withContext(Dispatchers.IO) {
                runCatching {
                    val dir = File(context.filesDir, "chat_images").apply { mkdirs() }
                    val file = File(dir, "img_${System.currentTimeMillis()}.jpg")
                    context.contentResolver.openInputStream(uri)?.use { input ->
                        file.outputStream().use { output -> input.copyTo(output) }
                    } ?: error("openInputStream 返回 null")
                    file.takeIf { it.isFile && it.length() > 0 }?.absolutePath
                }.getOrNull()
            }
            if (path == null) {
                _events.tryEmit("无法读取所选图片，请重试")
                return@launch
            }
            runCatching {
                repository.sendAttachment(
                    conversationId,
                    Message.Image(
                        id = UUID.randomUUID().toString(),
                        sender = MessageSender.ME,
                        asset = path,
                    ),
                )
            }.onFailure { _events.tryEmit("图片发送失败，请稍后重试") }
        }
    }

    /** 系统相机拍好的照片（缓存里的临时文件）：转存到 chat_images 并作为我方图片发送。 */
    fun sendCapturedPhoto(cacheFile: File) {
        viewModelScope.launch {
            val path = withContext(Dispatchers.IO) {
                runCatching {
                    val dir = File(context.filesDir, "chat_images").apply { mkdirs() }
                    val target = File(dir, "photo_${System.currentTimeMillis()}.jpg")
                    cacheFile.copyTo(target, overwrite = true)
                    cacheFile.delete()
                    target.takeIf { it.isFile && it.length() > 0 }?.absolutePath
                }.getOrNull()
            }
            if (path == null) {
                _events.tryEmit("读取照片失败，请重试")
                return@launch
            }
            runCatching {
                repository.sendAttachment(
                    conversationId,
                    Message.Image(
                        id = UUID.randomUUID().toString(),
                        sender = MessageSender.ME,
                        asset = path,
                    ),
                )
            }.onFailure { _events.tryEmit("照片发送失败，请稍后重试") }
        }
    }

    // ─── 录音 ─────────────────────────────────────────────────────────

    /** 开始录音。失败（设备/权限异常）时返回 false 并抛出 toast 事件。 */
    fun startRecording(): Boolean {
        if (recording) return true
        val file = VoiceFileStore.newRecordingFile(context, conversationId)
        return try {
            val r = newRecorder()
            recorder = r
            r.setAudioSource(MediaRecorder.AudioSource.MIC)
            r.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
            r.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
            r.setAudioEncodingBitRate(64_000)
            r.setAudioSamplingRate(44_100)
            r.setOutputFile(file.absolutePath)
            r.prepare()
            r.start()
            recordingFile = file
            recordStartedAt = SystemClock.elapsedRealtime()
            recordSeconds = 0
            recording = true
            timerJob = viewModelScope.launch {
                while (isActive) {
                    delay(200)
                    recordSeconds = ((SystemClock.elapsedRealtime() - recordStartedAt) / 1000).toInt()
                }
            }
            true
        } catch (e: Exception) {
            runCatching { recorder?.release() }
            recorder = null
            recordingFile = null
            file.delete()
            recording = false
            _events.tryEmit("无法开始录音，请检查麦克风权限")
            false
        }
    }

    /** 录音发送：上传后端走 ASR → LLM → TTS，返回可播放的语音回复。 */
    fun stopRecordingAndSend() {
        val file = finishRecording()
        if (file == null) {
            _events.tryEmit("录音太短或没有录到声音，请重试")
            return
        }
        val seconds = max(1, lastRecordedSeconds)
        viewModelScope.launch {
            pendingSends++
            typing = true
            try {
                repository.sendVoice(conversationId, file, seconds)
            } catch (e: AgentException) {
                _events.tryEmit(e.userMessage)
            } catch (e: Exception) {
                _events.tryEmit(e.message?.takeIf { it.isNotBlank() } ?: "语音发送失败，请稍后重试")
            } finally {
                pendingSends = (pendingSends - 1).coerceAtLeast(0)
                typing = pendingSends > 0
            }
        }
    }

    /** 录音取消：停止并删除临时文件，不产生消息。 */
    fun cancelRecording() {
        val file = finishRecording(keepFile = false)
        file?.delete()
    }

    /**
     * 结束录音并释放 MediaRecorder。
     * @return 可用的录音文件；停止失败 / 文件为空 / 取消录音时返回 null（文件已删除）。
     */
    private fun finishRecording(keepFile: Boolean = true): File? {
        if (!recording) return null
        lastRecordedSeconds = ((SystemClock.elapsedRealtime() - recordStartedAt) / 1000).toInt()
        timerJob?.cancel()
        timerJob = null
        val file = recordingFile
        val stopped = runCatching { recorder?.stop() }.isSuccess
        runCatching { recorder?.release() }
        recorder = null
        recordingFile = null
        recording = false
        recordSeconds = 0
        if (!stopped || file == null || !file.isFile || file.length() == 0L) {
            file?.delete()
            return null
        }
        if (!keepFile) {
            file.delete()
            return null
        }
        return file
    }

    @Suppress("DEPRECATION")
    private fun newRecorder(): MediaRecorder =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) MediaRecorder(context) else MediaRecorder()

    // ─── 语音播放 ─────────────────────────────────────────────────────

    /** 点击语音气泡：播放 / 停止；同一时间只播放一条。 */
    fun toggleVoice(message: Message.Voice) {
        if (playingVoiceId == message.id) {
            stopPlayback()
            return
        }
        stopPlayback()
        if (VoiceFileStore.isPlayable(message)) {
            val started = runCatching {
                val player = MediaPlayer()
                player.setDataSource(message.filePath)
                player.setOnCompletionListener { stopPlayback() }
                player.setOnErrorListener { _, _, _ ->
                    stopPlayback()
                    true
                }
                player.prepare()
                player.start()
                mediaPlayer = player
            }.isSuccess
            if (started) {
                playingVoiceId = message.id
                return
            }
            _events.tryEmit("语音播放失败，请重试")
        }
        // 种子演示语音没有真实文件：只跑波形动画，按消息时长自动停止。
        playingVoiceId = message.id
        simulatedPlayback = viewModelScope.launch {
            delay(message.durationSeconds.coerceAtLeast(1) * 1000L)
            if (playingVoiceId == message.id) playingVoiceId = null
        }
    }

    fun stopPlayback() {
        simulatedPlayback?.cancel()
        simulatedPlayback = null
        mediaPlayer?.let { player ->
            runCatching { if (player.isPlaying) player.stop() }
            runCatching { player.release() }
        }
        mediaPlayer = null
        playingVoiceId = null
    }

    // ─── 清空聊天 ─────────────────────────────────────────────────────

    fun clearHistory() {
        viewModelScope.launch {
            runCatching { repository.clearMessages(conversationId) }
                .onSuccess {
                    stopPlayback()
                    _events.tryEmit("聊天记录已清空")
                }
                .onFailure { _events.tryEmit("清空失败，请稍后重试") }
        }
    }

    override fun onCleared() {
        stopPlayback()
        timerJob?.cancel()
        timerJob = null
        runCatching { recorder?.stop() }
        runCatching { recorder?.release() }
        recorder = null
        recordingFile?.delete()
        recordingFile = null
        super.onCleared()
    }
}

private val WANTS_VOICE = listOf(
    "要语音", "发语音", "语音回", "用语音", "发条语音", "发个语音", "语音消息", "说给我听",
)
