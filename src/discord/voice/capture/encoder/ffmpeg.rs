//! Optional system-libavcodec backends. All native objects stay on the capture thread.
use super::{
    EncodedH264Frame, I420Frame, STREAM_CAPTURE_FPS, STREAM_CAPTURE_HEIGHT, STREAM_CAPTURE_WIDTH,
    STREAM_ENCODER_BITRATE, annex_b_contains_idr, copy_i420_to_nv12,
    validate_parameterized_h264_idr,
};
use ffmpeg_sys_next as av;
use std::{
    ffi::{CStr, CString},
    ptr,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum Kind {
    Nvenc,
    Vulkan,
}
impl Kind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Nvenc => "nvenc",
            Self::Vulkan => "vulkan",
        }
    }
}

pub(super) struct FfmpegEncoder {
    context: *mut av::AVCodecContext,
    input: *mut av::AVFrame,
    hardware: *mut av::AVFrame,
    packet: *mut av::AVPacket,
    device: *mut av::AVBufferRef,
    frames: *mut av::AVBufferRef,
    index: i64,
    kind: Kind,
    interval: u32,
    since_idr: u32,
}

fn check(code: i32, operation: &str) -> Result<(), String> {
    if code >= 0 {
        return Ok(());
    }
    let mut buffer = [0 as std::ffi::c_char; 256];
    // SAFETY: buffer is writable and its length is passed to FFmpeg.
    unsafe {
        av::av_strerror(code, buffer.as_mut_ptr(), buffer.len());
        Err(format!(
            "{operation}: {} ({code})",
            CStr::from_ptr(buffer.as_ptr()).to_string_lossy()
        ))
    }
}

impl FfmpegEncoder {
    pub(super) fn new(kind: Kind, interval: u32, device: Option<&str>) -> Result<Self, String> {
        let mut probe = Self::create(kind, interval, device)?;
        let width = STREAM_CAPTURE_WIDTH as usize;
        let height = STREAM_CAPTURE_HEIGHT as usize;
        let y = vec![16; width * height];
        let uv = vec![128; width * height / 4];
        let frame = I420Frame::new(&y, &uv, &uv, width, height, width, width / 2, width / 2);
        let encoded = probe
            .encode(frame, true)?
            .ok_or("hardware startup probe returned no frame")?;
        validate_parameterized_h264_idr(&encoded, kind.name())?;
        validate_profile(&encoded.annex_b)?;
        drop(probe);
        // Never expose probe reference pictures to the live stream.
        Self::create(kind, interval, device)
    }

