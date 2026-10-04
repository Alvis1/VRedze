#!/usr/bin/env bash
# Re-encodes videos the headsets can't decode in hardware as 8-bit HEVC that
# both the Quest 3 and the Steam Frame can, using the Mac's hardware encoder:
#   - too large for the Quest 3 (at most 138240 macroblocks: 8192x4320, or
#     5760x5760 square): scaled to the largest common size that fits;
#   - 10-bit, 4:2:2/4:4:4, or not H.264/HEVC (ProRes, AV1, VP9…): same size.
# Layout tags (_360/_180, _TB/_LR) from the file's metadata are added to the
# name, so players still detect the layout if the metadata doesn't survive.
# Apple spatial (MV-HEVC) videos are left to tools/spatial2sbs.sh.
#
# Usage: tools/fit-for-quest.sh [--quality Q] [--out DIR] <files>...
#   --quality Q   HEVC quality 1-100 (default 62)
#   --out DIR     write here (default: next to each source)
set -uo pipefail
QUALITY=62
OUT_DIR=""
FILES=()
usage() { sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; }
while [ $# -gt 0 ]; do
  case "$1" in
    --quality) QUALITY="$2"; shift ;;
    --out) OUT_DIR="$2"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) FILES+=("$1") ;;
  esac
  shift
done
[ ${#FILES[@]} -gt 0 ] || { usage; exit 2; }

for src in "${FILES[@]}"; do
  [ -f "$src" ] || { echo "$src: not found"; continue; }
  field() { ffprobe -v error -select_streams v:0 "${@:2}" -show_entries "$1" -of default=nw=1:nk=1 "$src" 2>/dev/null | grep -v '^$' | head -1; }
  w=$(field stream=width); h=$(field stream=height)
  [[ "${w:-}" =~ ^[0-9]+$ && "${h:-}" =~ ^[0-9]+$ ]] || { echo "$src: no readable video stream"; continue; }
  codec=$(field stream=codec_name); pix=$(field stream=pix_fmt)
  fps=$(field stream=r_frame_rate | awk '{split($1,f,"/"); print (f[2]>0 ? f[1]/f[2] : 30)}')
  views=$(field stream=view_ids_available -view_ids -1)
  blocks=$(( ((w + 15) / 16) * ((h + 15) / 16) ))
  echo "$src: ${w}x${h} ${codec} ${pix} at ${fps} fps ($blocks macroblocks)"
  if [[ "$views" == *,* ]]; then
    echo "  Apple spatial video: use tools/spatial2sbs.sh (side-by-side plays with hardware decoding)"
    continue
  fi
  reasons=()
  [ "$blocks" -gt 138240 ] && reasons+=("too large for the Quest 3's decoder")
  case "$codec" in h264|hevc) ;; *) reasons+=("$codec isn't decoded in hardware on both headsets") ;; esac
  case "$pix" in yuv420p|yuvj420p) ;; *) reasons+=("$pix isn't 8-bit 4:2:0") ;; esac
  if [ ${#reasons[@]} -eq 0 ]; then
    echo "  already plays with hardware decoding on both headsets"
    continue
  fi
  # Largest common width that fits, height in proportion (multiple of 16).
  read -r nw nh < <(python3 -c "
import math
w, h = $w, $h
blocks = ((w + 15) // 16) * ((h + 15) // 16)
if blocks <= 138240:
    print(w, h)
else:
    most = w * math.sqrt(138240 / blocks)
    nw = next((c for c in (7680, 6144, 5760, 5120, 4096, 3840, 2880, 1920) if c <= most), 1920)
    print(nw, int(h * nw / w) // 16 * 16)")
  projection=$(field stream_side_data=projection)
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
  mkdir -p "$dir"
  out="$dir/${base}${tags}_${nw}.mp4"
  echo "  $(IFS=';'; echo "${reasons[*]}" | sed 's/;/; /g')"
  echo "  -> ${nw}x${nh} 8-bit HEVC: $out"
  ffmpeg -hide_banner -loglevel error -stats -y -i "$src" -map 0:v:0 -map '0:a:0?' \
    -vf "scale=${nw}:${nh}:flags=lanczos,format=yuv420p" \
    -c:v hevc_videotoolbox -q:v "$QUALITY" -profile:v main -tag:v hvc1 -c:a copy -movflags +faststart "$out" ||
    echo "  !! failed"
done
