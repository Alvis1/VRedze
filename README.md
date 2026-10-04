# VRedze

A VR video player for **Steam Frame** and **Meta Quest 2/3**. It plays flat,
3D (side by side or over-under), 180° and 360° video, and Apple spatial video
(MV-HEVC), up to 8K. Videos come from the headset itself or from SMB shares.
Forked from [kumorig/just-video](https://github.com/kumorig/just-video).


## Install

Download the latest release from the
[Releases page](https://github.com/Alvis1/VRedze/releases): `VRedze.apk` for
the Quest, `VRedze` for the Steam Frame.

### Meta Quest

1. Turn on Developer Mode for the headset (in the Meta Horizon phone app).
2. Connect the Quest to your computer over USB and allow USB debugging in the
   headset.
3. Drag `VRedze.apk` into [SideQuest](https://sidequestvr.com).

VRedze is under **Unknown Sources** in the app library. If your videos don't
show up, allow VRedze to access files in the Quest's settings, under app
permissions.

### Steam Frame

On the headset itself, in Desktop Mode (Power menu → Switch to Desktop):

1. Download `VRedze` with the browser.
2. Make it runnable and put it where Steam expects it. In Konsole:
   ```sh
   mkdir -p ~/Applications/VRedze
   mv ~/Downloads/VRedze ~/Applications/VRedze/
   chmod +x ~/Applications/VRedze/VRedze
   ```
   Or in Dolphin: move it to `Applications/VRedze` in your home folder, then
   right-click → Properties → Permissions → tick "Is executable".
3. In Steam: Games → Add a Non-Steam Game to My Library → Browse, and pick
   `~/Applications/VRedze/VRedze`.
4. Right-click VRedze in the library → Properties → tick **Include in VR
   Library**. Without it, VRedze starts behind SteamVR's menu.
5. Return to Gaming Mode.

To update, replace the file in `~/Applications/VRedze`. The library entry stays.

## Videos that stutter or don't play

Point at a video in the headset to see why and what to do. Two scripts convert
videos on a Mac:

- `tools/fit-for-quest.sh`: makes 8-bit HEVC that both headsets decode in
  hardware (8K 360° becomes 5760×5760).
- `tools/spatial2sbs.sh`: converts Apple spatial video to side by side.

## Building from source

### Steam Frame

Turn on Developer Mode, and connect once with FramePort, or copy an SSH key
over (`ssh-copy-id -i ~/.ssh/steam_frame_ed25519 steamos@frame.local`). Then:

```sh
bash scripts/build-frame-media.sh   # once: FFmpeg + dav1d
bash scripts/build-frame.sh
bash scripts/install-frame.sh       # FRAME_HOST=<ip> if frame.local doesn't resolve
```

### Meta Quest

Turn on Developer Mode and USB debugging. You need the Android NDK and SDK. Then:

```sh
bash scripts/build-android-media.sh  # once
bash scripts/build-quest.sh && bash scripts/package-quest.sh && bash scripts/install-quest.sh
```

MIT licensed. See `LICENSE` and `THIRD_PARTY_NOTICES.md`.
