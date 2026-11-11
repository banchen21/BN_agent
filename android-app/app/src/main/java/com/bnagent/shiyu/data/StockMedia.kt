package com.bnagent.shiyu.data

import com.bnagent.shiyu.R

/**
 * 内置素材映射（design-spec 第 8 节）。
 *
 * 资源文件位于 `res/drawable-nodpi/`：
 * avatar_me / avatar_lin / avatar_chen / avatar_zhao / avatar_wu / avatar_team、
 * photo_cat / photo_coffee / photo_desk。
 *
 * 领域模型里只存资源名字符串（如 `avatar_lin`），UI 用 [drawableFor] 换成
 * Coil `AsyncImage` 可直接消费的 drawable id。
 */
object StockMedia {

    const val AVATAR_ME = "avatar_me"
    const val AVATAR_LIN = "avatar_lin"
    const val AVATAR_CHEN = "avatar_chen"
    const val AVATAR_ZHAO = "avatar_zhao"
    const val AVATAR_WU = "avatar_wu"
    const val AVATAR_TEAM = "avatar_team"

    const val PHOTO_CAT = "photo_cat"
    const val PHOTO_COFFEE = "photo_coffee"
    const val PHOTO_DESK = "photo_desk"

    /** 图片发送池（循环使用）：cat → coffee → desk。 */
    data class Photo(val asset: String, val alt: String)

    /** 文件选择池。 */
    data class FileTemplate(val name: String, val sizeLabel: String)

    val photos: List<Photo> = listOf(
        Photo(PHOTO_CAT, "橙色猫咪躺在沙发上"),
        Photo(PHOTO_COFFEE, "桌上一杯拉花咖啡"),
        Photo(PHOTO_DESK, "白桌上的笔记本与书"),
    )

    val files: List<FileTemplate> = listOf(
        FileTemplate("拾语视觉规范_v3.pdf", "2.4 MB"),
        FileTemplate("气泡组件.fig", "8.1 MB"),
        FileTemplate("聊天页走查清单.xlsx", "126 KB"),
    )

    /** 按全局计数循环取图片（负数也能安全取模）。 */
    fun photoFor(index: Int): Photo = photos[Math.floorMod(index, photos.size)]

    /** 资源名 → drawable id；未知名字返回 null，UI 走文字头像 / 占位。 */
    fun drawableFor(asset: String): Int? = when (asset) {
        AVATAR_ME -> R.drawable.avatar_me
        AVATAR_LIN -> R.drawable.avatar_lin
        AVATAR_CHEN -> R.drawable.avatar_chen
        AVATAR_ZHAO -> R.drawable.avatar_zhao
        AVATAR_WU -> R.drawable.avatar_wu
        AVATAR_TEAM -> R.drawable.avatar_team
        PHOTO_CAT -> R.drawable.photo_cat
        PHOTO_COFFEE -> R.drawable.photo_coffee
        PHOTO_DESK -> R.drawable.photo_desk
        else -> null
    }
}
