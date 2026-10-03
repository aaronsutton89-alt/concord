# Concord Linux NVENC and Vulkan Video encoding plan

Created: 2026-10-03. Status: implementation in progress on `feat/linux-hardware-encoding`; NVENC synthetic hardware validation passed, Vulkan constrained-baseline initialization is blocked on the tested FFmpeg/driver combination.


## Implementation record (2026-10-03)

- Personal fork: https://github.com/aaronsutton89-alt/concord (`origin`); original repository is `upstream`.
- Updated implementation baseline to upstream `5bac7a2` (v2.6.1). Preserved upstream KLIPY configuration while resolving additive config conflicts.
- Chosen integration: `ffmpeg-sys-next` 9.0.0 (generated FFI bindings, WTFPL), dynamically linked to system libavcodec/libavutil. Native driver loading, session resources and Vulkan synchronization are handled by FFmpeg. The Cargo feature `ffmpeg-encoding` is opt-in; ordinary builds keep the existing dependency set. Tested libraries are FFmpeg 9.0.2. Feature-enabled binaries require their linked FFmpeg runtime libraries; missing GPU drivers are recoverable, missing linked FFmpeg libraries are a loader error.
- Added config section `[screen_capture]`, enum selection, ordered startup attempts, optional device selector, and sticky software fallback across interval changes. Configuration follows existing tolerant parsing: invalid values warn/default rather than fail startup.
- NVENC and Vulkan share a small FFI adapter at `src/discord/voice/capture/encoder/ffmpeg.rs`. It uses CPU NV12 upload, explicit no-delay output checks, Constrained Baseline Level 3.1 verification, manual IDR cadence, runtime forced-IDR validation, and RAII cleanup. The capture/transport pipeline stays unchanged.
- Device selection limitation: NVENC currently accepts an explicit nonnegative ordinal, Vulkan uses FFmpeg's selector, and VA-API rejects an explicit selector and falls back to software. Stable UUID selection remains deferred and is not claimed complete. Omit device for automatic selection.
- Main-model work: dependency selection, GPU adapter/unsafe code, selection integration, upstream integration, final review. GPT-6 Luna tasks: config and preservation tests, selection tests, user documentation.
- Synthetic CLI NVENC probe: 60 frames at 1280×720, 30 FPS, 6 Mbps; independent FFmpeg decode passed and ffprobe reported Constrained Baseline level 31.
- Actual Rust NVENC adapter test: 90 changing synthetic frames, initial/forced/periodic IDRs and interval change; independent decode, frame count, luma and chroma checks passed. First debug run took 867 ms including test image generation, which is not a live-stream benchmark.
- Vulkan diagnostics: Constrained Baseline and Main fail during parameter feedback (`Unable to get feedback for H.264 units = 0`) on RTX 5090 / 615.71.09 / FFmpeg 9.0.2. High profile works in a CLI diagnostic, so Vulkan encode support exists but the required profile path fails. Keep the compatibility requirement; never silently substitute High. Actual Rust Vulkan test correctly fails without accepting software fallback. Vulkan remains experimental/unverified for compatible-profile encoding.
- All-features Clippy passed. Full all-features suite outside the sandbox passed: 1,861 library tests and one binary test, three hardware tests ignored. The first sandbox run had 13 failures from blocked localhost sockets; these passed with appropriate access.
- No-default-features Clippy passed. Default-build selection tests passed (three), including unavailable NVENC fallback.
- Release build passed and `target/release/concord --version` reported 2.6.1. Rust 1.90/musl all-features compilation passed in the CI Alpine 3.22 container against FFmpeg 6.1.2. Native macOS/Windows validation and live Discord receiver checks are still pending. No desktop capture or messages to other people have been performed.
- Live sender validation: user started a whole-desktop share with the release binary. Logs confirmed `backend=nvenc`, about 30 FPS captured/queued/sent, 6 Mbps encoded / 6.3 Mbps on the wire, around 1.2 ms mean encode time, audio packets and receiver feedback. Recent intervals had zero encoder skips/queue drops; requested retransmissions succeeded. Viewer picture/audio-sync confirmation and a software comparison remain pending.
- First remote CI found an existing `AtomicU32::fetch_update` deprecation on newer stable Rust. Added a narrowly scoped compatibility allowance to `take_nonce`, retaining the Rust 1.90 API and unchanged encryption nonce behavior. Remote checks are being rerun.
- User guide: [hardware-encoding.md](hardware-encoding.md).

