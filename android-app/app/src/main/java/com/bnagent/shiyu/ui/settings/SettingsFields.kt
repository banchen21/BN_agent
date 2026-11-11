package com.bnagent.shiyu.ui.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
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
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.bnagent.shiyu.ui.theme.ShiYuTheme

/** 设置页输入框标签：13sp/600 ink-2。 */
@Composable
internal fun SettingsFieldLabel(text: String) {
    Text(
        text = text,
        color = ShiYuTheme.colors.ink2,
        fontSize = 13.sp,
        fontWeight = FontWeight.SemiBold,
        lineHeight = 18.sp,
    )
}

/**
 * 服务器设置输入框：高 46、圆角 14、surface 底 + 1px line；
 * 聚焦时边框变 accent 并带 3dp 外发光（与搜索框口径一致）。
 *
 * @param password true 时按密码态显示（不可见字符）
 */
@Composable
internal fun SettingsTextField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    password: Boolean = false,
    keyboardType: KeyboardType = KeyboardType.Text,
    imeAction: ImeAction = ImeAction.Done,
) {
    val colors = ShiYuTheme.colors
    var focused by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(14.dp)
    Box(
        modifier = modifier
            .fillMaxWidth()
            .height(46.dp)
            .drawBehind {
                if (focused) {
                    val stroke = 3.dp.toPx()
                    drawRoundRect(
                        color = FieldFocusGlow,
                        topLeft = Offset(-stroke, -stroke),
                        size = Size(size.width + stroke * 2, size.height + stroke * 2),
                        cornerRadius = CornerRadius(17.dp.toPx()),
                        style = Stroke(stroke),
                    )
                }
            }
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, if (focused) colors.accent else colors.line, shape),
        contentAlignment = Alignment.CenterStart,
    ) {
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            modifier = Modifier
                .fillMaxWidth()
                .onFocusChanged { focused = it.isFocused }
                .padding(horizontal = 14.dp),
            textStyle = TextStyle(
                fontSize = 15.5.sp,
                fontWeight = FontWeight.Normal,
                lineHeight = 21.sp,
                color = colors.ink,
            ),
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = keyboardType, imeAction = imeAction),
            visualTransformation = if (password) PasswordVisualTransformation() else VisualTransformation.None,
            cursorBrush = SolidColor(colors.accent),
            decorationBox = { innerTextField ->
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.CenterStart) {
                    if (value.isEmpty()) {
                        Text(
                            text = placeholder,
                            color = colors.ink3,
                            fontSize = 15.5.sp,
                            lineHeight = 21.sp,
                        )
                    }
                    innerTextField()
                }
            },
        )
    }
}

/** 输入框聚焦外发光 rgba(201,59,36,.14)。 */
internal val FieldFocusGlow = Color(0x24C93B24)
