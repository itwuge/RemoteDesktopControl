//! rdc-codec / 编码器契约类型
//!
//! 层级：L3 编解码层
//! 用途：定义编码格式、色度、编码配置、能力描述、编码包与编码器 trait（平台无关）
//! 调用方：rdc-codec 各平台后端实现 [`VideoEncoder`]；rdc-core 通过该 trait 驱动编码

use rdc_runtime::PooledBuffer;

use crate::error::CodecError;

// =====** CodecKind =====**

/// 视频编码格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecKind {
    /// HEVC / H.265（默认格式，配合 4:4:4 可保留桌面文字锐度）。
    Hevc,
    /// H.264 / AVC（兼容性兜底）。
    H264,
    /// AV1（软编或新一代硬编）。
    Av1,
}

// =====** VideoChroma =====**

/// 编码色度采样。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoChroma {
    /// 4:2:0：带宽减半，适用于视频类内容。
    Yuv420,
    /// 4:4:4：保留色度细节，适用于桌面文字与彩色边缘。
    Yuv444,
}

// =====** VideoConfig =====**

/// 单次编码会话配置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoConfig {
    /// 目标编码格式。
    pub codec: CodecKind,
    /// 编码宽度，单位：像素。
    pub width: u32,
    /// 编码高度，单位：像素。
    pub height: u32,
    /// 目标帧率，单位：Hz。
    pub fps: u32,
    /// 目标码率，单位：bit/s。
    pub bitrate_bps: u32,
    /// 色度采样。
    pub chroma: VideoChroma,
    /// 位深（8 或 10）。
    pub bit_depth: u8,
    /// 是否启用低时延模式（关闭 B 帧与前视）。
    pub low_latency: bool,
}

// =====** EncoderCaps =====**

/// 单个编码格式的硬件能力描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderCaps {
    /// 能力对应的编码格式。
    pub codec: CodecKind,
    /// 最大支持宽度，单位：像素。
    pub max_width: u32,
    /// 最大支持高度，单位：像素。
    pub max_height: u32,
    /// 最大支持帧率，单位：Hz。
    pub max_fps: u32,
    /// 是否支持 4:4:4。
    pub supports_444: bool,
    /// 是否支持 10 bit。
    pub supports_10bit: bool,
    /// 是否支持运行期重配（不重建会话改分辨率/码率）。
    pub supports_runtime_reconfigure: bool,
}

// =====** EncodedPacket =====**

/// 一包编码输出。
#[derive(Debug, Clone, PartialEq)]
pub struct EncodedPacket {
    /// 编码后的码流字节（一个访问单元）。
    pub data: Vec<u8>,
    /// 对应采集帧的时间戳，单位：纳秒。
    pub pts_ns: u64,
    /// 是否为关键帧（IDR）；传输层据此划分可独立解码的分片组。
    pub is_keyframe: bool,
    /// 建议的传输分片大小，单位：字节；`0` 表示由传输层按 MTU 决定。
    pub suggested_shard_bytes: u16,
}

// =====** EncoderCapability / ProbeReport =====**

/// 单个编码器实例的实测能力（由平台后端真实枚举与类型协商得出）。
///
/// 与 [`EncoderCaps`] 的区别：[`EncoderCaps`] 是给人看的最小摘要，
/// 本结构是 S3a 实测报告用的完整证据（含名称、码率范围与数据来源）。
#[derive(Debug, Clone, PartialEq)]
pub struct EncoderCapability {
    /// 编码器名称（MFT 友好名称等）。
    pub name: String,
    /// 是否硬件编码器（`false` 表示软件 MFT，只允许显式启用）。
    pub hardware: bool,
    /// 编码格式。
    pub codec: CodecKind,
    /// 实测支持的最大宽。
    pub max_width: u32,
    /// 实测支持的最大高。
    pub max_height: u32,
    /// 实测支持的最大帧率。
    pub max_fps: u32,
    /// 输入色度采样。
    pub chroma: VideoChroma,
    /// 输入位深。
    pub bit_depth: u8,
    /// 实测可设置的码率下限（bps）。
    pub min_bitrate_bps: u32,
    /// 实测可设置的码率上限（bps）。
    pub max_bitrate_bps: u32,
    /// 是否支持运行期参数热更新。
    pub runtime_reconfigure: bool,
    /// 取值依据（说明哪些字段是枚举得出、哪些是协商尝试得出）。
    pub note: String,
}

