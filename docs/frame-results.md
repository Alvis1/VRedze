# Steam Frame results — 2026-09-26

Device: SteamOS 0.3.0 (`vr`, build 20260922), kernel 6.18, SM8650 (Snapdragon 8 Gen 3),
Adreno 750 / Mesa Turnip 26.3-devel, system FFmpeg 7.0, glibc 2.39. Full inventory:
`reports/frame-inventory.txt` (ignored).

## Decoders

| Path | Result |
|---|---|
| Vulkan Video | **Unavailable**: Turnip exposes no `VK_KHR_video_decode_*`. |
| V4L2 `iris_decoder` (`/dev/video-dec0`) | H.264, HEVC, VP9 only (no AV1); capture NV12 8-bit only; frame sizes up to 8192×8192. |
| H.264 / HEVC 8-bit via `*_v4l2m2m` | Works, including 8K, if capture buffers are ≤ 8 (FFmpeg's default 20 → ENOMEM at 8K). |
| HEVC 10-bit via V4L2 | **Crashes iris firmware** (kernel WARN in `vidc_streamon`, firmware reload); the player never sends 10-bit to it. |
| Software (FFmpeg 7.0 CPU) | AV1 10-bit: 4K ~88 fps, 8K ~35 fps (dav1d). HEVC 10-bit: 4K 142 fps, 8K 42 fps (via just-video over SMB). |

## Streaming from SMB with the ARM64 `just-video` (cross-built, no files copied)

| Source (over SMB, direct link) | Decoder | Speed |
|---|---|---|
| 1080p H.264 | h264_v4l2m2m (hw) | 960 fps, 40× real time |
| 1280×640 HEVC 8-bit | hevc_v4l2m2m (hw) | 1688 fps |
| 4K60 HEVC 10-bit | software | 142 fps, 2.4× real time |
| 8K60 HEVC 10-bit VR180 | software | 42 fps, **0.70× real time: not playable at 60 fps** |

## Network

The headset has two Wi-Fi interfaces: `wlan0` (home network, here 2.4 GHz / 40 MHz) and
`wlanap`, a 6 GHz / 160 MHz link to the PC's Frame USB adapter (PC side `10.35.78.21`).
SMB read throughput over `wlanap`: 1.24 Gb/s with 32 × 1 MiB in flight, 377 Mb/s with 8.
Over `wlan0` reads stalled until the 10 s SMB timeout. Serve media over the direct link.

## Bundled, tuned software decoding (later on 2026-09-26)

The Frame build now statically links its own media stack (`scripts/build-frame-media.sh`):
FFmpeg 8.1.3 (minimal, `-O3`, `-mcpu=cortex-a720`), dav1d 1.5.4 and zlib 1.3.1, plus
`third_party/ffmpeg-patches/0001` (10-bit HEVC NEON motion compensation). The binary
only needs `libc`/`libm` from SteamOS.

| 8K60 HEVC Main10 over SMB | fps |
|---|---|
| SteamOS FFmpeg 7.0 | 42 |
| Bundled FFmpeg 8.1.3 | 46.5 |
| + 10-bit pel NEON patch | 47–48 (0.8× real time) |

Thread count (6–16), frame vs. frame+slice threading, big-core pinning, `skip_loop_filter`
(up to `all`), `flags2=fast` and THP via `GLIBC_TUNABLES=glibc.malloc.hugetlb=1` each
changed the result by less than ±4%. `perf stat` shows 77–82% backend-stalled cycles and
~3.3 G LLC misses per 15 s. At 8K 10-bit, motion compensation is **memory-latency
bound** (~100 MB reference frames). Faster arithmetic does not help. The CPU runs
at ~85 °C during this load.

The NEON patch pays off when data fits in cache. 4K60 HEVC Main10 went from 142 to
**178 fps (3.0× real time)**. Hardware paths are unchanged: H.264 1080p runs at 37× and
HEVC 8-bit at 75× real time.

Conclusion: 8K60 10-bit HEVC is not playable on the Frame CPU. It needs the iris
hardware decoder to gain 10-bit support (SteamOS/kernel update). Every other format we
tested plays.
