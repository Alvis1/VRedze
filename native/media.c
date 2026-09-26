#include "media.h"
#include <stdio.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/error.h>
#include <libavutil/hwcontext.h>
#include <libavutil/pixdesc.h>
#include <libavutil/spherical.h>
#include <libavutil/stereo3d.h>
#include <libavutil/time.h>
#include <libavutil/channel_layout.h>
#include <libswresample/swresample.h>

#define IO_BUFFER_SIZE (256 * 1024)

struct JVMedia {
    AVFormatContext *format;
    AVIOContext *io;
    int video_stream;
    int audio_stream;
};

typedef struct {
    enum AVPixelFormat hw_format;
    int allow_software;
    // Hardware decoder that returns ordinary frames (V4L2 mem2mem wrappers).
    int hardware_wrapper;
} FormatChoice;

static void set_error(char *out, int size, const char *operation, int code) {
    char message[AV_ERROR_MAX_STRING_SIZE];
    av_strerror(code, message, sizeof(message));
    snprintf(out, size, "%s: %s", operation, message);
}

static void copy_name(char *out, size_t size, const char *value) {
    snprintf(out, size, "%s", value ? value : "");
}

typedef struct {
    jv_read_fn read;
    jv_seek_fn seek;
    void *opaque;
} Callbacks;

static int io_read(void *opaque, uint8_t *buf, int size) {
    const Callbacks *cb = opaque;
    int n = cb->read(cb->opaque, buf, size);
    return n == 0 ? AVERROR_EOF : n;
}

static int64_t io_seek(void *opaque, int64_t offset, int whence) {
    const Callbacks *cb = opaque;
    return cb->seek(cb->opaque, offset, whence & ~AVSEEK_FORCE);
}

static void describe_vr(const AVCodecParameters *par, JVMediaInfo *info) {
    const AVPacketSideData *sd = av_packet_side_data_get(
        par->coded_side_data, par->nb_coded_side_data, AV_PKT_DATA_STEREO3D);
    if (sd) {
        const AVStereo3D *stereo = (const AVStereo3D *)sd->data;
        copy_name(info->stereo_mode, sizeof(info->stereo_mode), av_stereo3d_type_name(stereo->type));
        info->stereo_inverted = !!(stereo->flags & AV_STEREO3D_FLAG_INVERT);
    }
    sd = av_packet_side_data_get(par->coded_side_data, par->nb_coded_side_data, AV_PKT_DATA_SPHERICAL);
    if (sd) {
        const AVSphericalMapping *map = (const AVSphericalMapping *)sd->data;
        copy_name(info->projection, sizeof(info->projection), av_spherical_projection_name(map->projection));
        info->bound_left = map->bound_left;
        info->bound_top = map->bound_top;
        info->bound_right = map->bound_right;
        info->bound_bottom = map->bound_bottom;
    }
}

