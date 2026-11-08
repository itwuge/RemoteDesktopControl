// 用途：AMF spike 测量程序：实测 1080p 下的 push 耗时、首包延迟、CBR 码率与纹理直入路径。
//
// 编译（见 build_amf.ps1）：
//   g++ -std=c++17 -O2 -I <amf-include> amf_enc.cpp amf_spike.cpp -o amf_spike.exe \
//       -ld3d11 -ldxgi -lole32 -luuid -loleaut32
//
// 用法：
//   amf_spike.exe <frames> <mode> [diag]   旧用法：60fps 一进一出基线（mode 2 = VP→NV12）
//   amf_spike.exe probe <codec>            设置→读回逐项表（codec 0=H.264 1=HEVC）
//   amf_spike.exe gate <codec>             出包门控：只送 1 帧，200ms 后再送第 2 帧
//   amf_spike.exe lat <codec> <frames> <pace_fps|0>   paced=一进一出；0=free-run 灌输入队列
//   amf_spike.exe sweep <codec> <frames>   逐配置跑 lat + 码率 + PSNR
// 输出：KEY=VALUE 逐行，便于直接落 logs/s7/。

#include "amf_shim.h"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <utility>
#include <vector>

#include <windows.h>
#include <d3d11.h>

namespace {

const int kWidth = 1920;
const int kHeight = 1080;
const int kFps = 60;
const int kBitrate = 8 * 1000 * 1000;
const int kStride = kWidth * 4;
const int kTexCount = 12;  // 预生成纹理数：12×8.3MB≈100MB 显存；600 帧全量≈5GB 会爆显存

double now_us() {
    static LARGE_INTEGER freq = {};
    if (freq.QuadPart == 0) {
        QueryPerformanceFrequency(&freq);
    }
    LARGE_INTEGER t;
    QueryPerformanceCounter(&t);
    return static_cast<double>(t.QuadPart) * 1e6 / static_cast<double>(freq.QuadPart);
}

double now_ms() { return now_us() / 1000.0; }

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
    double p50 = 0, p90 = 0, p99 = 0, mean = 0, max = 0;
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
    s.p50 = v[static_cast<size_t>(0.50 * (v.size() - 1))];
    s.p90 = v[static_cast<size_t>(0.90 * (v.size() - 1))];
    s.p99 = v[static_cast<size_t>(0.99 * (v.size() - 1))];
    return s;
}

void spin_until(double target_us) {
    while (now_us() < target_us) {
        // 目标精度 <1ms：不用 Sleep，直接让出时间片（Sleep(0) 也要 ~0.1ms 粒度）
        YieldProcessor();
    }
}

struct TexPool {
    ID3D11Device* dev = nullptr;
    ID3D11DeviceContext* ctx = nullptr;
    std::vector<ID3D11Texture2D*> tex;
};

bool make_pool(TexPool* p) {
    p->dev = reinterpret_cast<ID3D11Device*>(rdcamf_device());
    if (p->dev == nullptr) {
        std::printf("DEVICE=NULL\n");
        return false;
    }
    p->dev->GetImmediateContext(&p->ctx);
    D3D11_TEXTURE2D_DESC desc = {};
    desc.Width = kWidth;
    desc.Height = kHeight;
    desc.MipLevels = 1;
    desc.ArraySize = 1;
    desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    desc.SampleDesc.Count = 1;
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
    std::vector<uint8_t> cpu(static_cast<size_t>(kStride) * kHeight);
    for (int i = 0; i < kTexCount; ++i) {
        ID3D11Texture2D* t = nullptr;
        if (FAILED(p->dev->CreateTexture2D(&desc, nullptr, &t))) {
            std::printf("TEXTURE=FAIL\n");
            return false;
        }
        fill_bgra(cpu.data(), i * 7);  // 预生成内容：i*7 保证纹理间有运动差异
        p->ctx->UpdateSubresource(t, 0, nullptr, cpu.data(), static_cast<UINT>(kStride), 0);
        p->tex.push_back(t);
    }
    return true;
}

void free_pool(TexPool* p) {
    for (ID3D11Texture2D* t : p->tex) t->Release();
    p->tex.clear();
    if (p->ctx) p->ctx->Release();
    p->ctx = nullptr;
}

