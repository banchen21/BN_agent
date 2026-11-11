package com.bnagent.shiyu.ui.me

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.domain.model.UserProfile
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.LargeTitleBar
import com.bnagent.shiyu.ui.components.LocalToast
import com.bnagent.shiyu.ui.components.SheetCancelButton
import com.bnagent.shiyu.ui.components.SheetDescription
import com.bnagent.shiyu.ui.components.SheetHandle
import com.bnagent.shiyu.ui.components.SheetTitle
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.components.ShiYuSheet
import com.bnagent.shiyu.ui.components.ShiYuSwitch
import com.bnagent.shiyu.ui.components.ShiYuConfirmDialog
import com.bnagent.shiyu.ui.settings.SettingsFieldLabel
import com.bnagent.shiyu.ui.settings.SettingsTextField
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType

/**
 * 「我」（设计规格 4.6）：资料卡 + 菜单组（气泡与主题 / 深色模式 / 聊天通知 / 关于拾语）
 * + 服务器设置入口 + 版本脚注。
 *
 * 深色模式为应用内手动开关，默认浅色，切换立即全局生效并持久化。
 */
@Composable
fun MeScreen(
    onOpenTheme: () -> Unit,
    onOpenServerSettings: () -> Unit,
) {
    val container = LocalAppContainer.current
    val viewModel: MeViewModel = viewModel(
        factory = MeViewModel.factory(container.conversationRepository, container.settingsRepository),
    )
    val profile by viewModel.profile.collectAsStateWithLifecycle()
    val notificationsEnabled by viewModel.notificationsEnabled.collectAsStateWithLifecycle()
    val serverSettings by viewModel.serverSettings.collectAsStateWithLifecycle()
    val cacheBytes by viewModel.cacheBytes.collectAsStateWithLifecycle()
    val settings = ShiYuTheme.settings
    val themeController = ShiYuTheme.controller
    val toast = LocalToast.current
    val colors = ShiYuTheme.colors
    var showAbout by remember { mutableStateOf(false) }
    var showEditProfile by remember { mutableStateOf(false) }
    var showClearCache by remember { mutableStateOf(false) }

    LaunchedEffect(Unit) { viewModel.refreshCacheSize() }

    Column(Modifier.fillMaxSize().background(colors.bg)) {
        LargeTitleBar(title = "我")
        Column(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 26.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            MeProfileCard(profile = profile)

            MenuGroup {
                MenuRow(
                    icon = ShiYuIcons.Palette,
                    title = "气泡与主题",
                    value = settings.summary,
                    onClick = onOpenTheme,
                    divider = true,
                )
                MenuSwitchRow(
                    icon = ShiYuIcons.Moon,
                    title = "深色模式",
                    checked = settings.darkTheme,
                    divider = true,
                ) { dark ->
                    themeController.setDarkTheme(dark)
                    toast?.show(if (dark) "已切换到深色模式" else "已切换到浅色模式")
                }
                MenuSwitchRow(
                    icon = ShiYuIcons.Bell,
                    title = "聊天通知",
                    checked = notificationsEnabled,
                    divider = true,
                ) { enabled ->
                    viewModel.setNotificationsEnabled(enabled)
                    toast?.show(if (enabled) "聊天通知已开启" else "聊天通知已关闭")
                }
                MenuRow(
                    icon = ShiYuIcons.Info,
                    title = "关于拾语",
                    value = "v2.4.0",
                    onClick = { showAbout = true },
                    divider = false,
                )
            }

            MenuGroup {
                MenuRow(
                    icon = ShiYuIcons.Me,
                    title = "编辑资料",
                    value = profile.name,
                    onClick = { showEditProfile = true },
                    divider = true,
                )
                MenuRow(
                    icon = ShiYuIcons.File,
                    title = "清理缓存",
                    value = formatBytes(cacheBytes),
                    onClick = { showClearCache = true },
                    divider = false,
                )
            }

            MenuGroup {
                MenuRow(
                    icon = ShiYuIcons.Lock,
                    title = "服务器设置",
                    value = serverSettings.baseUrl,
                    onClick = onOpenServerSettings,
                    divider = false,
                )
            }

            Text(
                text = "拾语 · 即时通讯演示原型",
                modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
                color = colors.ink3,
                fontSize = 11.5.sp,
                lineHeight = 16.sp,
                textAlign = TextAlign.Center,
            )
        }
    }

    if (showAbout) {
        AboutSheet(onDismiss = { showAbout = false })
    }
    if (showEditProfile) {
        EditProfileSheet(
            profile = profile,
            onDismiss = { showEditProfile = false },
            onSave = { name, signature, avatar ->
                viewModel.updateProfile(name, signature, avatar)
                showEditProfile = false
                toast?.show("资料已更新")
            },
        )
    }
    if (showClearCache) {
        ShiYuConfirmDialog(
            title = "清理缓存？",
            message = "将删除本地语音文件（${formatBytes(cacheBytes)}），不影响聊天记录与后端数据。",
            confirmLabel = "清理",
            onConfirm = {
                viewModel.clearCache()
                showClearCache = false
                toast?.show("缓存已清理")
            },
            onDismiss = { showClearCache = false },
        )
    }
}