JVMedia *jv_media_open(const char *name, jv_read_fn read, jv_seek_fn seek, void *opaque,
                       JVMediaInfo *info, char *error, int error_size) {
    memset(info, 0, sizeof(*info));
    av_log_set_level(AV_LOG_ERROR);
    JVMedia *media = av_mallocz(sizeof(*media));
    Callbacks *cb = av_malloc(sizeof(*cb));
    uint8_t *buffer = av_malloc(IO_BUFFER_SIZE);
    int ret = AVERROR(ENOMEM);
    if (!media || !cb || !buffer) goto fail;
    *cb = (Callbacks){ read, seek, opaque };
    media->io = avio_alloc_context(buffer, IO_BUFFER_SIZE, 0, cb, io_read, NULL, io_seek);
    if (!media->io) goto fail;
    buffer = NULL;
    cb = NULL;
    media->format = avformat_alloc_context();
    if (!media->format) goto fail;
    media->format->pb = media->io;
    media->format->flags |= AVFMT_FLAG_CUSTOM_IO;
    AVDictionary *options = NULL;
    // Nested references (playlists, external tracks) must not reach FFmpeg's own protocols.
    av_dict_set(&options, "protocol_whitelist", "", 0);
    ret = avformat_open_input(&media->format, name, NULL, &options);
    av_dict_free(&options);
    if (ret < 0) { set_error(error, error_size, "Open media", ret); goto fail; }
    ret = avformat_find_stream_info(media->format, NULL);
    if (ret < 0) { set_error(error, error_size, "Read stream information", ret); goto fail; }

    AVFormatContext *fmt = media->format;
    copy_name(info->container, sizeof(info->container), fmt->iformat->name);
    info->duration_seconds = fmt->duration > 0 ? fmt->duration / (double)AV_TIME_BASE : 0;
    info->bit_rate = fmt->bit_rate;
    media->video_stream = av_find_best_stream(fmt, AVMEDIA_TYPE_VIDEO, -1, -1, NULL, 0);
    if (media->video_stream >= 0) {
        AVStream *video = fmt->streams[media->video_stream];
        const AVCodecParameters *par = video->codecpar;
        copy_name(info->video_codec, sizeof(info->video_codec), avcodec_get_name(par->codec_id));
        copy_name(info->video_profile, sizeof(info->video_profile), avcodec_profile_name(par->codec_id, par->profile));
        copy_name(info->pixel_format, sizeof(info->pixel_format), av_get_pix_fmt_name(par->format));
        info->width = par->width;
        info->height = par->height;
        const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(par->format);
        info->bit_depth = desc ? desc->comp[0].depth : 0;
        AVRational rate = av_guess_frame_rate(fmt, video, NULL);
        info->fps = rate.den ? av_q2d(rate) : 0;
        describe_vr(par, info);
    }
    int audio = av_find_best_stream(fmt, AVMEDIA_TYPE_AUDIO, -1, media->video_stream, NULL, 0);
    media->audio_stream = audio;
    // Demux only what we play: skip subtitles, extra audio tracks, attachments.
    for (unsigned i = 0; i < fmt->nb_streams; i++)
        if ((int)i != media->video_stream && (int)i != audio) fmt->streams[i]->discard = AVDISCARD_ALL;
    if (audio >= 0) {
        const AVCodecParameters *par = fmt->streams[audio]->codecpar;
        copy_name(info->audio_codec, sizeof(info->audio_codec), avcodec_get_name(par->codec_id));
        info->audio_channels = par->ch_layout.nb_channels;
        info->audio_sample_rate = par->sample_rate;
    }
    return media;
fail:
    if (!error[0]) set_error(error, error_size, "Allocate media", ret);
    av_free(buffer);
    av_free(cb);
    jv_media_close(media);
    return NULL;
}

static enum AVPixelFormat choose_format(AVCodecContext *ctx, const enum AVPixelFormat *formats) {
    const FormatChoice *choice = ctx->opaque;
    enum AVPixelFormat software = AV_PIX_FMT_NONE;
    for (const enum AVPixelFormat *p = formats; *p != AV_PIX_FMT_NONE; ++p) {
        if (*p == choice->hw_format) return *p;
        const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(*p);
        if (software == AV_PIX_FMT_NONE && desc && !(desc->flags & AV_PIX_FMT_FLAG_HWACCEL))
            software = *p;
    }
    return choice->allow_software ? software : AV_PIX_FMT_NONE;
}

// Attaches a hardware device when the backend offers one for this decoder.
static int attach_hardware(AVCodecContext *ctx, const AVCodec *codec, const char *backend,
                           FormatChoice *choice, AVBufferRef **device) {
    enum AVHWDeviceType type = av_hwdevice_find_type_by_name(backend);
    if (type == AV_HWDEVICE_TYPE_NONE) return AVERROR(ENOSYS);
    for (int i = 0; ; ++i) {
        const AVCodecHWConfig *config = avcodec_get_hw_config(codec, i);
        if (!config) return AVERROR(ENOSYS);
        if (config->device_type == type && (config->methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX)) {
            choice->hw_format = config->pix_fmt;
            break;
        }
    }
    int ret = av_hwdevice_ctx_create(device, type, NULL, NULL, 0);
    if (ret < 0) return ret;
    ctx->hw_device_ctx = av_buffer_ref(*device);
    return ctx->hw_device_ctx ? 0 : AVERROR(ENOMEM);
}

