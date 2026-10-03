# Optional Linux FFmpeg screen-share encoders

Concord can optionally use FFmpeg's H.264 encoders for Linux screen sharing.
This integration is opt-in at build time and is not required for the default
build.

## Build requirements

Enable the optional integration when building from source:

```sh
cargo build --release --features ffmpeg-encoding
```

The `ffmpeg-sys-next` 9 bindings use the system FFmpeg development headers and
link to the system `libavcodec` and `libavutil` libraries. Those libraries must
be available when building and when running the resulting binary. The tested
setup was FFmpeg 9.0.2 on Arch Linux with NVIDIA driver 615.71.09 and an RTX
5090. A Rust 1.90 all-features compile check also passed on Alpine 3.22/musl with
FFmpeg 6.1.2; this is build coverage, not a hardware runtime test. Other
drivers and GPUs have not been verified here. Older FFmpeg builds without
a requested encoder fall back to software.

Without `ffmpeg-encoding`, Concord has no FFmpeg dependency. The existing
VA-API and OpenH264 software paths remain available. Enabling the feature adds
FFmpeg-backed NVENC and experimental Vulkan encoder choices; it does not make
either hardware backend mandatory.

## Selecting an encoder

The optional settings are under `[screen_capture]` in `config.toml`:

```toml
[screen_capture]
encoder = "auto"
# device = "0"
```

`encoder` accepts `"auto"`, `"nvenc"`, `"vulkan"`, `"vaapi"`, or
`"software"`. The optional `device` value is a string interpreted by the
selected backend: NVENC uses a nonnegative GPU ordinal, while Vulkan passes the
value to FFmpeg's Vulkan device creation. Leave it unset to let the backend
choose a device. Explicit VA-API device selection is not currently supported;
if a device is set while VA-API is selected, Concord falls back to software.

With `"auto"`, Concord tries NVENC, VA-API, Vulkan, then software. An explicit
hardware choice also falls back to software if initialization fails. The
fallback reason is visible in debug logs; launch with `CONCORD_DEBUG=1` to
inspect them. Invalid values follow Concord's configuration convention: a
warning is logged and the setting defaults to `"auto"`.

The TUI voice header displays the active encoder during a share, for example
`🔴 [NVENC]`, `🔴 [Vulkan]`, or `🔴 [Software]`. This is the selected runtime
backend, not the configured preference; it updates if hardware falls back to
software and clears when the share ends.

Encoder settings are read when screen sharing starts. Saving configuration
through the TUI does not change an already-running share; stop and start the
share to apply new settings. If an encoder fails during a share and Concord
switches to software, that fallback stays in effect for the rest of that share
instead of repeatedly retrying hardware. A new share starts selection again.

## Current verification and limits

On the tested RTX 5090 setup, the Rust NVENC path encoded 90 synthetic frames,
including forced and periodic IDRs and an interval change. Independent decoding
verified the frame count and changing image planes. A user-started whole-desktop
share then transmitted for over 11 minutes at about 30 FPS and 6.3 Mbps wire
bitrate, with 1.2–1.8 ms interval-average encoding time. After transmission
started, the inspected capture intervals had no queue drops or encoder skips;
all 19 requested retransmissions were served. The user confirmed that the
viewer picture and audio looked correct. These observations apply to this host;
a controlled CPU comparison against software encoding remains pending.

Vulkan support is experimental and opt-in. On FFmpeg 9.0.2, the H.264 Vulkan
profile mapping recognized `AV_PROFILE_H264_CONSTRAINED_BASELINE` but omitted
`AV_PROFILE_H264_BASELINE`, the profile value stored in the constrained-baseline
SPS. Adding the missing baseline mapping in `libavcodec/vulkan_video.c` allowed
the Rust Vulkan round-trip test to encode and independently decode 90 changing
frames, including forced and periodic IDRs and an interval change; the run took
781 ms. The corresponding FFmpeg 9.0.2 source is
[`vulkan_video.c`](https://github.com/FFmpeg/FFmpeg/blob/n9.0.2/libavcodec/vulkan_video.c).

The isolated runtime diagnostic uses the patched `libavcodec.so.63` with the
system `libavutil`, without installing or replacing system libraries. Its
minimal FFmpeg runtime supports only `h264_vulkan`; this does not validate the
normal automatic or NVENC runtime. A live screen share of Deadlock selected `backend=vulkan`. Over the first 65 seconds, steady-state transmission was 29.9–30.2 FPS with 0.8–0.9 ms interval-average encoding time, zero capture queue drops or encoder skips after the first interval, and zero screen-share audio capture drops. The first interval had 32 startup queue drops. The user confirmed correct viewer picture and audio. Longer-duration and repeated stop/start validation remain open; Vulkan remains experimental and requires this workaround on the tested installation.
For a user test, explicitly select `encoder = "vulkan"` under `[screen_capture]`
and restart the share; do not use automatic selection to validate Vulkan.

FFmpeg's native library diagnostics are quiet to avoid cluttering the terminal.
Concord logs returned operation and error strings at debug level; use
`CONCORD_DEBUG=1` to inspect those messages.

The hardware-only round-trip tests are ignored by default because they require
appropriate hardware and runtime support. To run either test explicitly:

```sh
cargo test --features ffmpeg-encoding --lib nvenc_round_trip -- --ignored --nocapture
cargo test --features ffmpeg-encoding --lib vulkan_round_trip -- --ignored --nocapture
```

These tests are named `discord::voice::capture::encoder::ffmpeg::tests::nvenc_round_trip`
and `discord::voice::capture::encoder::ffmpeg::tests::vulkan_round_trip`. A test
must actually initialize the requested backend; a software fallback is not
evidence that the corresponding hardware test passed.

## Experimental FFmpeg 9.0.2 Baseline workaround

On a host with system FFmpeg 9 (`libavutil.so.61`), the following builds the
isolated Vulkan codec library from pinned sources and launches the existing
feature-enabled Concord binary:

```sh
scripts/build-vulkan-runtime.sh
cargo build --locked --release --features ffmpeg-encoding
# Set [screen_capture] encoder = "vulkan" in your Concord config first.
scripts/concord-vulkan-debug.sh
```

The build script requires a C compiler, make, pkg-config, nasm, curl, tar and
patch. It downloads FFmpeg 9.0.2 and Vulkan headers into `target/`, checks their
SHA-256 hashes, and applies `patches/ffmpeg-9.0.2-vulkan-baseline.patch`.
It does not run sudo or install anything system-wide. The launcher sets
`CONCORD_DEBUG=1` and a process-local `LD_LIBRARY_PATH` to find the patched
codec library. Its child processes inherit that library path as well; do not
use this minimal runtime for unrelated FFmpeg operations.

For the live check, close the previous Concord instance and launch this script,
share Deadlock, and verify `backend=vulkan` in the debug log. Confirm moving
picture and synchronized audio at the viewer, then stop and restart the share.
A fallback to software must not count as Vulkan success. To return to the
normal NVENC setup, restore `encoder = "auto"` and launch Concord normally.
The system FFmpeg installation remains unchanged.