## Objective and scope

Add reliable H.264 hardware encoding for Linux screen sharing through NVIDIA NVENC and Vulkan Video. Retain VA-API and OpenH264 fallback. Deliver NVENC first, then Vulkan Video, with each independently testable. This plan is the persistent handoff for this Codex session and later coding sessions.

First release keeps the existing capture, resizing, CPU frame conversion, stream transport, and 1280×720 at 30 FPS / 6 Mbps target. Hardware encoding alone will not remove CPU capture and conversion costs. GPU buffer sharing and GPU scaling are a later optimization, after profiling. AV1, HEVC, higher resolution/FPS, and new Windows/macOS encoders are outside this first implementation.

## Verified starting point

- Checkout: `/home/asutton-noct/Documents/Codex/2026-09-27/does/work/concord-review`.
- Origin: `https://github.com/chojs23/concord.git`.
- Inspected revision: `72814de`, release v2.6.0. Initial working tree was clean. Upstream freshness was not checked; rebase the plan on the chosen implementation revision before coding.
- `Cargo.toml` specifies Rust 1.90, edition 2024. CONTRIBUTING.md still mentions 1.85; follow the manifest and verify any new dependency against 1.90.
- `src/discord/voice/capture/encoder.rs` owns `StreamEncoder`, `StreamEncoderBackend`, `I420Frame`, and `EncodedH264Frame`. Linux currently tries VA-API, then OpenH264. The VA-API module is inline in this file.
- Existing hardware startup probes validate H.264 output; runtime hardware failure recreates OpenH264 and validates a recovery IDR with SPS/PPS. Preserve these guarantees.
- `capture.rs` prepares I420 frames, applies bounded queue backpressure, requests keyframes, and reconstructs the encoder when the requested keyframe interval changes. Current baseline is 720p30, 6 Mbps, two-second nominal keyframe interval.
- `capture/encoder/h264.rs` normalizes Annex B output for WebRTC. `broadcast.rs` advertises H264. Verify actual profile constraints across this path before configuring a new backend; existing VA-API uses Baseline level 3.1.
- `capture/linux/portal.rs` and `capture/linux/egl.rs` contain PipeWire capture and DMA-BUF handling. An existing capture path using GPU buffers does not imply that encoding is GPU accelerated.
- Host GPU query outside the sandbox: NVIDIA GeForce RTX 5090, driver 615.71.09. The same query inside the sandbox failed to communicate with the driver.
- Installed FFmpeg lists `h264_nvenc` and `h264_vulkan`. This establishes compiled support only; neither encoder was exercised and Vulkan Video capability remains unverified.

## Architecture decisions

### Encoder contract

Extend the existing backend enum with Linux-only NVENC and Vulkan variants. Keep its small synchronous interface until a measured need requires changing it. Inputs remain borrowed I420 planes with explicit strides. Outputs remain one complete Annex B H.264 access unit per returned frame, normalized through `EncodedH264Frame::new`.

Use low-delay encoding without B frames or lookahead. Keep frame association correct: capture currently assigns RTP time when encoding returns, so queued/delayed output cannot be silently substituted into this interface. If a backend cannot meet this contract, carry input timestamps through the encoder and update the transport boundary before enabling it. Distinguish skipped output from errors, and reject indefinitely buffering encoders during startup.

Share checked I420-to-NV12 conversion where suitable. Validate lengths, pitches, offsets, overflow, and device alignment. Own device resources on the encoding worker and document every unsafe FFI lifetime. Bound buffer pools and waits, clean up partial initialization, and preserve the stop path. Driver hangs may require process isolation; a thread timeout alone cannot safely cancel an arbitrary blocked driver call.

### Integration approach and decision gate

Preferred fit is in-process backends matching the current native encoder design: an audited NVENC binding with runtime-loaded driver APIs, and an audited Vulkan Video wrapper or a narrowly contained implementation using Vulkan bindings. Do not handwrite large C ABI structures or choose a crate based only on its name.

