// rdc-codec / AMF shim 实现（H.264，D3D11 输入）
//
// 编译方式见 shim/build_amf.ps1（MinGW g++）：
//   g++ -std=c++17 -O2 -I <amf-include> -c amf_enc.cpp -o amf_enc.o
//   g++ -shared -o rdc-amf.dll amf_enc.o -ld3d11 -ldxgi -lole32 -luuid
//
// 设计边界（与 docs/决策待办.md S7 口径一致）：
// - 只动态加载系统 amfrt64.dll，不静态链接、不随包分发 AMF 运行库；
// - 排队深度 = 1：每次只驻留一帧未取回的输入，QueryOutput 用 QueryTimeout 事件等待；
// - B 帧=0、参考帧=1、pre-analysis 关闭、IDR 周期=1s、CBR+EnforceHRD+FillerData；
// - 输入三形态（host BGRA / DX11 BGRA / VideoProcessor→NV12），实测取快者；
// - 输出取包必然有一次回读（D3D11 纹理→系统内存），不宣称「完全零拷贝」。

#include "amf_shim.h"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <string>

#include <windows.h>

#include "core/Buffer.h"
#include "core/Context.h"
#include "core/Data.h"
#include "core/Factory.h"
#include "core/Interface.h"
#include "core/Plane.h"
#include "core/Surface.h"
#include "core/Version.h"
#include "components/VideoEncoderHEVC.h"
#include "components/VideoEncoderVCE.h"

// AMF 的枚举（AMF_OK / AMF_SURFACE_BGRA / AMF_MEMORY_HOST 等）都在 amf 命名空间里。
using namespace amf;

namespace {

const char* result_name(AMF_RESULT r) {
    switch (r) {
    case AMF_OK: return "AMF_OK";
    case AMF_FAIL: return "AMF_FAIL";
    case AMF_INVALID_ARG: return "AMF_INVALID_ARG";
    case AMF_OUT_OF_MEMORY: return "AMF_OUT_OF_MEMORY";
    case AMF_NOT_SUPPORTED: return "AMF_NOT_SUPPORTED";
    case AMF_ALREADY_INITIALIZED: return "AMF_ALREADY_INITIALIZED";
    case AMF_NOT_INITIALIZED: return "AMF_NOT_INITIALIZED";
    case AMF_NO_DEVICE: return "AMF_NO_DEVICE";
    case AMF_EOF: return "AMF_EOF";
    case AMF_REPEAT: return "AMF_REPEAT";
    case AMF_INPUT_FULL: return "AMF_INPUT_FULL";
    case AMF_NEED_MORE_INPUT: return "AMF_NEED_MORE_INPUT";
    default: return "AMF_OTHER";
    }
}

HMODULE load_amf() {
    static HMODULE lib = nullptr;
    if (lib == nullptr) {
        lib = LoadLibraryA("amfrt64.dll");
    }
    return lib;
}

const AMFInit_Fn amf_init_fn() {
    HMODULE lib = load_amf();
    if (lib == nullptr) {
        return nullptr;
    }
    return reinterpret_cast<AMFInit_Fn>(GetProcAddress(lib, AMF_INIT_FUNCTION_NAME));
}

} // namespace

namespace rdcamf {

AmfEncoder::~AmfEncoder() { close(); }

bool AmfEncoder::ensure_device(std::string* err) {
    if (device_ != nullptr) {
        return true;
    }
    ID3D11Device* dev = nullptr;
    ID3D11DeviceContext* ctx = nullptr;
    D3D_FEATURE_LEVEL level = D3D_FEATURE_LEVEL_11_0;
    HRESULT hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr,
                                   D3D11_CREATE_DEVICE_BGRA_SUPPORT |
                                       D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                                   nullptr, 0, D3D11_SDK_VERSION, &dev, &level, &ctx);
    if (FAILED(hr) || dev == nullptr) {
        if (err) *err = "D3D11CreateDevice 失败";
        return false;
    }
    device_ = dev;
    context_ = ctx;
    owns_device_ = true;
    return true;
}

