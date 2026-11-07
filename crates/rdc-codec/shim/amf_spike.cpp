// AMF spike：实测 1080p60 下的 push 耗时、首包延迟、CBR 码率与纹理直入路径。
//
// 编译（见 build_amf.ps1）：
//   g++ -std=c++17 -O2 -I <amf-include> amf_enc.cpp amf_spike.cpp -o amf_spike.exe \
//       -ld3d11 -ldxgi -lole32 -luuid
//
// 用法：amf_spike.exe <frames> <mode>   mode: 0=host BGRA, 1=DX11 BGRA, 2=VideoProcessor→NV12
// 输出：KEY=VALUE 逐行，便于直接落 logs/s7/。

#include "amf_shim.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include <windows.h>
#include <d3d11.h>

namespace {

const int kWidth = 1920;
const int kHeight = 1080;
const int kFps = 60;
const int kBitrate = 8 * 1000 * 1000;
const int kStride = kWidth * 4;

double now_ms() {
    static LARGE_INTEGER freq = {};
    if (freq.QuadPart == 0) {
        QueryPerformanceFrequency(&freq);
    }
    LARGE_INTEGER t;
    QueryPerformanceCounter(&t);
    return static_cast<double>(t.QuadPart) * 1000.0 / static_cast<double>(freq.QuadPart);
}

std::vector<double> g_push_ms;

/// 生成一帧桌面类内容：斜向渐变 + 移动竖条 + 棋盘纹理，保证码率不是「静止画面」下的假值。
void fill_bgra(uint8_t* dst, int frame_index) {
    const int bar = (frame_index * 6) % kWidth;
    for (int y = 0; y < kHeight; ++y) {
        uint8_t* row = dst + static_cast<size_t>(y) * kStride;
        for (int x = 0; x < kWidth; ++x) {
            int gx = (x + y) / 8;
            int gy = (x - y) / 8;
            int checker = ((gx & 1) ^ (gy & 1)) ? 18 : 0;
            int dx = std::abs(x - bar);
            int band = dx < 60 ? 70 : 0;
            row[x * 4 + 0] = static_cast<uint8_t>((x * 255 / kWidth + checker + band) & 0xFF);
            row[x * 4 + 1] = static_cast<uint8_t>((y * 255 / kHeight + checker) & 0xFF);
            row[x * 4 + 2] = static_cast<uint8_t>((x + y + checker + band) & 0xFF);
            row[x * 4 + 3] = 0xFF;
        }
    }
}

struct Stats {
    double p50 = 0;
    double p99 = 0;
    double mean = 0;
    double max = 0;
};

Stats summarize(std::vector<double> v) {
    Stats s;
    if (v.empty()) {
        return s;
    }
    std::sort(v.begin(), v.end());
    double sum = 0;
    for (double x : v) sum += x;
    s.mean = sum / static_cast<double>(v.size());
    s.max = v.back();
    size_t i50 = static_cast<size_t>(0.50 * (v.size() - 1));
    size_t i99 = static_cast<size_t>(0.99 * (v.size() - 1));
    s.p50 = v[i50];
    s.p99 = v[i99];
    return s;
}

} // namespace

