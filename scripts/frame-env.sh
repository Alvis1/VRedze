#!/usr/bin/env bash
# Sourced by the other scripts: how to reach the Steam Frame over SSH.
# Defaults come from FramePort's saved connection (address, user, key, host key),
# so they follow the headset if its address changes. Override with FRAME_HOST,
# FRAME_USER, FRAME_KEY and FRAME_KNOWN_HOSTS.

framport_dir="$HOME/Library/Application Support/FramePort"
[ -d "$framport_dir" ] || framport_dir="$HOME/.local/share/FramePort"

if [ -z "${FRAME_HOST:-}" ] && [ -f "$framport_dir/frames.json" ]; then
  FRAME_HOST=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["host"])' "$framport_dir/frames.json")
  FRAME_USER=${FRAME_USER:-$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["user"])' "$framport_dir/frames.json")}
fi
FRAME_HOST=${FRAME_HOST:-frame.local}
FRAME_USER=${FRAME_USER:-steamos}
if [ -z "${FRAME_KEY:-}" ]; then
  if [ -f "$framport_dir/ssh/id_ed25519" ]; then FRAME_KEY="$framport_dir/ssh/id_ed25519"
  else FRAME_KEY="$HOME/.ssh/steam_frame_ed25519"; fi
fi
if [ -z "${FRAME_KNOWN_HOSTS:-}" ] && [ -f "$framport_dir/ssh/known_hosts" ]; then
  FRAME_KNOWN_HOSTS="$framport_dir/ssh/known_hosts"
fi

FRAME_TARGET="$FRAME_USER@$FRAME_HOST"
# The quotes around the known_hosts path matter: ssh splits that option on spaces
# and FramePort's folder is "Application Support".
FRAME_SSH_OPTS=(-i "$FRAME_KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=10)
[ -n "${FRAME_KNOWN_HOSTS:-}" ] && FRAME_SSH_OPTS+=(-o "UserKnownHostsFile=\"$FRAME_KNOWN_HOSTS\"" -o HostKeyAlgorithms=ssh-ed25519)

frame_ssh() { ssh "${FRAME_SSH_OPTS[@]}" "$FRAME_TARGET" "$@"; }

# rsync's -e takes a single command string, which can't carry the spaces in the
# key and known_hosts paths reliably. Give it a generated wrapper instead.
frame_ssh_wrapper() {
  local wrapper
  wrapper="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.local-deps/frame-ssh-wrapper"
  mkdir -p "$(dirname "$wrapper")"
  { printf '#!/bin/sh\nexec ssh'; printf ' %q' "${FRAME_SSH_OPTS[@]}"; printf ' "$@"\n'; } > "$wrapper"
  chmod +x "$wrapper"
  printf '%s' "$wrapper"
}
