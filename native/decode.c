#include "decode.h"
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/error.h>
#include <libavutil/hwcontext.h>
#include <libavutil/pixdesc.h>
#include <libavutil/time.h>

typedef struct {
    enum AVPixelFormat required_format;
    int dedicated_v4l2;
    int readback;
    int limit;
    JVDecodeResult *result;
} DecodeState;

static int fail(JVDecodeResult *r, const char *operation, int code) {
    char message[AV_ERROR_MAX_STRING_SIZE];
    av_strerror(code, message, sizeof(message));
    snprintf(r->error, sizeof(r->error), "%s: %s", operation, message);
    return code;
}

static enum AVPixelFormat hardware_only(AVCodecContext *ctx, const enum AVPixelFormat *formats) {
    const DecodeState *state = ctx->opaque;
    for (const enum AVPixelFormat *p = formats; *p != AV_PIX_FMT_NONE; ++p)
        if (*p == state->required_format) return *p;
    // Returning NONE explicitly prohibits FFmpeg's normal software fallback.
    return AV_PIX_FMT_NONE;
}

static int receive_frames(AVCodecContext *ctx, AVFrame *frame, AVFrame *download) {
    DecodeState *state = ctx->opaque;
    JVDecodeResult *r = state->result;
    while (r->frames < state->limit) {
        int ret = avcodec_receive_frame(ctx, frame);
        if (ret < 0) return ret;
        const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(frame->format);
        int hardware = desc && (desc->flags & AV_PIX_FMT_FLAG_HWACCEL) && frame->hw_frames_ctx;
        if (!state->dedicated_v4l2 && (!hardware || frame->format != state->required_format)) {
            av_frame_unref(frame);
            return fail(r, "Decoder returned a software surface", AVERROR(ENOSYS));
        }
        if (frame->decode_error_flags || (frame->flags & AV_FRAME_FLAG_CORRUPT)) {
            av_frame_unref(frame);
            return fail(r, "Corrupt decoded frame", AVERROR_INVALIDDATA);
        }
        if (state->readback && hardware) {
            ret = av_hwframe_transfer_data(download, frame, 0);
            if (ret < 0) {
                av_frame_unref(frame);
                return fail(r, "Hardware frame readback", ret);
            }
            ++r->readback_frames;
            av_frame_unref(download);
        }
        r->width = frame->width;
        r->height = frame->height;
        const char *format = av_get_pix_fmt_name(frame->format);
        snprintf(r->pixel_format, sizeof(r->pixel_format), "%s", format ? format : "unknown");
        ++r->frames;
        r->hardware_frames += !!hardware;
        av_frame_unref(frame);
    }
    return 0;
}

