#!/usr/bin/env bash
# Isolated diagnostic runtime: does not install or replace system FFmpeg.
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
build_dir="$repo_dir/target/vulkan-runtime-build"
mkdir -p "$build_dir" "$repo_dir/target/vulkan-runtime/lib"
cd "$build_dir"
curl -fsSL https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz -o ffmpeg-9.0.2.tar.xz
curl -fsSL https://github.com/KhronosGroup/Vulkan-Headers/archive/refs/tags/v1.4.357.tar.gz -o vulkan-headers.tar.gz
sha256sum --check <<'SUMS'
8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e  ffmpeg-9.0.2.tar.xz
7dc0dbcf1d49dd3d7da3761c251c6097dfbaac475321a4a8a99269d3d5abecdc  vulkan-headers.tar.gz
SUMS
mkdir -p source headers
tar -xf ffmpeg-9.0.2.tar.xz -C source --strip-components=1
tar -xf vulkan-headers.tar.gz -C headers --strip-components=1
cd source
patch -p1 < "$repo_dir/patches/ffmpeg-9.0.2-vulkan-baseline.patch"
./configure --extra-cflags="-I$build_dir/headers/include" \
    --disable-everything --disable-programs --disable-doc --disable-static \
    --enable-shared --enable-vulkan --enable-encoder=h264_vulkan \
    --enable-decoder=h264 --enable-parser=h264 --enable-protocol=file
make -j"${JOBS:-4}" libavcodec/libavcodec.so libavutil/libavutil.so
# Use the installed FFmpeg 9 libavutil. Only override libavcodec for this launcher.
# This intentionally minimal codec library is for Vulkan diagnostics only.
cp libavcodec/libavcodec.so.63 "$repo_dir/target/vulkan-runtime/lib/"
echo 'Vulkan runtime built. Requires system FFmpeg 9 libavutil.so.61.'
