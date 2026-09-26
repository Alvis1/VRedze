use anyhow::{Context, ensure};
use clap::{Args, ValueEnum};
use serde::Serialize;
use std::{
    ffi::{CStr, CString, c_char, c_int},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
};

#[derive(Clone, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Vulkan,
    Vaapi,
    V4l2m2m,
}

impl Backend {
    fn name(&self) -> &'static str {
        match self {
            Self::Vulkan => "vulkan",
            Self::Vaapi => "vaapi",
            Self::V4l2m2m => "v4l2m2m",
        }
    }
}

#[derive(Args)]
pub struct Options {
    /// Local file only; isolates decoder performance from SMB.
    pub input: PathBuf,
    #[arg(long, value_enum)]
    pub backend: Backend,
    /// Backend device identifier (e.g. Vulkan index or VAAPI render node).
    #[arg(long)]
    pub device: Option<String>,
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u16).range(1..))]
    pub frames: u16,
    /// Download every hardware frame to confirm completion; deliberately adds copies.
    #[arg(long)]
    pub verify_readback: bool,
}

#[repr(C)]
struct RawResult {
    frames: i32,
    hardware_frames: i32,
    readback_frames: i32,
    width: i32,
    height: i32,
    elapsed_seconds: f64,
    stream_fps: f64,
    codec: [c_char; 32],
    pixel_format: [c_char; 32],
    error: [c_char; 256],
}

unsafe extern "C" {
    fn jv_decode(
        path: *const c_char,
        backend: *const c_char,
        device: *const c_char,
        frame_limit: c_int,
        readback: c_int,
        result: *mut RawResult,
    ) -> c_int;
}

#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    architecture: &'static str,
    backend: Backend,
    pub sample_decoded: bool,
    frame_hardware_decoding_verified: bool,
    gpu_import_verified: bool,
    presentation_verified: bool,
    frames: i32,
    requested_frames: u16,
    hardware_surface_frames: i32,
    readback_frames: i32,
    width: i32,
    height: i32,
    codec: String,
    pixel_format: String,
    elapsed_seconds: f64,
    decode_api_frames_per_second: f64,
    stream_fps: f64,
    error: Option<String>,
    limitation: &'static str,
}

pub fn run(options: Options) -> anyhow::Result<Report> {
    // Canonicalization prevents a local-looking path from becoming an FFmpeg URL.
    let path = options
        .input
        .canonicalize()
        .context("Sample must be an existing local file")?;
    ensure!(path.is_file(), "Sample must be a regular file");
    ensure!(
        !matches!(options.backend, Backend::V4l2m2m) || options.device.is_none(),
        "V4L2 device selection is not implemented; omit --device"
    );
    let path = CString::new(path.as_os_str().as_bytes())?;
    let backend = CString::new(options.backend.name())?;
    let device = options.device.map(CString::new).transpose()?;
    // SAFETY: repr(C) layout matches decode.h. All fields accept zero initialization.
    let mut raw: RawResult = unsafe { std::mem::zeroed() };
    let code = unsafe {
        jv_decode(
            path.as_ptr(),
            backend.as_ptr(),
            device.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            options.frames.into(),
            options.verify_readback.into(),
            &mut raw,
        )
    };
    fn string(bytes: &[c_char]) -> String {
        // Native side always zero-terminates each fixed-size buffer.
        unsafe { CStr::from_ptr(bytes.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }
    let error = string(&raw.error);
    Ok(Report {
        schema_version: 1,
        architecture: std::env::consts::ARCH,
        backend: options.backend,
        sample_decoded: code == 0 && raw.frames == i32::from(options.frames),
        // A sample run is evidence, never automatic certification of the target device.
        frame_hardware_decoding_verified: false,
        gpu_import_verified: false,
        presentation_verified: false,
        frames: raw.frames,
        requested_frames: options.frames,
        hardware_surface_frames: raw.hardware_frames,
        readback_frames: raw.readback_frames,
        width: raw.width,
        height: raw.height,
        codec: string(&raw.codec),
        pixel_format: string(&raw.pixel_format),
        elapsed_seconds: raw.elapsed_seconds,
        decode_api_frames_per_second: if raw.elapsed_seconds > 0.0 {
            f64::from(raw.frames) / raw.elapsed_seconds
        } else {
            0.0
        },
        stream_fps: raw.stream_fps,
        error: (!error.is_empty()).then_some(error),
        limitation: "Decode-only evidence. Without readback, asynchronous GPU completion is not timed. Readback adds copies. Neither mode measures OpenXR playback, power, visual correctness, or verifies Steam Frame identity.",
    })
}