// Why the V4L2 stateful decoder (Qualcomm iris on Steam Frame) must not get
// this stream, or NULL if it may. Allow-list: only streams positively known to
// be 8-bit 4:2:0 within the advertised size. On SteamOS 0.3 / kernel 6.18,
// 10-bit HEVC crashes the iris firmware, so anything uncertain goes to software.
static const char *v4l2_unsuitable(const AVCodecParameters *par) {
    if (par->codec_id != AV_CODEC_ID_H264 && par->codec_id != AV_CODEC_ID_HEVC &&
        par->codec_id != AV_CODEC_ID_VP9)
        return "hardware decoder supports H.264/HEVC/VP9 only";
    if (par->width <= 0 || par->height <= 0 || par->width > 8192 || par->height > 8192)
        return "frame size outside the hardware decoder's 8192x8192 limit";
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(par->format);
    if (desc) {
        if (desc->comp[0].depth != 8)
            return "hardware decoder is 8-bit only (10-bit crashes its firmware)";
        if (desc->log2_chroma_w != 1 || desc->log2_chroma_h != 1 || desc->nb_components < 3)
            return "hardware decoder supports 4:2:0 chroma only";
        return NULL;
    }
    // No pixel format in the headers: trust only 8-bit 4:2:0 profiles.
    int profile = par->profile;
    int eight_bit =
        (par->codec_id == AV_CODEC_ID_H264 &&
         ((profile & 0xff) == AV_PROFILE_H264_BASELINE || profile == AV_PROFILE_H264_MAIN ||
          profile == AV_PROFILE_H264_EXTENDED || profile == AV_PROFILE_H264_HIGH)) ||
        (par->codec_id == AV_CODEC_ID_HEVC &&
         (profile == AV_PROFILE_HEVC_MAIN || profile == AV_PROFILE_HEVC_MAIN_STILL_PICTURE)) ||
        (par->codec_id == AV_CODEC_ID_VP9 && profile == AV_PROFILE_VP9_0);
    return eight_bit ? NULL : "cannot confirm the stream is 8-bit, so the hardware decoder is not used";
}

struct JVDecoder {
    JVMedia *media;
    AVCodecContext *ctx;
    AVBufferRef *device;
    AVPacket *packet;
    AVFrame *transfer;  // scratch for hwaccel -> CPU transfers
    FormatChoice choice;
    int flushing;
    // Audio (optional): decoded and resampled to interleaved float.
    AVCodecContext *audio;
    SwrContext *swr;
    AVFrame *audio_frame;
    int out_rate, out_channels;
    float *samples;      // interleaved, `sample_count` frames buffered
    int sample_count, sample_capacity;
    double samples_pts;  // time of samples[0], seconds from video start; < 0 unknown
};

static double video_start_seconds(const JVMedia *media) {
    const AVStream *video = media->format->streams[media->video_stream];
    return video->start_time != AV_NOPTS_VALUE ? video->start_time * av_q2d(video->time_base) : 0;
}

void jv_decoder_close(JVDecoder *d) {
    if (!d) return;
    avcodec_free_context(&d->audio);
    swr_free(&d->swr);
    av_frame_free(&d->audio_frame);
    av_free(d->samples);
    avcodec_free_context(&d->ctx);
    av_buffer_unref(&d->device);
    av_packet_free(&d->packet);
    av_frame_free(&d->transfer);
    av_free(d);
}

