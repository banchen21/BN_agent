package com.bnagent.shiyu.ui.navigation

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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.ChatHeader
import com.bnagent.shiyu.ui.components.IconButton44
import com.bnagent.shiyu.ui.components.LargeTitleBar
import com.bnagent.shiyu.ui.components.LocalToast
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.components.ShiYuSwitch
import com.bnagent.shiyu.ui.components.SmallTitleBar
import com.bnagent.shiyu.ui.components.TextActionButton
import com.bnagent.shiyu.ui.theme.ShiYuCallColors
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType

/**
 * 业务页面占位（本文件归工程骨架所有）。
 *
 * 其它成员实现各自页面后，只需在 [ShiYuNavHost] 里把对应占位替换成真实 composable：
 * - ui/chats、ui/chat            → android-ui-chat
 * - ui/contacts、ui/profile、ui/me → android-ui-shell
 * - ui/call                      → android-ui-chat
 * - settings/theme、settings/server 的正式实现 → android-ui-shell / android-data
 *
 * 每个占位都保留了真实的跳转回调（进入聊天、进资料、进主题、发起通话），
 * 方便在真实页面就绪前验证整条导航链。
 */

@Composable
fun ChatsPlaceholder(
    onOpenChat: (String) -> Unit,
    onOpenTheme: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    Column(Modifier.fillMaxSize().background(colors.bg)) {
        LargeTitleBar(title = "消息") {
            IconButton44(ShiYuIcons.Search, "搜索", onClick = {})
            IconButton44(ShiYuIcons.Plus, "发起新聊天", onClick = {})
        }
        Column(Modifier.fillMaxSize().padding(horizontal = 8.dp)) {
            PlaceholderDemoRow(name = "林夏", note = "置顶会话 · 未读 2", onClick = { onOpenChat("lin") })
            PlaceholderDemoRow(name = "设计组", note = "群聊 · 静音", onClick = { onOpenChat("team") })
            PlaceholderDemoRow(name = "陈屿", note = "未读 1", onClick = { onOpenChat("chen") })
            Spacer(Modifier.height(16.dp))
            PlaceholderNote("消息列表占位 · 由 android-ui-chat 实现")
            PlaceholderAction("预览「气泡与主题」") { onOpenTheme() }
        }
    }
}

@Composable
fun ChatPlaceholder(
    id: String,
    onBack: () -> Unit,
    onOpenProfile: () -> Unit,
    onOpenTheme: () -> Unit,
    onVoiceCall: () -> Unit,
    onVideoCall: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    Column(Modifier.fillMaxSize().background(colors.bg)) {
        ChatHeader(
            name = id,
            subtitle = "占位聊天 · 安卓实现待接入",
            onBack = onBack,
            onOpenProfile = onOpenProfile,
            onVoiceCall = onVoiceCall,
            onVideoCall = onVideoCall,
            onMore = onOpenTheme,
        )
        Column(
            modifier = Modifier
                .fillMaxSize()
                .navigationBarsPadding()
                .padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Spacer(Modifier.height(40.dp))
            PlaceholderNote("聊天页占位 · 由 android-ui-chat 实现")
            PlaceholderAction("进入个人资料") { onOpenProfile() }
            PlaceholderAction("打开气泡与主题（更多）") { onOpenTheme() }
            PlaceholderAction("发起语音通话") { onVoiceCall() }
            PlaceholderAction("发起视频通话") { onVideoCall() }
        }
    }
}

@Composable
fun ContactsPlaceholder(
    onOpenProfile: (String) -> Unit,
) {
    val colors = ShiYuTheme.colors
    Column(Modifier.fillMaxSize().background(colors.bg)) {
        LargeTitleBar(title = "通讯录")
        Column(Modifier.fillMaxSize().padding(horizontal = 8.dp)) {
            PlaceholderDemoRow(name = "林夏", note = "设计 · 视觉组 · 在线", onClick = { onOpenProfile("lin") })
            PlaceholderDemoRow(name = "陈屿", note = "产品 · 增长 · 离线", onClick = { onOpenProfile("chen") })
            PlaceholderDemoRow(name = "赵敏", note = "设计 · 动效 · 在线", onClick = { onOpenProfile("zhao") })
            PlaceholderDemoRow(name = "吴其", note = "研发 · 客户端 · 离线", onClick = { onOpenProfile("wu") })
            Spacer(Modifier.height(16.dp))
            PlaceholderNote("通讯录占位 · 由 android-ui-shell 实现")
        }
    }
}

