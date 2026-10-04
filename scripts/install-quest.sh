#!/usr/bin/env bash
# Installs target/quest/SpatialPlayer.apk on the Quest connected over USB
# (Developer Mode on, USB debugging allowed), grants its permissions, starts it
# and follows its log. QUEST_LOG=0 skips the log. Uses SideQuest's adb if there
# is no adb on PATH.
set -euo pipefail
cd "$(dirname "$0")/.."
apk=target/quest/SpatialPlayer.apk
package=com.spatialplayer.app
[ -f "$apk" ] || { echo "Package first: scripts/package-quest.sh" >&2; exit 1; }
adb=$(command -v adb || true)
for candidate in "$HOME/Library/Application Support/SideQuest/platform-tools/adb" \
                 "$HOME/Library/Android/sdk/platform-tools/adb"; do
  [ -n "$adb" ] || { [ -x "$candidate" ] && adb=$candidate; }
done
[ -n "$adb" ] || { echo "adb not found" >&2; exit 1; }
state=$("$adb" get-state 2>&1 || true)
[ "$state" = device ] || { echo "Quest not ready ($state): put it on and allow USB debugging" >&2; exit 1; }
# -g grants the runtime permissions (reading videos) at install.
"$adb" install -r -g "$apk"
"$adb" shell am force-stop "$package"
"$adb" logcat -c
"$adb" shell am start -n "$package/android.app.NativeActivity"
if [ "${QUEST_LOG:-1}" = 1 ]; then
  sleep 2
  pid=$("$adb" shell pidof -s "$package" || true)
  echo "Following the log (Ctrl-C stops)…"
  if [ -n "$pid" ]; then
    exec "$adb" logcat --pid="$pid" -v brief
  else
    exec "$adb" logcat -v brief -s JustVideo AndroidRuntime DEBUG
  fi
fi
