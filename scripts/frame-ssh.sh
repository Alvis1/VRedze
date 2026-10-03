#!/usr/bin/env bash
# SSH to the Steam Frame (connection settings: scripts/frame-env.sh).
source "$(dirname "$0")/frame-env.sh"
frame_ssh "$@"
