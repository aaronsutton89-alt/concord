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

Vulkan support is experimental and opt-in. On the tested RTX 5090 / driver
615.71.09 / FFmpeg 9.0.2 setup, the required constrained-baseline H.264 profile
at level 3.1 fails to initialize in FFmpeg's Vulkan path. A diagnostic probe
with FFmpeg's default High profile succeeded, but that profile does not meet
Concord's required constrained-baseline contract; Main also failed. The Rust
Vulkan round-trip test reproduced the constrained-baseline failure. Vulkan
hardware encoding and live sharing therefore remain unverified. This is a
profile-specific incompatibility in the tested FFmpeg/driver path, not evidence
that the GPU lacks Vulkan Video H.264 support altogether.

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