Before implementing, compare candidate bindings against: maintenance, license compatibility with this GPL-3.0-only project, Rust 1.90, Linux build targets, force-IDR support, SPS/PPS access, low-delay output, resource ownership, driver version compatibility, and packaging. Record the selected dependency/version and a working synthetic-frame proof here.

FFmpeg libavcodec is the alternative if it substantially reduces risk for both encoders and its library/ABI/distribution requirements are acceptable. FFmpeg provides H.264 Vulkan encoding, but system FFmpeg availability is not a reason to make it an undeclared dependency. If chosen, document minimum tested versions, feature checks, hardware frame uploads, send/receive semantics, and package changes. A long-running FFmpeg CLI subprocess is useful for a diagnostic prototype; do not ship it without solving frame boundaries, runtime keyframe requests, pipe backpressure, stderr drainage, timeouts, and child cleanup. Never spawn an encoder process per frame.

### Backend selection and failure policy

Add a documented preference, using existing project configuration conventions after locating them: `auto`, `nvenc`, `vulkan`, `vaapi`, `software`. Add an optional stable device identifier for multi-GPU systems; avoid relying solely on enumeration order.

Proposed automatic startup order:

1. NVENC on an NVIDIA device that passes the actual encode probe.
2. Existing VA-API on compatible devices, preserving the established non-NVIDIA path.
3. Vulkan Video on devices that pass the required H.264 probe.
4. OpenH264.

An explicit hardware preference attempts that backend, then falls back visibly to software if unavailable. Tests need a strict constructor that reports failure rather than counting fallback as hardware success. Invalid preference values produce a clear configuration error.

Log selected backend/device and concise rejection reasons, distinguishing missing libraries, incompatible API versions, unsupported profiles/formats, unavailable device access, probe failures, and runtime failures. Keep debug logs free of authentication data and raw screen contents.

On runtime failure, use the existing direct OpenH264 recovery with a parameterized IDR. Remember the failed backend for this share so keyframe-interval reconstruction does not repeatedly retry it. Reset that suppression on a new share. Startup may walk the full candidate list; avoid cycling through hardware backends during an active failure.

## Implementation milestones

### 0. Establish the implementation baseline

- [x] Read applicable AGENTS.md instructions and CONTRIBUTING.md; inspect Git status and current revision. Preserve unrelated work.
- [x] Confirm whether this review checkout or a fresh development checkout is the intended code location; use a feature branch when implementation starts.
- [x] Inspect settings conventions, H.264 negotiation/normalization, device discovery, capture shutdown, and CI/release targets.
- [ ] Record a software baseline using a repeatable desktop workload: startup time, CPU, capture/prepare/encode timing, output rate, receiver quality and latency.
- [ ] Run synthetic NVENC and Vulkan H.264 probes with device access. Do not capture the user's desktop for a synthetic capability test. Record exact versions, device, command, output, and decode result.
- [ ] Complete the dependency decision above. Validate native library absence is recoverable and hardware is not required to compile or run ordinary tests.

Exit: selected binding strategy, verified development hardware, known receiver requirements, recorded baseline. A failed Vulkan probe leaves Vulkan support pending rather than silently declaring success through NVENC.

### 1. Refactor selection with existing behavior covered

Primary file: `src/discord/voice/capture/encoder.rs`; settings and config tests at locations found in milestone 0.

- [x] Generalize one-hardware selection into a lazy ordered factory list. Do not construct all devices eagerly.
- [ ] Add backend preference, structured failure reporting, device choice, and per-share failed-backend suppression.
- [x] Keep OpenH264 recovery and startup H.264 validation shared across backends.
- [ ] Add dependency-free tests for ordering, missing backends, explicit preferences, complete failure, software selection, and failure suppression across encoder reconstruction.

Exit: existing VA-API/software behavior passes tests with no new GPU dependency required at runtime.

### 2. Implement NVENC H.264

Proposed file: `src/discord/voice/capture/encoder/nvenc.rs`; register in `encoder.rs`.

