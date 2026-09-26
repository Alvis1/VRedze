#!/usr/bin/env bash
# Runs `just-video bench` on the headset over SMB and prints one summary line.
# Usage: JUST_VIDEO_SMB_PASSWORD=... scripts/frame-bench.sh <smb-url> [bench args...]
# The password is passed through ssh stdin, never on a command line.
set -euo pipefail
url=$1; shift
printf '%s\n' "${JUST_VIDEO_SMB_PASSWORD:?set JUST_VIDEO_SMB_PASSWORD}" |
  bash "$(dirname "$0")/frame-ssh.sh" "read -r P; export JUST_VIDEO_SMB_PASSWORD=\$P; ${FRAME_PREFIX:-} ~/just-video/bin/just-video bench '$url' $*" |
  jq -r '"\(.decode.decoder)\thw=\(.decode.hardware_frames) sw=\(.decode.software_frames)\t\(.decode.frames_per_second*10|round/10) fps\t\(.realtime_factor*100|round/100)x realtime\t\(.decode.error // "")"'
