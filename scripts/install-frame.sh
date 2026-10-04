#!/usr/bin/env bash
# Installs the Steam Frame build into ~/Applications/VRedze on the headset
# and adds "VRedze" to the Steam library as a non-Steam game (first run only).
# Build first: scripts/build-frame-media.sh (once), then scripts/build-frame.sh.
# The entry must be marked as a VR app, or SteamVR keeps its own menu in front
# and the app starts "in the background". Steam only reads its shortcuts at
# startup and rewrites them while running, so marking the entry, and renaming
# an entry from before the rename ("Just Video" in ~/Applications/JustVideo),
# need one Steam restart: FRAME_RESTART_STEAM=1 does that (the headset's
# interface restarts briefly). Without it, the old entry keeps working: its
# launcher starts the new build.
set -euo pipefail
cd "$(dirname "$0")/.."
binary=target/aarch64-unknown-linux-gnu/release/vredze
[ -x "$binary" ] || { echo "Build first: scripts/build-frame.sh" >&2; exit 1; }
source scripts/frame-env.sh
frame_ssh 'mkdir -p ~/Applications/VRedze'
rsync -a -e "$(frame_ssh_wrapper)" "$binary" "$FRAME_TARGET:Applications/VRedze/vredze"
# Library artwork (tools/make-steam-art.py).
rsync -a -e "$(frame_ssh_wrapper)" assets/steam/ "$FRAME_TARGET:Applications/VRedze/art/"
frame_ssh "RESTART_STEAM=${FRAME_RESTART_STEAM:-0} bash -s" <<'REMOTE'
set -euo pipefail
dir=$HOME/Applications/VRedze
launcher="$dir/VRedze"
old_launcher="$HOME/Applications/JustVideo/Just Video"
# Steam names the library entry after this file.
cat > "$launcher" <<'SH'
#!/bin/sh
exec "$(dirname "$0")/vredze" "$@"
SH
chmod +x "$launcher" "$dir/vredze"

# Our entry in Steam's shortcuts.vdf (binary VDF), by its launcher path:
#   check   prints "<new|old|none> <OpenVR flag>"
#   art     copies the library artwork for the entry's appid
#   fix     (Steam stopped) renames an old entry and marks it as a VR app
shortcuts() {
    python3 - "$1" <<'PY'
import glob, os, shutil, struct, sys

NEW = b"/Applications/VRedze/VRedze"
OLD = b"/Applications/JustVideo/Just Video"
OLD_DIR, NEW_DIR = b"/Applications/JustVideo/", b"/Applications/VRedze/"
mode = sys.argv[1]
art = os.path.expanduser("~/Applications/VRedze/art")

def parse(data, i):
    """Binary VDF map at data[i]: ({key: (type, value)}, next index); keys and
    strings stay bytes, so writing it back gives the same file."""
    out = {}
    while data[i] != 0x08:
        kind = data[i]
        end = data.index(0, i + 1)
        key = bytes(data[i + 1:end])
        i = end + 1
        if kind == 0x00:
            out[key], i = parse(data, i)
            out[key] = (0, out[key])
        elif kind == 0x01:
            end = data.index(0, i)
            out[key] = (1, bytes(data[i:end]))
            i = end + 1
        elif kind == 0x02:
            out[key] = (2, struct.unpack_from("<i", data, i)[0])
            i += 4
        else:
            raise ValueError(f"VDF type {kind} at {i}")
    return out, i + 1

def dump(m):
    out = bytearray()
    for key, (kind, value) in m.items():
        out += bytes([kind]) + key + b"\0"
        if kind == 0:
            out += dump(value)
        elif kind == 1:
            out += value + b"\0"
        else:
            out += struct.pack("<i", value)
    return out + b"\x08"

def field(entry, name):
    """The key spelled as in this file (Steam uses AppName, older files appname)."""
    return next((k for k in entry if k.lower() == name.lower().encode()), None)

found = False
for path in glob.glob(os.path.expanduser("~/.local/share/Steam/userdata/*/config/shortcuts.vdf")):
    data = open(path, "rb").read()
    root, _ = parse(data, 0)
    if dump(root) != data:
        sys.exit(f"{path}: unexpected format, left alone")
    changed = False
    for _, entry in root.get(b"shortcuts", (0, {}))[1].values():
        exe = entry.get(field(entry, "Exe"), (1, b""))[1]
        kind = "new" if NEW in exe else "old" if OLD in exe else None
        if kind is None or found:
            continue
        found = True
        flag = field(entry, "OpenVR")
        if mode == "check":
            print(kind, entry[flag][1] if flag else 0)
        elif mode == "art":
            appid = entry.get(field(entry, "appid"), (2, 0))[1] & 0xFFFFFFFF
            grid = os.path.join(os.path.dirname(path), "grid")
            os.makedirs(grid, exist_ok=True)
            for source, name in (("capsule.png", f"{appid}p.png"), ("header.png", f"{appid}.png"),
                                 ("hero.png", f"{appid}_hero.png"), ("logo.png", f"{appid}_logo.png")):
                if os.path.exists(os.path.join(art, source)):
                    shutil.copyfile(os.path.join(art, source), os.path.join(grid, name))
            print(f"Library artwork set for app {appid}.")
        elif mode == "fix":
            # The same entry (appid, play time, artwork) under the new name and path.
            for key, (t, value) in list(entry.items()):
                if t == 1:
                    entry[key] = (1, value.replace(OLD, NEW).replace(OLD_DIR, NEW_DIR))
            entry[field(entry, "AppName") or b"AppName"] = (1, b"VRedze")
            entry[flag or b"OpenVR"] = (2, 1)
            changed = True
    if changed:
        open(path, "wb").write(dump(root))
if not found and mode == "check":
    print("none 0")
PY
}

read -r entry vr < <(shortcuts check)
case "$entry" in
    none)
        steamos-add-to-steam "$launcher"
        echo "Added VRedze to the Steam library (Non-Steam)."
        sleep 2
        read -r entry vr < <(shortcuts check) ;;
    old)
        # Until Steam restarts, the "Just Video" entry starts the new build.
        cat > "$old_launcher" <<SH
#!/bin/sh
exec "$dir/vredze" "\$@"
SH
        chmod +x "$old_launcher"
        echo "Updated VRedze (the library entry is still called Just Video)." ;;
    new)
        echo "Updated VRedze (already in the Steam library)." ;;
esac
shortcuts art

if [ "$entry" = new ] && [ "$vr" = 1 ]; then
    :
elif [ "$RESTART_STEAM" = 1 ]; then
    echo "Restarting Steam to name the library entry VRedze and mark it as a VR app…"
    steam -shutdown >/dev/null 2>&1 || true
    for _ in $(seq 60); do pgrep -x steam >/dev/null || break; sleep 1; done
    shortcuts fix
    echo "Done. Steam restarts on its own; if it doesn't, restart the headset."
elif [ "$entry" = old ]; then
    echo "Note: the library entry is called Just Video until Steam restarts. Run again"
    echo "      with FRAME_RESTART_STEAM=1 to rename it (restarts Steam)."
else
    echo "Note: VRedze isn't marked as a VR app yet, so it starts in the background."
    echo "      Run again with FRAME_RESTART_STEAM=1 to fix (restarts Steam once)."
fi
REMOTE
