package com.bnagent.shiyu.ui.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.viewmodel.compose.viewModel
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.domain.model.ServerSettings
import com.bnagent.shiyu.ui.components.LocalToast
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.components.SmallTitleBar
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType

/**
 * 服务器设置：Base URL（默认取数据层 [ServerSettings.DEFAULT_BASE_URL]）、
 * API Key（可空、密码态）、测试连接（/api/health）、保存。
 */
@Composable
fun ServerSettingsScreen(
    onBack: () -> Unit,
) {
    val container = LocalAppContainer.current
    val viewModel: ServerSettingsViewModel = viewModel(
        factory = ServerSettingsViewModel.factory(
            repository = container.settingsRepository,
            agentClient = container.agentClient,
        ),
    )
    val toast = LocalToast.current
    val colors = ShiYuTheme.colors

    Column(Modifier.fillMaxSize().background(colors.bg)) {
        SmallTitleBar(title = "服务器设置", onBack = onBack)
        Column(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .imePadding()
                .padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            FieldsCard(viewModel = viewModel)

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                SecondaryButton(
                    text = "测试连接",
                    onClick = viewModel::testConnection,
                    modifier = Modifier.weight(1f),
                    enabled = viewModel.testState != ServerSettingsViewModel.TestState.Testing,
                )
                PrimaryButton(
                    text = if (viewModel.saving) "保存中…" else "保存",
                    onClick = {
                        viewModel.save { toast?.show("服务器设置已保存") }
                    },
                    modifier = Modifier.weight(1f),
                    enabled = !viewModel.saving,
                )
            }

            TestResult(viewModel.testState)

            HintCard()
        }
    }
}

// ─── 表单卡片 ───────────────────────────────────────────────────────

@Composable
private fun FieldsCard(viewModel: ServerSettingsViewModel) {
    val colors = ShiYuTheme.colors
    val shape = RoundedCornerShape(18.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, colors.line, shape)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            SettingsFieldLabel("Base URL")
            SettingsTextField(
                value = viewModel.baseUrl,
                onValueChange = viewModel::onBaseUrlChange,
                placeholder = ServerSettings.DEFAULT_BASE_URL,
                keyboardType = KeyboardType.Uri,
                imeAction = ImeAction.Next,
            )
            viewModel.baseUrlError?.let { error ->
                Text(
                    text = error,
                    color = colors.danger,
                    fontSize = 12.sp,
                    lineHeight = 17.sp,
                )
            }
        }
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            SettingsFieldLabel("API Key")
            SettingsTextField(
                value = viewModel.apiKey,
                onValueChange = viewModel::onApiKeyChange,
                placeholder = "可留空",
                password = true,
                keyboardType = KeyboardType.Password,
                imeAction = ImeAction.Done,
            )
            Text(
                text = "留空时不携带鉴权头；填写后同时发送 Authorization: Bearer 与 X-API-Key。",
                color = colors.ink3,
                style = ShiYuType.settingSubtitle,
            )
        }
    }
}

// ─── 测试结果 ───────────────────────────────────────────────────────

@Composable
private fun TestResult(state: ServerSettingsViewModel.TestState) {
    val colors = ShiYuTheme.colors
    when (state) {
        ServerSettingsViewModel.TestState.Idle -> Unit
        ServerSettingsViewModel.TestState.Testing -> {
            Row(verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(
                    modifier = Modifier.size(14.dp),
                    strokeWidth = 2.dp,
                    color = colors.accent,
                )
                Spacer(Modifier.width(8.dp))
                Text(
                    text = "正在测试连接…",
                    color = colors.ink2,
                    fontSize = 12.5.sp,
                    fontWeight = FontWeight.Medium,
                    lineHeight = 18.sp,
                )
            }
        }

        is ServerSettingsViewModel.TestState.Success -> {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = ShiYuIcons.Check,
                    contentDescription = null,
                    tint = colors.presence,
                    modifier = Modifier.size(16.dp),
                )
                Spacer(Modifier.width(6.dp))
                Text(
                    text = state.message,
                    color = colors.presence,
                    fontSize = 12.5.sp,
                    fontWeight = FontWeight.SemiBold,
                    lineHeight = 18.sp,
                )
            }
        }

        is ServerSettingsViewModel.TestState.Failure -> {
            Row(verticalAlignment = Alignment.Top) {
                Icon(
                    imageVector = ShiYuIcons.Info,
                    contentDescription = null,
                    tint = colors.danger,
                    modifier = Modifier.padding(top = 1.dp).size(16.dp),
                )
                Spacer(Modifier.width(6.dp))
                Text(
                    text = state.message,
                    color = colors.danger,
                    fontSize = 12.5.sp,
                    fontWeight = FontWeight.SemiBold,
                    lineHeight = 18.sp,
                )
            }
        }
    }
}

// ─── 真机调试说明 ───────────────────────────────────────────────────

@Composable
private fun HintCard() {
    val colors = ShiYuTheme.colors
    val shape = RoundedCornerShape(18.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, colors.line, shape)
            .padding(14.dp),
    ) {
        Text(
            text = "真机调试提示",
            color = colors.ink2,
            fontSize = 12.5.sp,
            fontWeight = FontWeight.Bold,
            lineHeight = 18.sp,
        )
        Text(
            text = "手机通过 USB 连接时，先执行 adb reverse tcp:18080 tcp:18080，" +
                "再把 Base URL 改为 http://127.0.0.1:18080，即可连到本机后端。",
            modifier = Modifier.padding(top = 4.dp),
            color = colors.ink3,
            fontSize = 12.5.sp,
            lineHeight = 18.sp,
        )
        Text(
            text = "默认地址：${ServerSettings.DEFAULT_BASE_URL}",
            modifier = Modifier.padding(top = 6.dp),
            color = colors.ink3,
            fontSize = 11.5.sp,
            lineHeight = 16.sp,
        )
    }
}

// ─── 按钮 ───────────────────────────────────────────────────────────

@Composable
private fun PrimaryButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val colors = ShiYuTheme.colors
    Box(
        modifier = modifier
            .height(46.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(if (enabled) colors.accent else colors.accent.copy(alpha = 0.5f))
            .clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            color = colors.onAccent,
            style = ShiYuType.button,
        )
    }
}

@Composable
private fun SecondaryButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val colors = ShiYuTheme.colors
    Box(
        modifier = modifier
            .height(46.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(colors.surface2)
            .border(1.dp, colors.line, RoundedCornerShape(14.dp))
            .clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            color = if (enabled) colors.ink else colors.ink3,
            style = ShiYuType.button,
        )
    }
}
