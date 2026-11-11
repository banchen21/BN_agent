package com.bnagent.shiyu.ui.contacts

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.data.StockMedia
import com.bnagent.shiyu.domain.model.Contact
import com.bnagent.shiyu.ui.components.Avatar
import com.bnagent.shiyu.ui.components.LargeTitleBar
import com.bnagent.shiyu.ui.components.ShiYuIcons
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType

/**
 * 通讯录（设计规格 4.4）：大标题 + 常驻搜索 + 「全部联系人」分组。
 *
 * 点击行进入个人资料；搜索匹配姓名与职能，无结果时展示空态。
 */
@Composable
fun ContactsScreen(
    onOpenProfile: (String) -> Unit,
) {
    val container = LocalAppContainer.current
    val viewModel: ContactsViewModel = viewModel(
        factory = ContactsViewModel.factory(container.conversationRepository),
    )
    val contacts by viewModel.contacts.collectAsStateWithLifecycle()
    val query by viewModel.query.collectAsStateWithLifecycle()
    val colors = ShiYuTheme.colors

    Column(Modifier.fillMaxSize().background(colors.bg)) {
        LargeTitleBar(title = "通讯录")
        ContactsSearchBar(
            query = query,
            onQueryChange = viewModel::onQueryChange,
            onClear = viewModel::clearQuery,
        )
        when {
            contacts.isNotEmpty() -> {
                LazyColumn(
                    modifier = Modifier.weight(1f).fillMaxWidth(),
                    contentPadding = PaddingValues(start = 8.dp, end = 8.dp, top = 8.dp, bottom = 18.dp),
                ) {
                    item(key = "group-title") {
                        Text(
                            text = "全部联系人",
                            modifier = Modifier.padding(start = 10.dp, end = 10.dp, top = 8.dp, bottom = 6.dp),
                            color = colors.ink3,
                            style = ShiYuType.contactsGroupTitle,
                        )
                    }
                    items(items = contacts, key = { it.conversationId }) { contact ->
                        ContactRow(contact = contact, onClick = { onOpenProfile(contact.conversationId) })
                    }
                }
            }

            query.isNotBlank() -> {
                ContactsEmptyState(onClear = viewModel::clearQuery)
            }
        }
    }
}

// ─── 搜索行 ──────────────────────────────────────────────────────────

@Composable
private fun ContactsSearchBar(
    query: String,
    onQueryChange: (String) -> Unit,
    onClear: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    val focusRequester = remember { FocusRequester() }
    var focused by remember { mutableStateOf(false) }
    val textStyle = TextStyle(
        fontSize = 15.5.sp,
        fontWeight = FontWeight.Normal,
        lineHeight = 21.sp,
        color = colors.ink,
    )

    BoxWithConstraints(Modifier.fillMaxWidth().background(colors.bg)) {
        val horizontalPadding = if (maxWidth <= 380.dp) 12.dp else 14.dp
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(start = horizontalPadding, end = horizontalPadding, bottom = 8.dp)
                .height(44.dp)
                .drawBehind {
                    if (focused) {
                        val stroke = 3.dp.toPx()
                        drawRoundRect(
                            color = SearchFocusGlow,
                            topLeft = Offset(-stroke, -stroke),
                            size = Size(size.width + stroke * 2, size.height + stroke * 2),
                            cornerRadius = CornerRadius(17.dp.toPx()),
                            style = Stroke(stroke),
                        )
                    }
                }
                .clip(RoundedCornerShape(14.dp))
                .background(colors.surface)
                .border(1.dp, if (focused) colors.accent else colors.line, RoundedCornerShape(14.dp)),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                imageVector = ShiYuIcons.Search,
                contentDescription = null,
                tint = colors.ink3,
                modifier = Modifier.padding(start = 12.dp).size(18.dp),
            )
            BasicTextField(
                value = query,
                onValueChange = onQueryChange,
                modifier = Modifier
                    .weight(1f)
                    .padding(horizontal = 10.dp)
                    .focusRequester(focusRequester)
                    .onFocusChanged { focused = it.isFocused },
                textStyle = textStyle,
                singleLine = true,
                cursorBrush = SolidColor(colors.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                decorationBox = { innerTextField ->
                    Box(Modifier.fillMaxWidth()) {
                        if (query.isEmpty()) {
                            Text("搜索联系人", color = colors.ink3, style = textStyle)
                        }
                        innerTextField()
                    }
                },
            )
            if (query.isNotEmpty()) {
                Box(
                    modifier = Modifier
                        .padding(end = 7.dp)
                        .size(30.dp)
                        .clip(CircleShape)
                        .background(colors.surface2)
                        .clickable {
                            onClear()
                            focusRequester.requestFocus()
                        },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        imageVector = ShiYuIcons.Close,
                        contentDescription = "清空搜索词",
                        tint = colors.ink3,
                        modifier = Modifier.size(15.dp),
                    )
                }
            }
        }
    }
}

// ─── 联系人行 ────────────────────────────────────────────────────────

@Composable
private fun ContactRow(
    contact: Contact,
    onClick: () -> Unit,
) {
    val colors = ShiYuTheme.colors
    val roleStyle = ShiYuType.caption.copy(
        fontSize = 12.5.sp,
        lineHeight = 17.5.sp,
    )
    val statusStyle = TextStyle(
        fontSize = 11.5.sp,
        fontWeight = FontWeight.SemiBold,
        lineHeight = 16.sp,
        color = colors.ink3,
    )
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .clickable(onClick = onClick)
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(
            name = contact.name,
            size = 46.dp,
            model = StockMedia.drawableFor(contact.avatar),
            online = contact.online,
        )
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                text = contact.name,
                color = colors.ink,
                style = ShiYuType.rowTitle,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = contact.role,
                color = colors.ink2,
                style = roleStyle,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Spacer(Modifier.width(8.dp))
        Text(
            text = if (contact.online) "在线" else "离线",
            style = statusStyle,
        )
    }
}

// ─── 搜索空态（设计规格 3.11） ────────────────────────────────────────

@Composable
private fun ColumnScope.ContactsEmptyState(onClear: () -> Unit) {
    val colors = ShiYuTheme.colors
    Column(
        modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 22.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(
            imageVector = ShiYuIcons.Contacts,
            contentDescription = null,
            tint = colors.ink3,
            modifier = Modifier.size(34.dp),
        )
        Spacer(Modifier.height(8.dp))
        Text(
            text = "没有找到联系人",
            color = colors.ink2,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            lineHeight = 21.sp,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            text = "试试名字或职能关键词",
            color = colors.ink3,
            style = ShiYuType.caption,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(12.dp))
        Box(
            modifier = Modifier
                .height(44.dp)
                .clip(RoundedCornerShape(50))
                .background(colors.surface)
                .border(1.dp, colors.line, RoundedCornerShape(50))
                .clickable(onClick = onClear)
                .padding(horizontal = 18.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = "清空搜索词",
                color = colors.ink,
                fontSize = 13.5.sp,
                fontWeight = FontWeight.SemiBold,
                lineHeight = 19.sp,
            )
        }
    }
}

/** 搜索框聚焦外发光 rgba(201,59,36,.14)（设计规格 3.6）。 */
private val SearchFocusGlow = Color(0x24C93B24)
