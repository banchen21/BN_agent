package com.bnagent.shiyu.data

import kotlinx.serialization.json.Json

/**
 * 数据层共用 JSON 配置：
 * - `ignoreUnknownKeys`：后端多返回字段（token 缓存、tool_calls 等）不导致解析失败；
 * - `encodeDefaults`：持久化时写全字段，升级模型后旧数据仍可读；
 * - 默认 `classDiscriminator = "type"`：Message 多态靠它区分 text/image/file/voice/system。
 */
val ShiYuJson: Json = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
    explicitNulls = true
}
