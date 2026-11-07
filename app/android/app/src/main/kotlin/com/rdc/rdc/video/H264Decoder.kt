// =====** H.264 硬解 + 外部纹理 =====**
//
// 层级：Android 客户端 / 视频解码
// 边界：接收 Annex-B 字节流 → MediaCodec(video/avc) → SurfaceTexture（Flutter 外部纹理）。
//       全程 GPU 直出：输出缓冲区只 releaseOutputBuffer(render=true)，不做像素回读。
//
// 线程模型：TextureRegistry 的创建/释放必须在主线程；解码、排队、统计全部在独立
// HandlerThread 上串行执行，避免 MediaCodec 的输入/输出状态被多线程并发访问。

package com.rdc.rdc.video

import android.content.Context
import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.SystemClock
import android.view.Surface
import io.flutter.view.TextureRegistry
import java.nio.ByteBuffer
import java.util.ArrayDeque

/// H.264 解码器：一个实例对应一个 Flutter 外部纹理。
class H264Decoder(
    private val registry: TextureRegistry,
    @Suppress("UNUSED_PARAMETER") context: Context,
) {
    companion object {
        /// H.264 基线/主档 MIME。
        const val MIME_AVC = "video/avc"
        /// 等待队列上限（帧）。超过后丢弃到下一个 IDR，避免延迟无限增长。
        const val MAX_PENDING_FRAMES = 6
        /// 解码泵轮询间隔（毫秒）；30fps 下单帧 33ms，5ms 足够跟得上。
        const val PUMP_INTERVAL_MS = 5L
        /// 延迟直方图保留的样本数（约 8 秒 @30fps）。
        const val LATENCY_WINDOW = 256
    }

    // =====** 纹理与表面（主线程创建，解码线程使用） =====**

    private var producer: TextureRegistry.SurfaceProducer? = null
    /// Surface 是否被引擎回收过（后台/尺寸切换），下次启动必须取新 Surface。
    private var surfaceCleanupSeen = false
    /// Flutter 合成必须在主线程触发。
    private val mainHandler: Handler by lazy { Handler(Looper.getMainLooper()) }
    private var textureId: Long = -1
    private var width = 0
    private var height = 0
    private var fps = 60

    // =====** MediaCodec（只在解码线程访问） =====**

    private var codec: MediaCodec? = null
    private var sps: ByteArray? = null
    private var pps: ByteArray? = null
    /// 首个 IDR 门控（纯逻辑，JVM 单测覆盖）。
    private val gate = FrameGate()
    private var paused = false
    private var released = false
    /// 是否已被 `dispose`（实例不可复用；插件会在下次 `create` 时新建实例）。
    ///
    /// 为什么不能复用：`released`/`paused` 由解码线程异步置位，复用实例时新周期
    /// 可能先看到旧值，导致 `pump` 直接退出、所有 `push` 被丢弃（黑屏）。
    @Volatile
    private var disposed = false
    private val pending = ArrayDeque<FeedChunk>()

    /// 实例是否已释放；`true` 表示调用方必须新建实例再 `create`。
    val isDisposed: Boolean
        get() = disposed

    // =====** 线程 =====**

    private var thread: HandlerThread? = null
    private var handler: Handler? = null

    // =====** 统计（解码线程写，stats() 在解码线程读） =====**

    private var decoded = 0L
    private var dropped = 0L
    private var surfaceRebuilds = 0L
    private var lastError: String? = null
    private val latencies = LongArray(LATENCY_WINDOW)
    private var latencyCount = 0
    private var latencyCursor = 0
    private var fpsWindowStartNs = 0L
    private var fpsWindowFrames = 0
    private var measuredFps = 0.0

    /// 提交给解码线程的一帧。
    private class FeedChunk(val data: ByteArray, val ptsUs: Long, val keyFrame: Boolean)

    // =====** 生命周期 =====**

    /// 创建外部纹理。
    ///
    /// # Arguments
    /// * `width`/`height` - 初始视频尺寸（0 表示未知，需等 `configure`）
    ///
    /// # Returns
    /// `textureId` 与尺寸；重复调用返回同一个纹理。
    ///
    /// # Errors
    /// 纹理注册失败时抛出异常，由插件转成 `PlatformException`。
    fun create(width: Int, height: Int): Map<String, Any> {
        producer?.let {
            return mapOf(
                "textureId" to textureId,
                "width" to this.width,
                "height" to this.height,
            )
        }
        // 本 SDK 的 Flutter 引擎使用 SurfaceProducer 代替旧的
        // SurfaceTextureEntry.markTextureFrameAvailable()：每帧解码后由
        // scheduleFrame() 通知合成器，不是 CPU 回读。
        val created = registry.createSurfaceProducer()
        created.setCallback(
            object : TextureRegistry.SurfaceProducer.Callback {
                override fun onSurfaceCreated() {
                    // 新 Surface 就绪；真正取值在下次 ensureCodec。
                }

                override fun onSurfaceCleanup() {
                    // 引擎回收了 Surface：必须停用旧 Surface，否则会花屏。
                    submit({ decoder ->
                        decoder.surfaceCleanupSeen = true
                        decoder.releaseCodec()
                    }, "surfaceCleanup")
                }
            },
        )
        if (width > 0 && height > 0) {
            created.setSize(width, height)
        }
        producer = created
        textureId = created.id()
        this.width = width
        this.height = height

        val worker = HandlerThread("rdc-h264").also { it.start() }
        thread = worker
        handler = Handler(worker.looper).also { h ->
            h.postDelayed(pump, PUMP_INTERVAL_MS)
        }
        return mapOf("textureId" to textureId, "width" to width, "height" to height)
    }

    /// 更新视频尺寸（远端分辨率或旋转变化时调用）。
    ///
    /// 只改外部纹理的缓冲尺寸，**不更换 textureId**，因此 Flutter 侧的
    /// `Texture` 组件无需重建；若解码器已在运行，则等待下一帧 SPS/PPS 后重建。
    fun configure(width: Int, height: Int, fps: Int) {
        // setSize 必须在平台线程调用（通道回调本身就在主线程）。
        if (width > 0 && height > 0) {
            producer?.setSize(width, height)
        }
        submit({ decoder ->
            decoder.width = width
            decoder.height = height
            decoder.fps = fps.coerceIn(1, 240)
            if (decoder.codec != null) {
                decoder.releaseCodec()
                decoder.gate.reset()
            }
        }, "configure")
    }

    /// 推入一段 Annex-B 码流（可含多帧）。
    fun push(data: ByteArray) {
        submit({ decoder -> decoder.enqueue(data) }, "push")
    }

    /// 暂停解码（App 进入后台）：释放解码器与 Surface，保留纹理。
    fun pause() {
        submit({ decoder ->
            decoder.paused = true
            decoder.pending.clear()
            decoder.releaseCodec()
        }, "pause")
    }

    /// 恢复解码（App 回到前台）：重建 Surface 与解码器，等下一个 IDR。
    ///
    /// 为什么重建 Surface 而不是复用：后台期间系统可能回收 GPU 资源，
    /// 直接复用旧 Surface 在部分机型上会花屏；textureId 不变，Flutter 无感。
    fun resume() {
        submit({ decoder ->
            decoder.paused = false
            decoder.gate.reset()
        }, "resume")
    }

    /// 释放全部资源（纹理、Surface、MediaCodec、线程）。
    ///
    /// 释放后本实例不可再用：`isDisposed` 立即为 `true`，调用方须新建实例。
    fun dispose() {
        // 同步置位：必须在返回前让调用方看到，避免新建实例的判断落空。
        disposed = true
        val worker = handler
        val textureProducer = producer
        producer = null
        if (worker != null) {
            worker.post {
                released = true
                pauseInternal()
                worker.looper.quitSafely()
            }
        } else {
            released = true
        }
        textureProducer?.release()
        handler = null
        thread = null
        textureId = -1
    }

    /// 统计快照（在解码线程上取，保证与计数器一致）。
    ///
    /// `p50/p99` 是最近 [LATENCY_WINDOW] 帧的近似分位（排序后取值），
    /// 统计的是“入队时间 → releaseOutputBuffer(render=true)”的本地解码+渲染耗时，
    /// 不含网络传输。
    fun stats(): Map<String, Any> {
        var result = emptyMap<String, Any>()
        submit({ decoder ->
            decoder.drainOutput()
            val sorted = decoder.latencySamples().sorted()
            val p50 = if (sorted.isEmpty()) 0L else sorted[sorted.size / 2]
            val p99 = if (sorted.isEmpty()) 0L else
                sorted[((sorted.size - 1) * 0.99).toInt()]
            result = mapOf(
                "textureId" to decoder.textureId,
                "decoded" to decoder.decoded,
                "dropped" to decoder.dropped,
                "latencyP50Us" to p50,
                "latencyP99Us" to p99,
                "fps" to decoder.measuredFps,
                "width" to decoder.width,
                "height" to decoder.height,
                "codec" to (decoder.codec?.name ?: "none"),
                "paused" to decoder.paused,
                "surfaceRebuilds" to decoder.surfaceRebuilds,
                "lastError" to (decoder.lastError ?: ""),
            )
        }, "stats", wait = true)
        return result
    }

    // =====** 解码线程入口 =====**

    /// 把操作投递到解码线程；`wait=true` 时等待执行完成（用于 stats）。
    private fun submit(
        action: (H264Decoder) -> Unit,
        operation: String,
        wait: Boolean = false,
    ) {
        val target = handler
        if (target == null) {
            lastError = "$operation: 解码线程未启动"
            return
        }
        val runnable = Runnable {
            try {
                action(this)
            } catch (error: Throwable) {
                lastError = "$operation: ${error.javaClass.simpleName}: ${error.message}"
            }
        }
        if (!wait) {
            target.post(runnable)
            return
        }
        val latch = java.util.concurrent.CountDownLatch(1)
        target.post {
            runnable.run()
            latch.countDown()
        }
        latch.await(200, java.util.concurrent.TimeUnit.MILLISECONDS)
    }

    /// 解码泵：喂输入 + 收输出；解码器运行时持续自调度。
    private val pump = object : Runnable {
        override fun run() {
            if (released) {
                return
            }
            if (!paused) {
                feedPending()
                drainOutput()
            }
            handler?.postDelayed(this, PUMP_INTERVAL_MS)
        }
    }

    // =====** 输入 =====**

    /// 解析并排队一段码流（解码线程）。
    private fun enqueue(data: ByteArray) {
        if (released || paused || data.isEmpty()) {
            return
        }
        val units = AnnexBParser.split(data)
        if (units.isEmpty()) {
            // 不是 Annex-B（或分片未到齐）：计为丢弃，但不崩溃。
            dropped += 1
            return
        }
        AnnexBParser.parameterSets(units).let { (newSps, newPps) ->
            if (newSps != null) {
                sps = newSps
            }
            if (newPps != null) {
                pps = newPps
            }
        }
        val accessUnit = AnnexBParser.accessUnit(units) ?: return
        val keyFrame = AnnexBParser.isKeyFrame(units)

        // 关键帧之前的数据喂进去只会花屏，直接丢到下一个 IDR。
        if (!gate.accept(keyFrame)) {
            dropped += 1
            return
        }

        // 队列过深说明消费跟不上：丢到下一个 IDR，宁可花一次也不让延迟累积。
        if (pending.size >= MAX_PENDING_FRAMES) {
            dropped += 1
            pending.clear()
            gate.reset()
            if (!gate.accept(keyFrame)) {
                return
            }
        }

        ensureCodec()
        if (codec == null) {
            dropped += 1
            return
        }
        pending.addLast(
            FeedChunk(
                data = accessUnit,
                ptsUs = SystemClock.elapsedRealtimeNanos() / 1000,
                keyFrame = keyFrame,
            ),
        )
    }

    /// 懒启动解码器：必须已有 SPS/PPS 与有效尺寸。
    private fun ensureCodec() {
        if (codec != null || paused || released) {
            return
        }
        val spsBytes = sps ?: return
        val ppsBytes = pps ?: return
        if (width <= 0 || height <= 0) {
            return
        }
        val targetSurface = acquireSurface() ?: return
        val format = MediaFormat.createVideoFormat(MIME_AVC, width, height)
        format.setByteBuffer("csd-0", ByteBuffer.wrap(spsBytes))
        format.setByteBuffer("csd-1", ByteBuffer.wrap(ppsBytes))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            // 低延迟解码：不缓冲多余帧；部分解码器不支持，失败后退回普通模式。
            format.setInteger(MediaFormat.KEY_OPERATING_RATE, fps)
            format.setInteger(MediaFormat.KEY_PRIORITY, 0)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
        }

        codec = try {
            startCodec(format, targetSurface, lowLatency = true)
        } catch (first: Throwable) {
            // KEY_LOW_LATENCY 在个别解码器上会直接 configure 失败，退回普通路径。
            val fallback = MediaFormat.createVideoFormat(MIME_AVC, width, height)
            fallback.setByteBuffer("csd-0", ByteBuffer.wrap(spsBytes))
            fallback.setByteBuffer("csd-1", ByteBuffer.wrap(ppsBytes))
            try {
                startCodec(fallback, targetSurface, lowLatency = false)
            } catch (second: Throwable) {
                // Surface 归 SurfaceProducer 所有，不能在这里 release。
                lastError = "MediaCodec 配置失败: ${second.javaClass.simpleName}: ${second.message}"
                null
            }
        }
        if (codec != null) {
            surfaceRebuilds += 1
            fpsWindowStartNs = SystemClock.elapsedRealtimeNanos()
            fpsWindowFrames = 0
        }
    }

    /// 取当前可用的输出 Surface。
    ///
    /// SurfaceProducer 在引擎回收 Surface（后台/trim）后，旧 Surface 不可再写；
    /// `getForcedNewSurface()` 会返回新的 Surface，textureId 保持不变。
    private fun acquireSurface(): Surface? {
        val active = producer ?: return null
        return if (surfaceCleanupSeen) {
            surfaceCleanupSeen = false
            active.forcedNewSurface
        } else {
            active.surface
        }
    }

    /// 配置并启动一个 `MediaCodec` 实例。
    private fun startCodec(format: MediaFormat, target: Surface, lowLatency: Boolean): MediaCodec {
        // KEY_LOW_LATENCY 只在 API 30+ 设置；removeKey 本身要求 API 29+。
        if (!lowLatency && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            format.removeKey(MediaFormat.KEY_LOW_LATENCY)
        }
        val created = MediaCodec.createDecoderByType(MIME_AVC)
        try {
            created.configure(format, target, null, 0)
            created.start()
            return created
        } catch (error: Throwable) {
            created.release()
            throw error
        }
    }

    /// 把队列里的帧送进解码器；输入缓冲满时留到下一轮泵。
    private fun feedPending() {
        val active = codec ?: return
        while (pending.isNotEmpty()) {
            val chunk = pending.removeFirst()
            val inputIndex = active.dequeueInputBuffer(0)
            if (inputIndex < 0) {
                pending.addFirst(chunk)
                return
            }
            val buffer = active.getInputBuffer(inputIndex) ?: continue
            buffer.clear()
            buffer.put(chunk.data)
            val flags = if (chunk.keyFrame) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0
            active.queueInputBuffer(inputIndex, 0, chunk.data.size, chunk.ptsUs, flags)
        }
    }

    /// 收输出：`render=true` 表示直接渲染到 SurfaceTexture，不做像素回读。
    private fun drainOutput() {
        val active = codec ?: return
        while (true) {
            val info = MediaCodec.BufferInfo()
            val outputIndex = try {
                active.dequeueOutputBuffer(info, 0)
            } catch (error: IllegalStateException) {
                lastError = "dequeueOutputBuffer: ${error.message}"
                return
            }
            if (outputIndex == MediaCodec.INFO_TRY_AGAIN_LATER) {
                return
            }
            if (outputIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                // 输出格式在 configure 时已确定，这里无需处理，继续收帧。
                continue
            }
            if (outputIndex >= 0) {
                if (info.size > 0) {
                    val nowUs = SystemClock.elapsedRealtimeNanos() / 1000
                    recordLatency(nowUs - info.presentationTimeUs)
                    decoded += 1
                    countFpsFrame()
                    // render=true：直接渲染到外部纹理，GPU 直出，不回读像素。
                    active.releaseOutputBuffer(outputIndex, true)
                    // SurfaceProducer 需要显式通知合成器（本 SDK 无 markTextureFrameAvailable）。
                    mainHandler.post { producer?.scheduleFrame() }
                } else {
                    active.releaseOutputBuffer(outputIndex, false)
                }
            } else {
                // 其它负值（INFO_OUTPUT_BUFFERS_CHANGED 等）不消费缓冲区，结束本轮。
                return
            }
        }
    }

    // =====** 内部释放 =====**

    /// 只释放解码器与 Surface，不释放纹理（pause 复用）。
    private fun releaseCodec() {
        pending.clear()
        codec?.let {
            try {
                it.stop()
            } catch (_: IllegalStateException) {
                // 已经停止/错误状态：忽略，继续 release。
            }
            it.release()
        }
        codec = null
        // Surface 由 SurfaceProducer 持有，MediaCodec.release 会自动解绑；
        // 这里不能 release Surface，否则后续 scheduleFrame 会拿到失效缓冲。
    }

    /// 彻底暂停并释放（dispose 使用）。
    private fun pauseInternal() {
        paused = true
        releaseCodec()
    }

    // =====** 统计辅助 =====**

    private fun recordLatency(latencyUs: Long) {
        // 负值/异常值（时钟回拨）不进入分位统计。
        if (latencyUs < 0) {
            return
        }
        latencies[latencyCursor] = latencyUs
        latencyCursor = (latencyCursor + 1) % LATENCY_WINDOW
        if (latencyCount < LATENCY_WINDOW) {
            latencyCount += 1
        }
    }

    private fun latencySamples(): LongArray =
        latencies.copyOfRange(0, latencyCount)

    private fun countFpsFrame() {
        fpsWindowFrames += 1
        val nowNs = SystemClock.elapsedRealtimeNanos()
        val elapsedNs = nowNs - fpsWindowStartNs
        if (elapsedNs >= 1_000_000_000L) {
            measuredFps = (fpsWindowFrames * 1_000_000_000.0) / elapsedNs
            fpsWindowStartNs = nowNs
            fpsWindowFrames = 0
        }
    }
}