bool AmfEncoder::open(int width, int height, int fps, int bitrate_bps, int codec,
                      int input_mode, void* device, std::string* err) {
    close();
    width_ = width;
    height_ = height;
    fps_ = fps > 0 ? fps : 60;
    bitrate_ = bitrate_bps;
    codec_ = codec;
    input_mode_ = input_mode;

    if (device != nullptr) {
        device_ = device;
        ID3D11Device* dev = reinterpret_cast<ID3D11Device*>(device_);
        dev->GetImmediateContext(reinterpret_cast<ID3D11DeviceContext**>(&context_));
        owns_device_ = false;
    } else if (!ensure_device(err)) {
        return false;
    }

    // HEVC 的属性集与 H.264 不同，本 shim 只落地 H.264；调用方必须显式走 MFT。
    if (codec != 0) {
        error_ = "rdc-amf shim 仅实现 H.264";
        if (err) *err = error_;
        return false;
    }

    AMFInit_Fn init_fn = amf_init_fn();
    if (init_fn == nullptr) {
        error_ = "amfrt64.dll 不存在或未导出 AMFInit";
        if (err) *err = error_;
        return false;
    }
    amf::AMFFactory* factory = nullptr;
    AMF_RESULT r = init_fn(AMF_FULL_VERSION, &factory);
    if (r != AMF_OK || factory == nullptr) {
        error_ = std::string("AMFInit: ") + result_name(r);
        if (err) *err = error_;
        return false;
    }
    amf_factory_ = factory;
    amf::AMFContextPtr context;
    r = factory->CreateContext(&context);
    if (r != AMF_OK) {
        error_ = std::string("CreateContext: ") + result_name(r);
        if (err) *err = error_;
        return false;
    }
    r = context->InitDX11(device_, AMF_DX11_0);
    if (r != AMF_OK) {
        error_ = std::string("InitDX11: ") + result_name(r);
        if (err) *err = error_;
        return false;
    }
    amf_context_ = context.Detach();

    amf::AMFComponentPtr component;
    // ----- 低延迟参数（显式映射表见 crates/rdc-codec/src/backend.rs） -----
    AMF_SURFACE_FORMAT input_format =
        (input_mode_ == kHostBgra || input_mode_ == kDx11Bgra) ? AMF_SURFACE_BGRA
                                                              : AMF_SURFACE_NV12;
    auto apply_props = [&](amf::AMFComponent* comp, amf_int64 usage) {
        comp->SetProperty(AMF_VIDEO_ENCODER_USAGE, usage);
        comp->SetProperty(AMF_VIDEO_ENCODER_LOWLATENCY_MODE, true);
        comp->SetProperty(AMF_VIDEO_ENCODER_B_PIC_PATTERN, static_cast<amf_int64>(0));
        comp->SetProperty(AMF_VIDEO_ENCODER_MAX_CONSECUTIVE_BPICTURES, static_cast<amf_int64>(0));
        comp->SetProperty(AMF_VIDEO_ENCODER_MAX_NUM_REFRAMES, static_cast<amf_int64>(1));
        comp->SetProperty(AMF_VIDEO_ENCODER_QUALITY_PRESET,
                          static_cast<amf_int64>(AMF_VIDEO_ENCODER_QUALITY_PRESET_SPEED));
        comp->SetProperty(AMF_VIDEO_ENCODER_PRE_ANALYSIS_ENABLE, false);
        comp->SetProperty(AMF_VIDEO_ENCODER_HEADER_INSERTION_SPACING, static_cast<amf_int64>(0));
        comp->SetProperty(AMF_VIDEO_ENCODER_IDR_PERIOD, static_cast<amf_int64>(fps_));
        comp->SetProperty(AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD,
                          static_cast<amf_int64>(AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD_CBR));
        comp->SetProperty(AMF_VIDEO_ENCODER_TARGET_BITRATE, static_cast<amf_int64>(bitrate_));
        comp->SetProperty(AMF_VIDEO_ENCODER_PEAK_BITRATE, static_cast<amf_int64>(bitrate_));
        // VBV 按「一帧预算」给：bitrate/fps 位。NVENC 备忘 §5 的同类约束。
        comp->SetProperty(AMF_VIDEO_ENCODER_VBV_BUFFER_SIZE,
                          static_cast<amf_int64>(bitrate_ / fps_));
        comp->SetProperty(AMF_VIDEO_ENCODER_ENFORCE_HRD, true);
        comp->SetProperty(AMF_VIDEO_ENCODER_FILLER_DATA_ENABLE, true);
        AMFRate rate;
        rate.num = static_cast<amf_uint32>(fps_);
        rate.den = 1;
        comp->SetProperty(AMF_VIDEO_ENCODER_FRAMERATE, rate);
        // QueryOutput 事件等待上限（ms）：不忙等，也不把「暂无输出」当错误。
        comp->SetProperty(AMF_VIDEO_ENCODER_QUERY_TIMEOUT, static_cast<amf_int64>(5));
    };
    auto build = [&](amf_int64 usage) -> AMF_RESULT {
        component = nullptr;
        AMF_RESULT cr = factory->CreateComponent(static_cast<amf::AMFContext*>(amf_context_),
                                                 AMFVideoEncoderVCE_AVC, &component);
        if (cr != AMF_OK) {
            return cr;
        }
        apply_props(component, usage);
        return component->Init(input_format, width_, height_);
    };

    // 能力探测式落地：ULTRA_LOW_LATENCY 在部分驱动上返回 INVALID_ARG(3)
    // （本机 RX 580 / 31.0.21912.14 实测），此时退回 LOW_LATENCY，并在启动日志里标明。
    r = build(AMF_VIDEO_ENCODER_USAGE_ULTRA_LOW_LATENCY);
    usage_selected_ = AMF_VIDEO_ENCODER_USAGE_ULTRA_LOW_LATENCY;
    if (r != AMF_OK) {
        std::printf("[amf] Usage=ULTRA_LOW_LATENCY 不可用（code=%d），回退 LOW_LATENCY\n",
                    static_cast<int>(r));
        usage_fallback_ = true;
        r = build(AMF_VIDEO_ENCODER_USAGE_LOW_LATENCY);
        usage_selected_ = AMF_VIDEO_ENCODER_USAGE_LOW_LATENCY;
    }
    if (r != AMF_OK) {
        error_ = std::string("Init(") + (input_format == AMF_SURFACE_BGRA ? "BGRA" : "NV12") +
                 "): " + result_name(r) + " code=" + std::to_string(static_cast<int>(r));
        if (err) *err = error_;
        return false;
    }
    amf_component_ = component.Detach();

    if (input_mode_ == kDx11Nv12 && !ensure_video_processor(err)) {
        return false;
    }
    return true;
}

