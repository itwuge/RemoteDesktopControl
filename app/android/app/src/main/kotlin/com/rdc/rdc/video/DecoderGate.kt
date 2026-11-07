// =====** 解码启动门控 =====**
//
// 层级：Android 客户端 / 视频解码
// 用途：H.264 解码器必须从 IDR（关键帧）开始；IDR 之前的 P/B 帧没有参考帧，
//      喂进去既解不出画面，又可能让部分硬解器进入错误状态。
//
// 为什么单独成类：MediaCodec/Surface 依赖 Android SDK，宿主 JVM 单测不能实例化
// H264Decoder；把纯判定抽出来后，判据「IDR 前不启动解码」可以在构建机上验证，
// MediaCodec 的实际行为仍留给真机。

package com.rdc.rdc.video

/// 关键帧门控（非线程安全，只在解码线程使用）。
class FrameGate {
    /// 是否仍在等待首个关键帧。
    var waitingForKeyFrame = true
        private set

    /// 判定一个访问单元是否放行。
    ///
    /// 返回：等待期内的非关键帧返回 false；首个关键帧或已开门后返回 true。
    fun accept(keyFrame: Boolean): Boolean {
        if (waitingForKeyFrame && !keyFrame) {
            return false
        }
        waitingForKeyFrame = false
        return true
    }

    /// 重新等待关键帧（暂停/恢复/分辨率切换/队列丢弃后）。
    fun reset() {
        waitingForKeyFrame = true
    }
}
