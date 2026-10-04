#!/usr/bin/env bash
# Builds the Meta Quest (Android arm64) library, libjust_video.so: the player
# with FFmpeg + dav1d linked in statically (scripts/build-android-media.sh).
# scripts/package-quest.sh then wraps it into an APK.
set -euo pipefail
cd "$(dirname "$0")/.."
media=$PWD/.local-deps/android-media
[ -f "$media/lib/libavcodec.a" ] || { echo "Run scripts/build-android-media.sh first" >&2; exit 1; }
API=${ANDROID_API:-29}
if [ -z "${ANDROID_NDK_HOME:-}" ]; then
  ANDROID_NDK_HOME=$( (ls -d "$HOME"/Library/Android/sdk/ndk/* "$HOME"/Android/Sdk/ndk/* 2>/dev/null || true) | sort -V | tail -1)
fi
case "$(uname -s)" in Darwin) host=darwin-x86_64 ;; *) host=linux-x86_64 ;; esac
TC=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host/bin
[ -x "$TC/aarch64-linux-android$API-clang" ] || { echo "Android NDK not found: set ANDROID_NDK_HOME" >&2; exit 1; }

t=aarch64_linux_android
export "CC_$t=$TC/aarch64-linux-android$API-clang"
export "AR_$t=$TC/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TC/aarch64-linux-android$API-clang"
export PKG_CONFIG_ALLOW_CROSS=1
export "PKG_CONFIG_LIBDIR_$t=$media/lib/pkgconfig"
export "PKG_CONFIG_PATH_$t="
export JUST_VIDEO_STATIC_FFMPEG=1
cargo rustc --release --lib --target aarch64-linux-android --crate-type cdylib "$@"
"$TC/llvm-strip" --strip-debug target/aarch64-linux-android/release/libjust_video.so
ls -l target/aarch64-linux-android/release/libjust_video.so
