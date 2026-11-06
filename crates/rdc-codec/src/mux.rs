//! rdc-codec / Windows MP4 封装与码流格式转换
//!
//! 层级：L3 编解码层 · 验证支撑
//! 用途：把 MFT 输出的 H.264 访问单元写成 MP4（SinkWriter，不插入编码器），
//!       并提供 Annex-B ↔ AVCC 转换与 SPS/PPS 提取，供 S3a 端到端实测文件使用
//! 调用方：rdc-cli 的 S3a 端到端命令
//!
//! 安全边界（unsafe）：SinkWriter 为 COM 对象，由单一写线程独占使用；
//! 传入的样本数据先复制进 MF 缓冲，锁指针不跨调用保存。

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFByteStream, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMemoryBuffer,
    MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video, MFTranscodeContainerType_MPEG4,
    MFVideoFormat_H264, MFVideoInterlace_Progressive, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER,
    MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_SINK_WRITER_DISABLE_THROTTLING, MF_TRANSCODE_CONTAINERTYPE,
    MF_TRANSCODE_DONOT_INSERT_ENCODER,
};

use crate::error::CodecError;

// =====** MP4 写入 =====**

/// MP4 写入器（H.264 直通封装）。
pub struct Mp4Writer {
    /// SinkWriter 实例。
    writer: IMFSinkWriter,
    /// 视频流索引。
    stream: u32,
}

impl Mp4Writer {
    /// 创建一个只封装不转码的 MP4 写入器。
    ///
    /// # Arguments
    /// * `path` - 输出文件路径（已存在会被覆盖）。
    /// * `width` / `height` - 视频尺寸。
    /// * `fps` - 帧率（Hz）。
    /// * `bitrate_bps` - 目标码率（写入容器元数据，不参与编码）。
    /// * `sequence_header` - Annex-B 形式的 SPS/PPS；为空时容器可能拒绝写入。
    ///
    /// # Returns
    /// 写入器实例。
    ///
    /// # Errors
    /// 属性创建、URL 打开、流添加或输入类型设置失败时返回 [`CodecError::EncoderFailure`]。
    pub fn create(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: u32,
        sequence_header: &[u8],
    ) -> Result<Self, CodecError> {
        crate::platform::windows::mf::ensure_mf()?;
        let url = path.to_string_lossy().into_owned();
        let mut url_wide: Vec<u16> = url.encode_utf16().collect();
        url_wide.push(0);

        unsafe {
            let mut attributes_opt: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes_opt, 4)
                .map_err(|e| CodecError::EncoderFailure(format!("MFCreateAttributes: {e}")))?;
            let attributes = attributes_opt
                .ok_or_else(|| CodecError::EncoderFailure("MFCreateAttributes 返回空对象".to_owned()))?;
            // 容器固定 MP4；明确禁止 SinkWriter 自己插编码器（我们喂的是已编码码流）。
            attributes
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
                .map_err(|e| CodecError::EncoderFailure(format!("SetGUID(container): {e}")))?;
            attributes
                .SetUINT32(&MF_TRANSCODE_DONOT_INSERT_ENCODER, 1)
                .map_err(|e| CodecError::EncoderFailure(format!("SetUINT32(no_encoder): {e}")))?;
            attributes
                .SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)
                .map_err(|e| CodecError::EncoderFailure(format!("SetUINT32(no_throttle): {e}")))?;

            let writer = MFCreateSinkWriterFromURL(
                PCWSTR(url_wide.as_ptr()),
                None::<&IMFByteStream>,
                &attributes,
            )
            .map_err(|e| CodecError::EncoderFailure(format!("MFCreateSinkWriterFromURL: {e}")))?;

