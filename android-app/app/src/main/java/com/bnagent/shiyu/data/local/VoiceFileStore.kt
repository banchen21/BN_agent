package com.bnagent.shiyu.data.local

import android.content.Context
import java.io.File

/**
 * 语音录音文件（应用私有目录 `files/voice/`）。
 *
 * 团队决策：语音消息保存真实录音文件，消息模型用 [com.bnagent.shiyu.domain.model.Message.Voice.filePath]
 * 记录本地路径与时长；通话不落库。
 */
object VoiceFileStore {

    fun voiceDir(context: Context): File =
        File(context.applicationContext.filesDir, "voice").apply { mkdirs() }

    /** 为某个会话生成一个新的录音文件（m4a/aac 由录音端决定编码）。 */
    fun newRecordingFile(context: Context, conversationId: String): File {
        val safeId = conversationId.replace(Regex("[^A-Za-z0-9_-]"), "_")
        return File(voiceDir(context), "voice_${safeId}_${System.currentTimeMillis()}.m4a")
    }

    /** 保存后端 TTS 返回的 WAV（Agent 语音回复）。 */
    fun saveAgentReply(context: Context, conversationId: String, bytes: ByteArray): File {
        val safeId = conversationId.replace(Regex("[^A-Za-z0-9_-]"), "_")
        val file = File(voiceDir(context), "reply_${safeId}_${System.currentTimeMillis()}.wav")
        file.writeBytes(bytes)
        return file
    }

    /** 文件是否还存在且非空（UI 播放前可校验）。 */
    fun isPlayable(message: com.bnagent.shiyu.domain.model.Message.Voice): Boolean {
        val path = message.filePath ?: return false
        val file = File(path)
        return file.isFile && file.length() > 0
    }
}
