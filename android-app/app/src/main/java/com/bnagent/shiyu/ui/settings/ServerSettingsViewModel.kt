package com.bnagent.shiyu.ui.settings

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.bnagent.shiyu.data.remote.AgentClient
import com.bnagent.shiyu.data.remote.AgentException
import com.bnagent.shiyu.data.repository.SettingsRepository
import com.bnagent.shiyu.domain.model.ServerSettings
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

/**
 * 服务器设置页状态：Base URL / API Key 编辑、连接测试、保存。
 *
 * 默认地址来自数据层 [ServerSettings.DEFAULT_BASE_URL]（当前为本机 BN_agent 后端），
 * 不在 UI 里写死。
 */
class ServerSettingsViewModel(
    private val repository: SettingsRepository,
    private val agentClient: AgentClient,
) : ViewModel() {

    var baseUrl by mutableStateOf(ServerSettings.DEFAULT_BASE_URL)
        private set
    var apiKey by mutableStateOf("")
        private set
    var baseUrlError by mutableStateOf<String?>(null)
        private set
    var testState by mutableStateOf<TestState>(TestState.Idle)
        private set
    var saving by mutableStateOf(false)
        private set

    init {
        viewModelScope.launch {
            val saved = repository.serverSettings.first()
            baseUrl = saved.baseUrl
            apiKey = saved.apiKey
        }
    }

    fun onBaseUrlChange(value: String) {
        baseUrl = value
        baseUrlError = null
        testState = TestState.Idle
    }

    fun onApiKeyChange(value: String) {
        apiKey = value
        testState = TestState.Idle
    }

    /** 测试 /api/health；结果区分成功、401 与连接失败。 */
    fun testConnection() {
        val validation = validate(baseUrl)
        if (validation != null) {
            baseUrlError = validation
            return
        }
        if (testState == TestState.Testing) return
        testState = TestState.Testing
        viewModelScope.launch {
            testState = try {
                val info = agentClient.health(
                    ServerSettings(baseUrl = baseUrl.trim(), apiKey = apiKey.trim()),
                )
                if (info.ok) {
                    TestState.Success(info.summary)
                } else {
                    TestState.Failure(info.summary)
                }
            } catch (error: AgentException) {
                TestState.Failure(error.describe())
            } catch (error: Exception) {
                TestState.Failure("测试失败：${error.message ?: "未知错误"}")
            }
        }
    }

    fun save(onSaved: () -> Unit) {
        val validation = validate(baseUrl)
        if (validation != null) {
            baseUrlError = validation
            return
        }
        if (saving) return
        saving = true
        viewModelScope.launch {
            repository.updateServerSettings(
                ServerSettings(baseUrl = baseUrl.trim(), apiKey = apiKey.trim()),
            )
            saving = false
            onSaved()
        }
    }

    private fun validate(raw: String): String? {
        val trimmed = raw.trim()
        return when {
            trimmed.isEmpty() -> "请填写 Base URL"
            !trimmed.startsWith("http://", ignoreCase = true) &&
                !trimmed.startsWith("https://", ignoreCase = true) ->
                "地址需要以 http:// 或 https:// 开头"
            else -> null
        }
    }

    private fun AgentException.describe(): String = when (this) {
        is AgentException.Unauthorized -> "鉴权失败（HTTP $code）：$userMessage"
        is AgentException.Network -> "连接失败：$userMessage"
        is AgentException.Timeout -> "测试超时：$userMessage"
        is AgentException.InvalidConfig -> userMessage
        is AgentException.ClientError -> "请求失败（HTTP $code）：$serverMessage"
        is AgentException.ServerError -> "服务器错误（HTTP $code）：$serverMessage"
        is AgentException.InvalidResponse -> userMessage
    }

    sealed interface TestState {
        data object Idle : TestState
        data object Testing : TestState
        data class Success(val message: String) : TestState
        data class Failure(val message: String) : TestState
    }

    companion object {
        fun factory(
            repository: SettingsRepository,
            agentClient: AgentClient,
        ) = viewModelFactory {
            initializer { ServerSettingsViewModel(repository, agentClient) }
        }
    }
}
