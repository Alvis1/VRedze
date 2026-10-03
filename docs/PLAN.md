# Spatial Player — plan

A native Steam Frame (SteamOS, aarch64, OpenXR + Vulkan) player for stereoscopic and
Apple spatial video. Forked from [just-video](https://github.com/kumorig/just-video) (MIT)
with the four open community branches from lgruen/just-video merged (macOS build,
local file browsing, half-packed 3D, control-bar distance).

## What it plays

| Content | Detection | Decode |
|---|---|---|
| Apple spatial video (MV-HEVC, iPhone 1080p/eye, Vision Pro ~2200²/eye, up to 4320²/eye) | `view_ids_available` has two views | Software, both views (`view_ids=-1`) |
| APMP projections: rectilinear, half-equirect (180), equirect (360), fisheye / parametric immersive | `AVSphericalMapping` | — |
| Side-by-side L/R or R/L, over-under, full or half width | `AVStereo3D`, filename tags, per-file override | Hardware if 8-bit H.264/HEVC, else software |
| Mono 2D, 180, 360, fisheye | same | same |
| Up to 8K (≤ 8192 px per side) | — | Hardware 8-bit with ≤ 8 capture buffers; 10-bit/AV1 in software |

## Device facts (measured on the user's Frame, SteamOS 0.3.0 build 20260922)

- Hardware decoder: vendor qcom iris V4L2 stateful driver at `/dev/video-dec0`
  (H.264, HEVC, VP9; 8-bit NV12 only). FFmpeg's default 20 capture buffers fail with
  ENOMEM above ~36864 macroblocks/frame; just-video measured 8K working with ≤ 8.
  Two large sessions can run at once; a third crashed the firmware. `steamwebhelper`
  holds one session.
- **Never** send 10-bit HEVC or VP9 to iris (firmware crash; a crash can take the headset down).
- 8K60 HEVC Main10 in software: ~0.8× real time — not playable.
- OpenXR: SteamVR Linux runtime, request API 1.0, projection + quad layers only
  (no cylinder/equirect), 1728² per eye default. Vulkan: Turnip (Adreno 750), no Vulkan Video.
- On-device toolchain exists (gcc, clang, cmake, headers for OpenXR/Vulkan/FFmpeg 7.0/V4L2).

## Architecture

- Rust: `openxr` (loader-less negotiation), `ash` (Vulkan), static FFmpeg 8.1.3 + dav1d
  via a C shim (`native/`), cross-built on macOS with zig / cargo-zigbuild (glibc 2.39).
- Video drawn by a per-pixel ray-casting shader into the projection layer (flat, curved,
  180, 360, fisheye); UI panels as quads.
- Audio is the master clock (Pulse/PipeWire).

## Milestones

1. **Baseline** — build on the Mac, deploy over SSH, play a local file on the Frame.
2. **Safe device probes** — runtime layer/format limits; confirm 8K 8-bit hardware decode
   with ≤ 8 capture buffers. One decoder session at a time, never 10-bit/VP9.
3. **Apple spatial video** — MV-HEVC both views to the right eye; APMP projections; eye
   mapping (view position → hero eye → manual swap).
4. **Zero-copy hardware decode** — own V4L2 stateful client, `VIDIOC_EXPBUF` dma-bufs
   imported into Vulkan as NV12; replace the CPU copy + per-frame wait.
5. **Controls** — 3D-mode menu (layout, projection, swap), screen size/distance/curve,
   recenter, per-file memory; rename to Spatial Player.
6. **Polish** — Steam library entry with artwork, one-command update, thermal/perf soak.

Companion tool: `tools/spatial2sbs.sh` converts MV-HEVC to side-by-side on the Mac
(useful for other players, and for files too heavy to decode in software).

## Testing

- `cargo test` on macOS for parsing, layout detection, eye mapping.
- On the Frame over SSH: decode benchmarks and logs. Visual checks need the user in the
  headset (the session only reaches FOCUSED when worn).