/// 平台编码器探测报告。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProbeReport {
    /// 是否存在可用硬件编码器。
    pub hardware_available: bool,
    /// 硬件编码器列表（按系统排序）。
    pub hardware: Vec<EncoderCapability>,
    /// 软件编码器列表（仅用于报告，默认不被选用）。
    pub software: Vec<EncoderCapability>,
    /// 探测过程中的错误；有值时上述列表可能不完整。
    pub error: Option<String>,
}

// =====** VideoEncoder =====**

/// 平台无关的视频编码器。
///
/// 生命周期：`probe` → `configure` →（`push` / `poll_packet` 循环）→ 可选 `reconfigure`。
/// 实现必须是 `Send`：会话建立后由 rdc-ctl 移交给编码线程独占使用。
pub trait VideoEncoder: Send {
    /// 探测硬件对给定编码格式的支持。
    ///
    /// 调用线程：rdc-ctl（会话建立前一次性调用）。
    /// 阻塞性：可能阻塞（枚举硬件实例），禁止在编码线程调用。
    /// 内部队列：无。
    ///
    /// # Arguments
    /// * `codecs` - 待探测的编码格式；空切片表示探测全部已知格式。
    ///
    /// # Returns
    /// 每个受支持格式一条能力记录；全不支持时返回空列表（不报错）。
    ///
    /// # Errors
    /// 不会失败；能力缺失用空列表表达。
    fn probe(&self, codecs: &[CodecKind]) -> Vec<EncoderCaps>;

    /// 配置编码器（创建或重建硬件会话）。
    ///
    /// 调用线程：rdc-ctl。
    /// 阻塞性：阻塞（硬件会话创建可能耗时数十毫秒），仅允许在编码线程启动前调用。
    /// 内部队列：无。
    ///
    /// # Arguments
    /// * `config` - 编码格式、分辨率、帧率、码率、色度与位深。
    ///
    /// # Returns
    /// 配置成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 配置超出硬件能力返回 [`CodecError::InvalidConfig`]；
    /// 硬件不可用返回 [`CodecError::HardwareUnsupported`]。
    fn configure(&mut self, config: VideoConfig) -> Result<(), CodecError>;

    /// 提交一帧到编码器输入。
    ///
    /// 调用线程：编码线程（非 rdc-ctl）。
    /// 阻塞性：非阻塞；输入队列满时立即丢帧或返回错误，禁止忙等。
    /// 内部队列：编码器私有提交队列（此处仅入队，不等输出）。
    ///
    /// # Arguments
    /// * `frame` - 池化帧；入队后所有权转移给编码器，输出后归还内存池。
    /// * `pts_ns` - 对应采集帧的时间戳，单位：纳秒。
    /// * `force_keyframe` - 是否强制本帧为 IDR。
    ///
    /// # Returns
    /// 入队成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 编码器未配置或提交失败时返回 [`CodecError`]。
    fn push(
        &mut self,
        frame: PooledBuffer,
        pts_ns: u64,
        force_keyframe: bool,
    ) -> Result<(), CodecError>;

    /// 绑定调用方的 D3D11 设备，启用「纹理直入」。
    ///
    /// 为什么在 trait 上而不是各后端私有：管线需要在会话建立前一次性决定
    /// 「采集出纹理还是出 CPU 缓冲」，这个决定必须由编码器能力参与。
    ///
    /// 调用线程：rdc-ctl（`configure` 之前）。
    /// 阻塞性：不阻塞。
    ///
    /// # Arguments
    /// * `device` - `ID3D11Device*`（`usize`）；调用方保证生命周期长于编码器会话。
    /// * `context` - `ID3D11DeviceContext*`（当前实现由后端自行获取立即上下文）。
    /// * `prefer_texture` - 是否希望纹理直入；后端不支持时返回 `false`。
    ///
    /// # Returns
    /// 接受纹理直入返回 `true`；否则返回 `false`（调用方继续走 CPU 缓冲）。
    ///
    /// # Errors
    /// 不会失败。
    fn attach_d3d11(&mut self, _device: usize, _context: usize, _prefer_texture: bool) -> bool {
        false
    }