int jv_decode(const char *path, const char *backend, const char *device,
              int frame_limit, int readback, JVDecodeResult *r) {
    AVFormatContext *format = NULL;
    AVCodecContext *ctx = NULL;
    AVBufferRef *hw = NULL;
    AVPacket *packet = NULL;
    AVFrame *frame = NULL, *download = NULL;
    AVDictionary *input_options = NULL;
    DecodeState state = { .required_format = AV_PIX_FMT_NONE,
        .dedicated_v4l2 = !strcmp(backend, "v4l2m2m"),
        .readback = readback, .limit = frame_limit, .result = r };
    int ret = 0, stream = -1;
    int64_t start = 0;
    memset(r, 0, sizeof(*r));
    av_log_set_level(AV_LOG_WARNING);
    if (frame_limit < 1) { ret = fail(r, "Invalid frame limit", AVERROR(EINVAL)); goto done; }
    // Probe samples cannot cause network access via a playlist or nested URL.
    av_dict_set(&input_options, "protocol_whitelist", "file", 0);
    ret = avformat_open_input(&format, path, NULL, &input_options);
    av_dict_free(&input_options);
    if (ret < 0) { fail(r, "Open sample", ret); goto done; }
    ret = avformat_find_stream_info(format, NULL);
    if (ret < 0) { fail(r, "Read stream information", ret); goto done; }
    stream = av_find_best_stream(format, AVMEDIA_TYPE_VIDEO, -1, -1, NULL, 0);
    if (stream < 0) { ret = fail(r, "Find video stream", stream); goto done; }
    AVStream *video = format->streams[stream];
    const char *codec_name = avcodec_get_name(video->codecpar->codec_id);
    if (video->codecpar->codec_id != AV_CODEC_ID_H264 &&
        video->codecpar->codec_id != AV_CODEC_ID_HEVC &&
        video->codecpar->codec_id != AV_CODEC_ID_AV1) {
        ret = fail(r, "Only H.264, HEVC and AV1 are in this gate", AVERROR(ENOSYS)); goto done;
    }
    char decoder_name[64];
    snprintf(decoder_name, sizeof(decoder_name), "%s%s", codec_name,
             state.dedicated_v4l2 ? "_v4l2m2m" : "");
    // Select native hwaccel-capable decoder explicitly, not libdav1d or a
    // Vulkan compute software decoder such as *_vulkan.
    const AVCodec *codec = avcodec_find_decoder_by_name(decoder_name);
    if (!codec) { ret = fail(r, "Decoder absent from FFmpeg build", AVERROR_DECODER_NOT_FOUND); goto done; }
    snprintf(r->codec, sizeof(r->codec), "%s", codec_name);
    AVRational rate = av_guess_frame_rate(format, video, NULL);
    r->stream_fps = rate.den ? av_q2d(rate) : 0;
    ctx = avcodec_alloc_context3(codec);
    packet = av_packet_alloc();
    frame = av_frame_alloc();
    download = av_frame_alloc();
    if (!ctx || !packet || !frame || !download) { ret = fail(r, "Allocate decoder", AVERROR(ENOMEM)); goto done; }
    ret = avcodec_parameters_to_context(ctx, video->codecpar);
    if (ret < 0) { fail(r, "Copy codec parameters", ret); goto done; }
    ctx->opaque = &state;
    ctx->pkt_timebase = video->time_base;
    ctx->err_recognition = AV_EF_EXPLODE;
    if (!state.dedicated_v4l2) {
        enum AVHWDeviceType type = av_hwdevice_find_type_by_name(backend);
        if (type != AV_HWDEVICE_TYPE_VULKAN && type != AV_HWDEVICE_TYPE_VAAPI) {
            ret = fail(r, "Unsupported hardware backend", AVERROR(ENOSYS)); goto done;
        }
        for (int i = 0; ; ++i) {
            const AVCodecHWConfig *config = avcodec_get_hw_config(codec, i);
            if (!config) break;
            if (config->device_type == type && (config->methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX)) {
                state.required_format = config->pix_fmt;
                break;
            }
        }
        if (state.required_format == AV_PIX_FMT_NONE) {
            ret = fail(r, "No matching decoder hardware configuration", AVERROR(ENOSYS)); goto done;
        }
        ret = av_hwdevice_ctx_create(&hw, type, device, NULL, 0);
        if (ret < 0) { fail(r, "Create hardware device", ret); goto done; }
        ctx->hw_device_ctx = av_buffer_ref(hw);
        if (!ctx->hw_device_ctx) { ret = fail(r, "Reference hardware device", AVERROR(ENOMEM)); goto done; }
        ctx->get_format = hardware_only;
    }
    ret = avcodec_open2(ctx, codec, NULL);
    if (ret < 0) { fail(r, "Open hardware decoder", ret); goto done; }
    start = av_gettime_relative();
    while (r->frames < frame_limit) {
        ret = av_read_frame(format, packet);
        if (ret == AVERROR_EOF) break;
        if (ret < 0) { fail(r, "Read packet", ret); goto done; }
        if (packet->stream_index != stream) { av_packet_unref(packet); continue; }
        ret = avcodec_send_packet(ctx, packet);
        if (ret == AVERROR(EAGAIN)) {
            ret = receive_frames(ctx, frame, download);
            if (ret < 0 && ret != AVERROR(EAGAIN)) { fail(r, "Drain decoder", ret); goto done; }
            if (r->frames >= frame_limit) { av_packet_unref(packet); break; }
            ret = avcodec_send_packet(ctx, packet);
        }
        av_packet_unref(packet);
        if (ret < 0) { fail(r, "Send packet", ret); goto done; }
        ret = receive_frames(ctx, frame, download);
        if (ret < 0 && ret != AVERROR(EAGAIN)) {
            if (!r->error[0]) fail(r, "Receive hardware frame", ret);
            goto done;
        }
    }
    if (r->frames < frame_limit) {
        ret = avcodec_send_packet(ctx, NULL);
        if (ret < 0) { fail(r, "Flush decoder", ret); goto done; }
        ret = receive_frames(ctx, frame, download);
        if (ret < 0 && ret != AVERROR_EOF) {
            if (!r->error[0]) fail(r, "Receive final frames", ret);
            goto done;
        }
    }
    ret = r->frames == frame_limit ? 0 : fail(r, "Sample ended before requested frame count", AVERROR_EOF);
done:
    if (start) r->elapsed_seconds = (av_gettime_relative() - start) / 1000000.0;
    av_frame_free(&frame);
    av_frame_free(&download);
    av_packet_free(&packet);
    avcodec_free_context(&ctx);
    av_buffer_unref(&hw);
    avformat_close_input(&format);
    return ret;
}
