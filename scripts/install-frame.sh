#!/usr/bin/env bash
# Installs the Steam Frame build into ~/Applications/JustVideo on the headset
# and adds "Just Video" to the Steam library as a non-Steam game (first run only).
# Build first: scripts/build-frame-media.sh (once), then scripts/build-frame.sh.
# The entry must be marked as a VR app, or SteamVR keeps its own menu in front
# and the app starts "in the background". Steam only reads that flag at startup
# and rewrites the file while running, so setting it needs one Steam restart:
# FRAME_RESTART_STEAM=1 does that (the headset's interface restarts briefly).
set -euo pipefail
cd "$(dirname "$0")/.."
binary=target/aarch64-unknown-linux-gnu/release/just-video
[ -x "$binary" ] || { echo "Build first: scripts/build-frame.sh" >&2; exit 1; }
source scripts/frame-env.sh
frame_ssh 'mkdir -p ~/Applications/JustVideo'
rsync -a -e "$(frame_ssh_wrapper)" "$binary" "$FRAME_TARGET:Applications/JustVideo/just-video"
# Library artwork (tools/make-steam-art.py).
rsync -a -e "$(frame_ssh_wrapper)" assets/steam/ "$FRAME_TARGET:Applications/JustVideo/art/"
frame_ssh "RESTART_STEAM=${FRAME_RESTART_STEAM:-0} bash -s" <<'REMOTE'
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

# Library artwork: Steam looks for grid/<appid>{p,,_hero,_logo}.png, the
# appid being the shortcut's (an unsigned 32-bit number in shortcuts.vdf).
python3 - <<'PY'
import glob, os, shutil, struct
art = os.path.expanduser("~/Applications/JustVideo/art")
marker = "Applications/JustVideo/Just Video"

def parse(data, i):
    """Binary VDF map starting at data[i]: returns (dict, next index)."""
    out = {}
    while data[i] != 0x08:
        kind = data[i]
        end = data.index(0, i + 1)
        key = data[i + 1:end].decode("utf-8", "replace")
        i = end + 1
        if kind == 0x00:
            out[key], i = parse(data, i)
        elif kind == 0x01:
            end = data.index(0, i)
            out[key] = data[i:end].decode("utf-8", "replace")
            i = end + 1
        elif kind == 0x02:
            out[key] = struct.unpack_from("<i", data, i)[0]
            i += 4
        else:
            raise ValueError(f"VDF type {kind} at {i}")
    return out, i + 1

for path in glob.glob(os.path.expanduser("~/.local/share/Steam/userdata/*/config/shortcuts.vdf")):
    data = open(path, "rb").read()
    root, _ = parse(data, 0)
    for entry in root.get("shortcuts", {}).values():
        exe = entry.get("Exe") or entry.get("exe") or ""
        if marker not in exe:
            continue
        appid = entry.get("appid", 0) & 0xFFFFFFFF
        grid = os.path.join(os.path.dirname(path), "grid")
        os.makedirs(grid, exist_ok=True)
        for source, name in (("capsule.png", f"{appid}p.png"), ("header.png", f"{appid}.png"),
                             ("hero.png", f"{appid}_hero.png"), ("logo.png", f"{appid}_logo.png")):
            if os.path.exists(os.path.join(art, source)):
                shutil.copyfile(os.path.join(art, source), os.path.join(grid, name))
        print(f"Library artwork set for app {appid}.")
PY

# Mark the entry as a VR app (OpenVR = 1). Prints the value it found.
openvr() {
    python3 - "$1" "$2" <<'PY'
import glob, os, sys
mode, marker = sys.argv[1], b"Applications/JustVideo/Just Video"
for path in glob.glob(os.path.expanduser("~/.local/share/Steam/userdata/*/config/shortcuts.vdf")):
    data = bytearray(open(path, "rb").read())
    start = data.find(marker)
    flag = data.find(b"\x02OpenVR\x00", start)
    if start < 0 or flag < 0 or flag - start > 2000:
        continue
    at = flag + len(b"\x02OpenVR\x00")
    print(int.from_bytes(data[at:at + 4], "little"))
    if mode == "set":
        data[at:at + 4] = (1).to_bytes(4, "little")
        open(path, "wb").write(data)
PY
}
if [ "$(openvr check x)" = 1 ]; then
    :
elif [ "$RESTART_STEAM" = 1 ]; then
    echo "Restarting Steam to mark Just Video as a VR app…"
    steam -shutdown >/dev/null 2>&1 || true
    for _ in $(seq 60); do pgrep -x steam >/dev/null || break; sleep 1; done
    openvr set x >/dev/null
    echo "Marked. Steam restarts on its own; if it doesn't, restart the headset."
else
    echo "Note: Just Video isn't marked as a VR app yet, so it starts in the background."
    echo "      Run again with FRAME_RESTART_STEAM=1 to fix (restarts Steam once)."
fi
REMOTE
