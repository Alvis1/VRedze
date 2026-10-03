#!/usr/bin/env bash
# Apple spatial video (MV-HEVC) test samples, made on macOS with avconvert:
# the left eye red, the right eye blue, so eye order is easy to check (close
# one eye in the headset, or compare the chroma planes in tests).
#   samples/spatial-red-left.mov      flat (rectilinear)
#   samples/spatial-red-left-180.mov  half-equirectangular (VR180)
#   samples/spatial-red-left-360.mov  equirectangular (VR360)
set -euo pipefail
cd "$(dirname "$0")/.."
[ "$(uname -s)" = Darwin ] || { echo "Needs macOS (avconvert)" >&2; exit 1; }
mkdir -p samples
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Side-by-side source: one test pattern tinted red (left) and blue (right).
ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i "testsrc2=s=1440x1440:r=30:d=5" -f lavfi -i "sine=f=440:d=5" \
  -filter_complex "[0:v]split[a][b];[a]lutrgb=g=0:b=0[l];[b]lutrgb=r=0:g=0[r];[l][r]hstack,format=yuv420p[v]" \
  -map "[v]" -map 1:a -c:v hevc_videotoolbox -q:v 70 -tag:v hvc1 -c:a aac "$tmp/sbs.mov"

convert() {
  avconvert -s "$tmp/sbs.mov" -o "samples/$1" -p PresetMVHEVC1440x1440 \
    --sourceViewPacking SideBySide "${@:2}" --replace >/dev/null
  echo "samples/$1"
}
convert spatial-red-left.mov
convert spatial-red-left-180.mov --sourceProjection HalfEquirectangular
convert spatial-red-left-360.mov --sourceProjection Equirectangular
