// =====** 视频解码插件（Flutter 侧入口） =====**
//
// 层级：Android 客户端 / 视频解码
// 边界：只做 MethodChannel 参数校验与线程切换，解码逻辑在 H264Decoder。
//
// 通道契约（Dart: app/lib/model/video/android_video_decoder.dart）：
//   create    {width:int, height:int} → {textureId:int, width:int, height:int}
//   configure {width:int, height:int, fps:int} → null
//   push      {data:Uint8List} → null
//   stats     {} → {textureId, decoded, dropped, latencyP50Us, latencyP99Us,
//                  fps, width, height, codec, paused, surfaceRebuilds, lastError}
//   pause / resume → null
//   dispose   {} → null
//
// 前后台：插件注册 Activity 生命周期观察者；onPause 暂停解码并释放 Surface，
// onResume 重建 Surface（textureId 不变）。

package com.rdc.rdc.video

import android.content.Context
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.embedding.engine.plugins.activity.ActivityAware
import io.flutter.embedding.engine.plugins.activity.ActivityPluginBinding
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.view.TextureRegistry

/// Flutter 插件：把 `rdc/video_decoder` 通道接到 [H264Decoder]。
class VideoDecoderPlugin :
    FlutterPlugin,
    MethodChannel.MethodCallHandler,
    ActivityAware {

    companion object {
        /// 与 Dart 侧 `AndroidVideoDecoder.channelName` 必须一致。
        const val CHANNEL = "rdc/video_decoder"
    }

    private var channel: MethodChannel? = null
    private var decoder: H264Decoder? = null
    private var lifecycle: Lifecycle? = null

    /// 重建解码器实例所需的引擎资源（dispose 后 create 会新建实例）。
    private var registry: TextureRegistry? = null
    private var appContext: Context? = null

    /// 前后台观察者：后台暂停解码，前台重建 Surface（保留 textureId）。
    private val observer = object : DefaultLifecycleObserver {
        override fun onResume(owner: LifecycleOwner) {
            decoder?.resume()
        }

        override fun onPause(owner: LifecycleOwner) {
            decoder?.pause()
        }
    }

    // =====** FlutterPlugin =====**

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        registry = binding.textureRegistry
        appContext = binding.applicationContext
        val created = H264Decoder(binding.textureRegistry, binding.applicationContext)
        decoder = created
        channel = MethodChannel(binding.binaryMessenger, CHANNEL).also {
            it.setMethodCallHandler(this)
        }
    }

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        channel?.setMethodCallHandler(null)
        channel = null
        decoder?.dispose()
        decoder = null
        registry = null
        appContext = null
    }

    // =====** MethodChannel =====**

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        // `create` 允许在 dispose 之后重新打开：已释放的实例不可复用，必须新建。
        // 否则 `released`/`paused` 会带着上一周期的值，解码泵与入队全部静默失效（黑屏）。
        val target = if (call.method == "create") {
            val current = decoder
            if (current == null || current.isDisposed) {
                val registry = this.registry
                val context = this.appContext
                if (registry == null || context == null) {
                    result.error("unattached", "解码插件未绑定到 FlutterEngine", null)
                    return
                }
                H264Decoder(registry, context).also { decoder = it }
            } else {
                current
            }
        } else {
            decoder
        }
        if (target == null) {
            result.error("unattached", "解码插件未绑定到 FlutterEngine", null)
            return
        }
        try {
            when (call.method) {
                "create" -> result.success(
                    target.create(
                        width = call.argument<Int>("width") ?: 0,
                        height = call.argument<Int>("height") ?: 0,
                    ),
                )

                "configure" -> {
                    target.configure(
                        width = call.argument<Int>("width") ?: 0,
                        height = call.argument<Int>("height") ?: 0,
                        fps = call.argument<Int>("fps") ?: 60,
                    )
                    result.success(null)
                }

                "push" -> {
                    val data = call.argument<ByteArray>("data")
                    if (data == null) {
                        result.error("bad_args", "push 缺少 data", null)
                    } else {
                        target.push(data)
                        result.success(null)
                    }
                }

                "stats" -> result.success(target.stats())
                "pause" -> {
                    target.pause()
                    result.success(null)
                }

                "resume" -> {
                    target.resume()
                    result.success(null)
                }

                "dispose" -> {
                    target.dispose()
                    result.success(null)
                }

                else -> result.notImplemented()
            }
        } catch (error: Throwable) {
            result.error(
                "decoder_error",
                "${error.javaClass.simpleName}: ${error.message}",
                null,
            )
        }
    }

    // =====** ActivityAware =====**

    override fun onAttachedToActivity(binding: ActivityPluginBinding) {
        // Flutter 的 ActivityPluginBinding.getLifecycle() 返回 Object（引擎刻意
        // 不硬依赖 androidx.lifecycle）；FlutterActivity 本身是 LifecycleOwner，
        // 因此从 Activity 取 Lifecycle。
        val activity = binding.activity
        if (activity is LifecycleOwner) {
            observe(activity.lifecycle)
        }
    }

    override fun onDetachedFromActivityForConfigChanges() {
        stopObserving()
    }

    override fun onReattachedToActivityForConfigChanges(binding: ActivityPluginBinding) {
        val activity = binding.activity
        if (activity is LifecycleOwner) {
            observe(activity.lifecycle)
        }
    }

    override fun onDetachedFromActivity() {
        stopObserving()
        // 只暂停、不释放纹理：纹理属于 FlutterEngine，Activity 重建后仍可复用；
        // 真正释放留给 onDetachedFromEngine。
        decoder?.pause()
    }

    private fun observe(target: Lifecycle) {
        stopObserving()
        lifecycle = target
        target.addObserver(observer)
    }

    private fun stopObserving() {
        lifecycle?.removeObserver(observer)
        lifecycle = null
    }
}
