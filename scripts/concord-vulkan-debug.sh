#!/usr/bin/env bash
# Experimental FFmpeg 9.0.2 Vulkan runtime; select encoder="vulkan" in config.toml.
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
runtime_dir="$repo_dir/target/vulkan-runtime/lib"
binary="$repo_dir/target/release/concord"
if [[ ! -f "$runtime_dir/libavcodec.so.63" || ! -x "$binary" ]]; then
    echo 'Build the Vulkan runtime and Concord release binary first; see docs/hardware-encoding.md.' >&2
    exit 1
fi
export CONCORD_DEBUG=1
export LD_LIBRARY_PATH="$runtime_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$binary" "$@"
