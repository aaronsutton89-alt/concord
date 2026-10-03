use crate::config::ScreenCaptureEncoderPreference as Preference;

use super::{I420Frame, StreamEncoder, linux_candidates};

#[test]
fn linux_encoder_candidates_follow_the_requested_preference() {
    assert_eq!(
        linux_candidates(Preference::Auto),
        &[Preference::Nvenc, Preference::Vaapi, Preference::Vulkan]
    );
    assert_eq!(linux_candidates(Preference::Nvenc), &[Preference::Nvenc]);
    assert_eq!(linux_candidates(Preference::Vaapi), &[Preference::Vaapi]);
    assert_eq!(linux_candidates(Preference::Vulkan), &[Preference::Vulkan]);
    assert!(linux_candidates(Preference::Software).is_empty());
}

#[test]
fn software_preference_stays_software_after_interval_reconfiguration() {
    let options = crate::config::ScreenCaptureOptions {
        encoder: Preference::Software,
        device: None,
    };
    let mut encoder = StreamEncoder::new_linux(60, &options)
        .expect("OpenH264 should initialize for an explicit software preference");
    assert_eq!(encoder.backend.name(), "openh264");

    encoder
        .reconfigure_interval(75)
        .expect("software encoder should reconfigure its interval");
    assert_eq!(encoder.backend.name(), "openh264");

    let (y, u, v) = test_planes();
    let frame = test_frame(&y, &u, &v);
    let encoded = encoder
        .encode(frame, true)
        .expect("software encoder should encode a forced keyframe")
        .expect("software encoder should produce a frame");
    super::validate_parameterized_h264_idr(&encoded, "software selection test")
        .expect("forced software keyframe should contain SPS, PPS, and IDR NALs");
}

#[cfg(not(feature = "ffmpeg-encoding"))]
#[test]
fn explicit_nvenc_falls_back_to_software_without_ffmpeg_support() {
    let options = crate::config::ScreenCaptureOptions {
        encoder: Preference::Nvenc,
        device: None,
    };
    let mut encoder = StreamEncoder::new_linux(60, &options)
        .expect("OpenH264 should initialize when NVENC support is not compiled in");
    assert_eq!(encoder.backend.name(), "openh264");

    let (y, u, v) = test_planes();
    let frame = test_frame(&y, &u, &v);
    let encoded = encoder
        .encode(frame, true)
        .expect("software fallback should encode a forced keyframe")
        .expect("software fallback should produce a frame");
    super::validate_parameterized_h264_idr(&encoded, "NVENC fallback test")
        .expect("forced fallback keyframe should contain SPS, PPS, and IDR NALs");
}

fn test_planes() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let width = super::STREAM_CAPTURE_WIDTH as usize;
    let height = super::STREAM_CAPTURE_HEIGHT as usize;
    let y = vec![16; width * height];
    let u = vec![128; width * height / 4];
    let v = vec![128; width * height / 4];
    (y, u, v)
}

fn test_frame<'a>(y: &'a [u8], u: &'a [u8], v: &'a [u8]) -> I420Frame<'a> {
    let width = super::STREAM_CAPTURE_WIDTH as usize;
    let height = super::STREAM_CAPTURE_HEIGHT as usize;
    I420Frame::new(y, u, v, width, height, width, width / 2, width / 2)
}
