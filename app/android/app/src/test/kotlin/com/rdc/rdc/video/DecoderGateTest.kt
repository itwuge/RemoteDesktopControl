// =====** 解码门控 JVM 单测 =====**
//
// 判据：首个 IDR 之前不启动/不放行；重置后必须再等一个关键帧。
// 这些用例不依赖 Android SDK，构建机即可判定「IDR 前不启动解码」的纯逻辑部分。

package com.rdc.rdc.video

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DecoderGateTest {
    @Test
    fun `non keyframe before idr is rejected`() {
        val gate = FrameGate()
        assertFalse(gate.accept(false))
        assertTrue(gate.waitingForKeyFrame)
    }

    @Test
    fun `idr opens the gate and following p frames pass`() {
        val gate = FrameGate()
        assertTrue(gate.accept(true))
        assertFalse(gate.waitingForKeyFrame)
        assertTrue(gate.accept(false))
        assertTrue(gate.accept(false))
    }

    @Test
    fun `reset waits for a new idr`() {
        val gate = FrameGate()
        assertTrue(gate.accept(true))
        gate.reset()
        assertTrue(gate.waitingForKeyFrame)
        assertFalse(gate.accept(false))
        assertTrue(gate.accept(true))
    }
}
