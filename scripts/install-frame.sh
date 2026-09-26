#!/usr/bin/env bash
# Installs the Steam Frame build into ~/Applications/JustVideo on the headset
# and adds "Just Video" to the Steam library as a non-Steam game (first run only).
# Build first: scripts/build-frame-media.sh (once), then scripts/build-frame.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
binary=target/aarch64-unknown-linux-gnu/release/just-video
[ -x "$binary" ] || { echo "Build first: scripts/build-frame.sh" >&2; exit 1; }
host=steamos@${FRAME_HOST:-frame.local}
ssh_opts=(-i "$HOME/.ssh/steam_frame_ed25519" -o IdentitiesOnly=yes -o BatchMode=yes)
ssh "${ssh_opts[@]}" "$host" 'mkdir -p ~/Applications/JustVideo'
rsync -a -e "ssh ${ssh_opts[*]}" "$binary" "$host:Applications/JustVideo/just-video"
ssh "${ssh_opts[@]}" "$host" 'bash -s' <<'REMOTE'
set -euo pipefail
dir=$HOME/Applications/JustVideo
launcher="$dir/Just Video"
# Steam names the library entry after this file.
cat > "$launcher" <<'SH'
#!/bin/sh
exec "$(dirname "$0")/just-video" "$@"
SH
chmod +x "$launcher" "$dir/just-video"
if grep -rqs --text "Applications/JustVideo/Just Video" ~/.local/share/Steam/userdata/*/config/shortcuts.vdf; then
    echo "Updated Just Video (already in the Steam library)."
else
    steamos-add-to-steam "$launcher"
    echo "Added Just Video to the Steam library (Non-Steam)."
fi
REMOTE