    fn create(kind: Kind, interval: u32, device: Option<&str>) -> Result<Self, String> {
        if interval == 0 {
            return Err("keyframe interval must be positive".into());
        }
        // libavcodec otherwise writes directly over the terminal UI. This is
        // Concord's only libavcodec consumer; operation errors are returned below.
        static LOG_INIT: std::sync::Once = std::sync::Once::new();
        LOG_INIT.call_once(|| {
            // SAFETY: setting the process-wide log level requires no context.
            unsafe {
                av::av_log_set_level(av::AV_LOG_QUIET);
            }
        });
        let name = match kind {
            Kind::Nvenc => c"h264_nvenc",
            Kind::Vulkan => c"h264_vulkan",
        };
        // SAFETY: codec names are static NUL-terminated strings; all allocated
        // objects are owned by this value and released by Drop even on errors.
        unsafe {
            let codec = av::avcodec_find_encoder_by_name(name.as_ptr());
            if codec.is_null() {
                return Err(format!("system FFmpeg lacks {}", kind.name()));
            }
            let mut this = Self {
                context: av::avcodec_alloc_context3(codec),
                input: av::av_frame_alloc(),
                hardware: av::av_frame_alloc(),
                packet: av::av_packet_alloc(),
                device: ptr::null_mut(),
                frames: ptr::null_mut(),
                index: 0,
                kind,
                interval,
                since_idr: 0,
            };
            if this.context.is_null()
                || this.input.is_null()
                || this.hardware.is_null()
                || this.packet.is_null()
            {
                return Err("FFmpeg allocation failed".into());
            }
            let context = &mut *this.context;
            context.width = STREAM_CAPTURE_WIDTH as i32;
            context.height = STREAM_CAPTURE_HEIGHT as i32;
            context.time_base = av::AVRational {
                num: 1,
                den: STREAM_CAPTURE_FPS as i32,
            };
            context.framerate = av::AVRational {
                num: STREAM_CAPTURE_FPS as i32,
                den: 1,
            };
            context.bit_rate = STREAM_ENCODER_BITRATE as i64;
            context.rc_max_rate = STREAM_ENCODER_BITRATE as i64;
            context.rc_buffer_size = (STREAM_ENCODER_BITRATE / STREAM_CAPTURE_FPS) as i32;
            context.gop_size = i32::MAX;
            context.max_b_frames = 0;
            context.color_range = av::AVColorRange::AVCOL_RANGE_MPEG;
            context.colorspace = av::AVColorSpace::AVCOL_SPC_BT709;
            context.color_primaries = av::AVColorPrimaries::AVCOL_PRI_BT709;
            context.color_trc = av::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            let private = context.priv_data;
            let set = |key: &CStr, value: &CStr| {
                check(
                    av::av_opt_set(private, key.as_ptr(), value.as_ptr(), 0),
                    &format!("set {}", key.to_string_lossy()),
                )
            };
            match kind {
                Kind::Nvenc => {
                    context.pix_fmt = av::AVPixelFormat::AV_PIX_FMT_NV12;
                    for (key, value) in [
                        (c"profile", c"baseline"),
                        (c"level", c"3.1"),
                        (c"tune", c"ull"),
                        (c"rc", c"cbr"),
                        (c"zerolatency", c"1"),
                        (c"delay", c"0"),
                        (c"forced-idr", c"1"),
                        (c"rc-lookahead", c"0"),
                    ] {
                        set(key, value)?;
                    }
                    if let Some(device) = device {
                        // FFmpeg's NVENC selector is an ordinal, not a stable UUID.
                        // Refuse other strings rather than silently selecting another GPU.
                        let _: u32 = device
                            .parse()
                            .map_err(|_| "NVENC device must be a nonnegative GPU index")?;
                        let device = CString::new(device).map_err(|_| "invalid device")?;
                        set(c"gpu", &device)?;
                    }
                }
                Kind::Vulkan => {
                    for (key, value) in [
                        (c"profile", c"constrained_baseline"),
                        (c"level", c"3.1"),
                        (c"rc_mode", c"cbr"),
                        (c"async_depth", c"1"),
                    ] {
                        set(key, value)?;
                    }
                    let device = device
                        .map(CString::new)
                        .transpose()
                        .map_err(|_| "device contains NUL")?;
                    check(
                        av::av_hwdevice_ctx_create(
                            &mut this.device,
                            av::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN,
                            device.as_ref().map_or(ptr::null(), |d| d.as_ptr()),
                            ptr::null_mut(),
                            0,
                        ),
                        "create Vulkan device",
                    )?;
                    this.frames = av::av_hwframe_ctx_alloc(this.device);
                    if this.frames.is_null() {
                        return Err("allocate Vulkan frame context failed".into());
                    }
                    let frames = &mut *((*this.frames).data as *mut av::AVHWFramesContext);
                    frames.format = av::AVPixelFormat::AV_PIX_FMT_VULKAN;
                    frames.sw_format = av::AVPixelFormat::AV_PIX_FMT_NV12;
                    frames.width = context.width;
                    frames.height = context.height;
                    frames.initial_pool_size = 3;
                    check(
                        av::av_hwframe_ctx_init(this.frames),
                        "initialize Vulkan frames",
                    )?;
                    context.pix_fmt = av::AVPixelFormat::AV_PIX_FMT_VULKAN;
                    context.hw_frames_ctx = av::av_buffer_ref(this.frames);
                    if context.hw_frames_ctx.is_null() {
                        return Err("reference Vulkan frame context failed".into());
                    }
                }
            }
            check(
                av::avcodec_open2(this.context, codec, ptr::null_mut()),
                "open hardware H264 encoder",
            )?;
            (*this.input).format = av::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
            (*this.input).width = STREAM_CAPTURE_WIDTH as i32;
            (*this.input).height = STREAM_CAPTURE_HEIGHT as i32;
            check(
                av::av_frame_get_buffer(this.input, 32),
                "allocate NV12 frame",
            )?;
            Ok(this)
        }
    }