/** 字节数 → `0 B` / `12.3 KB` / `4.5 MB`。 */
private fun formatBytes(bytes: Long): String = when {
    bytes <= 0 -> "0 B"
    bytes < 1024 -> "$bytes B"
    bytes < 1024 * 1024 -> String.format(java.util.Locale.ROOT, "%.1f KB", bytes / 1024.0)
    else -> String.format(java.util.Locale.ROOT, "%.1f MB", bytes / (1024.0 * 1024.0))
}

// ─── 资料卡 ─────────────────────────────────────────────────────────

@Composable
private fun MeProfileCard(profile: UserProfile) {
    val colors = ShiYuTheme.colors
    val shape = RoundedCornerShape(22.dp)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, colors.line, shape)
            .padding(horizontal = 16.dp, vertical = 18.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(
            name = profile.name,
            size = 64.dp,
            model = StockMedia.drawableFor(profile.avatar),
        )
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(text = profile.name, color = colors.ink, style = ShiYuType.meName)
            Text(
                text = profile.handleLine,
                modifier = Modifier.padding(top = 2.dp),
                color = colors.ink3,
                fontSize = 12.sp,
                lineHeight = 17.sp,
            )
            Text(
                text = profile.signature,
                modifier = Modifier.padding(top = 2.dp),
                color = colors.ink2,
                fontSize = 12.5.sp,
                lineHeight = 18.125.sp,
            )
        }
    }
}

// ─── 菜单组 ─────────────────────────────────────────────────────────

@Composable
private fun MenuGroup(content: @Composable ColumnScope.() -> Unit) {
    val colors = ShiYuTheme.colors
    val shape = RoundedCornerShape(18.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, colors.line, shape),
        content = content,
    )
}

@Composable
private fun MenuRow(
    icon: ImageVector,
    title: String,
    value: String?,
    onClick: () -> Unit,
    divider: Boolean,
) {
    val colors = ShiYuTheme.colors
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 15.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = colors.ink2,
            modifier = Modifier.size(20.dp),
        )
        Spacer(Modifier.width(12.dp))
        Text(
            text = title,
            modifier = Modifier.weight(1f),
            color = colors.ink,
            style = MenuTitleStyle,
        )
        if (!value.isNullOrEmpty()) {
            Text(
                text = value,
                modifier = Modifier.widthIn(max = 160.dp),
                color = colors.ink3,
                style = MenuValueStyle,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.End,
            )
            Spacer(Modifier.width(4.dp))
        }
        Icon(
            imageVector = ShiYuIcons.Chevron,
            contentDescription = null,
            tint = colors.ink3,
            modifier = Modifier.size(17.dp),
        )
    }
    if (divider) {
        MenuDivider()
    }
}

@Composable
private fun MenuSwitchRow(
    icon: ImageVector,
    title: String,
    checked: Boolean,
    divider: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    val colors = ShiYuTheme.colors
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 15.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = colors.ink2,
            modifier = Modifier.size(20.dp),
        )
        Spacer(Modifier.width(12.dp))
        Text(
            text = title,
            modifier = Modifier.weight(1f),
            color = colors.ink,
            style = MenuTitleStyle,
        )
        ShiYuSwitch(checked = checked, onCheckedChange = onCheckedChange)
    }
    if (divider) {
        MenuDivider()
    }
}

@Composable
private fun MenuDivider() {
    Box(Modifier.fillMaxWidth().height(1.dp).background(ShiYuTheme.colors.line))
}