bool AmfEncoder::ensure_video_processor(std::string* err) {
    if (vp_processor_ != nullptr && nv12_texture_ != nullptr) {
        return true;
    }
    ID3D11Device* dev = reinterpret_cast<ID3D11Device*>(device_);
    ID3D11DeviceContext* ctx = reinterpret_cast<ID3D11DeviceContext*>(context_);

    D3D11_TEXTURE2D_DESC desc = {};
    desc.Width = static_cast<UINT>(width_);
    desc.Height = static_cast<UINT>(height_);
    desc.MipLevels = 1;
    desc.ArraySize = 1;
    desc.Format = DXGI_FORMAT_NV12;
    desc.SampleDesc.Count = 1;
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = D3D11_BIND_RENDER_TARGET;
    ID3D11Texture2D* nv12 = nullptr;
    if (FAILED(dev->CreateTexture2D(&desc, nullptr, &nv12))) {
        if (err) *err = "创建 NV12 目标纹理失败";
        return false;
    }
    nv12_texture_ = nv12;

    ID3D11VideoDevice* vdev = nullptr;
    if (FAILED(dev->QueryInterface(__uuidof(ID3D11VideoDevice),
                                   reinterpret_cast<void**>(&vdev)))) {
        if (err) *err = "设备不支持 ID3D11VideoDevice";
        return false;
    }
    vp_device_ = vdev;
    ID3D11VideoContext* vctx = nullptr;
    ctx->QueryInterface(__uuidof(ID3D11VideoContext), reinterpret_cast<void**>(&vctx));
    vp_context_ = vctx;

    D3D11_VIDEO_PROCESSOR_CONTENT_DESC content = {};
    content.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
    content.InputWidth = static_cast<UINT>(width_);
    content.InputHeight = static_cast<UINT>(height_);
    content.OutputWidth = static_cast<UINT>(width_);
    content.OutputHeight = static_cast<UINT>(height_);
    content.Usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
    ID3D11VideoProcessorEnumerator* enumerator = nullptr;
    if (FAILED(vdev->CreateVideoProcessorEnumerator(&content, &enumerator))) {
        if (err) *err = "CreateVideoProcessorEnumerator 失败";
        return false;
    }
    vp_enumerator_ = enumerator;
    ID3D11VideoProcessor* processor = nullptr;
    if (FAILED(vdev->CreateVideoProcessor(enumerator, 0, &processor))) {
        if (err) *err = "CreateVideoProcessor 失败";
        return false;
    }
    vp_processor_ = processor;
    if (vctx) {
        vctx->VideoProcessorSetStreamFrameFormat(processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        vctx->VideoProcessorSetStreamOutputRate(processor, 0, D3D11_VIDEO_PROCESSOR_OUTPUT_RATE_NORMAL,
                                                 TRUE, nullptr);
        // 输入是 full-range RGB，输出给编码器的是 studio-range BT.709 NV12。
        D3D11_VIDEO_PROCESSOR_COLOR_SPACE in_cs = {};
        in_cs.Usage = 0;          // playback
        in_cs.RGB_Range = 0;      // full range RGB
        in_cs.YCbCr_Matrix = 1;   // BT.709
        in_cs.Nominal_Range = 0;  // 0-255
        D3D11_VIDEO_PROCESSOR_COLOR_SPACE out_cs = in_cs;
        out_cs.Nominal_Range = 1; // 16-235
        vctx->VideoProcessorSetStreamColorSpace(processor, 0, &in_cs);
        vctx->VideoProcessorSetOutputColorSpace(processor, &out_cs);
    }
    // 输出视图（NV12 目标）
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC odesc = {};
    odesc.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
    odesc.Texture2D.MipSlice = 0;
    ID3D11VideoProcessorOutputView* out_view = nullptr;
    if (FAILED(vdev->CreateVideoProcessorOutputView(nv12, enumerator, &odesc, &out_view))) {
        if (err) *err = "CreateVideoProcessorOutputView 失败";
        return false;
    }
    vp_out_view_ = out_view;
    return true;
}

void AmfEncoder::release_video_processor() {
    auto release = [](void*& p) {
        if (p) {
            reinterpret_cast<IUnknown*>(p)->Release();
            p = nullptr;
        }
    };
    release(vp_in_view_);
    release(vp_out_view_);
    release(vp_processor_);
    release(vp_enumerator_);
    release(vp_context_);
    release(vp_device_);
    release(nv12_texture_);
    vp_in_texture_ = nullptr;
}

bool AmfEncoder::push_rgba(const void* bgra, int stride_bytes, int64_t pts_us, std::string* err) {
    if (amf_component_ == nullptr || input_mode_ != kHostBgra) {
        if (err) *err = "push_rgba 仅在 kHostBgra 模式可用";
        return false;
    }
    amf::AMFContext* c = static_cast<amf::AMFContext*>(amf_context_);
    amf::AMFSurfacePtr surface;
    AMF_RESULT r = c->AllocSurface(AMF_MEMORY_HOST, AMF_SURFACE_BGRA, width_, height_, &surface);
    if (r != AMF_OK) {
        if (err) *err = std::string("AllocSurface(host BGRA): ") + result_name(r);
        return false;
    }
    amf::AMFPlane* plane = surface->GetPlaneAt(0);
    uint8_t* dst = static_cast<uint8_t*>(plane->GetNative());
    int pitch = plane->GetHPitch();
    const uint8_t* src = static_cast<const uint8_t*>(bgra);
    int row = std::min(width_ * 4, stride_bytes);
    for (int y = 0; y < height_; ++y) {
        std::memcpy(dst + static_cast<size_t>(y) * pitch, src + static_cast<size_t>(y) * stride_bytes,
                    static_cast<size_t>(row));
    }
    surface->SetPts(static_cast<amf_pts>(pts_us) * 10);
    amf::AMFComponent* comp = static_cast<amf::AMFComponent*>(amf_component_);
    r = comp->SubmitInput(surface);
    if (r != AMF_OK) {
        if (err) *err = std::string("SubmitInput: ") + result_name(r);
        return false;
    }
    return true;
}

bool AmfEncoder::push_texture(void* texture, uint32_t slice, int64_t pts_us, std::string* err) {
    if (amf_component_ == nullptr || texture == nullptr) {
        if (err) *err = "编码器未打开或纹理为空";
        return false;
    }
    if (slice != 0) {
        // AMF 的 CreateSurfaceFromDX11Native 只接受纹理本身，不接受数组层号；
        // 采集路径当前给的是单层纹理，这里显式拒绝多层，避免静默编错帧。
        if (err) *err = "纹理数组层号非 0，shim 不支持";
        return false;
    }
    ID3D11Texture2D* tex = reinterpret_cast<ID3D11Texture2D*>(texture);
    ID3D11Texture2D* submit_tex = tex;
    if (input_mode_ == kDx11Nv12) {
        if (!ensure_video_processor(err)) {
            return false;
        }
        ID3D11VideoDevice* vdev = reinterpret_cast<ID3D11VideoDevice*>(vp_device_);
        ID3D11VideoContext* vctx = reinterpret_cast<ID3D11VideoContext*>(vp_context_);
        if (vctx == nullptr) {
            if (err) *err = "缺少 ID3D11VideoContext";
            return false;
        }
        // 源纹理变化时重建输入视图（spike 复用同一纹理时只建一次）。
        if (vp_in_texture_ != tex) {
            if (vp_in_view_) {
                reinterpret_cast<IUnknown*>(vp_in_view_)->Release();
                vp_in_view_ = nullptr;
            }
            D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC idesc = {};
            idesc.FourCC = 0;
            idesc.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;
            idesc.Texture2D.MipSlice = 0;
            idesc.Texture2D.ArraySlice = 0;
            ID3D11VideoProcessorInputView* in_view = nullptr;
            if (FAILED(vdev->CreateVideoProcessorInputView(
                    tex, reinterpret_cast<ID3D11VideoProcessorEnumerator*>(vp_enumerator_), &idesc,
                    &in_view))) {
                if (err) *err = "CreateVideoProcessorInputView 失败";
                return false;
            }
            vp_in_view_ = in_view;
            vp_in_texture_ = tex;
        }
        D3D11_VIDEO_PROCESSOR_STREAM stream = {};
        stream.Enable = TRUE;
        stream.pInputSurface = reinterpret_cast<ID3D11VideoProcessorInputView*>(vp_in_view_);
        if (FAILED(vctx->VideoProcessorBlt(
                reinterpret_cast<ID3D11VideoProcessor*>(vp_processor_),
                reinterpret_cast<ID3D11VideoProcessorOutputView*>(vp_out_view_), 0, 1, &stream))) {
            if (err) *err = "VideoProcessorBlt 失败";
            return false;
        }
        submit_tex = reinterpret_cast<ID3D11Texture2D*>(nv12_texture_);
    }
    amf::AMFContext* c = static_cast<amf::AMFContext*>(amf_context_);
    amf::AMFSurfacePtr surface;
    AMF_RESULT r = c->CreateSurfaceFromDX11Native(submit_tex, &surface, nullptr);
    if (r != AMF_OK) {
        if (err) *err = std::string("CreateSurfaceFromDX11Native: ") + result_name(r);
        return false;
    }
    surface->SetPts(static_cast<amf_pts>(pts_us) * 10);
    amf::AMFComponent* comp = static_cast<amf::AMFComponent*>(amf_component_);
    r = comp->SubmitInput(surface);
    if (r != AMF_OK) {
        if (err) *err = std::string("SubmitInput: ") + result_name(r);
        return false;
    }
    return true;
}

int AmfEncoder::poll_raw(Packet* out) {
    if (amf_component_ == nullptr) {
        return -1;
    }
    amf::AMFComponent* comp = static_cast<amf::AMFComponent*>(amf_component_);
    amf::AMFDataPtr data;
    AMF_RESULT r = comp->QueryOutput(&data);
    if (r == AMF_REPEAT || r == AMF_NEED_MORE_INPUT) {
        return 0;
    }
    if (r == AMF_EOF) {
        return 0;
    }
    if (r != AMF_OK || data == nullptr) {
        error_ = std::string("QueryOutput: ") + result_name(r);
        return -1;
    }
    amf::AMFBufferPtr buffer(data);
    if (buffer == nullptr) {
        error_ = "输出不是 AMFBuffer";
        return -1;
    }
    amf_size size = buffer->GetSize();
    void* native = buffer->GetNative();
    if (out != nullptr) {
        out->data.assign(static_cast<uint8_t*>(native), static_cast<uint8_t*>(native) + size);
        out->pts_us = static_cast<int64_t>(data->GetPts() / 10);
        out->keyframe = false;
        amf::AMFVariant var;
        if (data->GetProperty(AMF_VIDEO_ENCODER_OUTPUT_DATA_TYPE, &var) == AMF_OK) {
            amf_int64 type = var.ToInt64();
            // IDR/I 都可作为「可独立解码起点」；P/B 不是。
            out->keyframe = (type == AMF_VIDEO_ENCODER_OUTPUT_DATA_TYPE_IDR ||
                             type == AMF_VIDEO_ENCODER_OUTPUT_DATA_TYPE_I);
        }
    }
    return 1;
}

int AmfEncoder::poll(uint8_t* out, int cap, int* out_len, int64_t* pts_us, int* keyframe) {
    Packet pkt;
    int got = poll_raw(&pkt);
    if (got <= 0) {
        return got;
    }
    int size = static_cast<int>(pkt.data.size());
    if (out != nullptr && cap > 0) {
        size = std::min(size, cap);
        std::memcpy(out, pkt.data.data(), static_cast<size_t>(size));
    }
    if (out_len) *out_len = static_cast<int>(pkt.data.size());
    if (pts_us) *pts_us = pkt.pts_us;
    if (keyframe) *keyframe = pkt.keyframe ? 1 : 0;
    return 1;
}

int AmfEncoder::drain() {
    if (amf_component_ == nullptr) {
        return 0;
    }
    amf::AMFComponent* comp = static_cast<amf::AMFComponent*>(amf_component_);
    AMF_RESULT r = comp->Drain();
    if (r != AMF_OK) {
        error_ = std::string("Drain: ") + result_name(r);
        return -1;
    }
    return 0;
}

void AmfEncoder::close() {
    release_video_processor();
    if (amf_component_) {
        amf::AMFComponent* comp = static_cast<amf::AMFComponent*>(amf_component_);
        comp->Terminate();
        comp->Release();
        amf_component_ = nullptr;
    }
    if (amf_context_) {
        amf::AMFContext* ctx = static_cast<amf::AMFContext*>(amf_context_);
        ctx->Terminate();
        ctx->Release();
        amf_context_ = nullptr;
    }
    amf_factory_ = nullptr;
    if (owns_device_) {
        if (context_) {
            reinterpret_cast<ID3D11DeviceContext*>(context_)->Release();
            context_ = nullptr;
        }
        if (device_) {
            reinterpret_cast<ID3D11Device*>(device_)->Release();
            device_ = nullptr;
            device_ = nullptr;
        }
    } else {
        if (context_) {
            reinterpret_cast<ID3D11DeviceContext*>(context_)->Release();
        }
        context_ = nullptr;
        device_ = nullptr;
    }
    owns_device_ = false;
    width_ = height_ = fps_ = bitrate_ = 0;
}

std::string AmfEncoder::effective_json() const {
    if (amf_component_ == nullptr) {
        return "{}";
    }
    amf::AMFComponent* comp = static_cast<amf::AMFComponent*>(amf_component_);
    auto get_i = [&](const wchar_t* name) -> long long {
        amf::AMFVariant var;
        if (comp->GetProperty(name, &var) == AMF_OK) {
            return static_cast<long long>(var.ToInt64());
        }
        return -1;
    };
    char buf[512];
    std::snprintf(buf, sizeof(buf),
                  "{\"usage\":%lld,\"low_latency\":%lld,\"b_frames\":%lld,\"max_refs\":%lld,"
                  "\"rc_method\":%lld,\"target_bitrate\":%lld,\"peak_bitrate\":%lld,"
                  "\"vbv_bits\":%lld,\"gop\":%lld,\"quality_preset\":%lld,\"query_timeout_ms\":%lld,"
                  "\"queue_depth\":1,\"usage_selected\":%d,\"usage_fallback\":%d}",
                  get_i(AMF_VIDEO_ENCODER_USAGE), get_i(AMF_VIDEO_ENCODER_LOWLATENCY_MODE),
                  get_i(AMF_VIDEO_ENCODER_B_PIC_PATTERN), get_i(AMF_VIDEO_ENCODER_MAX_NUM_REFRAMES),
                  get_i(AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD),
                  get_i(AMF_VIDEO_ENCODER_TARGET_BITRATE), get_i(AMF_VIDEO_ENCODER_PEAK_BITRATE),
                  get_i(AMF_VIDEO_ENCODER_VBV_BUFFER_SIZE), get_i(AMF_VIDEO_ENCODER_IDR_PERIOD),
                  get_i(AMF_VIDEO_ENCODER_QUALITY_PRESET),
                  get_i(AMF_VIDEO_ENCODER_QUERY_TIMEOUT),
                  usage_selected_, usage_fallback_ ? 1 : 0);
    return buf ? std::string(buf) : std::string("{}");
}

} // namespace rdcamf

