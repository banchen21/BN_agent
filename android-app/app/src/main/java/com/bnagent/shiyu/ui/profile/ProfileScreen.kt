package com.bnagent.shiyu.ui.profile

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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.commonGroupLabel
import com.bnagent.shiyu.domain.model.handleLine
import com.bnagent.shiyu.domain.model.profileTags
import com.bnagent.shiyu.domain.model.remark
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.components.SmallTitleBar
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType

/**
 * 个人资料（设计规格 4.5）：Hero 卡（头像 / 名字 / 拾语号 / 签名 / 操作按钮）+ 信息组。
 *
 * 单聊：备注、标签、共同群聊，展示语音 / 视频；群聊：成员数、公告、标签，只保留发消息。
 */
@Composable
fun ProfileScreen(
    conversationId: String,
    onBack: () -> Unit,
    onOpenMessage: () -> Unit,
    onVoiceCall: () -> Unit,
    onVideoCall: () -> Unit,
) {
    val container = LocalAppContainer.current
    val viewModel: ProfileViewModel = viewModel(
        factory = ProfileViewModel.factory(container.conversationRepository, conversationId),
    )
    val conversation by viewModel.conversation.collectAsStateWithLifecycle()
    val colors = ShiYuTheme.colors

    Column(Modifier.fillMaxSize().background(colors.bg)) {
        SmallTitleBar(title = "个人资料", onBack = onBack)
        val current = conversation
        if (current == null) {
            Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                Text("资料加载中…", color = colors.ink3, style = ShiYuType.caption)
            }
        } else {
            Column(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .navigationBarsPadding()
                    .padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 28.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                ProfileHeroCard(
                    conversation = current,
                    onOpenMessage = onOpenMessage,
                    onVoiceCall = onVoiceCall,
                    onVideoCall = onVideoCall,
                )
                ProfileInfoCard(conversation = current)
            }
        }
    }
}

// ─── Hero 卡 ─────────────────────────────────────────────────────────