struct Outcome {
    int packets = 0, keyframes = 0, drops = 0, polls_empty = 0, peak_inflight = 0;
    size_t bytes = 0;
    double elapsed_ms = 0;
    double psnr_sum = 0;
    int psnr_n = 0;
    std::vector<double> lat_ms;
    std::vector<double> push_ms;
    std::vector<double> submit_gap_ms;
};

struct Cfg {
    const char* name;
    std::vector<std::pair<const char*, int64_t>> props;
};

/// 一进一出（pace>0）或 free-run（pace=0）。lat = SubmitInput 返回 → 该帧出包。
Outcome run_lat(int codec, int frames, double pace_fps, const std::vector<std::pair<const char*, int64_t>>& props) {
    Outcome o;
    rdcamf_clear_preprops();
    rdcamf_preprop("querytimeout", 0);  // 不让 QueryOutput 内部等 5ms，保留真实出包时刻
    rdcamf_preprop("psnr", 1);
    for (const auto& p : props) rdcamf_preprop(p.first, p.second);
    if (rdcamf_open(kWidth, kHeight, kFps, kBitrate, codec, 2, nullptr) != 0) {
        std::printf("OPEN=FAIL\nOPEN_ERROR=%s\n", rdcamf_last_error());
        return o;
    }
    std::printf("EFFECTIVE=%s\n", rdcamf_effective_json());
    TexPool pool;
    if (!make_pool(&pool)) {
        rdcamf_close();
        return o;
    }

    std::vector<uint8_t> out(4 * 1024 * 1024);
    std::vector<int64_t> submit_us(frames, 0);
    std::vector<int64_t> submit_pts(frames, 0);
    std::vector<char> got(frames, 0);
    double start = now_us();
    double last_submit = start;
    int submitted = 0;
    int inflight = 0;

    auto submit_one = [&](int i) -> bool {
        int64_t pts = static_cast<int64_t>(i) * 1000000 / kFps;
        double t0 = now_us();
        submit_us[i] = static_cast<int64_t>(t0);
        submit_pts[i] = pts;
        int ok = rdcamf_push_texture(pool.tex[i % kTexCount], 0, pts);
        o.push_ms.push_back((now_us() - t0) / 1000.0);
        if (ok != 0) {
            ++o.drops;
            return false;
        }
        ++submitted;
        ++inflight;
        o.peak_inflight = std::max(o.peak_inflight, inflight);
        return true;
    };

    // 预热 20 帧（PTS 偏到 +10s，避免与主循环帧号混淆），不计入统计。
    // 预热后必须排空到「连续 30ms 无包」，否则残留的输入队列会把主循环测量值系统性抬高。
    for (int i = 0; i < 20; ++i) {
        int64_t pts = 10'000'000 + static_cast<int64_t>(i) * 1000000 / kFps;
        double t0 = now_us();
        submit_us[i] = static_cast<int64_t>(t0);
        submit_pts[i] = pts;
        o.push_ms.push_back(0);
        rdcamf_push_texture(pool.tex[i % kTexCount], 0, pts);
    }

    {
        double quiet_since = now_us();
        double hard_stop = quiet_since + 200000;
        while (now_us() < hard_stop && now_us() - quiet_since < 30000) {
            int len = 0, key = 0;
            int64_t pts = 0;
            if (rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key) == 1) {
                quiet_since = now_us();
            } else {
                YieldProcessor();
            }
        }
    }
    o.push_ms.clear();

    start = now_us();
    last_submit = start;
    int drained = 0;
    for (int i = 0; i < frames; ++i) {
        if (pace_fps > 0) {
            double target = start + static_cast<double>(i) * 1e6 / pace_fps;
            spin_until(target);
        }
        double t_sub = now_us();
        if (i > 0) {
            o.submit_gap_ms.push_back((t_sub - last_submit) / 1000.0);
        }
        last_submit = t_sub;
        submit_one(i);

        if (pace_fps > 0) {
            // 一进一出：等到本帧出包（最多 200ms）
            double deadline = now_us() + 200000;
            while (now_us() < deadline) {
                int len = 0, key = 0;
                int64_t pts = 0;
                int r = rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key);
                if (r == 1) {
                    ++o.packets;
                    o.keyframes += key;
                    o.bytes += static_cast<size_t>(len);
                    int idx = static_cast<int>(std::lround(pts * kFps / 1e6));
                    if (idx >= 0 && idx < frames && !got[idx]) {
                        got[idx] = 1;
                        o.lat_ms.push_back((now_us() - static_cast<double>(submit_us[idx])) / 1000.0);
                        double pa = 0, py = 0;
                        rdcamf_last_psnr(&pa, &py);
                        if (pa > 0) {
                            o.psnr_sum += pa;
                            ++o.psnr_n;
                        }
                    }
                    inflight = 0;
                    break;
                }
                if (r < 0) {
                    std::printf("POLL=FAIL frame=%d ERR=%s\n", i, rdcamf_last_error());
                    break;
                }
                ++o.polls_empty;
            }
        } else {
            // free-run：不让输入队列堆满；能收就收
            for (int k = 0; k < 8; ++k) {
                int len = 0, key = 0;
                int64_t pts = 0;
                int r = rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key);
                if (r == 1) {
                    ++o.packets;
                    o.keyframes += key;
                    o.bytes += static_cast<size_t>(len);
                    if (inflight > 0) --inflight;
                    int idx = static_cast<int>(std::lround(pts * kFps / 1e6));
                    if (idx >= 0 && idx < frames && !got[idx]) {
                        got[idx] = 1;
                        o.lat_ms.push_back((now_us() - static_cast<double>(submit_us[idx])) / 1000.0);
                    }
                } else {
                    ++o.polls_empty;
                    break;
                }
            }
        }
    }

    // 排空：把还在流水线里的帧收完（最多 200ms）
    rdcamf_drain();
    double deadline = now_us() + 200000;
    while (now_us() < deadline) {
        int len = 0, key = 0;
        int64_t pts = 0;
        int r = rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key);
        if (r != 1) break;
        ++drained;
        ++o.packets;
        o.bytes += static_cast<size_t>(len);
        int idx = static_cast<int>(std::lround(pts * kFps / 1e6));
        if (idx >= 0 && idx < frames && !got[idx]) {
            got[idx] = 1;
            o.lat_ms.push_back((now_us() - static_cast<double>(submit_us[idx])) / 1000.0);
        }
    }
    o.elapsed_ms = (now_us() - start) / 1000.0;
    std::printf("SUBMITTED=%d DRAINED=%d INFLIGHT_PEAK=%d\n", submitted, drained, o.peak_inflight);
    rdcamf_close();
    free_pool(&pool);
    return o;
}

