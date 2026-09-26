# Just Video

A standalone Steam Frame VR video player (H.264, HEVC, AV1; flat, VR180, VR360).
All playback will run on the headset and read directly from SMB/Samba shares.
No companion server or runtime transcoding service.

**Status: early player pipeline.** The app lists SMB shares, streams files over
SMB with pipelined read-ahead, and decodes H.264/HEVC/AV1 in hardware through FFmpeg.
It also detects VR layouts (VR180/VR360, side-by-side/top-bottom). There is no
XR rendering or audio yet. Headset hardware decode is unverified (see
[hardware gate](docs/hardware-validation.md)).

## Player CLI (development)

```sh
cargo build --release
export JUST_VIDEO_SMB_PASSWORD=...   # or omit it to get a prompt; never put it in the URL
./target/release/just-video ls        smb://user@host/share/folder
./target/release/just-video info      smb://user@host/share/folder/clip_180_LR.mp4
./target/release/just-video read-bench smb://user@host/share/folder/clip.mp4 --mib 512
./target/release/just-video bench     smb://user@host/share/folder/clip.mp4 --frames 600
./target/release/just-video play      smb://user@host/share/folder/clip_180_LR.mp4   # in the headset
```

`play` detects the layout. You can override it with `--projection flat|180|360|fisheye`,
`--stereo mono|sbs|tb` and `--swap-eyes`, and seek with `--start SECONDS`. `--screenshot out.png`
saves the left eye so you can check rendering without the headset. Files the playability
check rejects need `--force`. On the Frame, build with `scripts/build-frame-media.sh` and then
`scripts/build-frame.sh`, and run over SSH (`scripts/frame-ssh.sh`) while the headset is worn.

`bench` decodes with Vulkan video first (`--hw vulkan|vaapi|none`). With
`--hw-only` it fails rather than fall back to software. It reports hardware
vs. software frames and the real-time factor. `read-bench` measures SMB
throughput; tune the read-ahead with `--block-kib` and `--blocks-ahead`.
Local paths work too.

The FFmpeg development headers are found through `.cargo/config.toml`, which
points `PKG_CONFIG_PATH` at `.local-deps/sysroot`.

## Start with the headset

1. Enable **Steam Settings → System → Enable Developer Mode**.
2. In **Developer**, choose **Set User Password**. Enter it only in the device or
   your local SSH prompt, never in this repository or a chat.
3. Find **System → Hostname**. Valve documents `frame` and user `steamos` as defaults.
4. On this project computer, run `bash scripts/connect-frame.sh`. It connects to
   `steamos@frame.local` and keeps a local SSH control socket available for 30
   minutes after exit, allowing subsequent checks without exposing a password.
   Complete SSH host verification and authentication interactively. If your
   hostname differs, use an equivalent SSH command for that hostname/IP.
5. Once connected, run the read-only inventory from the project computer:

```sh
mkdir -p reports
ssh -S "$PWD/.local-deps/frame-ssh" steamos@frame.local 'bash -s' < scripts/frame-inventory.sh > reports/frame-inventory.txt
```

The shell inventory needs no app installation. Missing tools are recorded. It
does not modify SteamOS or access credentials. This connection is a development
tool; the eventual player will work independently of it.

Source: [Valve setup](https://partner.steamgames.com/doc/steamhardware/steamframe/setup)
and [Valve debugging](https://partner.steamgames.com/doc/steamhardware/steamframe/debugging).

## Native probes

The Rust inventory dynamically loads Vulkan and OpenXR, so it needs neither SDK
headers nor FFmpeg development packages:

```sh
cargo run --locked --bin frame-probe -- inventory
```

The decode feature links FFmpeg through a small C shim, compiled against the
installed target headers to avoid hand-maintained FFmpeg structure layouts.
It requires a C compiler, pkg-config and development headers/libraries for
`libavformat`, `libavcodec` and `libavutil` (FFmpeg 7 or newer).

```sh
cargo build --locked --release
timeout 60s ./target/release/frame-probe decode /path/to/sample.mp4 --backend vulkan --frames 300
timeout 60s ./target/release/frame-probe decode /path/to/sample.mp4 --backend vulkan --frames 300 --verify-readback
```

Available backends: `vulkan`, `vaapi`, `v4l2m2m`. This is a diagnostic list, **not a
claim that any is supported by Frame**. Vulkan/VAAPI accept `--device`; V4L2 uses
FFmpeg device discovery. Probe on native ARM64 Linux first, outside Lepton.
An x86_64 build on your PC is not a Frame binary. We will choose packaging and a
matching ARM64 sysroot after reading the headset inventory; do not replace system
graphics drivers or unlock the SteamOS root filesystem just to run these checks.

The probe selects native FFmpeg codecs with hardware acceleration explicitly,
rejects software surfaces for Vulkan/VAAPI, and fails on corrupt frames or a short
sample. V4L2 uses dedicated `*_v4l2m2m` decoders, which may deliver CPU surfaces;
that is reported separately. Absent decoder support is a failed backend test,
not proof that the chip cannot decode the codec. No automatic fallback occurs.

JSON goes to stdout; decoder diagnostics go to stderr. Exit status 2 means the
sample did not pass; other nonzero statuses indicate command/setup errors.
The reported throughput excludes initialization and is **decode API throughput**.
Without readback it does not wait for all GPU work. Readback confirms transfer
completion and deliberately introduces CPU copies. Neither measures rendering,
power, A/V sync or visual correctness. Reports never automatically certify Frame.

## Local verification

```sh
cargo test --locked
cargo test --locked --no-default-features   # inventory-only build without FFmpeg
cargo clippy --locked --all-targets -- -D warnings
bash scripts/make-samples.sh
./target/debug/frame-probe decode samples/h264-720p.mp4 --backend vulkan --frames 60 --verify-readback
```

Fixture generation is a development-only operation, unrelated to playback. These
tiny synthetic clips check the probe; real 4K/8K samples are required for performance.
Generated media, machine reports and local dependency sysroots are ignored.

See [hardware gate](docs/hardware-validation.md), [architecture](docs/architecture.md)
and [remaining work](docs/roadmap.md).
