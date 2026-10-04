#!/usr/bin/env bash
# Wraps the Quest library (scripts/build-quest.sh) into a signed APK:
# target/quest/JustVideo.apk (the icon comes from android/res). Needs the Android SDK build-tools and a
# platform android.jar; downloads the Khronos OpenXR loader once (checked
# against Maven Central's SHA-1) into .local-deps/openxr-loader.
set -euo pipefail
cd "$(dirname "$0")/.."
LOADER=1.1.63
sdk=${ANDROID_HOME:-$HOME/Library/Android/sdk}
tools=$(ls -d "$sdk"/build-tools/* | sort -V | tail -1)
jar=$(ls -d "$sdk"/platforms/android-*/android.jar | sort -V | tail -1)
lib=target/aarch64-linux-android/release/libjust_video.so
[ -f "$lib" ] || { echo "Build first: scripts/build-quest.sh" >&2; exit 1; }

loader_dir=.local-deps/openxr-loader
loader=$loader_dir/libopenxr_loader.so
if [ ! -f "$loader" ]; then
  mkdir -p "$loader_dir"
  base=https://repo1.maven.org/maven2/org/khronos/openxr/openxr_loader_for_android/$LOADER
  aar=$loader_dir/openxr_loader_for_android-$LOADER.aar
  curl -sfL "$base/openxr_loader_for_android-$LOADER.aar" -o "$aar"
  expected=$(curl -sfL "$base/openxr_loader_for_android-$LOADER.aar.sha1")
  actual=$(shasum -a 1 "$aar" | cut -d' ' -f1)
  [ "$expected" = "$actual" ] || { echo "OpenXR loader checksum mismatch" >&2; rm -f "$aar"; exit 1; }
  unzip -p "$aar" prefab/modules/openxr_loader/libs/android.arm64-v8a/libopenxr_loader.so > "$loader" ||
    unzip -p "$aar" jni/arm64-v8a/libopenxr_loader.so > "$loader"
  [ -s "$loader" ] || { echo "libopenxr_loader.so not found in the AAR" >&2; exit 1; }
fi

out=target/quest
stage=$out/stage
rm -rf "$stage" && mkdir -p "$stage/lib/arm64-v8a"
cp "$lib" "$loader" "$stage/lib/arm64-v8a/"
"$tools/aapt2" compile --dir android/res -o "$out/res.zip"
"$tools/aapt2" link -o "$out/unsigned.apk" --manifest android/AndroidManifest.xml -I "$jar" \
  -R "$out/res.zip" --auto-add-overlay
# Libraries go in uncompressed so Android can map them straight from the APK.
(cd "$stage" && zip -q -0 -r ../unsigned.apk lib)
"$tools/zipalign" -f -P 16 4 "$out/unsigned.apk" "$out/aligned.apk"
"$tools/apksigner" sign --ks "$HOME/.android/debug.keystore" --ks-pass pass:android \
  --key-pass pass:android --ks-key-alias androiddebugkey --out "$out/JustVideo.apk" "$out/aligned.apk"
rm -f "$out/unsigned.apk" "$out/aligned.apk" "$out/res.zip"
ls -l "$out/JustVideo.apk"