// =====** C ABI 实现 =====**

static rdcamf::AmfEncoder g_encoder;

extern "C" int rdcamf_open(int width, int height, int fps, int bitrate_bps, int codec,
                          int input_mode, void* device) {
    std::string err;
    if (!g_encoder.open(width, height, fps, bitrate_bps, codec, input_mode, device, &err)) {
        return -1;
    }
    return 0;
}

extern "C" int rdcamf_push_rgba(const void* bgra, int stride_bytes, int64_t pts_us) {
    std::string err;
    return g_encoder.push_rgba(bgra, stride_bytes, pts_us, &err) ? 0 : -1;
}

extern "C" int rdcamf_push_texture(void* texture, uint32_t slice, int64_t pts_us) {
    std::string err;
    return g_encoder.push_texture(texture, slice, pts_us, &err) ? 0 : -1;
}

extern "C" int rdcamf_poll(uint8_t* out, int cap, int* out_len, int64_t* pts_us, int* keyframe) {
    return g_encoder.poll(out, cap, out_len, pts_us, keyframe);
}

extern "C" int rdcamf_drain(void) { return g_encoder.drain(); }

extern "C" void rdcamf_close(void) { g_encoder.close(); }

extern "C" const char* rdcamf_last_error(void) { return g_encoder.last_error().c_str(); }

