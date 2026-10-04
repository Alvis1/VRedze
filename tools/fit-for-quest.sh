#!/usr/bin/env bash
# Re-encodes videos too large for the Quest 3's hardware decoder (at most
# 138240 macroblocks: 8192x4320, or 5760x5760 square) as HEVC that fits,
# using the Mac's hardware encoder. Layout tags (_360/_180, _TB/_LR) from the
# file's metadata are added to the name, so players still detect the layout
# if the metadata doesn't survive.
#
# Usage: tools/fit-for-quest.sh [--quality Q] [--out DIR] <files>...
#   --quality Q   HEVC quality 1-100 (default 62)
#   --out DIR     write here (default: next to each source)
set -uo pipefail
QUALITY=62
OUT_DIR=""
FILES=()
while [ $# -gt 0 ]; do
  case "$1" in
    --quality) QUALITY="$2"; shift ;;
    --out) OUT_DIR="$2"; shift ;;
    -h|--help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) FILES+=("$1") ;;
  esac
  shift
done
[ ${#FILES[@]} -gt 0 ] || { sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

for src in "${FILES[@]}"; do
  [ -f "$src" ] || { echo "$src: not found"; continue; }
  read -r w h fps < <(ffprobe -v error -select_streams v:0 -show_entries stream=width,height,r_frame_rate \
    -of default=nw=1:nk=1 "$src" | tr '\n' ' ' | awk '{split($3,f,"/"); print $1, $2, (f[2]>0 ? f[1]/f[2] : 30)}')
  [ -n "${w:-}" ] && [ -n "${h:-}" ] || { echo "$src: no readable video stream"; continue; }
  blocks=$(( ((w + 15) / 16) * ((h + 15) / 16) ))
  echo "$src: ${w}x${h} at ${fps} fps ($blocks macroblocks)"
  if [ "$blocks" -le 138240 ]; then
    echo "  already fits the Quest 3's hardware decoder"
    continue
  fi
  # Largest common width that fits, height in proportion (even, multiple of 16).
  read -r nw nh < <(python3 -c "
import math
w, h = $w, $h
most = w * math.sqrt(138240 / ((w + 15) // 16 * ((h + 15) // 16)))
nw = next((c for c in (7680, 6144, 5760, 5120, 4096, 3840, 2880, 1920) if c <= most), 1920)
print(nw, int(h * nw / w) // 16 * 16)")
  projection=$(ffprobe -v error -select_streams v:0 -show_entries stream_side_data=projection \
    -of default=nw=1:nk=1 "$src" | grep -v '^$' | head -1)
  stereo=$(ffprobe -v error -select_streams v:0 -show_entries stream_side_data=type \
    -of default=nw=1:nk=1 "$src" | grep -iE "side by side|top and bottom" | head -1)
  tags=""
  case "$projection" in
    equirectangular) tags="_360" ;;
    "half equirectangular") tags="_180" ;;
  esac
  case "$stereo" in
    "top and bottom") tags="${tags}_TB" ;;
    "side by side") tags="${tags}_LR" ;;
  esac
  base=$(basename "${src%.*}")
  dir=${OUT_DIR:-$(dirname "$src")}
  out="$dir/${base}${tags}_${nw}.mp4"
  echo "  -> ${nw}x${nh} HEVC: $out"
  ffmpeg -hide_banner -loglevel error -stats -y -i "$src" -map 0:v:0 -map '0:a:0?' \
    -vf "scale=${nw}:${nh}:flags=lanczos,format=yuv420p" \
    -c:v hevc_videotoolbox -q:v "$QUALITY" -tag:v hvc1 -c:a copy -movflags +faststart "$out" ||
    echo "  !! failed"
done