@Composable
private fun ProfileHeroCard(
    conversation: Conversation,
    onOpenMessage: () -> Unit,
    onVoiceCall: () -> Unit,
    onVideoCall: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    val cardShape = RoundedCornerShape(24.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(cardShape)
            .background(colors.surface)
            .border(1.dp, colors.line, cardShape)
            .padding(start = 16.dp, end = 16.dp, top = 22.dp, bottom = 18.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Avatar(
            name = conversation.name,
            size = 88.dp,
            model = StockMedia.drawableFor(conversation.avatar),
            online = conversation.online && !conversation.isGroup,
        )
        Text(
            text = conversation.name,
            modifier = Modifier.padding(top = 8.dp),
            color = colors.ink,
            style = ShiYuType.profileName,
        )
        Text(
            text = conversation.handleLine,
            modifier = Modifier.padding(top = 4.dp),
            color = colors.ink3,
            fontSize = 12.sp,
            lineHeight = 17.sp,
        )
        if (!conversation.sign.isNullOrBlank()) {
            Text(
                text = conversation.sign,
                modifier = Modifier.padding(top = 8.dp).widthIn(max = 260.dp),
                color = colors.ink2,
                fontSize = 13.5.sp,
                lineHeight = 20.25.sp,
                textAlign = TextAlign.Center,
            )
        }
        Row(
            modifier = Modifier.fillMaxWidth().padding(top = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            PrimaryActionButton(
                text = "发消息",
                onClick = onOpenMessage,
                modifier = Modifier.weight(1f),
            )
            if (!conversation.isGroup) {
                SecondaryActionButton(icon = ShiYuIcons.Phone, label = "语音", onClick = onVoiceCall)
                SecondaryActionButton(icon = ShiYuIcons.Video, label = "视频", onClick = onVideoCall)
            }
        }
    }
}

/** 主按钮：高 46、圆角 14、accent 底、白字 15sp/700（设计规格 3.5）。 */
@Composable
private fun PrimaryActionButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = ShiYuTheme.colors
    Box(
        modifier = modifier
            .height(46.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(colors.accent)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(text = text, color = colors.onAccent, style = ShiYuType.button)
    }
}

/** 次级图标按钮：64×46、圆角 14、surface-2 + 1px line，图标 19 + 标签 11.5sp/650。 */
@Composable
private fun SecondaryActionButton(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    Row(
        modifier = Modifier
            .width(64.dp)
            .height(46.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(colors.surface2)
            .border(1.dp, colors.line, RoundedCornerShape(14.dp))
            .clickable(onClick = onClick),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.Center,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = colors.ink,
            modifier = Modifier.width(19.dp).height(19.dp),
        )
        Spacer(Modifier.width(4.dp))
        Text(
            text = label,
            color = colors.ink,
            fontSize = 11.5.sp,
            fontWeight = FontWeight.SemiBold,
            lineHeight = 16.sp,
        )
    }
}

// ─── 信息组 ─────────────────────────────────────────────────────────

@Composable
private fun ProfileInfoCard(conversation: Conversation) {
    val colors = ShiYuTheme.colors
    val cardShape = RoundedCornerShape(18.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(cardShape)
            .background(colors.surface)
            .border(1.dp, colors.line, cardShape)
            .padding(horizontal = 16.dp, vertical = 2.dp),
    ) {
        if (conversation.isGroup) {
            InfoRow(label = "群成员", value = "${conversation.memberCount ?: 0} 人", divider = true)
            InfoRow(label = "群公告", value = conversation.announcement.orEmpty(), divider = true)
            InfoChipsRow(label = "标签", chips = conversation.profileTags, divider = false)
        } else {
            InfoRow(label = "备注", value = conversation.remark, divider = true)
            InfoChipsRow(label = "标签", chips = conversation.profileTags, divider = true)
            InfoRow(label = "共同群聊", value = conversation.commonGroupLabel.orEmpty(), divider = false)
        }
    }
}

@Composable
private fun InfoRow(
    label: String,
    value: String,
    divider: Boolean,
) {
    val colors = ShiYuTheme.colors
    Column(Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(vertical = 14.dp),
            verticalAlignment = Alignment.Top,
        ) {
            Text(
                text = label,
                color = colors.ink2,
                fontSize = 14.sp,
                lineHeight = 20.3.sp,
            )
            Spacer(Modifier.width(16.dp))
            Text(
                text = value,
                modifier = Modifier.weight(1f),
                color = colors.ink,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                lineHeight = 20.3.sp,
                textAlign = TextAlign.End,
            )
        }
        if (divider) {
            Box(Modifier.fillMaxWidth().height(1.dp).background(colors.line))
        }
    }
}

@Composable
private fun InfoChipsRow(
    label: String,
    chips: List<String>,
    divider: Boolean,
) {
    val colors = ShiYuTheme.colors
    Column(Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                text = label,
                color = colors.ink2,
                fontSize = 14.sp,
                lineHeight = 20.3.sp,
            )
            Spacer(Modifier.width(16.dp))
            Row(
                modifier = Modifier.weight(1f),
                horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.End),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                chips.forEach { chip -> InfoChip(text = chip) }
            }
        }
        if (divider) {
            Box(Modifier.fillMaxWidth().height(1.dp).background(colors.line))
        }
    }
}

/** chip：padding 2 8、圆角 7、surface-2 + 1px line、12sp/600 ink-2（设计规格 4.5）。 */
@Composable
private fun InfoChip(text: String) {
    val colors = ShiYuTheme.colors
    Box(
        modifier = Modifier
            .clip(RoundedCornerShape(7.dp))
            .background(colors.surface2)
            .border(1.dp, colors.line, RoundedCornerShape(7.dp))
            .padding(horizontal = 8.dp, vertical = 2.dp),
    ) {
        Text(
            text = text,
            color = colors.ink2,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            lineHeight = 17.sp,
        )
    }
}