extern "C" const char* rdcamf_effective_json(void) {
    static std::string cached;
    cached = g_encoder.effective_json();
    return cached.c_str();
}

extern "C" void* rdcamf_device(void) { return g_encoder.device(); }

// 诊断：分阶段设置属性并 Init，定位究竟是哪一组参数导致 AMF_OTHER/INVALID_ARG。
// 返回值按位：1=裸 Init，2=Usage，4=码控，8=VBV，16=低延迟，32=B帧/参考帧，64=HRD/Filler/超时。
extern "C" int rdcamf_diag(void) {
    AMFInit_Fn init_fn = amf_init_fn();
    if (init_fn == nullptr) {
        std::printf("DIAG=NO_AMF\n");
        return -1;
    }
    AMFFactory* factory = nullptr;
    if (init_fn(AMF_FULL_VERSION, &factory) != AMF_OK || factory == nullptr) {
        std::printf("DIAG=NO_FACTORY\n");
        return -1;
    }
    ID3D11Device* dev = nullptr;
    ID3D11DeviceContext* ctx = nullptr;
    HRESULT hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr,
                                   D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                                   nullptr, 0, D3D11_SDK_VERSION, &dev, nullptr, &ctx);
    if (FAILED(hr)) {
        std::printf("DIAG=NO_D3D\n");
        return -1;
    }
    AMFContextPtr amf_ctx;
    if (factory->CreateContext(&amf_ctx) != AMF_OK || amf_ctx->InitDX11(dev, AMF_DX11_0) != AMF_OK) {
        std::printf("DIAG=NO_CONTEXT\n");
        dev->Release();
        ctx->Release();
        return -1;
    }
    int mask = 0;
    // 逐项验证：哪一组属性会让 Init 返回 INVALID_ARG。
    auto run = [&](const char* name, int bit, auto setup) -> void {
        AMFComponentPtr comp;
        AMF_RESULT r = factory->CreateComponent(amf_ctx, AMFVideoEncoderVCE_AVC, &comp);
        if (r != AMF_OK) {
            std::printf("STAGE_%s=CREATE_FAIL code=%d\n", name, static_cast<int>(r));
            return;
        }
        setup(comp);
        r = comp->Init(AMF_SURFACE_NV12, 1920, 1080);
        std::printf("STAGE_%s=%s code=%d\n", name, r == AMF_OK ? "OK" : "FAIL", static_cast<int>(r));
        if (r == AMF_OK) {
            mask |= bit;
            comp->Terminate();
        }
    };
    run("BARE", 1, [](AMFComponentPtr&) {});
    run("USAGE_ULL", 2, [](AMFComponentPtr& c) {
        c->SetProperty(AMF_VIDEO_ENCODER_USAGE, static_cast<amf_int64>(AMF_VIDEO_ENCODER_USAGE_ULTRA_LOW_LATENCY));
    });
    run("USAGE_LL", 4, [](AMFComponentPtr& c) {
        c->SetProperty(AMF_VIDEO_ENCODER_USAGE, static_cast<amf_int64>(AMF_VIDEO_ENCODER_USAGE_LOW_LATENCY));
    });
    run("USAGE_LLHQ", 8, [](AMFComponentPtr& c) {
        c->SetProperty(AMF_VIDEO_ENCODER_USAGE, static_cast<amf_int64>(AMF_VIDEO_ENCODER_USAGE_LOW_LATENCY_HIGH_QUALITY));
    });
    run("CBR", 16, [](AMFComponentPtr& c) {
        c->SetProperty(AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD, static_cast<amf_int64>(AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD_CBR));
        c->SetProperty(AMF_VIDEO_ENCODER_TARGET_BITRATE, static_cast<amf_int64>(8000000));
        c->SetProperty(AMF_VIDEO_ENCODER_PEAK_BITRATE, static_cast<amf_int64>(8000000));
    });
    run("CBR_VBV", 32, [](AMFComponentPtr& c) {
        c->SetProperty(AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD, static_cast<amf_int64>(AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD_CBR));
        c->SetProperty(AMF_VIDEO_ENCODER_TARGET_BITRATE, static_cast<amf_int64>(8000000));
        c->SetProperty(AMF_VIDEO_ENCODER_PEAK_BITRATE, static_cast<amf_int64>(8000000));
        c->SetProperty(AMF_VIDEO_ENCODER_VBV_BUFFER_SIZE, static_cast<amf_int64>(8000000 / 60));
    });
    run("LOWLATENCY", 64, [](AMFComponentPtr& c) {
        c->SetProperty(AMF_VIDEO_ENCODER_LOWLATENCY_MODE, true);
    });
    run("BFRAMES_REF", 128, [](AMFComponentPtr& c) {
        c->SetProperty(AMF_VIDEO_ENCODER_B_PIC_PATTERN, static_cast<amf_int64>(0));
        c->SetProperty(AMF_VIDEO_ENCODER_MAX_CONSECUTIVE_BPICTURES, static_cast<amf_int64>(0));
        c->SetProperty(AMF_VIDEO_ENCODER_MAX_NUM_REFRAMES, static_cast<amf_int64>(1));
        c->SetProperty(AMF_VIDEO_ENCODER_PRE_ANALYSIS_ENABLE, false);
        c->SetProperty(AMF_VIDEO_ENCODER_HEADER_INSERTION_SPACING, static_cast<amf_int64>(0));
    });
    run("HRD_FILLER", 256, [](AMFComponentPtr& c) {
        c->SetProperty(AMF_VIDEO_ENCODER_ENFORCE_HRD, true);
        c->SetProperty(AMF_VIDEO_ENCODER_FILLER_DATA_ENABLE, true);
        c->SetProperty(AMF_VIDEO_ENCODER_QUERY_TIMEOUT, static_cast<amf_int64>(5));
        c->SetProperty(AMF_VIDEO_ENCODER_IDR_PERIOD, static_cast<amf_int64>(60));
        AMFRate rate;
        rate.num = 60;
        rate.den = 1;
        c->SetProperty(AMF_VIDEO_ENCODER_FRAMERATE, rate);
    });
    std::printf("DIAG_MASK=%d\n", mask);
    amf_ctx->Terminate();
    dev->Release();
    ctx->Release();
    return mask;
}

