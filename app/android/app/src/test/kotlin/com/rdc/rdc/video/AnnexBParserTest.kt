// Annex-B 解析器 JVM 单测（不依赖 Android 运行时）。
//
// 覆盖：3/4 字节起始码、NAL 类型识别、SPS/PPS 重新打包、IDR 判定、
// 访问单元只保留 VCL 片、起始码之间的填充零裁剪。

package com.rdc.rdc.video

import java.io.ByteArrayOutputStream
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AnnexBParserTest {
    /// 构造一个指定 nal_unit_type 的 NAL（头字节 0x60|type，负载无 0x00 序列）。
    private fun nal(type: Int, payloadSize: Int): ByteArray {
        val bytes = ByteArray(payloadSize + 1)
        bytes[0] = (0x60 or type).toByte()
        for (index in 1 until bytes.size) {
            bytes[index] = (0x10 + index).toByte()
        }
        return bytes
    }

    /// 用 4 字节起始码拼多个 NAL。
    private fun annexB(vararg units: ByteArray): ByteArray {
        val out = ByteArrayOutputStream()
        for (unit in units) {
            out.write(byteArrayOf(0, 0, 0, 1))
            out.write(unit)
        }
        return out.toByteArray()
    }

    @Test
    fun split_readsNalTypesInOrder() {
        val stream = annexB(
            nal(AnnexBParser.NAL_AUD, 2),
            nal(AnnexBParser.NAL_SPS, 8),
            nal(AnnexBParser.NAL_PPS, 4),
            nal(AnnexBParser.NAL_IDR, 20),
            nal(AnnexBParser.NAL_SLICE, 12),
        )
        val units = AnnexBParser.split(stream)
        assertEquals(5, units.size)
        assertEquals(
            listOf(
                AnnexBParser.NAL_AUD,
                AnnexBParser.NAL_SPS,
                AnnexBParser.NAL_PPS,
                AnnexBParser.NAL_IDR,
                AnnexBParser.NAL_SLICE,
            ),
            units.map { it.type },
        )
        assertTrue(AnnexBParser.isKeyFrame(units))
    }

    @Test
    fun split_acceptsThreeByteStartCodes() {
        val stream = ByteArrayOutputStream().apply {
            write(byteArrayOf(0, 0, 1))
            write(nal(AnnexBParser.NAL_SPS, 6))
            write(byteArrayOf(0, 0, 1))
            write(nal(AnnexBParser.NAL_PPS, 3))
        }.toByteArray()
        val units = AnnexBParser.split(stream)
        assertEquals(2, units.size)
        assertEquals(AnnexBParser.NAL_SPS, units[0].type)
        assertEquals(AnnexBParser.NAL_PPS, units[1].type)
    }

    @Test
    fun split_trimsTrailingZeroPadding() {
        // 两个起始码之间常有 trailing_zero_8bits，不能算进上一个 NAL。
        val first = nal(AnnexBParser.NAL_SPS, 5)
        val stream = byteArrayOf(0, 0, 0, 1) + first + byteArrayOf(0, 0) +
            byteArrayOf(0, 0, 0, 1) + nal(AnnexBParser.NAL_PPS, 3)
        val units = AnnexBParser.split(stream)
        assertEquals(2, units.size)
        assertArrayEquals(first, units[0].payload)
    }

    @Test
    fun parameterSets_areWrappedWithStartCode() {
        val sps = nal(AnnexBParser.NAL_SPS, 7)
        val pps = nal(AnnexBParser.NAL_PPS, 3)
        val units = AnnexBParser.split(annexB(sps, pps))
        val (csd0, csd1) = AnnexBParser.parameterSets(units)
        assertEquals(0, csd0!![0].toInt())
        assertEquals(0, csd0[1].toInt())
        assertEquals(0, csd0[2].toInt())
        assertEquals(1, csd0[3].toInt())
        assertArrayEquals(sps, csd0.copyOfRange(4, csd0.size))
        assertArrayEquals(pps, csd1!!.copyOfRange(4, csd1.size))
    }

    @Test
    fun parameterSets_returnNullWhenMissing() {
        val units = AnnexBParser.split(annexB(nal(AnnexBParser.NAL_SLICE, 4)))
        val (csd0, csd1) = AnnexBParser.parameterSets(units)
        assertNull(csd0)
        assertNull(csd1)
    }

    @Test
    fun accessUnit_keepsOnlyVclSlices() {
        val idr = nal(AnnexBParser.NAL_IDR, 6)
        val slice = nal(AnnexBParser.NAL_SLICE, 4)
        val units = AnnexBParser.split(
            annexB(
                nal(AnnexBParser.NAL_SPS, 5),
                nal(AnnexBParser.NAL_PPS, 3),
                idr,
                slice,
            ),
        )
        val accessUnit = AnnexBParser.accessUnit(units)!!
        // 两个 VCL NAL，每个 4 字节起始码。
        assertEquals(idr.size + slice.size + 8, accessUnit.size)
        assertArrayEquals(byteArrayOf(0, 0, 0, 1), accessUnit.copyOfRange(0, 4))
        assertArrayEquals(idr, accessUnit.copyOfRange(4, 4 + idr.size))
    }

    @Test
    fun accessUnit_nullWithoutVcl() {
        val units = AnnexBParser.split(
            annexB(nal(AnnexBParser.NAL_SPS, 5), nal(AnnexBParser.NAL_PPS, 3)),
        )
        assertNull(AnnexBParser.accessUnit(units))
    }

    @Test
    fun split_returnsEmptyWithoutStartCode() {
        assertTrue(AnnexBParser.split(byteArrayOf(1, 2, 3, 4)).isEmpty())
        assertTrue(AnnexBParser.split(ByteArray(0)).isEmpty())
    }

    @Test
    fun isKeyFrame_falseForNonIdr() {
        val units = AnnexBParser.split(
            annexB(nal(AnnexBParser.NAL_SPS, 4), nal(AnnexBParser.NAL_SLICE, 9)),
        )
        assertFalse(AnnexBParser.isKeyFrame(units))
    }
}
