// rdc-codec / AMF 原生编码 shim（内部头，不对外发布）
//
// 用途：把 AMD AMF SDK（H.264/HEVC、D3D11 输入）包成薄薄一层 C++ 实现，
//       由 Rust 侧通过 rdc-amf.dll 的 C ABI 调用；同一份实现也被 amf_spike 复用来实测。
//
// 低延迟约束（与 docs/决策待办.md S7 口径一致）：
// - 排队深度 = 1：调用方每送一帧就取一帧，shim 内部用 QueryTimeout 做事件驱动等待，不预排。
// - B 帧 = 0、参考帧 = 1、lookahead/pre-analysis 关闭、IDR 周期 = 1 秒。
// - 码控 = CBR + EnforceHRD + FillerData（真正的恒定码率，不用 VBR 近似）。
// - 输入支持三种：host BGRA（AMF 内部上传）、DX11 BGRA（纹理直入）、
//   DX11 BGRA→D3D11 VideoProcessor→NV12（GPU 色彩转换后直入）。
#pragma once

#include <cstdint>
#include <string>
#include <vector>

#include <d3d11.h>

namespace rdcamf {

/// 输入形态。
enum InputMode {
    kHostBgra = 0,   ///< CPU BGRA，经 AMF host surface 上传
    kDx11Bgra = 1,   ///< D3D11 BGRA 纹理直入（AMF 内部做 BGRA→NV12）
    kDx11Nv12 = 2,   ///< D3D11 BGRA 纹理 → VideoProcessor 转 NV12 → 直入
    kDx11Nv12Pre = 3 ///< 调用方已给出 D3D11 NV12 纹理，直接送入
};

/// 一包编码输出。
struct Packet {
    std::vector<uint8_t> data;
    int64_t pts_us = 0;
    bool keyframe = false;
};

/// AMF H.264/HEVC 编码器。
class AmfEncoder {
public:
    AmfEncoder() = default;
    ~AmfEncoder();

    /// 打开编码器。
    /// device 为空时自行创建 D3D11 设备（带 BGRA/VIDEO 支持位）。
    bool open(int width, int height, int fps, int bitrate_bps, int codec /*0=h264,1=hevc*/,
              int input_mode, void* device, std::string* err);

    /// 提交一帧 CPU BGRA（仅 kHostBgra）。
    bool push_rgba(const void* bgra, int stride_bytes, int64_t pts_us, std::string* err);

    /// 提交一帧 D3D11 纹理（其它三种模式）。slice 为纹理数组层号。
    bool push_texture(void* texture, uint32_t slice, int64_t pts_us, std::string* err);

    /// 取一包输出：1=有包，0=暂无（需要更多输入），-1=错误。
    int poll(uint8_t* out, int cap, int* out_len, int64_t* pts_us, int* keyframe);

    /// 发送结束标记并抽出剩余包：0=正常，-1=错误。
    int drain();

    /// 关闭（释放 AMF 组件与自建设备）。
    void close();

    /// 实际生效参数（JSON 片段），用于启动日志与验收核查。
    std::string effective_json() const;

    /// 自建设备（供调用方创建纹理）；外部设备时返回传入的指针。
    void* device() const { return device_; }

    const std::string& last_error() const { return error_; }

private:
    bool ensure_device(std::string* err);
    bool ensure_video_processor(std::string* err);
    void release_video_processor();
    int poll_raw(Packet* out);

    void* device_ = nullptr;            ///< ID3D11Device*
    void* context_ = nullptr;           ///< ID3D11DeviceContext*
    bool owns_device_ = false;

    void* amf_factory_ = nullptr;       ///< amf::AMFFactory*（仅调试用，工厂本身不释放）
    void* amf_context_ = nullptr;       ///< amf::AMFContext*
    void* amf_component_ = nullptr;     ///< amf::AMFComponent*

    int width_ = 0;
    int height_ = 0;
    int fps_ = 0;
    int bitrate_ = 0;
    int codec_ = 0;
    int input_mode_ = 0;
    int usage_selected_ = 0;            ///< 实际生效的 AMF usage（按能力探测决定）
    bool usage_fallback_ = false;       ///< ULTRA_LOW_LATENCY 不可用时的回退标记

    // VideoProcessor（kDx11Nv12 专用）
    void* vp_device_ = nullptr;         ///< ID3D11VideoDevice*
    void* vp_context_ = nullptr;        ///< ID3D11VideoContext*
    void* vp_enumerator_ = nullptr;     ///< ID3D11VideoProcessorEnumerator*
    void* vp_processor_ = nullptr;      ///< ID3D11VideoProcessor*
    void* vp_in_view_ = nullptr;        ///< ID3D11VideoProcessorInputView*
    void* vp_out_view_ = nullptr;       ///< ID3D11VideoProcessorOutputView*
    void* vp_in_texture_ = nullptr;     ///< 已绑定输入视图的源纹理
    void* nv12_texture_ = nullptr;      ///< 内部 NV12 目标纹理

    std::string error_;
};

} // namespace rdcamf

// =====** C ABI（rdc-amf.dll 导出；Rust 侧按此绑定） =====**
extern "C" {
int rdcamf_open(int width, int height, int fps, int bitrate_bps, int codec, int input_mode,
                void* device);
int rdcamf_push_rgba(const void* bgra, int stride_bytes, int64_t pts_us);
int rdcamf_push_texture(void* texture, uint32_t slice, int64_t pts_us);
int rdcamf_poll(uint8_t* out, int cap, int* out_len, int64_t* pts_us, int* keyframe);
int rdcamf_drain(void);
void rdcamf_close(void);
const char* rdcamf_last_error(void);
const char* rdcamf_effective_json(void);
void* rdcamf_device(void);
int rdcamf_probe_caps(int* h264, int* hevc, int* driver_is_amd);
int rdcamf_diag(void);
}