extern "C" int rdcamf_probe_caps(int* h264, int* hevc, int* driver_is_amd) {
    if (h264) *h264 = 0;
    if (hevc) *hevc = 0;
    if (driver_is_amd) *driver_is_amd = 0;
    AMFInit_Fn init_fn = amf_init_fn();
    if (init_fn == nullptr) {
        return 0;
    }
    if (driver_is_amd) *driver_is_amd = 1;
    amf::AMFFactory* factory = nullptr;
    if (init_fn(AMF_FULL_VERSION, &factory) != AMF_OK || factory == nullptr) {
        return 0;
    }
    amf::AMFContextPtr ctx;
    if (factory->CreateContext(&ctx) != AMF_OK) {
        return 0;
    }
    if (ctx->InitDX11(nullptr, AMF_DX11_0) != AMF_OK) {
        return 0;
    }
    amf::AMFComponentPtr comp;
    if (factory->CreateComponent(ctx, AMFVideoEncoderVCE_AVC, &comp) == AMF_OK &&
        comp->Init(AMF_SURFACE_NV12, 1920, 1080) == AMF_OK) {
        if (h264) *h264 = 1;
        comp->Terminate();
    }
    comp = nullptr;
    amf::AMFComponentPtr hevc_comp;
    if (factory->CreateComponent(ctx, AMFVideoEncoder_HEVC, &hevc_comp) == AMF_OK &&
        hevc_comp->Init(AMF_SURFACE_NV12, 1920, 1080) == AMF_OK) {
        if (hevc) *hevc = 1;
        hevc_comp->Terminate();
    }
    ctx->Terminate();
    return 1;
}
