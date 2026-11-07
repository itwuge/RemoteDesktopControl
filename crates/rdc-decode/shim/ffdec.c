/*
 * rdc-decode / FFmpeg H.264 解码 C shim
 *
 * 层级：L3 编解码层 · 平台适配最内层
 * 用途：把 libavcodec 的「送包 → 收帧」循环收敛成极小的 C ABI，供 Rust 侧
 *       `ffmpeg_decoder.rs` 调用。只在 `ffmpeg` feature 打开时由 build.rs 编译，
 *       FFmpeg 以 **动态链接**（avcodec-*.dll / avutil-*.dll）方式引入，禁止静态链接。
 * 低延迟配置：`AV_CODEC_FLAG_LOW_DELAY` + `thread_count=1`（或 slice 线程），
 *       从源头避免 frame-level 多线程引入的 (N-1) 帧重排延迟。
 * 输出：默认系统内存 NV12（copied 成紧密排列，供 D3D11 上传）；绑定 D3D11 时
 *       直接返回 GPU 纹理指针（AV_PIX_FMT_D3D11），绝不做 GPU→CPU 回读。
 *
 * 许可：FFmpeg 为 LGPL v3（本构建 `--enable-version3 --disable-gpl`，不含 x264/x265 等 GPL 组件）。
 * 分发时必须随包附 LICENSE.txt 与源码获取方式。
 */

#include <libavcodec/avcodec.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_d3d11va.h>
#include <libavutil/imgutils.h>
#include <libavutil/time.h>

#include <stdint.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* 一帧输出。系统内存路径填 plane0/plane1；D3D11 路径 plane0 是 ID3D11Texture2D*，
 * array_slice 是纹理数组下标。结构对 Rust 侧 #[repr(C)] 可见。 */
typedef struct RdcFfFrame {
    const unsigned char *plane0;
    const unsigned char *plane1;
    int32_t stride0;
    int32_t stride1;
    int32_t width;
    int32_t height;
    int32_t is_texture;
    int32_t array_slice;
    int64_t pts_us;
} RdcFfFrame;

typedef int (*RdcFfFrameCb)(void *ctx, const RdcFfFrame *frame);

/* 累计统计：解码（send+receive）、NV12 整理拷贝各自耗时，供分段报告。 */
typedef struct RdcFfStats {
    int64_t feeds;
    int64_t frames;
    double decode_ms_total;
    double pack_ms_total;
    int32_t threads;
    int32_t delay_frames;
} RdcFfStats;

typedef struct RdcFfDec {
    AVCodecContext *ctx;
    AVPacket *pkt;
    AVFrame *frame;
    AVBufferRef *hw_ctx;
    unsigned char *pack;
    int pack_cap;
    int logged_fmt;
    RdcFfStats stats;
} RdcFfDec;

void rdff_destroy(RdcFfDec *d);

static void set_err(char *err, int errlen, const char *msg) {
    if (err && errlen > 0) {
        snprintf(err, (size_t)errlen, "%s", msg);
    }
}

static void set_av_err(char *err, int errlen, const char *what, int ret) {
    if (err && errlen > 0) {
        snprintf(err, (size_t)errlen, "%s: %s (%d)", what, av_err2str(ret), ret);
    }
}

static int emit_frame(RdcFfDec *d, const RdcFfFrame *frame, RdcFfFrameCb cb, void *cbctx,
                      char *err, int errlen) {
    d->stats.frames += 1;
    if (cb) {
        int rc = cb(cbctx, frame);
        if (rc != 0) {
            set_err(err, errlen, "帧回调请求中止");
            return AVERROR_EXIT;
        }
    }
    return 0;
}

/* 把 AVFrame 拷成紧密 NV12（Y 平面 + 交错 UV，行距 = width）。
 * 软解默认输出 yuv420p（U/V 各自独立平面），必须显式交错；
 * d3d11va 输出才是 NV12，可直接按行拷贝。两条都要支持，否则颜色会整体偏绿。 */
