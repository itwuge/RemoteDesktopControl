//! rdc-codec / Windows MP4 可解码性验证
//!
//! 层级：L3 编解码层 · 验证支撑
//! 用途：用 Media Foundation SourceReader 解码 S3a 产出的 MP4，作为 ffprobe 的等价手段，
//!       确认「文件可解码、分辨率与帧率正确、帧数一致」
//! 调用方：rdc-cli 的 S3a 端到端命令（实测报告）
//!
//! 为什么不用 ffprobe：本机 PATH 中没有 ffprobe/ffmpeg；用系统自带 MF 解码器
//! 能真实走一遍「容器解析 → 码流解码 → 输出像素」的完整链路，证据比解析元数据更强。
//!
//! 安全边界（unsafe）：SourceReader 为 COM 对象，单线程独占；样本缓冲只读。

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSourceReader, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFVideoFormat_NV12, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM,
};

use crate::error::CodecError;

// =====** 验证报告 =====**

/// 解码验证结果。
#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    /// 容器报告的解码宽。
    pub width: u32,
    /// 容器报告的解码高。
    pub height: u32,
    /// 帧率分子。
    pub fps_num: u32,
    /// 帧率分母。
    pub fps_den: u32,
    /// 成功解码并输出像素的帧数。
    pub decoded_frames: u64,
    /// 解码失败（有样本但取不出缓冲）的帧数。
    pub failed_frames: u64,
    /// 首个样本时间戳（毫秒）。
    pub first_pts_ms: u64,
    /// 末个样本时间戳（毫秒）。
    pub last_pts_ms: u64,
    /// 是否判定为整体可解码。
    pub ok: bool,
    /// 诊断说明（例如解码器名称、样本格式）。
    pub note: String,
}

impl VerifyReport {
    /// 实测帧率（按解码帧数与时间跨度计算）。
    ///
    /// # Returns
    /// 帧/秒；时间跨度不足时返回容器声明的帧率。
    ///
    /// # Errors
    /// 不会失败。
    pub fn measured_fps(&self) -> f64 {
        let span = self.last_pts_ms.saturating_sub(self.first_pts_ms);
        if span > 0 && self.decoded_frames > 1 {
            // 用「帧数-1」个帧间隔，避免把首帧时间戳算进跨度导致偏低。
            (self.decoded_frames as f64 - 1.0) / (span as f64 / 1000.0)
        } else if self.fps_den > 0 {
            self.fps_num as f64 / self.fps_den as f64
        } else {
            0.0
        }
    }
}

// =====** 验证入口 =====**

