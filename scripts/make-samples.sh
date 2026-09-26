#!/usr/bin/env bash
# Development fixtures only. The player never transcodes media.
set -euo pipefail
out=${1:-samples}
mkdir -p "$out"
common=(-hide_banner -loglevel error -nostdin -n -f lavfi -i 'testsrc2=size=1280x720:rate=30' -frames:v 60 -an)
ffmpeg "${common[@]}" -c:v libx264 -preset fast -pix_fmt yuv420p "$out/h264-720p.mp4"
ffmpeg "${common[@]}" -c:v libx265 -preset fast -pix_fmt yuv420p10le -x265-params 'log-level=error:pools=2' "$out/hevc-main10-720p.mp4"
ffmpeg "${common[@]}" -c:v libsvtav1 -preset 12 -pix_fmt yuv420p10le -svtav1-params 'lp=2' "$out/av1-main10-720p.mkv"
printf 'Fixtures ready in %s. These are smoke tests, not representative VR performance samples.\n' "$out"