void print_outcome(const char* tag, const Outcome& o, int frames) {
    Stats lat = summarize(o.lat_ms);
    Stats push = summarize(o.push_ms);
    Stats gap = summarize(o.submit_gap_ms);
    double sec = o.elapsed_ms > 0 ? o.elapsed_ms / 1000.0 : 1;
    std::printf(
        "CFG=%s LAT_P50_MS=%.3f LAT_P90_MS=%.3f LAT_P99_MS=%.3f LAT_MAX_MS=%.3f "
        "PUSH_P50_MS=%.3f PUSH_P99_MS=%.3f GAP_P50_MS=%.3f GAP_P99_MS=%.3f "
        "PKT=%d KBPS=%.0f MBPS=%.3f PSNR=%.2f NPSNR=%d FPS=%.2f\n",
        tag, lat.p50, lat.p90, lat.p99, lat.max, push.p50, push.p99, gap.p50, gap.p99,
        o.packets, static_cast<double>(o.bytes) * 8 / sec / 1000,
        static_cast<double>(o.bytes) * 8 / sec / 1e6,
        o.psnr_n > 0 ? o.psnr_sum / o.psnr_n : 0, o.psnr_n,
        static_cast<double>(o.packets) / sec);
    (void)frames;
}

int cmd_probe(int codec) {
    std::printf("AV1_COMPONENT=%d\n", rdcamf_probe_av1());
    rdcamf_clear_preprops();
    rdcamf_preprop("psnr", 1);
    if (rdcamf_open(kWidth, kHeight, kFps, kBitrate, codec, 2, nullptr) != 0) {
        std::printf("OPEN=FAIL\nOPEN_ERROR=%s\n", rdcamf_last_error());
        return 2;
    }
    std::printf("CODEC=%d\nEFFECTIVE=%s\n", codec, rdcamf_effective_json());
    struct Item {
        const char* name;
        int64_t value;
    };
    const int64_t q_speed = codec == 0 ? 1 : 10;
    const int64_t q_quality = codec == 0 ? 2 : 0;
    std::vector<Item> items = {
        {"usage", 2},          {"lowlatency", 1}, {"preanalysis", 1}, {"bpicpattern", 0},
        {"maxconsecb", 0},     {"reframes", 1},    {"quality", q_speed}, {"querytimeout", 0},
        {"idrperiod", 60},     {"target", kBitrate}, {"peak", kBitrate}, {"vbv", kBitrate / kFps},
        {"enforce_hrd", 1},    {"filler", 1},      {"header_spacing", 0}, {"slices", 1},
        {"inputqueue", 1},     {"outputmode", 0},  {"psnr", 1},
    };
    const char* names[] = {"usage", "lowlatency", "preanalysis", "bpicpattern", "maxconsecb",
                           "reframes", "quality", "querytimeout", "idrperiod", "rc", "target",
                           "peak", "vbv", "enforce_hrd", "filler", "header_spacing", "slices",
                           "inputqueue", "outputmode", "psnr"};
    for (const char* n : names) {
        int64_t before = 0;
        int rb = rdcamf_get_prop(n, &before);
        std::printf("PROP %-14s BEFORE rc=%d v=%lld", n, rb, static_cast<long long>(before));
        int64_t setv = 0;
        bool has_set = false;
        for (const Item& it : items) {
            if (std::strcmp(it.name, n) == 0) {
                setv = it.value;
                has_set = true;
            }
        }
        if (!has_set) {
            std::printf(" SET=- SETRC=- AFTER rc=- v=-\n");
            continue;
        }
        int rs = rdcamf_set_prop(n, setv);
        int64_t after = 0;
        int ra = rdcamf_get_prop(n, &after);
        std::printf(" SET=%lld SETRC=%d AFTER rc=%d v=%lld %s\n", static_cast<long long>(setv), rs,
                    ra, static_cast<long long>(after),
                    (ra == 0 && after == setv) ? "EFFECTIVE" : "NOT_EFFECTIVE");
    }
    // 质量档位对比：读回三个 preset 的真实数值（HEVC/H.264 枚举不同）
    (void)q_quality;
    rdcamf_close();
    return 0;
}