@Composable
fun ProfilePlaceholder(
    id: String,
    onBack: () -> Unit,
    onOpenMessage: () -> Unit,
    onVoiceCall: () -> Unit,
    onVideoCall: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    Column(Modifier.fillMaxSize().background(colors.bg)) {
        SmallTitleBar(title = "个人资料", onBack = onBack)
        Column(
            modifier = Modifier
                .fillMaxSize()
                .navigationBarsPadding()
                .padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Avatar(name = id, size = 88.dp, online = true)
            Spacer(Modifier.height(4.dp))
            Text(id, color = colors.ink, style = ShiYuType.profileName)
            PlaceholderNote("个人资料占位（id = $id）· 由 android-ui-shell 实现")
            PlaceholderAction("发消息") { onOpenMessage() }
            PlaceholderAction("语音通话") { onVoiceCall() }
            PlaceholderAction("视频通话") { onVideoCall() }
        }
    }
}

@Composable
fun MePlaceholder(
    onOpenTheme: () -> Unit,
    onOpenServerSettings: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    val settings = ShiYuTheme.settings
    val controller = ShiYuTheme.controller
    val toast = LocalToast.current
    Column(Modifier.fillMaxSize().background(colors.bg)) {
        LargeTitleBar(title = "我") {
            TextActionButton(text = "服务器", onClick = onOpenServerSettings)
        }
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(22.dp))
                    .background(colors.surface)
                    .clickable { onOpenTheme() }
                    .padding(16.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Avatar(name = "苏雨", size = 64.dp)
                Column(Modifier.weight(1f)) {
                    Text("苏雨", color = colors.ink, style = ShiYuType.meName)
                    Text(
                        "气泡与主题：${settings.summary}",
                        color = colors.ink3,
                        style = ShiYuType.caption,
                    )
                }
                IconButton44(ShiYuIcons.Chevron, "气泡与主题", onClick = onOpenTheme)
            }
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(18.dp))
                    .background(colors.surface)
                    .padding(horizontal = 16.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text("深色模式", color = colors.ink, style = ShiYuType.settingTitle)
                    Text(
                        "单独适配通话与聊天界面的深色配色",
                        color = colors.ink3,
                        style = ShiYuType.settingSubtitle,
                    )
                }
                ShiYuSwitch(
                    checked = settings.darkTheme,
                    onCheckedChange = { dark ->
                        controller.setDarkTheme(dark)
                        toast?.show(if (dark) "已切换到深色模式" else "已切换到浅色模式")
                    },
                )
            }
            PlaceholderNote("「我」页占位 · 由 android-ui-shell 实现")
        }
    }
}

@Composable
fun ThemeSettingsPlaceholder(
    onBack: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    val settings = ShiYuTheme.settings
    val controller = ShiYuTheme.controller
    Column(Modifier.fillMaxSize().background(colors.bg)) {
        SmallTitleBar(
            title = "气泡与主题",
            onBack = onBack,
            trailing = { TextActionButton(text = "完成", onClick = onBack) },
        )
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(22.dp))
                    .background(colors.surface)
                    .padding(16.dp),
            ) {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("当前主题", color = colors.ink, style = ShiYuType.settingTitle)
                    Text(settings.summary, color = colors.ink2, style = ShiYuType.caption)
                    Row(
                        modifier = Modifier.padding(top = 6.dp),
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        com.bnagent.shiyu.ui.theme.BubblePalette.entries.forEach { palette ->
                            val selected = settings.palette == palette
                            Box(
                                modifier = Modifier
                                    .size(30.dp)
                                    .clip(RoundedCornerShape(50))
                                    .background(palette.bubbleOut)
                                    .border(
                                        width = if (selected) 2.dp else 1.dp,
                                        color = if (selected) colors.accentText else colors.line,
                                        shape = RoundedCornerShape(50),
                                    )
                                    .clickable { controller.setPalette(palette) },
                            )
                        }
                    }
                }
            }
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(18.dp))
                    .background(colors.surface)
                    .padding(horizontal = 16.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("深色模式", modifier = Modifier.weight(1f), color = colors.ink, style = ShiYuType.settingTitle)
                ShiYuSwitch(
                    checked = settings.darkTheme,
                    onCheckedChange = { controller.setDarkTheme(it) },
                )
            }
            PlaceholderNote("主题设置占位 · 正式页面由 android-ui-shell 实现（路由 settings/theme）")
        }
    }
}