static int pack_nv12(RdcFfDec *d, AVFrame *frame, char *err, int errlen) {
    int64_t t0 = av_gettime_relative();
    int w = frame->width;
    int h = frame->height;
    int need = w * h + (w * h) / 2;
    if (d->pack_cap < need) {
        unsigned char *next = (unsigned char *)realloc(d->pack, (size_t)need);
        if (!next) {
            set_err(err, errlen, "NV12 打包缓冲分配失败");
            return AVERROR(ENOMEM);
        }
        d->pack = next;
        d->pack_cap = need;
    }
    for (int row = 0; row < h; ++row) {
        memcpy(d->pack + (size_t)row * w, frame->data[0] + (size_t)row * frame->linesize[0],
               (size_t)w);
    }
    unsigned char *uv = d->pack + (size_t)w * h;
    if (frame->format == AV_PIX_FMT_YUV420P || frame->format == AV_PIX_FMT_YUVJ420P) {
        int cw = w / 2;
        for (int row = 0; row < h / 2; ++row) {
            const unsigned char *u = frame->data[1] + (size_t)row * frame->linesize[1];
            const unsigned char *v = frame->data[2] + (size_t)row * frame->linesize[2];
            unsigned char *dst = uv + (size_t)row * w;
            for (int col = 0; col < cw; ++col) {
                dst[col * 2] = u[col];
                dst[col * 2 + 1] = v[col];
            }
        }
    } else {
        /* NV12：data[1] 已是交错 UV。 */
        for (int row = 0; row < h / 2; ++row) {
            memcpy(uv + (size_t)row * w, frame->data[1] + (size_t)row * frame->linesize[1],
                   (size_t)w);
        }
    }
    d->stats.pack_ms_total += (double)(av_gettime_relative() - t0) / 1000.0;
    return 0;
}

/* 收帧直到 EAGAIN/EOF；每帧回调一次。 */
static int drain_frames(RdcFfDec *d, RdcFfFrameCb cb, void *cbctx, char *err, int errlen) {
    int ret;
    while ((ret = avcodec_receive_frame(d->ctx, d->frame)) >= 0) {
        RdcFfFrame out;
        memset(&out, 0, sizeof(out));
        out.width = d->frame->width;
        out.height = d->frame->height;
        out.pts_us = d->frame->best_effort_timestamp;

        if (!d->logged_fmt) {
            d->logged_fmt = 1;
            fprintf(stderr, "[ffdec] first frame format=%s hw_device_ctx=%s\n",
                    av_get_pix_fmt_name((enum AVPixelFormat)d->frame->format),
                    d->ctx->hw_device_ctx ? "yes" : "no");
        }
        if (d->frame->format == AV_PIX_FMT_D3D11) {
            /* data[0] 是 ID3D11Texture2D*，data[1] 是数组下标；由 AVFrame 持有纹理引用，
             * 回调内同步使用即可，不做任何回读。 */
            out.is_texture = 1;
            out.plane0 = (const unsigned char *)d->frame->data[0];
            out.array_slice = (int32_t)(intptr_t)d->frame->data[1];
            out.stride0 = 0;
            out.stride1 = 0;
        } else {
            if (pack_nv12(d, d->frame, err, errlen) != 0) {
                av_frame_unref(d->frame);
                return AVERROR(ENOMEM);
            }
            out.is_texture = 0;
            out.plane0 = d->pack;
            out.stride0 = d->frame->width;
            out.plane1 = d->pack + (size_t)d->frame->width * d->frame->height;
            out.stride1 = d->frame->width;
        }
        int rc = emit_frame(d, &out, cb, cbctx, err, errlen);
        av_frame_unref(d->frame);
        if (rc != 0) {
            return rc;
        }
    }
    if (ret == AVERROR(EAGAIN) || ret == AVERROR_EOF) {
        return 0;
    }
    static int logged_err = 0;
    if (!logged_err) {
        logged_err = 1;
        fprintf(stderr,
                "[ffdec] receive err=%d pix_fmt=%s sw_pix=%s hw_frames=%p coded=%dx%d\n",
                ret, av_get_pix_fmt_name(d->ctx->pix_fmt),
                av_get_pix_fmt_name(d->ctx->sw_pix_fmt), (void *)d->ctx->hw_frames_ctx,
                d->ctx->coded_width, d->ctx->coded_height);
    }
    set_av_err(err, errlen, "avcodec_receive_frame", ret);
    return ret;
}