    pub(super) fn set_interval(&mut self, interval: u32) -> Result<(), String> {
        if interval == 0 {
            return Err("keyframe interval must be positive".into());
        }
        self.interval = interval;
        Ok(())
    }

    pub(super) fn encode(
        &mut self,
        frame: I420Frame<'_>,
        force_keyframe: bool,
    ) -> Result<Option<EncodedH264Frame>, String> {
        if frame.width != STREAM_CAPTURE_WIDTH as usize
            || frame.height != STREAM_CAPTURE_HEIGHT as usize
        {
            return Err("hardware frame dimensions changed".into());
        }
        let force_keyframe = force_keyframe || self.since_idr >= self.interval;
        // SAFETY: these objects are exclusively owned on this worker. FFmpeg
        // allocates each plane using the positive stride and configured height.
        // Borrowed input slices are copied before returning; none escape to C.
        unsafe {
            check(
                av::av_frame_make_writable(self.input),
                "make input frame writable",
            )?;
            let input = &mut *self.input;
            let y_stride = usize::try_from(input.linesize[0]).map_err(|_| "invalid Y stride")?;
            let uv_stride = usize::try_from(input.linesize[1]).map_err(|_| "invalid UV stride")?;
            if input.data[0].is_null() || input.data[1].is_null() {
                return Err("missing NV12 planes".into());
            }
            let y = std::slice::from_raw_parts_mut(input.data[0], y_stride * frame.height);
            let uv = std::slice::from_raw_parts_mut(input.data[1], uv_stride * (frame.height / 2));
            copy_i420_to_nv12(frame, y, uv, y_stride, uv_stride)?;
            input.pts = self.index;
            input.pict_type = if force_keyframe || self.index == 0 {
                av::AVPictureType::AV_PICTURE_TYPE_I
            } else {
                av::AVPictureType::AV_PICTURE_TYPE_NONE
            };
            input.color_range = av::AVColorRange::AVCOL_RANGE_MPEG;
            input.colorspace = av::AVColorSpace::AVCOL_SPC_BT709;
            let submitted = match self.kind {
                Kind::Nvenc => self.input,
                Kind::Vulkan => {
                    av::av_frame_unref(self.hardware);
                    check(
                        av::av_hwframe_get_buffer(self.frames, self.hardware, 0),
                        "allocate Vulkan input",
                    )?;
                    check(
                        av::av_hwframe_transfer_data(self.hardware, self.input, 0),
                        "upload Vulkan frame",
                    )?;
                    check(
                        av::av_frame_copy_props(self.hardware, self.input),
                        "copy frame properties",
                    )?;
                    self.hardware
                }
            };
            check(
                av::avcodec_send_frame(self.context, submitted),
                "submit H264 frame",
            )?;
            av::av_packet_unref(self.packet);
            // A delayed encoder is incompatible with capture's timestamp contract.
            // Treat EAGAIN as a backend failure, not a successfully skipped frame.
            check(
                av::avcodec_receive_packet(self.context, self.packet),
                "receive low-delay H264 frame",
            )?;
            let packet = &*self.packet;
            if packet.pts != self.index || packet.size <= 0 || packet.data.is_null() {
                return Err("hardware encoder returned delayed or empty output".into());
            }
            let bytes = std::slice::from_raw_parts(packet.data, packet.size as usize).to_vec();
            self.index += 1;
            let keyframe = annex_b_contains_idr(&bytes);
            self.since_idr = if keyframe {
                1
            } else {
                self.since_idr.saturating_add(1)
            };
            let output = EncodedH264Frame::new(bytes, keyframe)?;
            if force_keyframe || self.index == 1 {
                validate_parameterized_h264_idr(&output, self.kind.name())?;
                validate_profile(&output.annex_b)?;
            }
            Ok(Some(output))
        }
    }
}