@Composable
fun ServerSettingsPlaceholder(
    onBack: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    Column(Modifier.fillMaxSize().background(colors.bg)) {
        SmallTitleBar(title = "服务器设置", onBack = onBack)
        Column(
            modifier = Modifier
                .fillMaxSize()
                .navigationBarsPadding()
                .padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Spacer(Modifier.height(40.dp))
            PlaceholderNote("服务器设置占位 · 由 android-data 实现（路由 settings/server）")
            PlaceholderNote("用于配置 BN_agent 后端地址与 API Key")
        }
    }
}

/**
 * 通话页占位：强制深色，验证 call 路由的 chrome 切换与挂断返回。
 * 正式实现（波纹、计时、静音/免提/翻转）由 android-ui-chat 负责。
 */
@Composable
fun CallPlaceholder(
    id: String,
    video: Boolean,
    onEnd: () -> Unit,
) {
    val toast = LocalToast.current
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(ShiYuCallColors.background),
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .navigationBarsPadding()
                .padding(start = 22.dp, end = 22.dp, top = 6.dp, bottom = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(
                    modifier = Modifier
                        .clip(RoundedCornerShape(50))
                        .background(ShiYuCallColors.controlBg)
                        .padding(horizontal = 12.dp, vertical = 7.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    androidx.compose.material3.Icon(
                        imageVector = ShiYuIcons.Lock,
                        contentDescription = null,
                        tint = ShiYuCallColors.pillText,
                        modifier = Modifier.size(13.dp),
                    )
                    Text(
                        if (video) "视频通话 · 已加密" else "语音通话 · 已加密",
                        color = ShiYuCallColors.pillText,
                        style = ShiYuType.meta,
                    )
                }
                IconButton44(
                    icon = ShiYuIcons.Close,
                    contentDescription = "挂断",
                    onClick = {
                        toast?.show("已取消通话")
                        onEnd()
                    },
                    tint = ShiYuCallColors.name,
                )
            }
            Column(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Avatar(name = id, size = 112.dp, circle = true)
                Spacer(Modifier.height(20.dp))
                Text("通话 · $id", color = ShiYuCallColors.name, style = ShiYuType.callName)
                Text(
                    "正在呼叫…（占位 · 由 android-ui-chat 实现）",
                    color = ShiYuCallColors.status,
                    style = ShiYuType.callStatus,
                )
            }
            Box(
                modifier = Modifier
                    .size(68.dp)
                    .clip(RoundedCornerShape(50))
                    .background(ShiYuCallColors.hangup)
                    .clickable {
                        toast?.show("已取消通话")
                        onEnd()
                    },
                contentAlignment = Alignment.Center,
            ) {
                androidx.compose.material3.Icon(
                    imageVector = ShiYuIcons.Hangup,
                    contentDescription = "挂断",
                    tint = Color.White,
                    modifier = Modifier.size(28.dp),
                )
            }
            Spacer(Modifier.height(12.dp))
            Text("挂断", color = ShiYuCallColors.controlLabel, style = ShiYuType.callControlLabel)
        }
    }
}

// ─── 占位内部小部件 ───────────────────────────────────────────────────

@Composable
private fun PlaceholderDemoRow(
    name: String,
    note: String,
    onClick: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 2.dp)
            .clip(RoundedCornerShape(16.dp))
            .clickable(onClick = onClick)
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Avatar(name = name, size = 48.dp, online = note.contains("在线"))
        Column(Modifier.weight(1f)) {
            Text(name, color = colors.ink, style = ShiYuType.rowTitle)
            Text(note, color = colors.ink2, style = ShiYuType.preview)
        }
        androidx.compose.material3.Icon(
            imageVector = ShiYuIcons.Chevron,
            contentDescription = null,
            tint = colors.ink3,
            modifier = Modifier.size(18.dp),
        )
    }
}

@Composable
private fun PlaceholderNote(text: String) {
    Text(
        text = text,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp),
        color = ShiYuTheme.colors.ink3,
        style = ShiYuType.caption,
        textAlign = TextAlign.Center,
    )
}

@Composable
private fun PlaceholderAction(
    text: String,
    onClick: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    Box(
        modifier = Modifier
            .width(240.dp)
            .height(46.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(colors.accent)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, color = colors.onAccent, style = ShiYuType.button)
    }
}