int main(int argc, char** argv) {
    int frames = argc > 1 ? std::atoi(argv[1]) : 600;
    int mode = argc > 2 ? std::atoi(argv[2]) : 1;
    if (frames < 60) frames = 60;
    if (argc > 3 && std::strcmp(argv[3], "diag") == 0) {
        return rdcamf_diag() >= 0 ? 0 : 1;
    }

    std::printf("MODE=%d\nFRAMES=%d\nWIDTH=%d\nHEIGHT=%d\nFPS=%d\nTARGET_BPS=%d\n",
                mode, frames, kWidth, kHeight, kFps, kBitrate);

    if (rdcamf_open(kWidth, kHeight, kFps, kBitrate, 0, mode, nullptr) != 0) {
        std::printf("OPEN=FAIL\nOPEN_ERROR=%s\n", rdcamf_last_error());
        return 2;
    }
    std::printf("OPEN=OK\nEFFECTIVE=%s\n", rdcamf_effective_json());

    ID3D11Device* dev = reinterpret_cast<ID3D11Device*>(rdcamf_device());
    ID3D11DeviceContext* ctx = nullptr;
    ID3D11Texture2D* tex = nullptr;
    std::vector<uint8_t> cpu(static_cast<size_t>(kStride) * kHeight);
    if (mode == 1 || mode == 2) {
        if (dev == nullptr) {
            std::printf("DEVICE=NULL\n");
            return 3;
        }
        dev->GetImmediateContext(&ctx);
        D3D11_TEXTURE2D_DESC desc = {};
        desc.Width = kWidth;
        desc.Height = kHeight;
        desc.MipLevels = 1;
        desc.ArraySize = 1;
        desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.SampleDesc.Count = 1;
        desc.Usage = D3D11_USAGE_DEFAULT;
        desc.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
        if (FAILED(dev->CreateTexture2D(&desc, nullptr, &tex))) {
            std::printf("TEXTURE=FAIL\n");
            return 4;
        }
    }

    std::vector<double> latencies;
    std::vector<double> uploads;
    std::vector<size_t> sizes;
    std::vector<double> instant_mbps;
    std::vector<int64_t> pts_out;
    int drops = 0;
    int packets = 0;
    int keyframes = 0;

    // ---- 首包测试：只送第 1 帧，看是否第 1 帧即出包 ----
    fill_bgra(cpu.data(), 0);
    double t0 = now_ms();
    if (mode >= 1) {
        ctx->UpdateSubresource(tex, 0, nullptr, cpu.data(), static_cast<UINT>(kStride), 0);
        if (rdcamf_push_texture(tex, 0, 0) != 0) {
            std::printf("PUSH0=FAIL\nERR=%s\n", rdcamf_last_error());
            return 5;
        }
    } else if (rdcamf_push_rgba(cpu.data(), kStride, 0) != 0) {
        std::printf("PUSH0=FAIL\nERR=%s\n", rdcamf_last_error());
        return 5;
    }
    double first_packet_ms = -1;
    int first_frame_index = -1;
    std::vector<uint8_t> out(4 * 1024 * 1024);
    while (now_ms() - t0 < 500.0) {
        int len = 0, key = 0;
        int64_t pts = 0;
        int r = rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key);
        if (r == 1) {
            first_packet_ms = now_ms() - t0;
            first_frame_index = 1;
            packets++;
            keyframes += key;
            sizes.push_back(static_cast<size_t>(len));
            break;
        }
        if (r < 0) {
            std::printf("POLL0=FAIL\nERR=%s\n", rdcamf_last_error());
            return 6;
        }
    }
    std::printf("FIRST_PACKET_FRAME=%d\nFIRST_PACKET_MS=%.3f\n", first_frame_index, first_packet_ms);

    // ---- 主循环：一进一出，排队深度 1，按 60 fps 节拍 ----
    double start = now_ms();
    double next_tick = start;
    for (int i = 1; i < frames; ++i) {
        double target = start + static_cast<double>(i) * 1000.0 / kFps;
        if (next_tick < target) {
            Sleep(static_cast<DWORD>(std::max(0.0, target - next_tick)));
        }
        next_tick = now_ms();

        fill_bgra(cpu.data(), i);
        double submit_start = now_ms();
        if (mode >= 1) {
            ctx->UpdateSubresource(tex, 0, nullptr, cpu.data(), static_cast<UINT>(kStride), 0);
            uploads.push_back(now_ms() - submit_start);
            submit_start = now_ms();
            if (rdcamf_push_texture(tex, 0, static_cast<int64_t>(i) * 1000000 / kFps) != 0) {
                ++drops;
                continue;
            }
        } else {
            if (rdcamf_push_rgba(cpu.data(), kStride, static_cast<int64_t>(i) * 1000000 / kFps) != 0) {
                ++drops;
                continue;
            }
        }
        double push_ms = now_ms() - submit_start;
        g_push_ms.push_back(push_ms);

        // 等到这一帧出包（QueryTimeout=5ms，事件驱动；不把暂无输出当错误）
        int64_t expect_pts = static_cast<int64_t>(i) * 1000000 / kFps;
        double wait_start = now_ms();
        bool got = false;
        while (now_ms() - wait_start < 100.0) {
            int len = 0, key = 0;
            int64_t pts = 0;
            int r = rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key);
            if (r == 1) {
                packets++;
                keyframes += key;
                sizes.push_back(static_cast<size_t>(len));
                pts_out.push_back(pts);
                latencies.push_back(wait_start - submit_start);
                instant_mbps.push_back(static_cast<double>(len) * 8.0 / 1e6 * kFps);
                got = true;
                break;
            }
            if (r < 0) {
                std::printf("POLL=FAIL frame=%d ERR=%s\n", i, rdcamf_last_error());
                return 7;
            }
            (void)expect_pts;
            Sleep(0);
        }
        if (!got) {
            ++drops;
        }
    }

    double elapsed_ms = now_ms() - start;
    double measured_s = elapsed_ms / 1000.0;
    double total_bits = 0;
    size_t total_bytes = 0;
    for (size_t s : sizes) total_bytes += s;
    total_bits = static_cast<double>(total_bytes) * 8.0;
    double fps_achieved = static_cast<double>(packets) / measured_s;

    Stats push = summarize(g_push_ms);
    Stats lat = summarize(latencies);
    Stats upl = summarize(uploads);
    double avg_mbps = total_bits / measured_s / 1e6;
    double max_inst = 0;
    double sum_inst = 0;
    for (double m : instant_mbps) {
        max_inst = std::max(max_inst, m);
        sum_inst += m;
    }
    double mean_inst = instant_mbps.empty() ? 0 : sum_inst / static_cast<double>(instant_mbps.size());

    std::printf("PACKETS=%d\nKEYFRAMES=%d\nDROPS=%d\n", packets, keyframes, drops);
    std::printf("PUSH_P50_MS=%.3f\nPUSH_P99_MS=%.3f\nPUSH_MEAN_MS=%.3f\nPUSH_MAX_MS=%.3f\n",
                push.p50, push.p99, push.mean, push.max);
    std::printf("UPLOAD_P50_MS=%.3f\nUPLOAD_P99_MS=%.3f\n", upl.p50, upl.p99);
    std::printf("ENC_LAT_P50_MS=%.3f\nENC_LAT_P99_MS=%.3f\nENC_LAT_MAX_MS=%.3f\n",
                lat.p50, lat.p99, lat.max);
    std::printf("FPS_ACHIEVED=%.2f\nELAPSED_MS=%.1f\n", fps_achieved, elapsed_ms);
    std::printf("AVG_MBPS=%.3f\nMEAN_INST_MBPS=%.3f\nMAX_INST_MBPS=%.3f\n",
                avg_mbps, mean_inst, max_inst);

    // drain：收尾必须排空，避免把尾帧当丢失
    int drained = 0;
    if (rdcamf_drain() == 0) {
        double t = now_ms();
        while (now_ms() - t < 100.0) {
            int len = 0, key = 0;
            int64_t pts = 0;
            int r = rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key);
            if (r == 1) {
                ++drained;
                continue;
            }
            break;
        }
    }
    std::printf("DRAIN_PACKETS=%d\n", drained);

    if (tex) tex->Release();
    if (ctx) ctx->Release();
    rdcamf_close();
    std::printf("DONE=1\n");
    return 0;
}
