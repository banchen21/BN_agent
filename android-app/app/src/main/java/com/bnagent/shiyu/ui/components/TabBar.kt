package com.bnagent.shiyu.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import com.bnagent.shiyu.ui.theme.ShiYuTheme
import com.bnagent.shiyu.ui.theme.ShiYuType

/** 主 Tab（设计规格 1.1）：只在 chats / contacts / me 三个根屏幕显示 tabbar。 */
enum class MainTab(val route: String, val label: String) {
    Chats("chats", "消息"),
    Contacts("contacts", "通讯录"),
    Me("me", "我"),
    ;

    val icon: ImageVector
        get() = when (this) {
            Chats -> ShiYuIcons.Message
            Contacts -> ShiYuIcons.Contacts
            Me -> ShiYuIcons.Me
        }
}

/**
 * 底部 tabbar（设计规格 3.2）：3 等分、padding 5/10/6、单个 tab 高 52 圆角 14、
 * 图标 24 + 标签 12sp；消息 tab 未读徽标由 [unreadCount]（非静音会话未读总数）驱动。
 *
 * 自带 `navigationBarsPadding()`；外层只应在 chats / contacts / me 显示。
 */
@Composable
fun ShiYuTabBar(
    selected: MainTab,
    onSelect: (MainTab) -> Unit,
    modifier: Modifier = Modifier,
    unreadCount: Int = 0,
) {
    val colors = ShiYuTheme.colors
    Column(
        modifier = modifier
            .fillMaxWidth()
            .background(colors.surface)
            .navigationBarsPadding(),
    ) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(colors.line))
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(start = 10.dp, end = 10.dp, top = 5.dp, bottom = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(0.dp),
        ) {
            MainTab.entries.forEach { tab ->
                val active = tab == selected
                Box(
                    modifier = Modifier
                        .weight(1f)
                        .height(52.dp)
                        .clip(RoundedCornerShape(14.dp))
                        .clickable { onSelect(tab) },
                    contentAlignment = Alignment.Center,
                ) {
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        Box {
                            Icon(
                                imageVector = tab.icon,
                                contentDescription = tab.label,
                                tint = if (active) colors.accentText else colors.ink3,
                                modifier = Modifier.size(24.dp),
                            )
                            if (tab == MainTab.Chats && unreadCount > 0) {
                                TabBadge(
                                    count = unreadCount,
                                    modifier = Modifier
                                        .align(Alignment.TopEnd)
                                        .offsetBadge(),
                                )
                            }
                        }
                        Text(
                            text = tab.label,
                            color = if (active) colors.accentText else colors.ink3,
                            style = ShiYuType.tabLabel,
                        )
                    }
                }
            }
        }
    }
}

/** 设计规格 3.2：徽标相对图标 top -5、right -12。 */
private fun Modifier.offsetBadge(): Modifier = this.offset(x = 12.dp, y = (-5).dp)
