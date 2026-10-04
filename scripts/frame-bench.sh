#!/usr/bin/env bash
# Runs `vredze bench` on the headset over SMB and prints one summary line.
# Usage: VREDZE_SMB_PASSWORD=... scripts/frame-bench.sh <smb-url> [bench args...]
# The password is passed through ssh stdin, never on a command line.
set -euo pipefail
url=$1; shift
printf '%s\n' "${VREDZE_SMB_PASSWORD:?set VREDZE_SMB_PASSWORD}" |
  bash "$(dirname "$0")/frame-ssh.sh" "read -r P; export VREDZE_SMB_PASSWORD=\$P; ${FRAME_PREFIX:-} ~/Applications/VRedze/vredze bench '$url' $*" |
  jq -r '"\(.decode.decoder)\thw=\(.decode.hardware_frames) sw=\(.decode.software_frames)\t\(.decode.frames_per_second*10|round/10) fps\t\(.realtime_factor*100|round/100)x realtime\t\(.decode.error // "")"'
