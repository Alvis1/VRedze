#!/bin/bash
# Convert Apple spatial video (MV-HEVC from iPhone / Apple Vision Pro) into side-by-side HEVC files
# that VR players on the Steam Frame understand, and optionally copy them to the Frame's Videos folder.
#
# Usage: spatial2sbs.sh [options] <files or folders>...
#   --swap          put the right eye on the left (fixes inverted depth)
#   --max-width N   scale the side-by-side result down to N pixels wide (e.g. 5760 for smoother playback)
#   --quality Q     HEVC quality 1-100 (default 65)
#   --out DIR       write results here (default: next to each source)
#   --to-frame      copy each result to ~/Videos on the Steam Frame (uses FramePort's SSH key)
#
# Spatial (MV-HEVC) inputs are always converted. Inputs that are already side-by-side are only
# re-encoded when --swap or --max-width is given. Output names carry the usual player tags:
#   name_LR.mp4 (flat 3D), name_180_LR.mp4 (VR180), name_360_LR.mp4 (VR360)

set -uo pipefail

FRAME_MAX_WIDTH=8192   # the Frame's hardware HEVC decoder handles up to 8192x8192
QUALITY=65
MAX_WIDTH=""
SWAP=0
OUT_DIR=""
TO_FRAME=0
INPUTS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --swap) SWAP=1 ;;
    --max-width) MAX_WIDTH="$2"; shift ;;
    --quality) QUALITY="$2"; shift ;;
    --out) OUT_DIR="$2"; shift ;;
    --to-frame) TO_FRAME=1 ;;
    -h|--help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; exit 2 ;;
    *) INPUTS+=("$1") ;;
  esac
  shift
done
[ ${#INPUTS[@]} -gt 0 ] || { sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

# Frame connection details come from FramePort, so they stay current if the Frame's address changes
FP="$HOME/Library/Application Support/FramePort"
frame_copy() {
  local host user
  host=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["host"])' "$FP/frames.json") || return 1
  user=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["user"])' "$FP/frames.json") || return 1
  scp -i "$FP/ssh/id_ed25519" -o "UserKnownHostsFile=\"$FP/ssh/known_hosts\"" -o HostKeyAlgorithms=ssh-ed25519 \
      -o BatchMode=yes -o ConnectTimeout=10 "$1" "$user@$host:Videos/"
}

probe() { ffprobe -v error -select_streams v:0 "$@"; }
# First non-empty value of one field of the first video stream
field() { probe "${@:2}" -show_entries "$1" -of default=nw=1:nk=1 "$SRC" 2>/dev/null | grep -v '^$' | head -1; }

convert_one() {
  local src="$1" views projection width pix_fmt suffix filter base dir out vw
  SRC="$src"
  views=$(field stream=view_ids_available -view_ids -1)
  projection=$(field stream_side_data=projection)
  width=$(field stream=width)
  pix_fmt=$(field stream=pix_fmt)
  [[ "$width" =~ ^[0-9]+$ ]] || { echo "  !! no readable video stream"; return 1; }

  case "$projection" in
    "half equirectangular") suffix="_180_LR" ;;
    equirectangular)        suffix="_360_LR" ;;
    ""|rectilinear)         suffix="_LR" ;;
    *) echo "  note: unknown projection '$projection', treating as flat"; suffix="_LR" ;;
  esac

  if [[ "$views" == *,* ]]; then
    vw=$((width * 2))
    if [ $SWAP = 1 ]; then
      filter="[0:V:vpos:right][0:V:vpos:left]hstack"
    else
      filter="[0:V:vpos:left][0:V:vpos:right]hstack"
    fi
    echo "  Apple spatial video (MV-HEVC), ${width}px per eye${projection:+, $projection}"
  elif [ $SWAP = 1 ] || [ -n "$MAX_WIDTH" ]; then
    vw=$width
    if [ $SWAP = 1 ]; then
      filter="[0:v:0]split[a][b];[a]crop=iw/2:ih:iw/2:0[r];[b]crop=iw/2:ih:0:0[l];[r][l]hstack"
    else
      filter="[0:v:0]null"
    fi
    echo "  side-by-side video, ${width}px wide"
  else
    echo "  not a spatial video, nothing to do (use --swap or --max-width to re-encode side-by-side files)"
    return 0
  fi

  # Keep within the requested width and the Frame's decoder limit
  local limit=$FRAME_MAX_WIDTH
  [ -n "$MAX_WIDTH" ] && [ "$MAX_WIDTH" -lt "$limit" ] && limit=$MAX_WIDTH
  if [ "$vw" -gt "$limit" ]; then
    filter="$filter,scale=$limit:-2:flags=lanczos"
    echo "  scaling to ${limit}px wide"
  fi

  # 10-bit sources stay 10-bit
  local fmt_args=(-pix_fmt yuv420p -profile:v main)
  [[ "$pix_fmt" == *10* ]] && fmt_args=(-pix_fmt p010le -profile:v main10)

  base=$(basename "${src%.*}")
  dir=${OUT_DIR:-$(dirname "$src")}
  mkdir -p "$dir"
  [ $SWAP = 1 ] && [[ ! "$views" == *,* ]] && suffix="_swapped"
  out="$dir/$base$suffix.mp4"

  local video_args=(-filter_complex "$filter[v]" -map "[v]" -c:v hevc_videotoolbox -q:v "$QUALITY"
                    "${fmt_args[@]}" -tag:v hvc1 -movflags +faststart)
  if ! ffmpeg -hide_banner -loglevel error -stats -y -i "$src" "${video_args[@]}" \
         -map '0:a:0?' -c:a aac -b:a 256k "$out"; then
    echo "  audio could not be converted, retrying without audio"
    ffmpeg -hide_banner -loglevel error -stats -y -i "$src" "${video_args[@]}" -an "$out" || return 1
  fi
  echo "  -> $out ($(probe -show_entries stream=width,height -of csv=s=x:p=0 "$out" | head -1))"

  if [ $TO_FRAME = 1 ]; then
    echo "  copying to the Frame's Videos folder"
    frame_copy "$out" || { echo "  !! copy to Frame failed (is it awake and on Wi-Fi?)"; return 1; }
  fi
}

FAILED=0
for input in "${INPUTS[@]}"; do
  if [ -d "$input" ]; then
    while IFS= read -r -d '' f; do
      echo "$f"; convert_one "$f" || FAILED=$((FAILED + 1))
    done < <(find "$input" -maxdepth 1 -type f \( -iname '*.mov' -o -iname '*.mp4' -o -iname '*.m4v' \) -print0)
  elif [ -f "$input" ]; then
    echo "$input"; convert_one "$input" || FAILED=$((FAILED + 1))
  else
    echo "$input: not found"; FAILED=$((FAILED + 1))
  fi
done
[ $FAILED = 0 ] || { echo "$FAILED file(s) failed"; exit 1; }
