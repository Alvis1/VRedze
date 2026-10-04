# VRedze

A VR video player for **Steam Frame** and **Meta Quest 2/3**. The name joins VR
and *redze*, Latvian for "sight". It plays flat,
3D (side by side or over-under), 180° and 360° video, and Apple spatial video
(MV-HEVC), up to 8K. Videos come from the headset itself or from SMB shares.
Forked from [kumorig/just-video](https://github.com/kumorig/just-video).

## Steam Frame

Turn on Developer Mode, and connect once with FramePort, or copy an SSH key
over (`ssh-copy-id -i ~/.ssh/steam_frame_ed25519 steamos@frame.local`). Then:

```sh
bash scripts/build-frame-media.sh   # once: FFmpeg + dav1d
bash scripts/build-frame.sh
bash scripts/install-frame.sh       # FRAME_HOST=<ip> if frame.local doesn't resolve
```

## Meta Quest

Turn on Developer Mode and USB debugging. You need the Android NDK and SDK. Then:

```sh
bash scripts/build-android-media.sh  # once
bash scripts/build-quest.sh && bash scripts/package-quest.sh && bash scripts/install-quest.sh
```

## Videos that stutter or don't play

Point at a video in the headset to see why and what to do. Two scripts convert
videos on a Mac:

- `tools/fit-for-quest.sh`: makes 8-bit HEVC that both headsets decode in
  hardware (8K 360° becomes 5760×5760).
- `tools/spatial2sbs.sh`: converts Apple spatial video to side by side.

MIT licensed. See `LICENSE` and `THIRD_PARTY_NOTICES.md`.
