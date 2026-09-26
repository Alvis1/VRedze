#!/usr/bin/env bash
# SSH to the Steam Frame with the dedicated dev key (installed once via password login).
# Override the address with FRAME_HOST (e.g. the direct-link 10.35.78.1).
exec ssh -i ~/.ssh/steam_frame_ed25519 -o IdentitiesOnly=yes -o BatchMode=yes \
    -o ConnectTimeout=8 "steamos@${FRAME_HOST:-frame.local}" "$@"
