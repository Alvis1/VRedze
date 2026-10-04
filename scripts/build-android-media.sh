#!/usr/bin/env bash
# Builds the media stack bundled into the Quest (Android arm64) library: static
# dav1d and a minimal static FFmpeg with MediaCodec decoders, cross-compiled
# with the Android NDK. Output: .local-deps/android-media.
# FFmpeg's MediaCodec support needs --enable-jni (only jni.h at build time);
# at run time the player sets the JavaVM and opens decoders with ndk_codec=1.
set -euo pipefail
cd "$(dirname "$0")/.."
FFMPEG=8.1.3 DAV1D=1.5.4 API=${ANDROID_API:-29}
deps=$PWD/.local-deps
prefix=$deps/android-media src=$deps/src build=$deps/build-android
jobs=$(getconf _NPROCESSORS_ONLN)

if [ -z "${ANDROID_NDK_HOME:-}" ]; then
  ANDROID_NDK_HOME=$( (ls -d "$HOME"/Library/Android/sdk/ndk/* "$HOME"/Android/Sdk/ndk/* 2>/dev/null || true) | sort -V | tail -1)
fi
[ -d "$ANDROID_NDK_HOME" ] || { echo "Android NDK not found: set ANDROID_NDK_HOME" >&2; exit 1; }
case "$(uname -s)" in Darwin) host=darwin-x86_64 ;; *) host=linux-x86_64 ;; esac  # macOS binaries are universal
TC=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host/bin
CC=$TC/aarch64-linux-android$API-clang
[ -x "$CC" ] || { echo "Missing $CC" >&2; exit 1; }
echo "NDK: $ANDROID_NDK_HOME (API $API)"

# meson/ninja for dav1d, shared with the Frame build.
if [ ! -x "$deps/buildtools/bin/meson" ] || [ ! -x "$deps/buildtools/bin/ninja" ]; then
  if command -v uv >/dev/null; then
    uv venv -q --allow-existing "$deps/buildtools" && uv pip install -q --python "$deps/buildtools" meson ninja
  else
    python3 -m venv "$deps/buildtools" && "$deps/buildtools/bin/pip" install -q meson ninja
  fi
fi

mkdir -p "$src" "$build"
[ -d "$src/dav1d-$DAV1D" ] || curl -sfL "https://downloads.videolan.org/pub/videolan/dav1d/$DAV1D/dav1d-$DAV1D.tar.xz" | tar xJ -C "$src"
# A pristine FFmpeg tree of its own: the Frame build configures in-tree, which
# blocks out-of-tree builds of the same sources.
ffsrc=$deps/src-android/ffmpeg-$FFMPEG
if [ ! -d "$ffsrc" ]; then
  mkdir -p "$deps/src-android"
  curl -sfL "https://ffmpeg.org/releases/ffmpeg-$FFMPEG.tar.xz" | tar xJ -C "$deps/src-android"
  for patch in "$PWD"/third_party/ffmpeg-patches/*.patch; do
    patch -d "$ffsrc" -p1 < "$patch"
  done
fi

cat > "$deps/android-arm64.ini" <<INI
[binaries]
c = '$CC'
cpp = '$TC/aarch64-linux-android$API-clang++'
ar = '$TC/llvm-ar'
strip = '$TC/llvm-strip'
pkg-config = 'pkg-config'

[properties]
needs_exe_wrapper = true

[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'armv8-a'
endian = 'little'
INI

(export PATH="$deps/buildtools/bin:$PATH" && rm -rf "$build/dav1d" &&
  meson setup "$build/dav1d" "$src/dav1d-$DAV1D" --cross-file "$deps/android-arm64.ini" \
    --prefix="$prefix" --libdir=lib --buildtype=release --default-library=static \
    -Denable_tools=false -Denable_tests=false -Denable_asm=true &&
  ninja -C "$build/dav1d" install)

rm -rf "$build/ffmpeg" && mkdir -p "$build/ffmpeg" && cd "$build/ffmpeg"
PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig" PKG_CONFIG_PATH= "$ffsrc/configure" --prefix="$prefix" \
  --enable-cross-compile --target-os=android --arch=aarch64 --cpu=armv8-a \
  --cc="$CC" --cxx="$TC/aarch64-linux-android$API-clang++" \
  --ar="$TC/llvm-ar" --ranlib="$TC/llvm-ranlib" --nm="$TC/llvm-nm" --strip="$TC/llvm-strip" \
  --pkg-config=pkg-config --pkg-config-flags=--static \
  --enable-static --disable-shared --enable-pic --disable-debug --disable-doc --disable-programs \
  --disable-network --disable-autodetect --enable-zlib --disable-avdevice --disable-avfilter --disable-swscale \
  --enable-swresample --disable-everything --enable-libdav1d --enable-jni --enable-mediacodec \
  --enable-decoder=h264,hevc,vp9,libdav1d,h264_mediacodec,hevc_mediacodec,vp9_mediacodec,av1_mediacodec,aac,aac_latm,ac3,eac3,opus,flac,mp3,mp2,vorbis,dca,truehd,alac,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le,ass,ssa,subrip,srt,webvtt,movtext,text,dvdsub,pgssub,dvbsub \
  --enable-demuxer=mov,matroska,mpegts,avi \
  --enable-parser=h264,hevc,vp9,av1,aac,aac_latm,ac3,opus,flac,mpegaudio,dca,vorbis \
  --enable-bsf=h264_mp4toannexb,hevc_mp4toannexb,vp9_superframe_split,extract_extradata,av1_frame_split \
  --extra-cflags="-O3"
make -j"$jobs"
make install
echo "Android media stack installed in $prefix"
