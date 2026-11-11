package com.bnagent.shiyu.ui.components

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.ArrowUpward
import androidx.compose.material.icons.rounded.CallEnd
import androidx.compose.material.icons.rounded.ChatBubble
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.ChevronRight
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Contacts
import androidx.compose.material.icons.rounded.DarkMode
import androidx.compose.material.icons.rounded.Description
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.FlipCameraAndroid
import androidx.compose.material.icons.rounded.Image
import androidx.compose.material.icons.rounded.Info
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.Lock
import androidx.compose.material.icons.rounded.Mic
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.Notifications
import androidx.compose.material.icons.rounded.NotificationsOff
import androidx.compose.material.icons.rounded.Palette
import androidx.compose.material.icons.rounded.Person
import androidx.compose.material.icons.rounded.Phone
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.Videocam
import androidx.compose.material.icons.rounded.VolumeUp
import androidx.compose.ui.graphics.vector.ImageVector

/**
 * 拾语图标表（设计规格 3.12，27 个）。
 *
 * 统一从 material-icons-extended 的 Rounded 集取，业务页面不要直接引用
 * `Icons.*`，通过 `ShiYuIcons.xxx` 引用，便于以后整批替换成自绘图标。
 * 电话挂断：设计稿为 phone 旋转 135°，这里用语义更准确的 CallEnd。
 */
object ShiYuIcons {
    val Back: ImageVector = Icons.AutoMirrored.Rounded.ArrowBack
    val Chevron: ImageVector = Icons.Rounded.ChevronRight
    val Down: ImageVector = Icons.Rounded.KeyboardArrowDown
    val Search: ImageVector = Icons.Rounded.Search
    val Plus: ImageVector = Icons.Rounded.Add
    val More: ImageVector = Icons.Rounded.MoreVert
    val Phone: ImageVector = Icons.Rounded.Phone
    val Video: ImageVector = Icons.Rounded.Videocam
    val Mic: ImageVector = Icons.Rounded.Mic
    val Speaker: ImageVector = Icons.Rounded.VolumeUp
    val Image: ImageVector = Icons.Rounded.Image
    val File: ImageVector = Icons.Rounded.Description
    val Download: ImageVector = Icons.Rounded.Download
    val Close: ImageVector = Icons.Rounded.Close
    val Play: ImageVector = Icons.Rounded.PlayArrow
    val Check: ImageVector = Icons.Rounded.Check
    val Bell: ImageVector = Icons.Rounded.Notifications
    val BellOff: ImageVector = Icons.Rounded.NotificationsOff
    val Message: ImageVector = Icons.Rounded.ChatBubble
    val Contacts: ImageVector = Icons.Rounded.Contacts
    val Me: ImageVector = Icons.Rounded.Person
    val Palette: ImageVector = Icons.Rounded.Palette
    val Moon: ImageVector = Icons.Rounded.DarkMode
    val Info: ImageVector = Icons.Rounded.Info
    val Send: ImageVector = Icons.Rounded.ArrowUpward
    val Flip: ImageVector = Icons.Rounded.FlipCameraAndroid
    val Lock: ImageVector = Icons.Rounded.Lock
    val Hangup: ImageVector = Icons.Rounded.CallEnd
}