fn validate_profile(annex_b: &[u8]) -> Result<(), String> {
    let sps = crate::discord::voice::media::annex_b_nals(annex_b)
        .find(|nal| nal.first().is_some_and(|header| header & 0x1f == 7))
        .ok_or("hardware IDR is missing SPS")?;
    if sps.len() < 4 || sps[1] != 66 || sps[2] & 0x40 == 0 || sps[3] != 31 {
        return Err("hardware encoder did not produce constrained baseline H264 Level 3.1".into());
    }
    Ok(())
}

impl Drop for FfmpegEncoder {
    fn drop(&mut self) {
        // SAFETY: FFmpeg free/unref APIs accept null pointers and clear them.
        // Codec references are released before the frame pool/device references.
        unsafe {
            av::avcodec_free_context(&mut self.context);
            av::av_frame_free(&mut self.hardware);
            av::av_frame_free(&mut self.input);
            av::av_packet_free(&mut self.packet);
            av::av_buffer_unref(&mut self.frames);
            av::av_buffer_unref(&mut self.device);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, process::Command};

    fn hardware_round_trip(kind: Kind) {
        let mut encoder = FfmpegEncoder::new(kind, 60, None)
            .expect("requested hardware must initialize; fallback is forbidden");
        let width = STREAM_CAPTURE_WIDTH as usize;
        let height = STREAM_CAPTURE_HEIGHT as usize;
        let mut y = vec![0; width * height];
        let u = vec![90; width * height / 4];
        let v = vec![170; width * height / 4];
        let mut stream = tempfile::NamedTempFile::new().unwrap();
        let started = std::time::Instant::now();
        let mut since_idr = 0;
        for index in 0..90 {
            if index == 40 {
                encoder.set_interval(10).unwrap();
            }
            for row in 0..height {
                for col in 0..width {
                    y[row * width + col] = 32 + ((col / 32 + row / 32 + index) % 6) as u8 * 30;
                }
            }
            let frame = I420Frame::new(&y, &u, &v, width, height, width, width / 2, width / 2);
            let force = index == 17 || index == 40;
            let output = encoder
                .encode(frame, force)
                .unwrap()
                .expect("one output per input");
            if index == 0 || force {
                validate_parameterized_h264_idr(&output, kind.name()).unwrap();
            }
            if output.is_keyframe {
                since_idr = 0;
            } else {
                since_idr += 1;
            }
            if index >= 40 {
                assert!(since_idr < 10, "updated IDR interval was ignored");
            }
            stream.write_all(&output.annex_b).unwrap();
        }
        eprintln!(
            "{}: 90 frames including startup/forced/periodic IDRs in {:?}",
            kind.name(),
            started.elapsed()
        );
        drop(encoder);
        stream.flush().unwrap();
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(stream.path())
            .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
            .output()
            .expect("FFmpeg CLI needed for independent decode");
        assert!(
            decoded.status.success(),
            "decode failed: {}",
            String::from_utf8_lossy(&decoded.stderr)
        );
        let frame_bytes = width * height * 3 / 2;
        assert_eq!(decoded.stdout.len(), frame_bytes * 90);
        // Lossy output should retain the moving pattern and correct chroma planes.
        for index in [0, 17, 40, 89] {
            let decoded = &decoded.stdout[index * frame_bytes..(index + 1) * frame_bytes];
            let expected_y = 32 + (index % 6) as i32 * 30;
            assert!((decoded[0] as i32 - expected_y).abs() < 12);
            assert!((decoded[width * height] as i32 - 90).abs() < 12);
            assert!((decoded[width * height * 5 / 4] as i32 - 170).abs() < 12);
        }
    }

    #[test]
    #[ignore = "requires NVENC hardware and FFmpeg CLI; never accepts software fallback"]
    fn nvenc_round_trip() {
        hardware_round_trip(Kind::Nvenc);
    }

    #[test]
    #[ignore = "requires Vulkan Video H264 hardware and FFmpeg CLI; never accepts software fallback"]
    fn vulkan_round_trip() {
        hardware_round_trip(Kind::Vulkan);
    }
}