- [x] Load required NVIDIA runtime libraries and check supported API version before creating a session. Discover a usable GPU and H.264 input/profile capabilities.
- [ ] Prefer a CUDA device context for the initial native Linux path; prove context creation, upload, and cleanup with the selected bindings. CUDA context use does not itself mean GPU color conversion is implemented.
- [x] Configure negotiated H.264 profile/level, 720p30, 6 Mbps CBR, low latency, no B frames/lookahead, and the requested keyframe interval. Query support instead of assuming every NVIDIA GPU has equivalent features.
- [x] Upload stride-correct input, encode, collect complete output, derive keyframe status from the bitstream, and normalize Annex B.
- [x] Implement forced IDR and SPS/PPS emission for stream startup, receiver recovery, and encoder reconstruction. Avoid emitting synthetic startup probe frames into the live stream.
- [ ] Bound pending work and release buffers, bitstream locks, sessions, and context on failure and normal stop.
- [ ] Pass synthetic decode tests and a real screen share on the RTX 5090. Confirm logs actually say NVENC and GPU encoder activity corroborates them.

Exit: NVENC works end to end; missing NVIDIA libraries or simulated hardware errors produce a decodable software fallback.

### 3. Implement Vulkan Video H.264

Proposed file: `src/discord/voice/capture/encoder/vulkan.rs`; register in `encoder.rs`.

Vulkan rendering support alone is insufficient. Require the relevant video queue, video encode queue, and H.264 encode extensions/features, plus usable queue families, profile capabilities, formats, extent/alignment limits, and rate-control capabilities. The wrapper may handle parts of this, but capability failures still need useful diagnostics.

- [ ] Select a physical device with actual H.264 encode support and record its stable identity.
- [ ] Create video session, parameter sets, required memory, input images, reference-picture storage, and bitstream buffers according to queried requirements.
- [ ] Upload I420/NV12 through supported formats; implement explicit layout transitions, synchronization, queue ownership, and safe resource reuse.
- [ ] Generate compatible SPS/PPS, IDR and subsequent reference frames; honor periodic and forced IDRs. If low-delay CBR/profile requirements cannot be met, reject the backend with a reason.
- [ ] Read completed bitstreams only after GPU completion; validate size/offsets and normalize output.
- [ ] Test unsupported-device and device-loss behavior. Use Vulkan validation layers during development and fix lifetime/synchronization findings.
- [ ] Pass synthetic decode and actual Discord receiver tests on a verified Vulkan Video device. Record which vendor/driver was tested; do not claim untested vendors are verified.

Exit: Vulkan Video independently passes the same contract as NVENC, with reliable fallback. Keep experimental/opt-in status until that gate passes.

### 4. Packaging, documentation, and release readiness

Files: `Cargo.toml`, `Cargo.lock`, `README.md`, relevant `docs/`, `.github/workflows/ci.yml`, release workflow, `flake.nix`/`nix/`, and `dist-workspace.toml` as required by the chosen dependency.

- [x] Keep dependencies optional and Linux gated. Decide whether backend features join default `stream-broadcast` only after build and packaging validation.
- [x] Document build dependencies separately from runtime driver requirements, backend selection, fallback messages, and diagnostics.
- [ ] Preserve no-default-features builds, Linux musl build coverage, and macOS/Windows native encoders. Do not assume NVIDIA proprietary runtime support on musl; unsupported combinations must build and fall back gracefully or have explicit feature constraints.
- [ ] Review relevant changes and run project gates below. Hardware tests should be explicitly opt-in and report skips honestly on ordinary CI.
- [ ] Update this plan with completed boxes, selected versions, commands/results, remaining limitations, and the next concrete task.

## Validation and acceptance criteria

### Automated checks

Use deterministic synthetic inputs and injected backend failures to cover backend selection, conversion with padded strides, malformed frames, forced IDRs, SPS/PPS availability, Annex B framing, and fallback continuity. Decode multiple output frames with an independent H.264 decoder; include color bars and changing frame content so broken plane layout and stale frames are detected. Verify no frame reordering, bounded buffering, and correct timestamp association.

Hardware tests must fail if the requested backend falls back. Mark device-requiring tests opt-in; record real hardware runs separately from ordinary unit test results.

