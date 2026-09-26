#!/usr/bin/env bash
# Interactive first login. Password entry remains inside the local SSH client.
set -euo pipefail
project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -p "$project/.local-deps"
chmod 700 "$project/.local-deps"
exec ssh -M -S "$project/.local-deps/frame-ssh" \
    -o ControlPersist=30m -o StrictHostKeyChecking=ask \
    steamos@frame.local