    /// 提交一帧 GPU 纹理（不做 CPU 拷贝）。
    ///
    /// 前置：`attach_d3d11` 返回 `true` 且已 `configure`。
    /// 约束：纹理必须与绑定的设备同源；调用方必须持有纹理租约直到该帧出包。
    ///
    /// 调用线程：编码线程。
    /// 阻塞性：短阻塞（提交到编码器，排队深度 1 时为微秒级）。
    ///
    /// # Arguments
    /// * `texture` - 纹理裸句柄（`usize`）。
    /// * `array_slice` - 数组层号。
    /// * `pts_ns` - 采集时间戳（纳秒）。
    /// * `force_keyframe` - 是否请求关键帧。
    ///
    /// # Returns
    /// 入队成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 后端不支持纹理输入、未配置或提交失败时返回 [`CodecError`]。
    fn push_texture(
        &mut self,
        _texture: usize,
        _array_slice: u32,
        _pts_ns: u64,
        _force_keyframe: bool,
    ) -> Result<(), CodecError> {
        Err(CodecError::Unsupported)
    }

    /// 非阻塞取出一包编码结果。
    ///
    /// 调用线程：编码线程。
    /// 阻塞性：不阻塞。
    /// 内部队列：读取编码器输出队列（FIFO）。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 有输出返回 [`EncodedPacket`]；暂无输出返回 `None`。
    ///
    /// # Errors
    /// 不会失败；运行期失败通过后续 `push` 返回或统计计数上报。
    fn poll_packet(&mut self) -> Option<EncodedPacket>;

    /// 运行期重配（码率/帧率等不改变会话结构的参数）。
    ///
    /// 调用线程：rdc-ctl。
    /// 阻塞性：短阻塞（下发新参数），不等待在途帧完成。
    /// 内部队列：无——在途帧按旧参数编码，不缓存。
    ///
    /// # Arguments
    /// * `config` - 新的编码配置。
    ///
    /// # Returns
    /// 重配成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 硬件不支持运行期重配或参数非法时返回 [`CodecError`]。
    fn reconfigure(&mut self, config: VideoConfig) -> Result<(), CodecError>;

    /// 请求下一个提交帧编码为关键帧。
    ///
    /// 调用线程：rdc-ctl。
    /// 阻塞性：不阻塞（仅置位请求标志）。
    /// 内部队列：无。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn request_keyframe(&mut self);

    // =====** 诊断接口（带默认实现，避免改动其它平台后端） =====**

    /// 返回实际生效的后端名称。
    ///
    /// 为什么提供默认实现：非 Windows 后端尚未落地，S3a 只要求 Windows 汇报路径。
    ///
    /// # Returns
    /// 后端标识，用于日志与实测报告。
    ///
    /// # Errors
    /// 不会失败。
    fn backend_name(&self) -> &str {
        "unknown"
    }

    /// 返回编码统计快照。
    ///
    /// # Returns
    /// 已实现统计的后端返回快照；默认返回 `None`（表示该后端未埋点）。
    ///
    /// # Errors
    /// 不会失败。
    fn stats_snapshot(&self) -> Option<crate::metrics::EncodeSnapshot> {
        None
    }

    /// 返回当前会话的序列头（Annex-B）。
    ///
    /// # Returns
    /// SPS/PPS 字节；未提供时为空切片。
    ///
    /// # Errors
    /// 不会失败。
    fn sequence_header(&self) -> &[u8] {
        &[]
    }

    /// 冲刷编码器内部缓存，返回剩余码流包。
    ///
    /// 为什么需要：硬件 MFT 为异步流水线，停在采集末尾时仍有 1~2 帧滞留在
    /// 输入/输出缓冲里；不冲刷就会丢尾帧，实测帧数与采集帧数对不上。
    ///
    /// # Returns
    /// 剩余包（按提交顺序）；未实现的后端返回空列表。
    ///
    /// # Errors
    /// 后端冲刷失败时返回 [`CodecError`]。
    fn flush(&mut self) -> Result<Vec<EncodedPacket>, CodecError> {
        Ok(Vec::new())
    }
}
