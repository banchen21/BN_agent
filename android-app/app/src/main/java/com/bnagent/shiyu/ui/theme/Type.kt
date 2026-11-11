package com.bnagent.shiyu.ui.theme

import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp

/**
 * 拾语文字样式（设计规格 2.7 / 2.8）。
 *
 * 字体：系统默认（中文机型为 Noto Sans SC / 厂商字体），不内置 HarmonyOS Sans。
 * 字重映射（team-lead 已拍板）：550→Medium(500)，650→SemiBold(600)，760→Bold(700)，
 * 680→Bold(700)，780/800→ExtraBold(800)，其余就近取整。
 */
object ShiYuType {

    /** 页面大标题（消息 / 通讯录 / 我）：28sp / 800 / lh 1.15。 */
    val pageTitle = TextStyle(
        fontSize = 28.sp,
        fontWeight = FontWeight.ExtraBold,
        lineHeight = 32.2.sp,
        letterSpacing = (-0.02).em,
    )

    /** 页面小标题（气泡与主题 / 个人资料，居中）：18sp / 700。 */
    val smallTitle = TextStyle(
        fontSize = 18.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 24.sp,
    )

    /** 会话名 / 联系人名：16sp / 650 → SemiBold。 */
    val rowTitle = TextStyle(
        fontSize = 16.sp,
        fontWeight = FontWeight.SemiBold,
        lineHeight = 22.sp,
    )

    /** 聊天头部对方名：16.5sp / 680 → Bold，-0.01em。 */
    val chatHeaderName = TextStyle(
        fontSize = 16.5.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 22.sp,
        letterSpacing = (-0.01).em,
    )

    /** 会话预览：14.5sp / 400。 */
    val preview = TextStyle(
        fontSize = 14.5.sp,
        fontWeight = FontWeight.Normal,
        lineHeight = 20.sp,
    )

    /** 会话时间 / 次级元信息：11.5sp / 550 → Medium，0.01em。 */
    val meta = TextStyle(
        fontSize = 11.5.sp,
        fontWeight = FontWeight.Medium,
        lineHeight = 16.sp,
        letterSpacing = 0.01.em,
    )

    /** 未读徽标：11.5sp / 700。 */
    val badge = TextStyle(
        fontSize = 11.5.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 16.sp,
    )

    /** tabbar 标签：12sp / 640 → SemiBold，0.02em。 */
    val tabLabel = TextStyle(
        fontSize = 12.sp,
        fontWeight = FontWeight.SemiBold,
        lineHeight = 16.sp,
        letterSpacing = 0.02.em,
    )

    /** 设置组标题：12.5sp / 700，0.05em。 */
    val groupTitle = TextStyle(
        fontSize = 12.5.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 18.sp,
        letterSpacing = 0.05.em,
    )

    /** 通讯录分组标题：12sp / 700，0.06em。 */
    val contactsGroupTitle = TextStyle(
        fontSize = 12.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 18.sp,
        letterSpacing = 0.06.em,
    )

    /** sheet 标题：17sp / 700。 */
    val sheetTitle = TextStyle(
        fontSize = 17.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 24.sp,
    )

    /** dialog 标题：17sp / 720 → Bold。 */
    val dialogTitle = TextStyle(
        fontSize = 17.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 24.sp,
    )

    /** dialog 正文：13sp / 400 / lh 1.55。 */
    val dialogBody = TextStyle(
        fontSize = 13.sp,
        fontWeight = FontWeight.Normal,
        lineHeight = 20.sp,
    )

    /** 主按钮 / 文字按钮：15sp / 700 → Bold。 */
    val button = TextStyle(
        fontSize = 15.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 20.sp,
    )

    /** dialog 按钮：14.5sp / 650 → SemiBold。 */
    val dialogButton = TextStyle(
        fontSize = 14.5.sp,
        fontWeight = FontWeight.SemiBold,
        lineHeight = 20.sp,
    )

    /** 输入框文字：16.5sp / 400。 */
    val input = TextStyle(
        fontSize = 16.5.sp,
        fontWeight = FontWeight.Normal,
        lineHeight = 22.sp,
    )

    /** 设置行主文：15sp / 650 → SemiBold。 */
    val settingTitle = TextStyle(
        fontSize = 15.sp,
        fontWeight = FontWeight.SemiBold,
        lineHeight = 21.sp,
    )

    /** 设置行说明：11.5sp / 400 / lh 1.4。 */
    val settingSubtitle = TextStyle(
        fontSize = 11.5.sp,
        fontWeight = FontWeight.Normal,
        lineHeight = 16.sp,
    )

    /** 个人资料名：22sp / 780 → Bold。 */
    val profileName = TextStyle(
        fontSize = 22.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 28.sp,
    )

    /** 我页名字：20sp / 760 → Bold。 */
    val meName = TextStyle(
        fontSize = 20.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 26.sp,
    )

    /** 通话名字：26sp / 760 → Bold，-0.01em。 */
    val callName = TextStyle(
        fontSize = 26.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 32.sp,
        letterSpacing = (-0.01).em,
    )

    /** 通话状态 / 计时：14sp / 550 → Medium，0.03em（tabular 由调用方设置）。 */
    val callStatus = TextStyle(
        fontSize = 14.sp,
        fontWeight = FontWeight.Medium,
        lineHeight = 20.sp,
        letterSpacing = 0.03.em,
    )

    /** 通话控件标签：11.5sp / 600。 */
    val callControlLabel = TextStyle(
        fontSize = 11.5.sp,
        fontWeight = FontWeight.SemiBold,
        lineHeight = 16.sp,
    )

    /** 通用说明文字：12.5sp / 400 / lh 1.5。 */
    val caption = TextStyle(
        fontSize = 12.5.sp,
        fontWeight = FontWeight.Normal,
        lineHeight = 18.75.sp,
    )
}