/* FFmpeg 只保留 warning 及以下级别，直接写 stderr；用于诊断硬解/软解的实际选择。 */
static void rdff_log_cb(void *avcl, int level, const char *fmt, va_list vl) {
    (void)avcl;
    if (level > AV_LOG_WARNING) {
        return;
    }
    fprintf(stderr, "[ffav] ");
    vfprintf(stderr, fmt, vl);
    fprintf(stderr, "\n");
}

/* 创建解码器实例（未 open）。threads<=0 表示默认；low_delay!=0 打开低延迟标志。 */
int rdff_create(int32_t threads, int32_t low_delay, void **out, char *err, int errlen) {
    if (!out) {
        set_err(err, errlen, "输出指针为空");
        return AVERROR(EINVAL);
    }
    const AVCodec *codec = avcodec_find_decoder(AV_CODEC_ID_H264);
    if (!codec) {
        set_err(err, errlen, "libavcodec 中没有 H.264 解码器");
        return AVERROR_DECODER_NOT_FOUND;
    }
    RdcFfDec *d = (RdcFfDec *)calloc(1, sizeof(RdcFfDec));
    if (!d) {
        set_err(err, errlen, "解码器分配失败");
        return AVERROR(ENOMEM);
    }
    d->ctx = avcodec_alloc_context3(codec);
    d->pkt = av_packet_alloc();
    d->frame = av_frame_alloc();
    if (!d->ctx || !d->pkt || !d->frame) {
        rdff_destroy(d);
        set_err(err, errlen, "avcodec 上下文分配失败");
        return AVERROR(ENOMEM);
    }
    if (low_delay) {
        d->ctx->flags |= AV_CODEC_FLAG_LOW_DELAY;
    }
    av_log_set_callback(rdff_log_cb);
    av_log_set_level(AV_LOG_WARNING);
    d->ctx->thread_count = threads > 0 ? threads : 1;
    /* 单线程即无 frame-level 重排；多线程只允许 slice，绝不使用 frame 线程。 */
    d->ctx->thread_type = FF_THREAD_SLICE;
    d->ctx->pkt_timebase = (AVRational){1, 1000000};
    d->stats.threads = d->ctx->thread_count;
    *out = d;
    return 0;
}

/* 显式优先 AV_PIX_FMT_D3D11。FFmpeg 默认 get_format 只取候选列表首项，
 * 精简构建下不保证硬件格式排在前面，因此绑定 D3D11 时由我们自己选。 */
static enum AVPixelFormat rdff_get_format(AVCodecContext *ctx, const enum AVPixelFormat *fmt) {
    (void)ctx;
    static int logged = 0;
    if (!logged) {
        logged = 1;
        fprintf(stderr, "[ffdec] get_format choices:");
        for (const enum AVPixelFormat *p = fmt; p && *p != AV_PIX_FMT_NONE; ++p) {
            fprintf(stderr, " %s", av_get_pix_fmt_name(*p));
        }
        fprintf(stderr, "\n");
    }
    for (const enum AVPixelFormat *p = fmt; p && *p != AV_PIX_FMT_NONE; ++p) {
        if (*p == AV_PIX_FMT_D3D11) {
            return *p;
        }
    }
    return fmt ? fmt[0] : AV_PIX_FMT_NONE;
}

/* 把解码器接到调用方已有的 D3D11 设备（硬解 d3d11va，输出 GPU 纹理，零回读）。
 * device 指针的所有权仍归调用方，FFmpeg 只在其生命周期内使用。 */
int rdff_bind_d3d11(RdcFfDec *d, void *device, char *err, int errlen) {
    if (!d || !device) {
        set_err(err, errlen, "D3D11 设备为空");
        return AVERROR(EINVAL);
    }
    AVBufferRef *ref = av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA);
    if (!ref) {
        set_err(err, errlen, "av_hwdevice_ctx_alloc 失败");
        return AVERROR(ENOMEM);
    }
    AVHWDeviceContext *hw = (AVHWDeviceContext *)ref->data;
    AVD3D11VADeviceContext *d3d = (AVD3D11VADeviceContext *)hw->hwctx;
    d3d->device = (ID3D11Device *)device;
    /* device_context 留空由 FFmpeg 自己取立即上下文；呈现窗口已开多线程保护。 */
    int ret = av_hwdevice_ctx_init(ref);
    if (ret < 0) {
        av_buffer_unref(&ref);
        set_av_err(err, errlen, "av_hwdevice_ctx_init", ret);
        return ret;
    }
    d->hw_ctx = ref;
    d->ctx->hw_device_ctx = av_buffer_ref(ref);
    if (!d->ctx->hw_device_ctx) {
        set_err(err, errlen, "hw_device_ctx 引用失败");
        return AVERROR(ENOMEM);
    }
    /* 输出表面池留出余量：呈现侧短暂持有纹理引用，池太浅会让解码器等表面回收。 */
    d->ctx->extra_hw_frames = 8;
    d->ctx->get_format = rdff_get_format;
    return 0;
}