            let output = make_h264_type(width, height, fps, bitrate_bps)?;
            // H.264 直通封装不会自己生成 SPS/PPS，必须由输入类型携带序列头，
            // 否则 WriteSample 返回 MF_E_INVALIDREQUEST（0xC00D36B2）。
            if !sequence_header.is_empty() {
                output
                    .SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, sequence_header)
                    .map_err(|e| {
                        CodecError::EncoderFailure(format!("SetBlob(seq_header): {e}"))
                    })?;
            }
            let stream = writer
                .AddStream(&output)
                .map_err(|e| CodecError::EncoderFailure(format!("IMFSinkWriter::AddStream: {e}")))?;
            writer
                .SetInputMediaType(stream, &output, None::<&IMFAttributes>)
                .map_err(|e| {
                    CodecError::EncoderFailure(format!("IMFSinkWriter::SetInputMediaType: {e}"))
                })?;
            // 必须先显式 BeginWriting，否则首个 WriteSample 返回 MF_E_INVALIDREQUEST。
            writer
                .BeginWriting()
                .map_err(|e| CodecError::EncoderFailure(format!("BeginWriting: {e}")))?;
            Ok(Self { writer, stream })
        }
    }

    /// 写入一帧已编码码流。
    ///
    /// # Arguments
    /// * `data` - H.264 访问单元，必须为 Annex-B（NAL 以 3/4 字节起始码分隔）。
    ///   MPEG-4 文件 sink 内部自行转换为 AVCC，因此这里不做长度前缀转换。
    /// * `pts_100ns` - 采样时间（100ns）。
    /// * `duration_100ns` - 采样时长（100ns）。
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 样本/缓冲创建或 `WriteSample` 失败时返回 [`CodecError::EncoderFailure`]。
    pub fn write(&mut self, data: &[u8], pts_100ns: i64, duration_100ns: i64) -> Result<(), CodecError> {
        let payload = data;
        unsafe {
            let buffer = MFCreateMemoryBuffer(payload.len() as u32)
                .map_err(|e| CodecError::EncoderFailure(format!("MFCreateMemoryBuffer: {e}")))?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur_len = 0u32;
            buffer
                .Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))
                .map_err(|e| CodecError::EncoderFailure(format!("IMFMediaBuffer::Lock: {e}")))?;
            // 目标缓冲由 MF 分配，长度即 payload.len()；这里按等长拷贝后再 Unlock。
            std::ptr::copy_nonoverlapping(payload.as_ptr(), ptr, payload.len());
            let _ = buffer.Unlock();
            buffer
                .SetCurrentLength(payload.len() as u32)
                .map_err(|e| CodecError::EncoderFailure(format!("SetCurrentLength: {e}")))?;

            let sample: IMFSample = MFCreateSample()
                .map_err(|e| CodecError::EncoderFailure(format!("MFCreateSample: {e}")))?;
            sample
                .AddBuffer(&buffer)
                .map_err(|e| CodecError::EncoderFailure(format!("AddBuffer: {e}")))?;
            sample
                .SetSampleTime(pts_100ns)
                .map_err(|e| CodecError::EncoderFailure(format!("SetSampleTime: {e}")))?;
            sample
                .SetSampleDuration(duration_100ns)
                .map_err(|e| CodecError::EncoderFailure(format!("SetSampleDuration: {e}")))?;
            self.writer
                .WriteSample(self.stream, &sample)
                .map_err(|e| CodecError::EncoderFailure(format!("WriteSample: {e}")))?;
        }
        Ok(())
    }

    /// 完成写入（写入 moov 等索引）。
    ///
    /// # Returns
    /// 落盘成功返回 `Ok(())`。
    ///
    /// # Errors
    /// `Finalize` 失败时返回 [`CodecError::EncoderFailure`]。
    pub fn finish(self) -> Result<(), CodecError> {
        unsafe {
            self.writer
                .Finalize()
                .map_err(|e| CodecError::EncoderFailure(format!("Finalize: {e}")))?;
        }
        Ok(())
    }
}

/// 构造 H.264 输入媒体类型。
///
/// # Arguments
/// * `width` / `height` - 尺寸。
/// * `fps` - 帧率。
/// * `bitrate_bps` - 目标码率。
///
/// # Returns
/// 媒体类型。
///
/// # Errors
/// 属性设置失败返回 [`CodecError::EncoderFailure`]。
unsafe fn make_h264_type(
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
) -> Result<windows::Win32::Media::MediaFoundation::IMFMediaType, CodecError> {
    use windows::Win32::Media::MediaFoundation::MFCreateMediaType;
    let media_type = MFCreateMediaType()
        .map_err(|e| CodecError::EncoderFailure(format!("MFCreateMediaType: {e}")))?;
    media_type
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(|e| CodecError::EncoderFailure(format!("SetGUID(major): {e}")))?;
    media_type
        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
        .map_err(|e| CodecError::EncoderFailure(format!("SetGUID(subtype): {e}")))?;
    media_type
        .SetUINT32(&MF_MT_AVG_BITRATE, bitrate_bps.max(1))
        .map_err(|e| CodecError::EncoderFailure(format!("SetUINT32(bitrate): {e}")))?;
    media_type
        .SetUINT64(&MF_MT_FRAME_SIZE, pack_u64(width, height))
        .map_err(|e| CodecError::EncoderFailure(format!("SetUINT64(size): {e}")))?;
    media_type
        .SetUINT64(&MF_MT_FRAME_RATE, pack_u64(fps.max(1), 1))
        .map_err(|e| CodecError::EncoderFailure(format!("SetUINT64(fps): {e}")))?;
    media_type
        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u64(1, 1))
        .map_err(|e| CodecError::EncoderFailure(format!("SetUINT64(par): {e}")))?;
    media_type
        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
        .map_err(|e| CodecError::EncoderFailure(format!("SetUINT32(interlace): {e}")))?;
    Ok(media_type)
}

