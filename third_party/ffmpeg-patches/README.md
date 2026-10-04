Patches applied to FFmpeg by `scripts/build-frame-media.sh` (Steam Frame build only).

- `0001-aarch64-hevc-10bit-pel-pixels-neon.patch`: NEON versions of the 10-bit
  HEVC full-sample MC functions (`pel_pixels`, `pel_bi_pixels`, `pel_uni_w_pixels`),
  which FFmpeg 8.1.3 (and master as of 2026-09) only implements in C at 10-bit.
  Verified bit-exact with `checkasm --test=hevc_pel` (376/376) on the headset.
  Steam Frame: 4K60 HEVC Main10 142 → 178 fps; 8K60 unchanged (memory-latency bound).
  Upstream candidate.
- `0002-v4l2m2m-bounded-capture-wait.patch`: `h264/hevc_v4l2m2m` waited for a
  decoded picture with an unbounded `poll()`. On Steam Frame (iris) the decoding
  thread could hang there for good (all capture buffers held, or the driver no
  longer returning them after decode errors), so the decoder never closed and every
  later video waited 10 s and fell back to the CPU. Now a 500 ms wait that returns
  `AVERROR(EAGAIN)`, which callers already handle by sending more input.
