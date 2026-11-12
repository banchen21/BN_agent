package com.bnagent.shiyu.domain.model

import kotlinx.serialization.Serializable

/**
 * 我方资料（设计规格 6.1）：苏雨 / shiyu_2049 / 上海 / 签名。
 *
 * 默认值即 mock 首装数据；「我」页与消息头像使用。
 */
@Serializable
data class UserProfile(
    val name: String = "苏雨",
    val handle: String = "shiyu_2049",
    val city: String = "上海",
    val signature: String = "先把话说清楚，再把事做漂亮。",
    /** drawable 资源名。 */
    val avatar: String = "avatar_me",
) {
    /** 我页拾语号行：`拾语号 shiyu_2049 · 上海`。 */
    val handleLine: String
        get() = if (city.isBlank()) "拾语号 $handle" else "拾语号 $handle · $city"
}
