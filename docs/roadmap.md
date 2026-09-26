# Implementation status

Performance is the primary constraint: hardware decode, GPU-resident frames,
and pipelined network reads. The decoder backend is swappable, so work does
not wait on the headset hardware gate.

- [x] Research the platform; headset inventory script; Vulkan/OpenXR inventory CLI.
- [x] Explicit-backend FFmpeg decode probe (`frame-probe`) with fail-closed reporting.
- [x] Direct SMB2/3: login, directory listing, seekable pipelined reads (`just-video ls`, `read-bench`).
- [x] FFmpeg custom-IO demux + hardware-first decode for H.264/HEVC/AV1 (`just-video info`, `bench`).
- [x] VR layout detection: metadata (stereo3d/spherical) then filename tags.
- [x] SMB decode end to end on the dev PC: 8K60 HEVC Main10 at 2.7x real time, ~25-30 Gb/s localhost reads (after vendored TCP_NODELAY fix).
- [x] Key-based SSH to the Frame; inventory; ARM64 cross-build (`scripts/build-frame.sh`); SMB decode on device (docs/frame-results.md).
- [x] Bundled FFmpeg 8.1.3 + dav1d + zlib, CPU-tuned, 10-bit HEVC NEON patch (4K10 at 3× real time).
- [ ] 8K60 HEVC 10-bit: memory-bound at 0.8× on the CPU; waits for iris 10-bit support. Explain this to users (playability check).
- [x] Playability verdicts with user-facing reasons (`src/playability.rs`, `just-video ls --check`); the V4L2 allow-list never sends 10-bit or uncertain streams to iris; hardware open failures fall back to the CPU; bad-file tests assert no panics. The UI must show `title`/`detail`/`hint` and refuse `unplayable`.
- [x] OpenXR + Vulkan renderer running standalone on the Frame (`just-video play`): loader-less runtime negotiation, per-pixel ray casting for flat/VR180/VR360/fisheye, SBS/TB/eye swap, YUV→RGB (601/709/2020, 8/10-bit, SDR/PQ/HLG), PTS timing against predicted display time. Verified on device: 1080p H.264 (hw), 4K60 HEVC10 (60 fps), 8K VR180.
- [ ] Zero-copy: import V4L2 decoder buffers as DMA-BUF into Vulkan (today: CPU copy into a staging buffer; ~100 MB/frame at 8K).
- [ ] Pipelining: overlap upload/render with the next frame (today the CPU waits for the GPU every frame).
- [ ] Playback clock, audio output, seek/pause and bounded frame queue.
- [ ] ARM64 build running standalone on Frame; select decode backend from probe results.
- [ ] In-headset browser UI, secure credential storage, opted-in auto-login.
- [ ] Controllers, recentering, view adjustments; per-file layout override.
- [ ] Sustained performance, reconnect and thermal validation on the headset.

Desktop results do not satisfy on-device acceptance.