JVDecoder *jv_decoder_open(JVMedia *media, const char *hw_backend, int allow_software,
                           const char *decoder_options, JVDecodeStats *s) {
    memset(s, 0, sizeof(*s));
    JVDecoder *d = av_mallocz(sizeof(*d));
    AVDictionary *open_options = NULL;
    int ret = AVERROR(ENOMEM);
    if (!d) goto fail;
    d->media = media;
    d->packet = av_packet_alloc();
    d->transfer = av_frame_alloc();
    if (!d->packet || !d->transfer) goto fail;
    d->choice = (FormatChoice){ AV_PIX_FMT_NONE, allow_software, 0 };
    if (media->video_stream < 0) { ret = AVERROR_STREAM_NOT_FOUND; set_error(s->error, sizeof(s->error), "Find video stream", ret); goto fail; }
    AVStream *video = media->format->streams[media->video_stream];
    enum AVCodecID id = video->codecpar->codec_id;
    // Hardware decoding needs FFmpeg's native decoders (e.g. "av1", not libdav1d);
    // software decoding takes FFmpeg's preferred (fastest) implementation.
    const AVCodec *codec = NULL;
    if (hw_backend) codec = avcodec_find_decoder_by_name(avcodec_get_name(id));
    if (!codec) codec = avcodec_find_decoder(id);
    if (!codec) { ret = AVERROR_DECODER_NOT_FOUND; set_error(s->error, sizeof(s->error), "Find decoder", ret); goto fail; }
    if (hw_backend && !strcmp(hw_backend, "v4l2m2m")) {
        const char *why = v4l2_unsuitable(video->codecpar);
        const AVCodec *wrapper = NULL;
        if (!why) {
            char name[48];
            snprintf(name, sizeof(name), "%s_v4l2m2m", avcodec_get_name(id));
            wrapper = avcodec_find_decoder_by_name(name);
            if (!wrapper) why = "FFmpeg build lacks the V4L2 decoder for this codec";
        }
        hw_backend = NULL;  // no hwdevice context: the wrapper talks to /dev/video* itself
        if (wrapper) {
            codec = wrapper;
            d->choice.hardware_wrapper = 1;
            copy_name(s->hw_backend, sizeof(s->hw_backend), "v4l2m2m");
            // FFmpeg's default of 20 capture buffers fails to allocate at 8K.
            int64_t pixels = (int64_t)video->codecpar->width * video->codecpar->height;
            av_dict_set_int(&open_options, "num_capture_buffers", pixels > 4096 * 2304 ? 6 : 12, 0);
            av_dict_set_int(&open_options, "num_output_buffers", 16, 0);
        } else if (!allow_software) {
            ret = AVERROR(ENOSYS);
            snprintf(s->error, sizeof(s->error), "%s", why);
            goto fail;
        } else {
            copy_name(s->note, sizeof(s->note), why);
            codec = avcodec_find_decoder(id);
        }
    }
reopen:
    for (int attempt = 0; attempt < 2; ++attempt) {
        avcodec_free_context(&d->ctx);
        av_buffer_unref(&d->device);
        d->choice.hw_format = AV_PIX_FMT_NONE;
        d->ctx = avcodec_alloc_context3(codec);
        if (!d->ctx) { ret = AVERROR(ENOMEM); goto fail; }
        ret = avcodec_parameters_to_context(d->ctx, video->codecpar);
        if (ret < 0) { set_error(s->error, sizeof(s->error), "Copy codec parameters", ret); goto fail; }
        d->ctx->pkt_timebase = video->time_base;
        d->ctx->opaque = &d->choice;
        d->ctx->get_format = choose_format;
        d->ctx->thread_count = 0;
        if (attempt == 0 && hw_backend) {
            ret = attach_hardware(d->ctx, codec, hw_backend, &d->choice, &d->device);
            if (ret < 0) {
                if (!allow_software) { set_error(s->error, sizeof(s->error), "Create hardware decoder", ret); goto fail; }
                set_error(s->note, sizeof(s->note), "Hardware decoder unavailable", ret);
                codec = avcodec_find_decoder(id);  // prefer e.g. libdav1d for software AV1
                continue;
            }
            copy_name(s->hw_backend, sizeof(s->hw_backend), hw_backend);
        }
        break;
    }
    copy_name(s->decoder, sizeof(s->decoder), codec->name);
    if (decoder_options && *decoder_options) {
        ret = av_dict_parse_string(&open_options, decoder_options, "=", ":", 0);
        if (ret < 0) { set_error(s->error, sizeof(s->error), "Parse decoder options", ret); goto fail; }
    }
    ret = avcodec_open2(d->ctx, codec, &open_options);
    if (ret >= 0 && av_dict_count(open_options)) {
        const AVDictionaryEntry *unused = av_dict_iterate(open_options, NULL);
        snprintf(s->error, sizeof(s->error), "Unknown decoder option: %s", unused->key);
        ret = AVERROR_OPTION_NOT_FOUND;
        goto fail;
    }
    if (ret < 0 && d->choice.hardware_wrapper && allow_software) {
        // Device missing, busy or its firmware recovering: never fail playback
        // over it when the CPU can decode instead.
        set_error(s->note, sizeof(s->note), "Hardware decoder failed to open", ret);
        d->choice.hardware_wrapper = 0;
        s->hw_backend[0] = '\0';
        av_dict_free(&open_options);
        codec = avcodec_find_decoder(id);
        goto reopen;
    }
    if (ret < 0) { set_error(s->error, sizeof(s->error), "Open decoder", ret); goto fail; }
    av_dict_free(&open_options);
    return d;
fail:
    if (!s->error[0]) set_error(s->error, sizeof(s->error), "Open decoder", ret);
    av_dict_free(&open_options);
    jv_decoder_close(d);
    return NULL;
}