/// 用 Media Foundation 解码指定 MP4，统计可解码帧数与分辨率/帧率。
///
/// # Arguments
/// * `path` - MP4 路径。
///
/// # Returns
/// 验证报告。
///
/// # Errors
/// 文件无法打开、无视频流或无法设置解码输出类型时返回 [`CodecError::DecoderFailure`]。
pub fn verify_mp4(path: &Path) -> Result<VerifyReport, CodecError> {
    crate::platform::windows::mf::ensure_mf()?;
    let url = path.to_string_lossy().into_owned();
    let mut url_wide: Vec<u16> = url.encode_utf16().collect();
    url_wide.push(0);

    // 必须启用视频处理（RGB32 与解码器原生 NV12 之间需要转换），否则设置输出类型会返回
    // MF_E_INVALIDMEDIATYPE（0xC00D36B4）。
    let mut attrs: Option<IMFAttributes> = None;
    let reader: IMFSourceReader = unsafe {
        MFCreateAttributes(&mut attrs, 1)
            .map_err(|e| CodecError::DecoderFailure(format!("MFCreateAttributes: {e}")))?;
        let attrs = attrs.ok_or_else(|| {
            CodecError::DecoderFailure("MFCreateAttributes 未返回属性对象".to_owned())
        })?;
        attrs
            .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
            .map_err(|e| CodecError::DecoderFailure(format!("SetUINT32(video_processing): {e}")))?;
        MFCreateSourceReaderFromURL(PCWSTR(url_wide.as_ptr()), &attrs)
            .map_err(|e| CodecError::DecoderFailure(format!("MFCreateSourceReaderFromURL: {e}")))?
    };
    let native: IMFMediaType = unsafe {
        reader
            .GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0)
            .map_err(|e| CodecError::DecoderFailure(format!("GetNativeMediaType: {e}")))?
    };
    let (width, height) = read_size(&native)?;
    let (fps_num, fps_den) = read_frame_rate(&native)?;

    // 请求解码器原生 NV12 输出：只要能连续读出样本，就证明文件头、参数集与帧数据可解码。
    let nv12: IMFMediaType = unsafe {
        MFCreateMediaType()
            .map_err(|e| CodecError::DecoderFailure(format!("MFCreateMediaType: {e}")))?
    };
    unsafe {
        native
            .CopyAllItems(&nv12)
            .map_err(|e| CodecError::DecoderFailure(format!("CopyAllItems: {e}")))?;
        nv12
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| CodecError::DecoderFailure(format!("SetGUID(major): {e}")))?;
        nv12
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|e| CodecError::DecoderFailure(format!("SetGUID(nv12): {e}")))?;
        reader
            .SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &nv12)
            .map_err(|e| CodecError::DecoderFailure(format!("SetCurrentMediaType: {e}")))?;
    }

    let mut decoded = 0u64;
    let mut failed = 0u64;
    let mut first_pts = i64::MIN;
    let mut last_pts = 0i64;
    loop {
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample: Option<windows::Win32::Media::MediaFoundation::IMFSample> = None;
        unsafe {
            reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(|e| CodecError::DecoderFailure(format!("ReadSample: {e}")))?;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
        let Some(sample) = sample else {
            // 空洞/间隙样本：不计入解码失败，但也不计入成功帧。
            continue;
        };
        // 有样本就必须能取出连续缓冲，才算「可解码」。
        let got_buffer = unsafe { sample.ConvertToContiguousBuffer() }.is_ok();
        if got_buffer {
            decoded += 1;
            if first_pts == i64::MIN {
                first_pts = ts;
            }
            last_pts = last_pts.max(ts);
        } else {
            failed += 1;
        }
    }

    let first_pts_ms = if first_pts == i64::MIN { 0 } else { (first_pts.max(0) / 10_000) as u64 };
    let last_pts_ms = (last_pts.max(0) / 10_000) as u64;
    Ok(VerifyReport {
        width,
        height,
        fps_num,
        fps_den,
        decoded_frames: decoded,
        failed_frames: failed,
        first_pts_ms,
        last_pts_ms,
        ok: decoded > 0 && failed == 0,
        note: format!(
            "MF SourceReader 解码为 NV12；容器声明 {width}x{height}@{fps_num}/{fps_den}"
        ),
    })
}

/// 读取媒体类型上的尺寸。
fn read_size(media_type: &IMFMediaType) -> Result<(u32, u32), CodecError> {
    let packed = unsafe {
        media_type
            .GetUINT64(&MF_MT_FRAME_SIZE)
            .map_err(|e| CodecError::DecoderFailure(format!("GetUINT64(size): {e}")))?
    };
    Ok(((packed >> 32) as u32, packed as u32))
}

/// 读取媒体类型上的帧率（分子/分母）。
fn read_frame_rate(media_type: &IMFMediaType) -> Result<(u32, u32), CodecError> {
    let packed = unsafe {
        media_type
            .GetUINT64(&MF_MT_FRAME_RATE)
            .map_err(|e| CodecError::DecoderFailure(format!("GetUINT64(fps): {e}")))?
    };
    Ok(((packed >> 32) as u32, packed as u32))
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 实测帧率优先按时间跨度计算。
    #[test]
    fn measured_fps_uses_timestamps() {
        let report = VerifyReport {
            fps_num: 60,
            fps_den: 1,
            decoded_frames: 600,
            first_pts_ms: 0,
            last_pts_ms: 9983,
            ..Default::default()
        };
        let fps = report.measured_fps();
        assert!((fps - 599.0 / 9.983).abs() < 0.01, "实测 fps 应在 60 附近，实际 {fps}");
    }

    /// 无时间跨度时回退到容器声明的帧率。
    #[test]
    fn measured_fps_fallback() {
        let report = VerifyReport {
            fps_num: 30,
            fps_den: 1,
            decoded_frames: 1,
            first_pts_ms: 0,
            last_pts_ms: 0,
            ..Default::default()
        };
        assert_eq!(report.measured_fps(), 30.0);
    }
}