int cmd_gate(int codec) {
    rdcamf_clear_preprops();
    rdcamf_preprop("querytimeout", 0);
    if (rdcamf_open(kWidth, kHeight, kFps, kBitrate, codec, 2, nullptr) != 0) {
        std::printf("OPEN=FAIL\nOPEN_ERROR=%s\n", rdcamf_last_error());
        return 2;
    }
    std::printf("CODEC=%d\nEFFECTIVE=%s\n", codec, rdcamf_effective_json());
    TexPool pool;
    if (!make_pool(&pool)) {
        rdcamf_close();
        return 3;
    }
    std::vector<uint8_t> out(4 * 1024 * 1024);
    double t0 = now_us();
    rdcamf_push_texture(pool.tex[0], 0, 0);
    double deadline = t0 + 200000;
    int len = 0, key = 0;
    int64_t pts = 0;
    double first_ms = -1;
    while (now_us() < deadline) {
        if (rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key) == 1) {
            first_ms = (now_us() - t0) / 1000.0;
            break;
        }
    }
    std::printf("GATE_WITHOUT_NEXT_MS=%.3f PTS=%lld\n", first_ms, static_cast<long long>(pts));
    if (first_ms < 0) {
        double t_sub2 = now_us();
        rdcamf_push_texture(pool.tex[1], 0, 1000000 / kFps);
        std::printf("GATE_NO_OUTPUT_IN_200MS=1\n");
        deadline = now_us() + 100000;
        while (now_us() < deadline) {
            if (rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key) == 1) {
                std::printf("GATE_FIRST_AFTER_2ND_MS=%.3f SINCE_2ND_MS=%.3f PTS=%lld\n",
                            (now_us() - t0) / 1000.0, (now_us() - t_sub2) / 1000.0,
                            static_cast<long long>(pts));
                break;
            }
        }
        // 还剩几包：确认第 2 帧是否也等到下一帧才出
        double t2 = now_us();
        int got = 0;
        while (now_us() - t2 < 200000) {
            if (rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key) == 1) {
                ++got;
            }
        }
        std::printf("GATE_AFTER_2ND_EXTRA_PACKETS=%d\n", got);
    }
    rdcamf_close();
    free_pool(&pool);
    return 0;
}