static int describe_frame(JVDecoder *d, AVFrame *frame, JVFrame *out) {
    AVStream *video = d->media->format->streams[d->media->video_stream];
    memset(out, 0, sizeof(*out));
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(frame->format);
    out->hardware = d->choice.hardware_wrapper ||
                    (desc && (desc->flags & AV_PIX_FMT_FLAG_HWACCEL) && frame->hw_frames_ctx);
    if (desc && (desc->flags & AV_PIX_FMT_FLAG_HWACCEL)) {
        // GPU surface (desktop Vulkan video): bring it to CPU memory for upload.
        av_frame_unref(d->transfer);
        int ret = av_hwframe_transfer_data(d->transfer, frame, 0);
        if (ret < 0) return ret;
        ret = av_frame_copy_props(d->transfer, frame);
        if (ret < 0) return ret;
        av_frame_unref(frame);
        av_frame_move_ref(frame, d->transfer);
    }
    switch (frame->format) {
    case AV_PIX_FMT_YUV420P:
    case AV_PIX_FMT_YUVJ420P:     out->layout = JV_LAYOUT_PLANAR; out->bits = 8; break;
    case AV_PIX_FMT_YUV420P10LE:  out->layout = JV_LAYOUT_PLANAR; out->bits = 10; break;
    case AV_PIX_FMT_NV12:         out->layout = JV_LAYOUT_SEMIPLANAR; out->bits = 8; break;
    case AV_PIX_FMT_P010LE:       out->layout = JV_LAYOUT_SEMIPLANAR_MSB; out->bits = 10; break;
    default: return AVERROR_PATCHWELCOME;
    }
    out->handle = frame;
    out->width = frame->width;
    out->height = frame->height;
    out->plane_count = out->layout == JV_LAYOUT_PLANAR ? 3 : 2;
    for (int i = 0; i < out->plane_count; i++) {
        out->data[i] = frame->data[i];
        out->linesize[i] = frame->linesize[i];
    }
    int64_t ts = frame->best_effort_timestamp;
    int64_t start = video->start_time != AV_NOPTS_VALUE ? video->start_time : 0;
    out->pts = ts == AV_NOPTS_VALUE ? -1 : (ts - start) * av_q2d(video->time_base);
    switch (frame->colorspace) {
    case AVCOL_SPC_BT709:      out->matrix = JV_MATRIX_BT709; break;
    case AVCOL_SPC_BT470BG:
    case AVCOL_SPC_SMPTE170M:  out->matrix = JV_MATRIX_BT601; break;
    case AVCOL_SPC_BT2020_NCL:
    case AVCOL_SPC_BT2020_CL:  out->matrix = JV_MATRIX_BT2020; break;
    default: out->matrix = frame->height >= 720 ? JV_MATRIX_BT709 : JV_MATRIX_BT601; break;
    }
    out->full_range = frame->color_range == AVCOL_RANGE_JPEG || frame->format == AV_PIX_FMT_YUVJ420P;
    out->transfer = frame->color_trc == AVCOL_TRC_SMPTE2084 ? JV_TRANSFER_PQ
                  : frame->color_trc == AVCOL_TRC_ARIB_STD_B67 ? JV_TRANSFER_HLG
                  : JV_TRANSFER_SDR;
    return 0;
}