/// 把宽高/分子分母打包为 `(high<<32)|low`。
///
/// # Arguments
/// * `hi` - 高 32 位。
/// * `lo` - 低 32 位。
///
/// # Returns
/// 打包结果。
///
/// # Errors
/// 不会失败。
fn pack_u64(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

// =====** 码流格式转换 =====**

/// 把 Annex-B（起始码）访问单元转成 AVCC（4 字节长度前缀）。
///
/// 为什么需要：MFT 输出的 H.264 是 Annex-B，而 MP4 容器里的样本是 AVCC。
///
/// # Arguments
/// * `data` - 一个访问单元。
///
/// # Returns
/// AVCC 字节；输入不含起始码时按「已为长度前缀」原样返回副本。
///
/// # Errors
/// 不会失败（纯格式转换）。
///
/// # 说明
/// 只处理 3/4 字节起始码，不解析 slice header；转换后的长度前缀固定 4 字节，
/// 与主流 MP4 封装惯例一致。
pub fn to_avcc(data: &[u8]) -> Vec<u8> {
    let nals = split_annexb(data);
    if nals.is_empty() {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(data.len() + nals.len() * 4);
    for nal in nals {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// 从 Annex-B 码流中提取 SPS(7)/PPS(8)，输出 Annex-B 形式。
///
/// 用途：驱动不提供 `MF_MT_MPEG_SEQUENCE_HEADER` 时，从首个 IDR 里补齐。
///
/// # Arguments
/// * `data` - 一个访问单元（Annex-B）。
///
/// # Returns
/// `00 00 00 01` 起始码 + SPS + PPS；没有 SPS/PPS 时为空。
///
/// # Errors
/// 不会失败。
pub fn extract_sps_pps(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in split_annexb(data) {
        if let Some(header) = nal.first() {
            // NAL 类型在 forbidden_zero/nal_ref_idc 之后低 5 位。
            if *header & 0x1F == 7 || *header & 0x1F == 8 {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(nal);
            }
        }
    }
    out
}

/// 拆分 Annex-B 访问单元为 NAL 负载切片。
///
/// # Arguments
/// * `data` - Annex-B 码流。
///
/// # Returns
/// NAL 负载列表；无起始码时返回空列表。
///
/// # Errors
/// 不会失败。
fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    if starts.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(starts.len());
    for (idx, start) in starts.iter().enumerate() {
        let end = if idx + 1 < starts.len() {
            // 去掉下一段起始码前面的零字节（4 字节起始码多出的那个 0）。
            let next = starts[idx + 1] - 3;
            if next > 0 && data[next - 1] == 0 {
                next - 1
            } else {
                next
            }
        } else {
            data.len()
        };
        if end > *start {
            out.push(&data[*start..end]);
        }
    }
    out
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// Annex-B 转 AVCC：每个 NAL 都要有 4 字节大端长度前缀。
    #[test]
    fn annexb_to_avcc_lengths() {
        let annexb = [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE];
        let avcc = to_avcc(&annexb);
        assert_eq!(&avcc[..4], &[0, 0, 0, 2]);
        assert_eq!(&avcc[4..6], &[0x67, 0x42]);
        assert_eq!(&avcc[6..10], &[0, 0, 0, 2]);
        assert_eq!(&avcc[10..12], &[0x68, 0xCE]);
    }

    /// 已是长度前缀的数据必须原样返回，不能被二次转换。
    #[test]
    fn avcc_passthrough() {
        let avcc = [0, 0, 0, 2, 0x67, 0x42];
        assert_eq!(to_avcc(&avcc), avcc.to_vec());
    }

    /// 只提取 SPS/PPS，其它 NAL（如 IDR 5）不得进入序列头。
    #[test]
    fn sps_pps_extraction() {
        let annexb = [
            0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE, 0, 0, 0, 1, 0x65, 0x88,
        ];
        let header = extract_sps_pps(&annexb);
        assert_eq!(header.len(), 4 + 2 + 4 + 2);
        assert_eq!(&header[..6], &[0, 0, 0, 1, 0x67, 0x42]);
        assert_eq!(&header[6..12], &[0, 0, 0, 1, 0x68, 0xCE]);
    }

    /// 三次字节起始码同样要能拆分。
    #[test]
    fn three_byte_start_codes() {
        let annexb = [0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x68, 0xCE];
        let avcc = to_avcc(&annexb);
        assert_eq!(&avcc[..4], &[0, 0, 0, 2]);
        assert_eq!(&avcc[6..10], &[0, 0, 0, 2]);
    }
}
