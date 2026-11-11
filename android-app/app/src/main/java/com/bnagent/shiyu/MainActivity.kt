package com.bnagent.shiyu

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.bnagent.shiyu.data.LocalAppContainer
import com.bnagent.shiyu.ui.ShiYuApp
import com.bnagent.shiyu.ui.theme.DataStoreThemeController
import com.bnagent.shiyu.ui.theme.LocalThemeController
import com.bnagent.shiyu.ui.theme.ShiYuTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Android 15+ 强制 edge-to-edge；显式开启保证全版本一致。
        enableEdgeToEdge()
        setContent {
            // 主题控制器在进程内只建一个（DataStore 单例约束），根节点 remember 即可。
            val themeController = remember { DataStoreThemeController(applicationContext) }
            val settings by themeController.settings.collectAsStateWithLifecycle()
            val container = (application as ShiYuApplication).container

            CompositionLocalProvider(
                LocalThemeController provides themeController,
                LocalAppContainer provides container,
            ) {
                ShiYuTheme(settings = settings) {
                    ShiYuApp()
                }
            }
        }
    }
}