int jv_decoder_enable_audio(JVDecoder *d, int rate, int channels) {
    JVMedia *m = d->media;
    if (m->audio_stream < 0) return AVERROR_STREAM_NOT_FOUND;
    const AVCodecParameters *par = m->format->streams[m->audio_stream]->codecpar;
    const AVCodec *codec = avcodec_find_decoder(par->codec_id);
    if (!codec) return AVERROR_DECODER_NOT_FOUND;
    d->audio = avcodec_alloc_context3(codec);
    d->audio_frame = av_frame_alloc();
    if (!d->audio || !d->audio_frame) return AVERROR(ENOMEM);
    int ret = avcodec_parameters_to_context(d->audio, par);
    if (ret < 0) return ret;
    d->audio->pkt_timebase = m->format->streams[m->audio_stream]->time_base;
    ret = avcodec_open2(d->audio, codec, NULL);
    if (ret < 0) return ret;
    AVChannelLayout out_layout;
    av_channel_layout_default(&out_layout, channels);
    ret = swr_alloc_set_opts2(&d->swr, &out_layout, AV_SAMPLE_FMT_FLT, rate,
                              &d->audio->ch_layout, d->audio->sample_fmt, d->audio->sample_rate, 0, NULL);
    av_channel_layout_uninit(&out_layout);
    if (ret < 0) return ret;
    ret = swr_init(d->swr);
    if (ret < 0) return ret;
    d->out_rate = rate;
    d->out_channels = channels;
    d->samples_pts = -1;
    return 0;
}

static void decode_audio_packet(JVDecoder *d, const AVPacket *packet) {
    if (avcodec_send_packet(d->audio, packet) < 0) return;  // damaged audio: skip it
    AVStream *stream = d->media->format->streams[d->media->audio_stream];
    while (avcodec_receive_frame(d->audio, d->audio_frame) == 0) {
        AVFrame *f = d->audio_frame;
        int max_out = swr_get_out_samples(d->swr, f->nb_samples);
        if (max_out > 0 && d->sample_count + max_out > d->sample_capacity) {
            int capacity = (d->sample_count + max_out) * 2;
            float *grown = av_realloc_array(d->samples, (size_t)capacity * d->out_channels, sizeof(float));
            if (!grown) { av_frame_unref(f); return; }
            d->samples = grown;
            d->sample_capacity = capacity;
        }
        if (d->sample_count == 0 && f->best_effort_timestamp != AV_NOPTS_VALUE)
            d->samples_pts = f->best_effort_timestamp * av_q2d(stream->time_base) - video_start_seconds(d->media)
                             - (double)swr_get_delay(d->swr, d->out_rate) / d->out_rate;
        uint8_t *dst = (uint8_t *)(d->samples + (size_t)d->sample_count * d->out_channels);
        int n = swr_convert(d->swr, &dst, max_out, (const uint8_t **)f->extended_data, f->nb_samples);
        if (n > 0) d->sample_count += n;
        av_frame_unref(f);
    }
}

int jv_decoder_audio_available(const JVDecoder *d) {
    return d->audio ? d->sample_count : 0;
}

int jv_decoder_audio_read(JVDecoder *d, float *out, int frames, double *pts) {
    if (!d->audio) return 0;
    int n = frames < d->sample_count ? frames : d->sample_count;
    if (n <= 0) return 0;
    *pts = d->samples_pts;
    memcpy(out, d->samples, (size_t)n * d->out_channels * sizeof(float));
    memmove(d->samples, d->samples + (size_t)n * d->out_channels,
            (size_t)(d->sample_count - n) * d->out_channels * sizeof(float));
    d->sample_count -= n;
    if (d->samples_pts >= 0) d->samples_pts += (double)n / d->out_rate;
    return n;
}

