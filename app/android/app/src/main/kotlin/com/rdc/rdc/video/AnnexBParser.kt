// =====** Annex-B 码流解析 =====**
//
// 层级：Android 客户端 / 视频解码
// 边界：只做字节级拆分（起始码、NAL 类型、SPS/PPS/IDR 识别），
//       不调用任何 Android API，因此可以在 JVM 单测里直接跑。
//
// 输入来自 RDP 视频通道的 H.264 Annex-B 码流（`00 00 01` 或 `00 00 00 01` 起始码）。

package com.rdc.rdc.video

/// 一个 Annex-B NAL 单元。
///
/// [payload] 含 NAL 头字节（第一个字节的低 5 位是 nal_unit_type），不含起始码。
class NalUnit(
    /// nal_unit_type（H.264 7.4.1）。
    val type: Int,
    /// NAL 数据（含头字节，不含起始码）。
    val payload: ByteArray,
)

/// Annex-B 码流拆分与关键帧识别。
object AnnexBParser {
    /// 非 IDR 的普通片（P/B 帧）。
    const val NAL_SLICE = 1
    /// IDR 片（关键帧）。
    const val NAL_IDR = 5
    /// 序列参数集。
    const val NAL_SPS = 7
    /// 图像参数集。
    const val NAL_PPS = 8
    /// 访问单元分隔符。
    const val NAL_AUD = 9

    /// 把一段字节流拆成 NAL 单元。
    ///
    /// 起始码有三种合法形式：`00 00 01`、`00 00 00 01`（以及前面可能出现的
    /// 更多 0，属于 trailing_zero_8bits）。实现上找出所有“0x01 且前两位为 0”
    /// 的位置，NAL 从 0x01 之后开始，到下一个起始码的零串之前结束。
    ///
    /// # Arguments
    /// * `data` - Annex-B 字节流（可含多帧）
    ///
    /// # Returns
    /// 按出现顺序排列的 NAL 单元；没有起始码时返回空列表。
    fun split(data: ByteArray): List<NalUnit> {
        val startCodes = ArrayList<Int>()
        for (index in 2 until data.size) {
            if (data[index] == 1.toByte() &&
                data[index - 1] == 0.toByte() &&
                data[index - 2] == 0.toByte()
            ) {
                startCodes.add(index)
            }
        }
        if (startCodes.isEmpty()) {
            return emptyList()
        }
        val units = ArrayList<NalUnit>(startCodes.size)
        for (order in startCodes.indices) {
            val nalStart = startCodes[order] + 1
            var nalEnd = if (order + 1 < startCodes.size) startCodes[order + 1] else data.size
            // 下一个起始码前的 0 属于起始码；末尾的 trailing_zero_8bits 也属于填充。
            while (nalEnd > nalStart && data[nalEnd - 1] == 0.toByte()) {
                nalEnd -= 1
            }
            if (nalEnd <= nalStart) {
                continue
            }
            val payload = data.copyOfRange(nalStart, nalEnd)
            units.add(NalUnit(type = payload[0].toInt() and 0x1F, payload = payload))
        }
        return units
    }

    /// 找 SPS/PPS。
    ///
    /// MediaCodec 的 `csd-0/csd-1` 需要带起始码的参数集字节（常见做法是
    /// 4 字节起始码 `00 00 00 01`），这里按该形式重新拼装。
    ///
    /// # Arguments
    /// * `units` - [split] 的输出
    ///
    /// # Returns
    /// `(csd0, csd1)`，缺失时为 `null`。
    fun parameterSets(units: List<NalUnit>): Pair<ByteArray?, ByteArray?> {
        val sps = units.firstOrNull { it.type == NAL_SPS }?.payload
        val pps = units.firstOrNull { it.type == NAL_PPS }?.payload
        return Pair(sps?.withStartCode(), pps?.withStartCode())
    }

    /// 是否包含 IDR 片。
    ///
    /// # Arguments
    /// * `units` - [split] 的输出
    ///
    /// # Returns
    /// 存在 nal_unit_type 5 时返回 `true`。
    fun isKeyFrame(units: List<NalUnit>): Boolean =
        units.any { it.type == NAL_IDR }

    /// 拼出一个可送 MediaCodec 的访问单元。
    ///
    /// 只保留 VCL 片（1~5），丢弃 AUD/SEI/SPS/PPS：参数集已通过 `csd-0/csd-1`
    /// 配置，混在帧数据里会让部分解码器报“重复参数集”。
    ///
    /// # Arguments
    /// * `units` - 同一帧的 NAL 单元
    ///
    /// # Returns
    /// 带 4 字节起始码的访问单元；没有 VCL 片时返回 `null`。
    fun accessUnit(units: List<NalUnit>): ByteArray? {
        val vcl = units.filter { it.type in NAL_SLICE..NAL_IDR }
        if (vcl.isEmpty()) {
            return null
        }
        val size = vcl.sumOf { it.payload.size + 4 }
        val out = ByteArray(size)
        var offset = 0
        for (unit in vcl) {
            out[offset] = 0
            out[offset + 1] = 0
            out[offset + 2] = 0
            out[offset + 3] = 1
            offset += 4
            unit.payload.copyInto(out, offset)
            offset += unit.payload.size
        }
        return out
    }

    /// 给裸 NAL 加 4 字节起始码。
    private fun ByteArray.withStartCode(): ByteArray {
        val out = ByteArray(size + 4)
        out[3] = 1
        copyInto(out, 4)
        return out
    }
}
