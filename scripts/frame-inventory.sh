#!/usr/bin/env bash
# Run on the headset: bash frame-inventory.sh > frame-inventory.txt
# Read-only. No root, installation, secret collection, or system reconfiguration.
set -u
export LC_ALL=C
section() { printf '\n## %s\n' "$1"; }
run() {
    if command -v "$1" >/dev/null 2>&1; then
        timeout 20s "$@" 2>&1 || printf 'Check failed or timed out (exit %s)\n' "$?"
    else
        printf '%s is not installed\n' "$1"
    fi
}
section 'OS and architecture'
uname -srm
cat /etc/os-release
section 'Device model'
if [[ -r /sys/firmware/devicetree/base/model ]]; then
    tr '\0' '\n' < /sys/firmware/devicetree/base/model
fi
section 'Graphics and video device nodes'
for node in /dev/dri/renderD* /dev/video* /dev/media*; do
    [[ -e "$node" ]] && ls -l "$node"
done
section 'FFmpeg build'
run ffmpeg -hide_banner -version
section 'FFmpeg hardware backends (compiled support only)'
run ffmpeg -hide_banner -hwaccels
section 'FFmpeg decoders (compiled support only)'
run ffmpeg -hide_banner -decoders
section 'Vulkan devices'
run vulkaninfo --summary
section 'Vulkan decode and frame-sharing extensions'
if command -v vulkaninfo >/dev/null 2>&1; then
    timeout 20s vulkaninfo 2>&1 | grep -E 'deviceName|driverName|driverInfo|video_decode|video_queue|VIDEO_DECODE|external_memory|external_semaphore|drm_format_modifier|sampler_ycbcr' || true
fi
section 'V4L2 device drivers'
run v4l2-ctl --list-devices
for node in /dev/video*; do
    [[ -e "$node" ]] || continue
    section "$node"
    run v4l2-ctl -d "$node" --info
    run v4l2-ctl -d "$node" --list-formats-out
    run v4l2-ctl -d "$node" --list-formats
done
section 'VAAPI'
run vainfo
section 'OpenXR loader manifests'
for manifest in /etc/openxr/1/active_runtime.json /usr/share/openxr/1/active_runtime.json \
    "${XDG_CONFIG_HOME:-$HOME/.config}/openxr/1/active_runtime.json"; do
    if [[ -r "$manifest" ]]; then
        printf '%s\n' "$manifest"
        cat "$manifest"
    fi
done
section 'Development tools'
for tool in cc cargo pkg-config; do run "$tool" --version; done
section 'FFmpeg headers'
run pkg-config --modversion libavcodec libavformat libavutil
section 'Result'
printf 'Inventory only. Hardware decode, GPU import, playback and credentials remain unverified.\n'
