package com.bnagent.shiyu.ui.chats

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.domain.model.Conversation
import com.bnagent.shiyu.domain.model.pickNote
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.IconButton44
import com.bnagent.shiyu.ui.components.LargeTitleBar
import com.bnagent.shiyu.ui.components.SheetCancelButton
import com.bnagent.shiyu.ui.components.SheetHandle
import com.bnagent.shiyu.ui.components.SheetTitle
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.components.ShiYuSheet
import com.bnagent.shiyu.ui.components.UnreadBadge
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType
import kotlinx.coroutines.delay

/**
 * 消息列表页（设计规格 4.1）。
 *
 * - 大标题栏「消息」+ 搜索 / 发起新聊天；
 * - 搜索行可展开并自动聚焦，输入即时过滤（name + preview），空态可清空搜索词；
 * - 会话行：头像 + 在线点、置顶内描边、静音图标、未读徽标（99+）、两行文本、时间；
 * - 排序由仓库保证（置顶优先、order 降序）；tabbar 未读总数由 AppRoot 统一接入。
 */
@Composable
fun ChatsScreen(onOpenChat: (String) -> Unit) {
    val colors = ShiYuTheme.colors
    val container = LocalAppContainer.current
    val conversations by container.conversationRepository.conversations
        .collectAsStateWithLifecycle(initialValue = emptyList())
    val allConversations by container.conversationRepository.allConversations
        .collectAsStateWithLifecycle(initialValue = emptyList())

    var searchOpen by rememberSaveable { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    var showNewChat by rememberSaveable { mutableStateOf(false) }
    val searchFocus = remember { FocusRequester() }

    LaunchedEffect(searchOpen) {
        if (searchOpen) {
            delay(60)
            searchFocus.requestFocus()
        }
    }

    val filtered = remember(conversations, query) {
        val keyword = query.trim()
        if (keyword.isEmpty()) {
            conversations
        } else {
            conversations.filter { conv ->
                (conv.name + " " + conv.preview).lowercase().contains(keyword.lowercase())
            }
        }
    }

    Column(Modifier.fillMaxSize().background(colors.bg)) {
        LargeTitleBar(title = "消息") {
            IconButton44(
                icon = ShiYuIcons.Search,
                contentDescription = if (searchOpen) "收起搜索" else "搜索",
                onClick = {
                    // 再点一次收起搜索并清空关键词，恢复完整列表。
                    if (searchOpen) {
                        searchOpen = false
                        query = ""
                    } else {
                        searchOpen = true
                    }
                },
            )
            IconButton44(
                icon = ShiYuIcons.Plus,
                contentDescription = "发起新聊天",
                onClick = { showNewChat = true },
            )
        }
        AnimatedVisibility(
            visible = searchOpen,
            enter = fadeIn() + expandVertically(),
            exit = fadeOut() + shrinkVertically(),
        ) {
            ChatsSearchField(
                query = query,
                onQueryChange = { query = it },
                focusRequester = searchFocus,
                onClear = {
                    query = ""
                    searchFocus.requestFocus()
                },
            )
        }
        if (query.isNotBlank() && filtered.isEmpty()) {
            SearchEmptyState(
                onClear = {
                    query = ""
                    searchFocus.requestFocus()
                },
            )
        } else {
            LazyColumn(
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = 8.dp, end = 8.dp, top = 6.dp, bottom = 14.dp),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                items(filtered, key = { it.id }) { conv ->
                    ConversationRow(conv = conv, onClick = { onOpenChat(conv.id) })
                }
            }
        }
    }

    if (showNewChat) {
        NewChatSheet(
            conversations = allConversations,
            onDismiss = { showNewChat = false },
            onPick = { id ->
                showNewChat = false
                onOpenChat(id)
            },
        )
    }
}

// ─── 会话行 ───────────────────────────────────────────────────────────

@Composable
private fun ConversationRow(conv: Conversation, onClick: () -> Unit) {
    val colors = ShiYuTheme.colors
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val shape = RoundedCornerShape(16.dp)
    val background = when {
        pressed -> colors.surface2
        conv.pinned -> colors.surface
        else -> Color.Transparent
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(background)
            .then(if (conv.pinned) Modifier.border(1.dp, colors.line, shape) else Modifier)
            .clickable(
                interactionSource = interaction,
                indication = null,
                onClick = onClick,
            )
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Avatar(
            name = conv.name,
            size = 48.dp,
            model = StockMedia.drawableFor(conv.avatar),
            online = conv.online,
        )
        Column(
            modifier = Modifier.weight(1f),
            verticalArrangement = Arrangement.spacedBy(5.dp),
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    text = conv.name,
                    modifier = Modifier.weight(1f),
                    color = colors.ink,
                    style = ShiYuType.rowTitle,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    if (conv.muted) {
                        Icon(
                            imageVector = ShiYuIcons.BellOff,
                            contentDescription = "已静音",
                            tint = colors.ink3,
                            modifier = Modifier.size(13.dp),
                        )
                    }
                    if (conv.timeLabel.isNotBlank()) {
                        Text(
                            text = conv.timeLabel,
                            color = colors.ink3,
                            style = ShiYuType.meta,
                            maxLines = 1,
                        )
                    }
                }
            }
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    text = conv.preview,
                    modifier = Modifier.weight(1f),
                    color = colors.ink2,
                    style = ShiYuType.preview,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                UnreadBadge(count = conv.unread, muted = conv.muted)
            }
        }
    }
}