int cmd_lat(int codec, int frames, double pace_fps) {
    Outcome o = run_lat(codec, frames, pace_fps, {});
    print_outcome(pace_fps > 0 ? "PACE" : "FREERUN", o, frames);
    Stats lat = summarize(o.lat_ms);
    int over30 = 0;
    for (double x : o.lat_ms) over30 += x >= 30.0;
    std::printf("LAT_OVER_30MS=%d/%zu SAMPLES=%zu POLLS_EMPTY=%d\n", over30, o.lat_ms.size(),
                o.lat_ms.size(), o.polls_empty);
    (void)lat;
    return 0;
}

int cmd_sweep(int codec, int frames) {
    const int64_t q_bal = codec == 0 ? 0 : 5;
    const int64_t q_qual = codec == 0 ? 2 : 0;
    std::vector<Cfg> cfgs;
    cfgs.push_back({"base", {}});
    cfgs.push_back({"ll_off", {{"lowlatency", 0}}});
    cfgs.push_back({"prean_on", {{"preanalysis", 1}}});
    cfgs.push_back({"q_balanced", {{"quality", q_bal}}});
    cfgs.push_back({"q_quality", {{"quality", q_qual}}});
    cfgs.push_back({"slices2", {{"slices", 2}}});
    cfgs.push_back({"qtimeout5", {{"querytimeout", 5}}});
    if (codec == 0) {
        cfgs.push_back({"inputq16", {{"inputqueue", 16}}});
        cfgs.push_back({"filler_off", {{"enforce_hrd", 0}, {"filler", 0}}});
        cfgs.push_back({"refs2", {{"reframes", 2}}});
    }
    for (const Cfg& c : cfgs) {
        std::printf("--- CFG %s ---\n", c.name);
        Outcome o = run_lat(codec, frames, 60.0, c.props);
        print_outcome(c.name, o, frames);
    }
    return 0;
}

/// 复刻生产主循环的取包节奏：每帧只做「poll 到空就停」，然后等下一帧。
/// retry=1 时改成「本轮没拿到包就继续 poll，直到拿到或超 20ms」——这是块 1 找出的调度修法。
int cmd_appsim(int codec, int frames, int qt_ms, int retry) {
    rdcamf_clear_preprops();
    rdcamf_preprop("querytimeout", qt_ms);
    if (rdcamf_open(kWidth, kHeight, kFps, kBitrate, codec, 2, nullptr) != 0) {
        std::printf("OPEN=FAIL\nOPEN_ERROR=%s\n", rdcamf_last_error());
        return 2;
    }
    std::printf("APPSIM codec=%d qt=%d retry=%d\nEFFECTIVE=%s\n", codec, qt_ms, retry,
                rdcamf_effective_json());
    TexPool pool;
    if (!make_pool(&pool)) {
        rdcamf_close();
        return 3;
    }
    std::vector<uint8_t> out(4 * 1024 * 1024);
    std::vector<int64_t> pending;  // 提交时刻（微秒），FIFO 对应生产端的 pending_submit
    std::vector<double> enc_wait;
    double start = now_us();
    int packets = 0;
    for (int i = 0; i < frames; ++i) {
        double target = start + static_cast<double>(i) * 1e6 / kFps;
        spin_until(target);
        rdcamf_push_texture(pool.tex[i % kTexCount], 0, static_cast<int64_t>(i) * 1000000 / kFps);
        pending.push_back(now_us());
        // 生产端节奏：poll 一次，空则本轮结束（到下一帧再 poll）
        int len = 0, key = 0;
        int64_t pts = 0;
        while (true) {
            int r = rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key);
            if (r == 1) {
                ++packets;
                if (!pending.empty()) {
                    enc_wait.push_back((now_us() - static_cast<double>(pending.front())) / 1000.0);
                    pending.erase(pending.begin());
                }
                continue;
            }
            if (retry && !pending.empty() && now_us() - pending.front() < 20000) {
                continue;  // 本轮帧还没出包：继续等，而不是推到下一帧
            }
            break;
        }
        if (pending.size() > 32) pending.clear();
    }
    rdcamf_drain();
    double t_end = now_us() + 100000;
    while (now_us() < t_end) {
        int len = 0, key = 0;
        int64_t pts = 0;
        if (rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key) != 1) break;
        ++packets;
    }
    Stats s = summarize(enc_wait);
    std::printf(
        "APPSIM_RESULT qt=%d retry=%d ENC_WAIT_P50_MS=%.3f P90=%.3f P99=%.3f MAX=%.3f "
        "N=%zu PKT=%d\n",
        qt_ms, retry, s.p50, s.p90, s.p99, s.max, enc_wait.size(), packets);
    rdcamf_close();
    free_pool(&pool);
    return 0;
}

