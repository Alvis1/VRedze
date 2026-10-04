#ifndef VREDZE_DECODE_H
#define VREDZE_DECODE_H
#include <stdint.h>
typedef struct {
    int32_t frames;
    int32_t hardware_frames;
    int32_t readback_frames;
    int32_t width;
    int32_t height;
    double elapsed_seconds;
    double stream_fps;
    char codec[32];
    char pixel_format[32];
    char error[256];
} JVDecodeResult;

int jv_decode(const char *path, const char *backend, const char *device,
              int frame_limit, int readback, JVDecodeResult *result);
#endif

