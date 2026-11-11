package com.bnagent.shiyu

import android.app.Application
import com.bnagent.shiyu.data.AppContainer

/**
 * 拾语 Application。
 *
 * 进程内唯一 [AppContainer]：持有 DataStore 仓库、OkHttp 客户端与首装种子写入。
 */
class ShiYuApplication : Application() {

    lateinit var container: AppContainer
        private set

    override fun onCreate() {
        super.onCreate()
        container = AppContainer(this)
        container.initialize()
    }
}
