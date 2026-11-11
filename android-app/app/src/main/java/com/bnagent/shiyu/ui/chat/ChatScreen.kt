package com.bnagent.shiyu.ui.chat

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.domain.model.Message
import com.bnagent.shiyu.domain.model.headerSubtitle
import com.bnagent.shiyu.ui.components.ChatHeader
import com.bnagent.shiyu.ui.components.LocalToast
import com.bnagent.shiyu.ui.components.ShiYuConfirmDialog
import com.bnagent.shiyu.ui.theme.ShiYuTheme

/**
 * 聊天页（设计规格 4.2）。
 *
 * - 顶栏：返回 / 头像名称 / 在线或群聊人数 / 语音视频（群聊隐藏）/ 更多；
 * - 消息区：日期分隔、四种气泡、群聊 sender 名、已读状态、正在输入、滚动到底、空会话提示；
 * - 输入区：文字发送走后端 Agent，真实录音 / Photo Picker 选图；
 * - 浮层：发送 / 文件 / 聊天设置 / 清空确认 / 图片查看器。
 */
@Composable
fun ChatScreen(
    id: String,
    onBack: () -> Unit,
    onOpenProfile: () -> Unit,
    onOpenTheme: () -> Unit,
    onVoiceCall: () -> Unit,
    onVideoCall: () -> Unit,
) {
    val container = LocalAppContainer.current
    val appContext = LocalContext.current.applicationContext
    val vm: ChatViewModel = viewModel(
        key = "chat:$id",
        factory = viewModelFactory {
            initializer {
                ChatViewModel(
                    repository = container.conversationRepository,
                    context = appContext,
                    conversationId = id,
                )
            }
        },
    )
    val conversation by vm.conversation.collectAsStateWithLifecycle()
    val messages by vm.messages.collectAsStateWithLifecycle()
    val toast = LocalToast.current
    LaunchedEffect(vm) {
        vm.events.collect { message -> toast?.show(message) }
    }

    var text by rememberSaveable(id) { mutableStateOf("") }
    var attachSheet by rememberSaveable { mutableStateOf(false) }
    var fileSheet by rememberSaveable { mutableStateOf(false) }
    var settingsSheet by rememberSaveable { mutableStateOf(false) }
    var clearDialog by rememberSaveable { mutableStateOf(false) }
    var viewer by remember { mutableStateOf<Message.Image?>(null) }

    // Photo Picker：系统相册选图，无需读写权限。
    val imagePicker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri != null) vm.sendPickedImage(uri)
    }
    val micPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) {
            vm.startRecording()
        } else {
            toast?.show("需要麦克风权限才能录音，请在系统设置中开启")
        }
    }
    val beginRecording: () -> Unit = {
        val granted = ContextCompat.checkSelfPermission(
            appContext,
            Manifest.permission.RECORD_AUDIO,
        ) == PackageManager.PERMISSION_GRANTED
        if (granted) {
            vm.startRecording()
        } else {
            micPermission.launch(Manifest.permission.RECORD_AUDIO)
        }
    }

    Column(Modifier.fillMaxSize().background(ShiYuTheme.colors.chatBg)) {
        ChatHeader(
            name = conversation?.name ?: id,
            subtitle = if (vm.typing) "正在输入…" else conversation?.headerSubtitle.orEmpty(),
            onBack = onBack,
            avatarModel = conversation?.let { StockMedia.drawableFor(it.avatar) },
            online = conversation?.online == true,
            typing = vm.typing,
            isGroup = conversation?.isGroup == true,
            onOpenProfile = onOpenProfile,
            onVoiceCall = onVoiceCall,
            onVideoCall = onVideoCall,
            onMore = { settingsSheet = true },
        )
        Box(Modifier.weight(1f).fillMaxWidth().chatWallpaper()) {
            val conv = conversation
            when {
                conv == null -> Unit
                messages.isEmpty() -> EmptyChatState(conv.name)
                else -> MessageList(
                    messages = messages,
                    conversation = conv,
                    typing = vm.typing,
                    pendingReply = vm.typing,
                    playingVoiceId = vm.playingVoiceId,
                    onImageClick = { viewer = it },
                    onVoiceClick = { vm.toggleVoice(it) },
                    onDownloadClick = { toast?.show("演示环境，暂不下载") },
                )
            }
        }
        Composer(
            text = text,
            onTextChange = { text = it },
            onSend = {
                vm.sendText(text)
                text = ""
            },
            recording = vm.recording,
            recordSeconds = vm.recordSeconds,
            onOpenAttach = { attachSheet = true },
            onStartRecording = beginRecording,
            onCancelRecording = { vm.cancelRecording() },
            onSendRecording = { vm.stopRecordingAndSend() },
        )
    }

    if (attachSheet) {
        AttachSheet(
            onDismiss = { attachSheet = false },
            onPickImage = {
                attachSheet = false
                imagePicker.launch(
                    PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly),
                )
            },
            onPickFile = {
                attachSheet = false
                fileSheet = true
            },
            onRecordVoice = {
                attachSheet = false
                beginRecording()
            },
        )
    }

    if (fileSheet) {
        FilePickerSheet(
            files = StockMedia.files,
            onDismiss = { fileSheet = false },
            onPick = { template ->
                fileSheet = false
                vm.sendFile(template)
            },
        )
    }

    if (settingsSheet) {
        ChatSettingsSheet(
            onDismiss = { settingsSheet = false },
            onOpenTheme = {
                settingsSheet = false
                onOpenTheme()
            },
            onClearClick = {
                settingsSheet = false
                clearDialog = true
            },
        )
    }

    if (clearDialog) {
        ShiYuConfirmDialog(
            title = "清空聊天记录？",
            message = "当前会话里的所有消息会被删除，且无法恢复。这个操作不会影响其他会话。",
            confirmLabel = "清空",
            onConfirm = {
                clearDialog = false
                vm.clearHistory()
            },
            onDismiss = { clearDialog = false },
        )
    }

    viewer?.let { image ->
        ImageViewer(message = image, onDismiss = { viewer = null })
    }
}