int legacy_main(int argc, char** argv) {
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
    std::vector<double> push_ms;
    std::vector<size_t> sizes;
    std::vector<double> instant_mbps;
    int drops = 0, packets = 0, keyframes = 0;

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
    std::vector<uint8_t> out(4 * 1024 * 1024);
    while (now_ms() - t0 < 500.0) {
        int len = 0, key = 0;
        int64_t pts = 0;
        int r = rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key);
        if (r == 1) {
            first_packet_ms = now_ms() - t0;
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
    std::printf("FIRST_PACKET_MS=%.3f\n", first_packet_ms);

    double start = now_ms();
    for (int i = 1; i < frames; ++i) {
        double target = start + static_cast<double>(i) * 1000.0 / kFps;
        if (now_ms() < target) {
            Sleep(static_cast<DWORD>(std::max(0.0, target - now_ms())));
        }
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
        } else if (rdcamf_push_rgba(cpu.data(), kStride, static_cast<int64_t>(i) * 1000000 / kFps) != 0) {
            ++drops;
            continue;
        }
        push_ms.push_back(now_ms() - submit_start);

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
                latencies.push_back(now_ms() - submit_start);
                instant_mbps.push_back(static_cast<double>(len) * 8.0 / 1e6 * kFps);
                got = true;
                break;
            }
            if (r < 0) {
                std::printf("POLL=FAIL frame=%d ERR=%s\n", i, rdcamf_last_error());
                return 7;
            }
            Sleep(0);
        }
        if (!got) ++drops;
    }

    double elapsed_ms = now_ms() - start;
    double measured_s = elapsed_ms / 1000.0;
    size_t total_bytes = 0;
    for (size_t s : sizes) total_bytes += s;
    double fps_achieved = static_cast<double>(packets) / measured_s;
    Stats push = summarize(push_ms);
    Stats lat = summarize(latencies);
    Stats upl = summarize(uploads);
    double avg_mbps = static_cast<double>(total_bytes) * 8.0 / measured_s / 1e6;
    double max_inst = 0, sum_inst = 0;
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

    int drained = 0;
    if (rdcamf_drain() == 0) {
        double t = now_ms();
        while (now_ms() - t < 100.0) {
            int len = 0, key = 0;
            int64_t pts = 0;
            if (rdcamf_poll(out.data(), static_cast<int>(out.size()), &len, &pts, &key) != 1) break;
            ++drained;
        }
    }
    std::printf("DRAIN_PACKETS=%d\n", drained);

    if (tex) tex->Release();
    if (ctx) ctx->Release();
    rdcamf_close();
    std::printf("DONE=1\n");
    return 0;
}

} // namespace

int main(int argc, char** argv) {
    if (argc > 1 && std::strcmp(argv[1], "probe") == 0) {
        return cmd_probe(argc > 2 ? std::atoi(argv[2]) : 0);
    }
    if (argc > 1 && std::strcmp(argv[1], "gate") == 0) {
        return cmd_gate(argc > 2 ? std::atoi(argv[2]) : 0);
    }
    if (argc > 1 && std::strcmp(argv[1], "lat") == 0) {
        return cmd_lat(argc > 2 ? std::atoi(argv[2]) : 0, argc > 3 ? std::atoi(argv[3]) : 200,
                       argc > 4 ? std::atof(argv[4]) : 60.0);
    }
    if (argc > 1 && std::strcmp(argv[1], "sweep") == 0) {
        return cmd_sweep(argc > 2 ? std::atoi(argv[2]) : 0, argc > 3 ? std::atoi(argv[3]) : 150);
    }
    if (argc > 1 && std::strcmp(argv[1], "appsim") == 0) {
        return cmd_appsim(argc > 2 ? std::atoi(argv[2]) : 0, argc > 3 ? std::atoi(argv[3]) : 300,
                          argc > 4 ? std::atoi(argv[4]) : 5, argc > 5 ? std::atoi(argv[5]) : 0);
    }
    return legacy_main(argc, argv);
}