/* 打开解码器并进入可送包状态。 */
int rdff_open(RdcFfDec *d, char *err, int errlen) {
    if (!d) {
        set_err(err, errlen, "解码器为空");
        return AVERROR(EINVAL);
    }
    int ret = avcodec_open2(d->ctx, d->ctx->codec, NULL);
    if (ret < 0) {
        set_av_err(err, errlen, "avcodec_open2", ret);
        return ret;
    }
    d->stats.delay_frames = d->ctx->delay + d->ctx->has_b_frames;
    return 0;
}

/* 送一个 Annex-B 访问单元并立刻收走所有已就绪帧。 */
int rdff_feed(RdcFfDec *d, const unsigned char *data, int32_t len, int64_t pts_us,
              RdcFfFrameCb cb, void *cbctx, char *err, int errlen) {
    if (!d || !data || len <= 0) {
        set_err(err, errlen, "码流为空");
        return AVERROR(EINVAL);
    }
    int64_t t0 = av_gettime_relative();
    int ret = drain_frames(d, cb, cbctx, err, errlen);
    if (ret < 0) {
        return ret;
    }
    av_packet_unref(d->pkt);
    d->pkt->data = (uint8_t *)data;
    d->pkt->size = len;
    d->pkt->pts = pts_us;
    d->pkt->dts = pts_us;
    ret = avcodec_send_packet(d->ctx, d->pkt);
    /* 包数据归调用方所有，收完立刻断开，避免留下悬垂指针。 */
    d->pkt->data = NULL;
    d->pkt->size = 0;
    if (ret < 0) {
        fprintf(stderr, "[ffdec] send failed rc=%d pix_fmt=%s hw_frames=%p\n", ret,
                av_get_pix_fmt_name(d->ctx->pix_fmt), (void *)d->ctx->hw_frames_ctx);
        set_av_err(err, errlen, "avcodec_send_packet", ret);
        return ret;
    }
    d->stats.feeds += 1;
    double decode_ms = (double)(av_gettime_relative() - t0) / 1000.0;
    d->stats.decode_ms_total += decode_ms;
    return drain_frames(d, cb, cbctx, err, errlen);
}

/* 只收帧（冲刷/EOS 或补收）。 */
int rdff_drain(RdcFfDec *d, RdcFfFrameCb cb, void *cbctx, char *err, int errlen) {
    if (!d) {
        set_err(err, errlen, "解码器为空");
        return AVERROR(EINVAL);
    }
    return drain_frames(d, cb, cbctx, err, errlen);
}

/* 冲刷解码器（EOS + 取尽剩余帧）。 */
int rdff_flush(RdcFfDec *d, RdcFfFrameCb cb, void *cbctx, char *err, int errlen) {
    if (!d) {
        set_err(err, errlen, "解码器为空");
        return AVERROR(EINVAL);
    }
    int ret = avcodec_send_packet(d->ctx, NULL);
    if (ret < 0 && ret != AVERROR_EOF) {
        set_av_err(err, errlen, "flush send_packet", ret);
        return ret;
    }
    return drain_frames(d, cb, cbctx, err, errlen);
}

/* 取累计统计。 */
void rdff_stats(RdcFfDec *d, RdcFfStats *out) {
    if (d && out) {
        *out = d->stats;
    }
}

/* 释放。 */
void rdff_destroy(RdcFfDec *d) {
    if (!d) {
        return;
    }
    if (d->ctx) {
        avcodec_free_context(&d->ctx);
    }
    if (d->pkt) {
        av_packet_free(&d->pkt);
    }
    if (d->frame) {
        av_frame_free(&d->frame);
    }
    av_buffer_unref(&d->hw_ctx);
    free(d->pack);
    free(d);
}