Required project checks for implementation:

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo clippy --locked --all-targets --no-default-features -- -D warnings
cargo test --locked --all-features
cargo build --locked --release
```

Also run applicable musl and native platform CI jobs; run `cargo dist plan` when release configuration changes. These checks were read from repository guidance/CI, not executed during this documentation-only session.

### Hardware and receiver matrix

| Scenario | Required result |
| --- | --- |
| RTX 5090, explicit NVENC | Correct backend; startup and forced-IDR decode; stable live share |
| Verified Vulkan Video device, explicit Vulkan | Actual Vulkan encoder used; same decode/recovery guarantees |
| Existing VA-API machine | No regression; automatic hardware selection still works |
| No hardware libraries / no accessible GPU | App starts and screen share uses OpenH264 |
| Injected encoder runtime failure | Software resumes with SPS/PPS + IDR; no repeated hardware retry loop |
| Stop/restart, cancellation during startup, interval changes | Prompt cleanup; no growing session/resource count |
| Wayland portal and X11 capture | Correct colors, frame updates, cursor behavior, unchanged capture permissions |
| Discord receiver joins/rejoins or requests keyframe | Playable H.264, recovery on next output frame, audio remains synchronized |

Run a 10-minute representative screen share and repeated start/stop cycles per implemented backend. Compare identical resolution, frame rate, bitrate, and content against the recorded software baseline. Report CPU utilization, encode time (including upload), achieved FPS, bitrate, memory trend, receiver latency and visible quality. Target sustained 30 FPS when the source supplies it, p95 encode time below the 33.3 ms frame budget, and a measurable CPU improvement. Treat these as acceptance targets requiring evidence, not predictions. If capture/conversion dominates, quantify that limitation before considering GPU frame sharing.

A feature is complete only when its real hardware path, receiver compatibility, graceful fallback, relevant builds, and documentation are verified. NVENC completion does not imply Vulkan completion.

## Later optimization

After both correctness gates, profile whether GPU scaling/color conversion and PipeWire DMA-BUF import are worthwhile. This needs explicit ownership of file descriptors, format modifiers, synchronization fences, buffer lifetime, cross-device copies, and a CPU fallback. Plan it separately; do not couple the first encoder implementation to a capture rewrite.

## Model choice for coding

### Authorized delegation policy

The user explicitly authorized this policy on 2026-10-03:

> When implementing the Concord plan, automatically delegate bounded settings, diagnostics, documentation, and unit-test tasks to GPT-6 Luna. Keep architecture, GPU integration, unsafe code, synchronization, and final review on the main model. Avoid delegation when its overhead outweighs the work.

Apply this without requesting permission for each delegation. Give each subagent a concrete independent task, limited context, owned files, and acceptance criteria. Use `gpt-6-luna` explicitly. The main agent reviews and integrates results. This authorizes delegation during implementation; it does not itself start implementation or change the main session model.


Use the stronger model for dependency selection, unsafe FFI, NVENC/Vulkan resource lifetimes, synchronization, bitstream compatibility, and difficult hardware failures. A cheaper model is reasonable for bounded tasks after these interfaces are established: settings plumbing, diagnostics, documentation, selection unit tests, and small mechanical refactors. Give it one milestone, exact files, acceptance tests, and a stop condition for architecture changes. Have the stronger model review the integration and unsafe code.

Official OpenAI guidance recommends choosing models against task complexity and validating cost/quality tradeoffs. The task-specific split above is an engineering recommendation, not a guarantee of any model's correctness. Use available models in the session's picker; availability and plan limits were not checked. Do not change models automatically as part of this plan.

## Sources

- Local source revision `72814de`, especially encoder.rs, capture.rs, broadcast.rs, Cargo.toml and CI.
- [NVIDIA NVENC programming guide](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/index.html): runtime APIs, device contexts, capability queries, low-latency configuration.
- [Khronos Vulkan H.264 encode extension](https://docs.vulkan.org/features/latest/features/proposals/VK_KHR_video_encode_h264.html): encoding capabilities and session parameters.
- [FFmpeg H.264 Vulkan encoder implementation](https://ffmpeg.org/pipermail/ffmpeg-cvslog/2024-September/145404.html): alternative integration reference; verify current API/version before adoption.
- [OpenAI model selection guidance](https://developers.openai.com/api/docs/guides/model-selection).

## Resume instruction

Read this file and applicable AGENTS.md guidance. Recheck the checkout revision/status, then begin the first incomplete milestone. Keep the checklist and evidence current. Implement NVENC before Vulkan Video. Preserve existing capture/transport behavior and software recovery. Record unavailable hardware tests as unverified; do not mark a backend complete because software fallback succeeds.