int jv_decoder_next(JVDecoder *d, JVFrame *out) {
    AVFrame *frame = av_frame_alloc();
    if (!frame) return AVERROR(ENOMEM);
    for (;;) {
        int ret = avcodec_receive_frame(d->ctx, frame);
        if (ret == 0) {
            ret = describe_frame(d, frame, out);
            if (ret < 0) av_frame_free(&frame);
            return ret;
        }
        if (ret != AVERROR(EAGAIN)) { av_frame_free(&frame); return ret; }  // EOF or error
        ret = av_read_frame(d->media->format, d->packet);
        if (ret == AVERROR_EOF && !d->flushing) {
            d->flushing = 1;
            avcodec_send_packet(d->ctx, NULL);
            continue;
        }
        if (ret < 0) { av_frame_free(&frame); return ret; }
        if (d->audio && d->packet->stream_index == d->media->audio_stream) {
            decode_audio_packet(d, d->packet);
            av_packet_unref(d->packet);
            continue;
        }
        if (d->packet->stream_index != d->media->video_stream) { av_packet_unref(d->packet); continue; }
        ret = avcodec_send_packet(d->ctx, d->packet);
        av_packet_unref(d->packet);
        // A damaged packet costs a glitch, not the whole playback.
        if (ret < 0 && ret != AVERROR_INVALIDDATA) { av_frame_free(&frame); return ret; }
    }
}

void jv_frame_release(void *handle) {
    AVFrame *frame = handle;
    av_frame_free(&frame);
}

int jv_decoder_seek(JVDecoder *d, double seconds) {
    AVStream *video = d->media->format->streams[d->media->video_stream];
    int64_t start = video->start_time != AV_NOPTS_VALUE ? video->start_time : 0;
    int64_t ts = start + (int64_t)(seconds / av_q2d(video->time_base));
    int ret = av_seek_frame(d->media->format, d->media->video_stream, ts, AVSEEK_FLAG_BACKWARD);
    if (ret < 0) return ret;
    avcodec_flush_buffers(d->ctx);
    if (d->audio) {
        avcodec_flush_buffers(d->audio);
        swr_close(d->swr);
        swr_init(d->swr);
        d->sample_count = 0;
        d->samples_pts = -1;
    }
    d->flushing = 0;
    return 0;
}

int jv_media_decode(JVMedia *media, const char *hw_backend, int allow_software,
                    const char *decoder_options, int frame_limit, JVDecodeStats *s) {
    JVDecoder *d = jv_decoder_open(media, hw_backend, allow_software, decoder_options, s);
    if (!d) return AVERROR(EINVAL);
    int ret = jv_decoder_seek(d, 0);
    if (ret < 0) avformat_seek_file(media->format, -1, INT64_MIN, 0, 0, 0);
    int64_t start = av_gettime_relative();
    JVFrame frame;
    while (s->frames < frame_limit) {
        ret = jv_decoder_next(d, &frame);
        if (ret == AVERROR_EOF) { ret = 0; break; }
        if (ret == AVERROR_PATCHWELCOME) { snprintf(s->error, sizeof(s->error), "Unsupported decoded pixel format"); break; }
        if (ret < 0) { set_error(s->error, sizeof(s->error), "Decode", ret); break; }
        if (frame.hardware) ++s->hardware_frames; else ++s->software_frames;
        const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(((AVFrame *)frame.handle)->format);
        copy_name(s->pixel_format, sizeof(s->pixel_format), desc ? desc->name : NULL);
        ++s->frames;
        jv_frame_release(frame.handle);
    }
    s->elapsed_seconds = (av_gettime_relative() - start) / 1000000.0;
    jv_decoder_close(d);
    return ret < 0 ? ret : 0;
}

void jv_media_close(JVMedia *media) {
    if (!media) return;
    avformat_close_input(&media->format);
    if (media->io) {
        av_freep(&media->io->opaque);
        av_freep(&media->io->buffer);
        avio_context_free(&media->io);
    }
    av_free(media);
}
