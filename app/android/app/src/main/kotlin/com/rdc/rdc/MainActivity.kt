package com.rdc.rdc

import com.rdc.rdc.video.VideoDecoderPlugin
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

class MainActivity : FlutterActivity() {
    /// 注册应用内插件（H.264 解码 + 外部纹理）。
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        flutterEngine.plugins.add(VideoDecoderPlugin())
    }
}
