package com.bnagent.shiyu.data

import android.content.Context
import androidx.compose.runtime.staticCompositionLocalOf
import com.bnagent.shiyu.data.local.ChatStore
import com.bnagent.shiyu.data.local.SettingsStore
import com.bnagent.shiyu.data.remote.AgentClient
import com.bnagent.shiyu.data.repository.ConversationRepository
import com.bnagent.shiyu.data.repository.SettingsRepository
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * 手写依赖容器（Application 持有，进程内单例）。
 *
 * - [chatStore] / [settingsStore]：两个独立的 DataStore Preferences 文件；
 * - [agentClient]：OkHttp 客户端（60s 超时，可选 API Key）；
 * - [settingsRepository] / [conversationRepository]：暴露 suspend + Flow；
 * - [initialize]：启动时写入首装种子（幂等）。
 *
 * UI / ViewModel 取用方式：
 * ```
 * val container = LocalAppContainer.current          // Composable
 * val repo = container.conversationRepository
 * // 或 (context.applicationContext as ShiYuApplication).container
 * ```
 */
class AppContainer(context: Context) {

    val applicationContext: Context = context.applicationContext

    private val applicationScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    val chatStore: ChatStore = ChatStore(applicationContext)

    val settingsStore: SettingsStore = SettingsStore(applicationContext)

    val agentClient: AgentClient = AgentClient()

    val settingsRepository: SettingsRepository = SettingsRepository(settingsStore)

    val conversationRepository: ConversationRepository = ConversationRepository(
        store = chatStore,
        settingsRepository = settingsRepository,
        agentClient = agentClient,
        appContext = applicationContext,
    )

    /** 首装种子写入（首次启动生效，后续启动无操作）。 */
    fun initialize() {
        applicationScope.launch {
            chatStore.ensureSeeded()
        }
    }
}

/** 由 MainActivity 在根部提供；UI 页面通过 `LocalAppContainer.current` 取仓库。 */
val LocalAppContainer = staticCompositionLocalOf<AppContainer> {
    error("LocalAppContainer 未提供：请在 MainActivity 根部注入 ShiYuApplication.container")
}