private val MenuTitleStyle = TextStyle(
    fontSize = 15.sp,
    fontWeight = FontWeight.SemiBold,
    lineHeight = 21.sp,
)

private val MenuValueStyle = TextStyle(
    fontSize = 12.sp,
    fontWeight = FontWeight.Medium,
    lineHeight = 17.sp,
)

// ─── 编辑资料 sheet ──────────────────────────────────────────────────

@Composable
private fun EditProfileSheet(
    profile: UserProfile,
    onDismiss: () -> Unit,
    onSave: (name: String, signature: String, avatar: String) -> Unit,
) {
    var name by remember(profile.name) { mutableStateOf(profile.name) }
    var signature by remember(profile.signature) { mutableStateOf(profile.signature) }
    var avatar by remember(profile.avatar) { mutableStateOf(profile.avatar) }
    val colors = ShiYuTheme.colors
    val choices = listOf("avatar_me", "avatar_lin", "avatar_chen", "avatar_zhao")

    ShiYuSheet(onDismissRequest = onDismiss) {
        SheetHandle()
        SheetTitle("编辑资料")
        SheetDescription("昵称、签名与头像保存在本地，聊天和通话都会使用。")
        SettingsFieldLabel("昵称")
        Spacer(Modifier.height(6.dp))
        SettingsTextField(value = name, onValueChange = { name = it }, placeholder = "昵称")
        Spacer(Modifier.height(12.dp))
        SettingsFieldLabel("签名")
        Spacer(Modifier.height(6.dp))
        SettingsTextField(value = signature, onValueChange = { signature = it }, placeholder = "写一句签名")
        Spacer(Modifier.height(14.dp))
        SettingsFieldLabel("头像")
        Spacer(Modifier.height(8.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            choices.forEach { asset ->
                Box(
                    modifier = Modifier
                        .size(56.dp)
                        .clip(androidx.compose.foundation.shape.CircleShape)
                        .clickable { avatar = asset }
                        .then(
                            if (avatar == asset) {
                                Modifier.border(2.dp, colors.accent, androidx.compose.foundation.shape.CircleShape)
                            } else {
                                Modifier.border(1.dp, colors.line, androidx.compose.foundation.shape.CircleShape)
                            },
                        ),
                    contentAlignment = Alignment.Center,
                ) {
                    Avatar(
                        name = name,
                        size = 50.dp,
                        model = StockMedia.drawableFor(asset),
                        circle = true,
                    )
                }
            }
        }
        Spacer(Modifier.height(16.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            SheetCancelButton(
                onClick = onDismiss,
                text = "取消",
                modifier = Modifier.weight(1f),
            )
            Button(
                onClick = { onSave(name, signature, avatar) },
                modifier = Modifier.weight(1f).height(48.dp),
                colors = ButtonDefaults.buttonColors(containerColor = colors.accent),
            ) {
                Text("保存", color = colors.accentText)
            }
        }
    }
}

// ─── 关于拾语 sheet（设计规格 7） ──────────────────────────────────────

@Composable
private fun AboutSheet(onDismiss: () -> Unit) {
    ShiYuSheet(onDismissRequest = onDismiss) {
        SheetHandle()
        SheetTitle("关于拾语")
        SheetDescription(
            "拾语 v2.4.0 · 即时通讯客户端。消息、语音与通话连接本机 BN_agent 后端，" +
                "语音识别与合成使用本地 SenseVoice / GSV-TTS。",
        )
        AboutInfoRow(label = "版本", value = "2.4.0（2026.11）")
        AboutInfoRow(label = "支持", value = "文字 · 图片 · 文件 · 语音 · 通话")
        AboutInfoRow(label = "数据", value = "本地缓存 + 本机后端")
        SheetCancelButton(onClick = onDismiss, text = "好的")
    }
}

@Composable
private fun AboutInfoRow(label: String, value: String) {
    val colors = ShiYuTheme.colors
    Row(
        modifier = Modifier.fillMaxWidth().padding(vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text = label, color = colors.ink3, fontSize = 12.5.sp, lineHeight = 18.sp)
        Spacer(Modifier.weight(1f))
        Text(
            text = value,
            color = colors.ink2,
            fontSize = 12.5.sp,
            fontWeight = FontWeight.Medium,
            lineHeight = 18.sp,
            textAlign = TextAlign.End,
        )
    }
}
