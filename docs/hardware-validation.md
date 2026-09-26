# Hardware decoding gate

Research date: 2026-09-25. Status: **unverified on Steam Frame**.

## Evidence and unknowns

| Question | Evidence | Decision |
| --- | --- | --- |
| Can an app run standalone? | Valve documents native ARM64 and OpenXR support. | Native ARM64 Linux is the first candidate. |
| Does the silicon contain video decoders? | Qualcomm's Snapdragon 8 Gen 3 brief lists hardware H.265, VP9 and AV1 decoding. | Chip capability does not establish OS API availability. H.264 also needs a device test. |
| Does Frame expose Vulkan Video for all three codecs? | No verified on-device results yet. Mesa's general Turnip documentation is insufficient. | Query the shipped driver and decode actual samples. |
| Can FFmpeg access that API? | FFmpeg supports hardware contexts, codec configurations and Vulkan video acceleration. | Verify the installed/buildable ARM64 FFmpeg and exact codec profiles. |
| Can decoded frames reach Vulkan/OpenXR without CPU copies? | No measurements or import test yet. | Separate gate after decoding. |
| Can Lepton expose the decoder? | Not verified. | Do not assume Android MediaCodec support from silicon specifications. |

Primary sources:

- [Valve Frame development](https://partner.steamgames.com/doc/steamhardware/steamframe)
- [Qualcomm product brief](https://docs.qualcomm.com/bundle/publicresource/87-71408-1_REV_D_Snapdragon_8_gen_3_Mobile_Platform_Product_Brief.pdf)
- [Mesa Freedreno/Turnip](https://docs.mesa3d.org/drivers/freedreno.html)
- [FFmpeg hardware API example](https://www.ffmpeg.org/doxygen/8.0/hw_decode_8c-example.html)
- [FFmpeg CLI hardware options](https://ffmpeg.org/ffmpeg.html)

## Required on-device evidence

1. Collect ARM64 architecture, model, SteamOS/kernel, driver and FFmpeg versions,
   GPU identity, OpenXR runtime, video devices and available APIs. Attach inventory
   to each set of results. Never combine a PC inventory with a Frame decode result.
2. Run local files to isolate decoder support from network throughput. Require
   actual output frames from the explicit hardware path, not a list of compiled
   accelerators. Test the file's real profile, chroma format, bit depth, resolution
   and frame rate. Record the exact sample and its SHA-256 outside public logs.
3. Confirm content correctness with a known test pattern and a presented frame.
   Decode/readback success alone cannot detect incorrect colors or image content.
4. Prove GPU import with the OpenXR-selected physical device. For Vulkan frames,
   share the device, frame ownership and timeline semaphores correctly. For V4L2
   or VAAPI, prove DMA-BUF export/import including modifier, plane layout, fences,
   NV12 and P010. A CPU-surface V4L2 result does not satisfy this gate.
5. Measure sustained decode plus render at the headset's selected refresh rate,
   with a bounded frame queue, audio enabled and current head poses on every XR
   frame. Capture missed frames, p95/p99 CPU/GPU time, memory, thermals and power.
6. Repeat with direct SMB playback, seek, pause/resume, reconnect and app restart.

## Sample matrix

| Codec | Minimum profiles | Workloads |
| --- | --- | --- |
| H.264 | High, 8-bit 4:2:0 | 1080p and 4K, 30/60 fps |
| HEVC | Main and Main10, 4:2:0 | 4K and 8K, 30/60 fps |
| AV1 | Main, 8/10-bit 4:2:0 | 4K and 8K, 30/60 fps |

These are test targets, not promised hardware limits. Start with short smoke
samples; then use representative high-bitrate media for at least 10 minutes.
Record unsupported combinations explicitly. Stereo packing and projection do
not change the codec, but they change frame dimensions and render cost.

Provisional acceptance: maintain source frame rate without sustained queue growth,
keep XR submission at the chosen runtime cadence, no steady-state CPU frame
download/upload, and no thermal collapse. Tune numerical budgets from device
measurements; do not label a fast decode-only benchmark as smooth VR playback.

## Backend selection

Prefer FFmpeg Vulkan Video if all required profiles and shared-device rendering
work. Otherwise evaluate the shipped Linux V4L2/Iris or VAAPI path with DMA-BUF
interop. If only CPU-copy output is accessible, measure and expose that tradeoff
before selecting it. If AV1 is inaccessible, report the missing requirement;
do not silently replace it with software decode or a transcoding service.
