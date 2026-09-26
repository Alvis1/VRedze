# TODO

## Hybrid (GPU compute) HEVC 10-bit decoding for 8K on Steam Frame

**Status:** idea, not started. Current product target is HEVC 8-bit (hardware) plus
4K 10-bit/AV1 (CPU). Do this only after the Vulkan/OpenXR renderer exists.

**Problem:** 8K60 HEVC Main10 (the user's VR library) plays at only ~47 fps (0.8× real time).
- The Frame's hardware decoder (V4L2 `iris`, `/dev/video-dec0`) is 8-bit only on
  SteamOS 0.3 / kernel 6.18, and sending it 10-bit crashes its firmware.
- Turnip has no Vulkan Video.
- CPU decoding is memory-latency bound: `perf` shows ~80% backend stalls in
  `put_hevc_pel_*` motion compensation, and thread/filter/THP tuning changed nothing.
- Details: `docs/frame-results.md`.

**Idea:** keep the entropy decoding (CABAC and syntax parsing) on the CPU; it is ~2% of the
profile. Run reconstruction on the Adreno 750 with Vulkan compute:
- motion compensation (qpel/epel, bi, weighted);
- inverse DCT/DST and residual add;
- intra prediction, in CTU wavefront order, because intra depends on neighbours in the same frame;
- deblocking and SAO.

Frames then stay in GPU memory for the renderer, so there is no ~100 MB/frame upload.
Precedent: Intel's hybrid VP9/HEVC-10 shader decode.

**Plan:**
1. Prototype, time-boxed to about 1 week. Patch FFmpeg's HEVC decoder (`libavcodec/hevc/`)
   to record per-CTU prediction info, MVs and coefficients instead of calling the
   `HEVCDSPContext` MC/transform functions. Upload them per frame and do inter
   MC + residual on the GPU. Compare fps against the CPU path on
   an 8K60 HEVC Main10 file on the SMB share with `scripts/frame-bench.sh`.
2. Continue only if the prototype shows a clear path to ≥ 60 fps while the renderer runs. Then add intra
   (wavefront), deblocking and SAO.
3. The output must be bit-exact with FFmpeg's C decoder. Verify with per-frame checksums;
   any drift compounds across reference frames.

**Build context:** Frame binaries come from `scripts/build-frame-media.sh`, which builds
FFmpeg 8.1.3 + dav1d + zlib and applies `third_party/ffmpeg-patches/`, then
`scripts/build-frame.sh`. Deploy with `scripts/frame-ssh.sh`. Until this lands, the playability
check tells users why 8K 10-bit HEVC won't play.