// ─── 搜索 ─────────────────────────────────────────────────────────────

@Composable
private fun ChatsSearchField(
    query: String,
    onQueryChange: (String) -> Unit,
    focusRequester: FocusRequester,
    onClear: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    var focused by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(14.dp)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 14.dp, end = 14.dp, bottom = 8.dp)
            .height(44.dp)
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, if (focused) colors.accent else colors.line, shape)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(
            imageVector = ShiYuIcons.Search,
            contentDescription = null,
            tint = colors.ink3,
            modifier = Modifier.size(18.dp),
        )
        BasicTextField(
            value = query,
            onValueChange = onQueryChange,
            modifier = Modifier
                .weight(1f)
                .focusRequester(focusRequester)
                .onFocusChanged { focused = it.isFocused },
            textStyle = TextStyle(
                color = colors.ink,
                fontSize = 15.5.sp,
                fontWeight = FontWeight.Normal,
                lineHeight = 21.sp,
            ),
            cursorBrush = SolidColor(colors.accent),
            singleLine = true,
            decorationBox = { innerTextField ->
                Box(contentAlignment = Alignment.CenterStart) {
                    if (query.isEmpty()) {
                        Text(
                            text = "搜索联系人或消息",
                            color = colors.ink3,
                            fontSize = 15.5.sp,
                            fontWeight = FontWeight.Normal,
                            lineHeight = 21.sp,
                        )
                    }
                    innerTextField()
                }
            },
        )
        if (query.isNotEmpty()) {
            Box(
                modifier = Modifier
                    .size(30.dp)
                    .clip(CircleShape)
                    .background(colors.surface2)
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClick = onClear,
                    ),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    imageVector = ShiYuIcons.Close,
                    contentDescription = "清空搜索",
                    tint = colors.ink3,
                    modifier = Modifier.size(15.dp),
                )
            }
        }
    }
}

@Composable
private fun SearchEmptyState(onClear: () -> Unit) {
    val colors = ShiYuTheme.colors
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(horizontal = 22.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Icon(
            imageVector = ShiYuIcons.Search,
            contentDescription = null,
            tint = colors.ink3,
            modifier = Modifier.size(32.dp),
        )
        Spacer(Modifier.height(8.dp))
        Text(
            text = "没有找到相关会话",
            color = colors.ink2,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            lineHeight = 21.sp,
        )
        Spacer(Modifier.height(6.dp))
        Text(
            text = "换个名字或关键词试试",
            color = colors.ink3,
            style = ShiYuType.caption,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(14.dp))
        EmptyActionButton(text = "清空搜索词", onClick = onClear)
    }
}

@Composable
private fun EmptyActionButton(text: String, onClick: () -> Unit) {
    val colors = ShiYuTheme.colors
    Box(
        modifier = Modifier
            .height(44.dp)
            .clip(RoundedCornerShape(50))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(50))
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                onClick = onClick,
            )
            .padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            color = colors.ink,
            fontSize = 13.5.sp,
            fontWeight = FontWeight.SemiBold,
            lineHeight = 19.sp,
        )
    }
}

// ─── 新聊天 sheet ─────────────────────────────────────────────────────

@Composable
private fun NewChatSheet(
    conversations: List<Conversation>,
    onDismiss: () -> Unit,
    onPick: (String) -> Unit,
) {
    val colors = ShiYuTheme.colors
    ShiYuSheet(onDismissRequest = onDismiss) {
        SheetHandle()
        SheetTitle("发起新聊天")
        conversations.forEachIndexed { index, conv ->
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClick = { onPick(conv.id) },
                    )
                    .padding(horizontal = 4.dp, vertical = 9.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Avatar(
                    name = conv.name,
                    size = 42.dp,
                    model = StockMedia.drawableFor(conv.avatar),
                    online = conv.online,
                )
                Text(
                    text = conv.name,
                    modifier = Modifier.weight(1f),
                    color = colors.ink,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                    lineHeight = 21.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = conv.pickNote,
                    color = colors.ink3,
                    fontSize = 11.5.sp,
                    fontWeight = FontWeight.Medium,
                    lineHeight = 16.sp,
                    maxLines = 1,
                )
            }
            if (index != conversations.lastIndex) {
                Box(Modifier.fillMaxWidth().height(1.dp).background(colors.line))
            }
        }
        SheetCancelButton(onClick = onDismiss)
    }
}
