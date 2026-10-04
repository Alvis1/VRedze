//! Safe wrapper over `native/media.c`: FFmpeg demux/decode over any `Read + Seek`.

use anyhow::bail;
use serde::Serialize;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    io::{Read, Seek, SeekFrom},
    panic::{AssertUnwindSafe, catch_unwind},
};

/// Hardware decoding backend for this platform: the V4L2 (Qualcomm iris)
/// decoder on Linux ARM64 / Steam Frame, Vulkan video on other Linux and
/// macOS, and MediaCodec on Android (Meta Quest).
pub fn default_hw_backend() -> Option<&'static str> {
    if cfg!(target_os = "android") {
        Some("mediacodec")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("v4l2m2m")
    } else {
        Some("vulkan")
    }
}

/// Gives FFmpeg the JavaVM (for its MediaCodec decoders). Call once, early.
///
/// # Safety
/// `vm` must be the process's `JavaVM*`.
#[cfg(target_os = "android")]
pub unsafe fn android_init(vm: *mut c_void) {
    unsafe extern "C" {
        fn jv_android_init(vm: *mut c_void);
    }
    unsafe { jv_android_init(vm) };
}

pub trait Source: Read + Seek + Send {}
impl<T: Read + Seek + Send> Source for T {}

const AVSEEK_SIZE: c_int = 0x10000;
const AVERROR_EIO: c_int = -5;

#[repr(C)]
struct RawInfo {
    container: [c_char; 64],
    duration_seconds: f64,
    bit_rate: i64,
    video_codec: [c_char; 32],
    video_profile: [c_char; 48],
    pixel_format: [c_char; 32],
    width: i32,
    height: i32,
    bit_depth: i32,
    fps: f64,
    stereo_mode: [c_char; 32],
    stereo_inverted: i32,
    projection: [c_char; 48],
    bound_left: u32,
    bound_top: u32,
    bound_right: u32,
    bound_bottom: u32,
    audio_codec: [c_char; 32],
    audio_channels: i32,
    audio_sample_rate: i32,
    // Spatial video (JVMediaInfo's appended fields, same order).
    multilayer: i32,
    view_count: i32,
    view_eye: [i32; 2],
    primary_eye: i32,
    projection_kind: i32,
    baseline_um: u32,
    reserved: i32,
    disparity_adjustment: f64,
    hfov_degrees: f64,
    yaw: f64,
    pitch: f64,
    roll: f64,
    rotation: f64,
}

#[repr(C)]
struct RawSubtitleTrack {
    codec: [c_char; 32],
    language: [c_char; 16],
    title: [c_char; 64],
    is_default: i32,
    forced: i32,
    supported: i32,
}

#[repr(C)]
struct RawAudioTrack {
    codec: [c_char; 32],
    language: [c_char; 16],
    title: [c_char; 64],
    channels: i32,
    is_default: i32,
}

/// An audio stream in the file.
#[derive(Clone, Debug, Serialize)]
pub struct AudioTrackInfo {
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub channels: u32,
    pub default: bool,
}

#[repr(C)]
struct RawCue {
    start: f64,
    end: f64,
    clear: i32,
    text: [c_char; 1024],
    rgba: *mut u8,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    frame_width: i32,
    frame_height: i32,
}

/// A subtitle stream in the file.
#[derive(Clone, Debug, Serialize)]
pub struct SubtitleTrackInfo {
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub default: bool,
    pub forced: bool,
    /// A text format we can show (bitmap subtitles, e.g. PGS, are not).
    pub supported: bool,
}

#[repr(C)]
struct RawStats {
    frames: i32,
    hardware_frames: i32,
    software_frames: i32,
    elapsed_seconds: f64,
    decoder: [c_char; 48],
    hw_backend: [c_char; 16],
    pixel_format: [c_char; 32],
    note: [c_char; 128],
    error: [c_char; 256],
}

type ReadFn = unsafe extern "C" fn(*mut c_void, *mut u8, c_int) -> c_int;
type SeekFn = unsafe extern "C" fn(*mut c_void, i64, c_int) -> i64;

#[repr(C)]
struct RawMedia {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn jv_media_open(
        name: *const c_char,
        read: ReadFn,
        seek: SeekFn,
        opaque: *mut c_void,
        info: *mut RawInfo,
        error: *mut c_char,
        error_size: c_int,
    ) -> *mut RawMedia;
    fn jv_media_decode(
        media: *mut RawMedia,
        hw_backend: *const c_char,
        allow_software: c_int,
        decoder_options: *const c_char,
        frame_limit: c_int,
        stats: *mut RawStats,
    ) -> c_int;
    fn jv_media_close(media: *mut RawMedia);
}

type BoxedSource = Box<dyn Source>;

unsafe extern "C" fn read_cb(opaque: *mut c_void, buf: *mut u8, size: c_int) -> c_int {
    // SAFETY: opaque is the stable Box<BoxedSource> owned by `Media`; FFmpeg
    // passes a writable buffer of `size` bytes.
    let source = unsafe { &mut *(opaque as *mut BoxedSource) };
    let out = unsafe { std::slice::from_raw_parts_mut(buf, size.max(0) as usize) };
    match catch_unwind(AssertUnwindSafe(|| source.read(out))) {
        Ok(Ok(n)) => n as c_int,
        _ => AVERROR_EIO,
    }
}

unsafe extern "C" fn seek_cb(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    let source = unsafe { &mut *(opaque as *mut BoxedSource) };
    let result = catch_unwind(AssertUnwindSafe(|| {
        if whence == AVSEEK_SIZE {
            let here = source.stream_position()?;
            let end = source.seek(SeekFrom::End(0))?;
            source.seek(SeekFrom::Start(here))?;
            return Ok(end);
        }
        let from = match whence {
            0 => SeekFrom::Start(offset.try_into().map_err(std::io::Error::other)?),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput)),
        };
        source.seek(from)
    }));
    match result {
        Ok(Ok(position)) => position as i64,
        _ => AVERROR_EIO as i64,
    }
}

fn text(raw: &[c_char]) -> String {
    // SAFETY: the C side always NUL-terminates via snprintf into zeroed arrays.
    unsafe { CStr::from_ptr(raw.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn optional(raw: &[c_char]) -> Option<String> {
    Some(text(raw)).filter(|s| !s.is_empty())
}

#[derive(Clone, Debug, Serialize)]
pub struct VideoInfo {
    pub codec: String,
    pub profile: Option<String>,
    pub pixel_format: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Luma bit depth (8, 10, 12); 0 when unknown.
    pub bit_depth: u32,
    pub fps: f64,
    /// FFmpeg stereo3d type name, e.g. "side by side", "top and bottom".
    pub stereo_mode: Option<String>,
    pub stereo_inverted: bool,
    /// FFmpeg spherical projection name, e.g. "equirectangular", "fisheye".
    pub projection: Option<String>,
    /// Horizontal coverage implied by equirectangular bounds, in degrees.
    pub horizontal_degrees: Option<f64>,
    /// More than one coded layer (MV-HEVC: Apple spatial video).
    pub multilayer: bool,
    /// Views the decoder outputs: 2 for stereo MV-HEVC, otherwise 1.
    pub views: u32,
    /// The eye of view 0 and view 1, when the stream says.
    pub view_eyes: [Option<Eye>; 2],
    /// The hero (main) eye, when the container says.
    pub primary_eye: Option<Eye>,
    /// The projection as a kind (the name above is for display).
    pub spherical: Option<SphericalKind>,
    /// Camera baseline in millimetres.
    pub baseline_mm: Option<f64>,
    /// Horizontal disparity adjustment, as a fraction of the width.
    pub disparity_adjustment: Option<f64>,
    /// Horizontal field of view of the camera, in degrees.
    pub hfov_degrees: Option<f64>,
    /// Orientation of the spherical video: yaw, pitch, roll in degrees.
    pub orientation: [f64; 3],
    /// Rotation from the display matrix, degrees counter-clockwise.
    pub rotation_degrees: f64,
}

impl Default for VideoInfo {
    /// Nothing known: one view, no stereo or projection metadata.
    fn default() -> Self {
        Self {
            codec: String::new(),
            profile: None,
            pixel_format: None,
            width: 0,
            height: 0,
            bit_depth: 0,
            fps: 0.0,
            stereo_mode: None,
            stereo_inverted: false,
            projection: None,
            horizontal_degrees: None,
            multilayer: false,
            views: 1,
            view_eyes: [None; 2],
            primary_eye: None,
            spherical: None,
            baseline_mm: None,
            disparity_adjustment: None,
            hfov_degrees: None,
            orientation: [0.0; 3],
            rotation_degrees: 0.0,
        }
    }
}

/// Which eye a view belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Eye {
    Left,
    Right,
}

impl Eye {
    fn from_raw(raw: i32) -> Option<Eye> {
        match raw {
            1 => Some(Eye::Left),
            2 => Some(Eye::Right),
            _ => None,
        }
    }
}

/// Spherical projection from the container (JV_PROJECTION_*).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SphericalKind {
    Equirectangular,
    EquirectangularTile,
    HalfEquirectangular,
    Rectilinear,
    Fisheye,
    ParametricImmersive,
    Cubemap,
}

impl SphericalKind {
    fn from_raw(raw: i32) -> Option<SphericalKind> {
        Some(match raw {
            1 => SphericalKind::Equirectangular,
            2 => SphericalKind::EquirectangularTile,
            3 => SphericalKind::HalfEquirectangular,
            4 => SphericalKind::Rectilinear,
            5 => SphericalKind::Fisheye,
            6 => SphericalKind::ParametricImmersive,
            7 => SphericalKind::Cubemap,
            _ => return None,
        })
    }

    /// Equirectangular kinds, whose crop bounds give their coverage.
    fn is_equirectangular(self) -> bool {
        matches!(
            self,
            SphericalKind::Equirectangular
                | SphericalKind::EquirectangularTile
                | SphericalKind::HalfEquirectangular
        )
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AudioInfo {
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct MediaInfo {
    pub container: String,
    pub duration_seconds: f64,
    pub bit_rate: i64,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
    pub subtitles: Vec<SubtitleTrackInfo>,
    pub audio_tracks: Vec<AudioTrackInfo>,
    /// The audio track played at first (index into `audio_tracks`).
    pub audio_track: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DecodeStats {
    pub frames: u32,
    pub hardware_frames: u32,
    pub software_frames: u32,
    pub elapsed_seconds: f64,
    pub frames_per_second: f64,
    pub decoder: String,
    pub hw_backend: Option<String>,
    pub output_pixel_format: Option<String>,
    /// Why hardware decoding was skipped, if it was.
    pub note: Option<String>,
    pub error: Option<String>,
}

pub struct Media {
    raw: *mut RawMedia,
    // Boxed twice so the pointer handed to C stays valid while `Media` moves.
    _source: Box<BoxedSource>,
    info: MediaInfo,
}

// SAFETY: the FFmpeg contexts are only touched through &mut self.
unsafe impl Send for Media {}

impl Media {
    pub fn open(name: &str, source: impl Source + 'static) -> anyhow::Result<Self> {
        // SAFETY: plain size queries.
        let sizes = unsafe { (jv_media_info_size(), jv_frame_size()) };
        if sizes != (size_of::<RawInfo>(), size_of::<RawFrame>()) {
            bail!("native/media.c and src/media.rs disagree on struct layouts: rebuild both");
        }
        let mut source: Box<BoxedSource> = Box::new(Box::new(source));
        let name = CString::new(name.replace('\0', ""))?;
        let mut raw_info = std::mem::MaybeUninit::<RawInfo>::zeroed();
        let mut error = [0 as c_char; 256];
        // SAFETY: all pointers are valid for the call; `source` outlives `raw`.
        let raw = unsafe {
            jv_media_open(
                name.as_ptr(),
                read_cb,
                seek_cb,
                (&mut *source) as *mut BoxedSource as *mut c_void,
                raw_info.as_mut_ptr(),
                error.as_mut_ptr(),
                error.len() as c_int,
            )
        };
        if raw.is_null() {
            bail!("{}", text(&error));
        }
        let r = unsafe { raw_info.assume_init() };
        let video = optional(&r.video_codec).map(|codec| VideoInfo {
            codec,
            profile: optional(&r.video_profile),
            pixel_format: optional(&r.pixel_format),
            width: r.width.max(0) as u32,
            height: r.height.max(0) as u32,
            bit_depth: r.bit_depth.max(0) as u32,
            fps: r.fps,
            stereo_mode: optional(&r.stereo_mode),
            stereo_inverted: r.stereo_inverted != 0,
            projection: optional(&r.projection),
            horizontal_degrees: SphericalKind::from_raw(r.projection_kind)
                .filter(|k| k.is_equirectangular())
                .map(|kind| {
                    if kind == SphericalKind::HalfEquirectangular {
                        return 180.0;
                    }
                    let covered = 1.0 - (r.bound_left as f64 + r.bound_right as f64) / 4294967296.0;
                    (covered * 360.0).clamp(0.0, 360.0)
                }),
            multilayer: r.multilayer != 0,
            views: r.view_count.max(1) as u32,
            view_eyes: r.view_eye.map(Eye::from_raw),
            primary_eye: Eye::from_raw(r.primary_eye),
            spherical: SphericalKind::from_raw(r.projection_kind),
            baseline_mm: (r.baseline_um > 0).then(|| r.baseline_um as f64 / 1000.0),
            disparity_adjustment: (r.disparity_adjustment != 0.0).then_some(r.disparity_adjustment),
            hfov_degrees: (r.hfov_degrees > 0.0).then_some(r.hfov_degrees),
            orientation: [r.yaw, r.pitch, r.roll],
            rotation_degrees: r.rotation,
        });
        let audio = optional(&r.audio_codec).map(|codec| AudioInfo {
            codec,
            channels: r.audio_channels.max(0) as u32,
            sample_rate: r.audio_sample_rate.max(0) as u32,
        });
        let count = unsafe { jv_media_subtitle_count(raw) };
        let subtitles = (0..count)
            .filter_map(|i| {
                let mut t = std::mem::MaybeUninit::<RawSubtitleTrack>::zeroed();
                // SAFETY: `raw` is open and `i` is in range.
                (unsafe { jv_media_subtitle_track(raw, i, t.as_mut_ptr()) } == 0).then(|| {
                    let t = unsafe { t.assume_init() };
                    SubtitleTrackInfo {
                        codec: text(&t.codec),
                        language: optional(&t.language),
                        title: optional(&t.title),
                        default: t.is_default != 0,
                        forced: t.forced != 0,
                        supported: t.supported != 0,
                    }
                })
            })
            .collect();
        let audio_count = unsafe { jv_media_audio_count(raw) };
        let audio_tracks = (0..audio_count)
            .filter_map(|i| {
                let mut t = std::mem::MaybeUninit::<RawAudioTrack>::zeroed();
                // SAFETY: `raw` is open and `i` is in range.
                (unsafe { jv_media_audio_track(raw, i, t.as_mut_ptr()) } == 0).then(|| {
                    let t = unsafe { t.assume_init() };
                    AudioTrackInfo {
                        codec: text(&t.codec),
                        language: optional(&t.language),
                        title: optional(&t.title),
                        channels: t.channels.max(0) as u32,
                        default: t.is_default != 0,
                    }
                })
            })
            .collect();
        let audio_track = usize::try_from(unsafe { jv_media_current_audio(raw) }).ok();
        Ok(Self {
            raw,
            _source: source,
            info: MediaInfo {
                container: text(&r.container),
                duration_seconds: r.duration_seconds,
                bit_rate: r.bit_rate,
                video,
                audio,
                subtitles,
                audio_tracks,
                audio_track,
            },
        })
    }

    pub fn info(&self) -> &MediaInfo {
        &self.info
    }

    /// Decodes up to `frames` video frames from the start. `hw_backend` is an
    /// FFmpeg device type ("vulkan", "vaapi"), "v4l2m2m", or `None` for software.
    pub fn decode(
        &mut self,
        hw_backend: Option<&str>,
        allow_software: bool,
        decoder_options: &str,
        frames: u32,
    ) -> anyhow::Result<DecodeStats> {
        let backend = hw_backend.map(CString::new).transpose()?;
        let options = CString::new(decoder_options)?;
        let mut raw = std::mem::MaybeUninit::<RawStats>::zeroed();
        // SAFETY: `self.raw` is live; the stats pointer is valid for the call.
        unsafe {
            jv_media_decode(
                self.raw,
                backend.as_ref().map_or(std::ptr::null(), |b| b.as_ptr()),
                allow_software as c_int,
                options.as_ptr(),
                frames.min(i32::MAX as u32) as c_int,
                raw.as_mut_ptr(),
            );
        }
        let s = unsafe { raw.assume_init() };
        Ok(DecodeStats {
            frames: s.frames.max(0) as u32,
            hardware_frames: s.hardware_frames.max(0) as u32,
            software_frames: s.software_frames.max(0) as u32,
            elapsed_seconds: s.elapsed_seconds,
            frames_per_second: if s.elapsed_seconds > 0.0 {
                s.frames as f64 / s.elapsed_seconds
            } else {
                0.0
            },
            decoder: text(&s.decoder),
            hw_backend: optional(&s.hw_backend),
            output_pixel_format: optional(&s.pixel_format),
            note: optional(&s.note),
            error: optional(&s.error),
        })
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        // SAFETY: closes FFmpeg before `_source` (declared later) is dropped.
        unsafe { jv_media_close(self.raw) };
    }
}

#[repr(C)]
struct RawDecoder {
    _private: [u8; 0],
}

#[repr(C)]
struct RawFrame {
    handle: *mut c_void,
    layout: i32,
    width: i32,
    height: i32,
    bits: i32,
    plane_count: i32,
    data: [*const u8; 3],
    linesize: [i32; 3],
    pts: f64,
    matrix: i32,
    full_range: i32,
    transfer: i32,
    hardware: i32,
    view_id: i32,
    eye: i32,
}

unsafe extern "C" {
    fn jv_media_info_size() -> usize;
    fn jv_frame_size() -> usize;
    fn jv_decoder_open(
        media: *mut RawMedia,
        hw_backend: *const c_char,
        allow_software: c_int,
        decoder_options: *const c_char,
        stats: *mut RawStats,
    ) -> *mut RawDecoder;
    fn jv_decoder_next(decoder: *mut RawDecoder, frame: *mut RawFrame) -> c_int;
    fn jv_decoder_seek(decoder: *mut RawDecoder, seconds: f64) -> c_int;
    fn jv_frame_release(handle: *mut c_void);
    fn jv_decoder_enable_audio(decoder: *mut RawDecoder, rate: c_int, channels: c_int) -> c_int;
    fn jv_decoder_audio_available(decoder: *const RawDecoder) -> c_int;
    fn jv_decoder_audio_read(
        decoder: *mut RawDecoder,
        out: *mut f32,
        frames: c_int,
        pts: *mut f64,
    ) -> c_int;
    fn jv_decoder_close(decoder: *mut RawDecoder);
    fn jv_media_subtitle_count(media: *const RawMedia) -> c_int;
    fn jv_media_audio_count(media: *const RawMedia) -> c_int;
    fn jv_media_audio_track(media: *const RawMedia, track: c_int, out: *mut RawAudioTrack)
    -> c_int;
    fn jv_media_current_audio(media: *const RawMedia) -> c_int;
    fn jv_decoder_select_audio(decoder: *mut RawDecoder, track: c_int) -> c_int;
    fn jv_media_subtitle_track(
        media: *const RawMedia,
        track: c_int,
        out: *mut RawSubtitleTrack,
    ) -> c_int;
    fn jv_decoder_select_subtitle(decoder: *mut RawDecoder, track: c_int) -> c_int;
    fn jv_free(pointer: *mut c_void);
    fn jv_decoder_subtitle_read(decoder: *mut RawDecoder, out: *mut RawCue) -> c_int;
}

const AVERROR_EOF: c_int = -0x20464F45; // FFERRTAG('E','O','F',' ')
const AVERROR_PATCHWELCOME: c_int = -0x45574150; // FFERRTAG('P','A','W','E')

/// How the planes of a decoded frame are arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaneLayout {
    /// Y, U, V planes; 10-bit samples are LSB-aligned in 16 bits.
    Planar,
    /// Y plane + interleaved UV (NV12).
    SemiPlanar,
    /// Y + interleaved UV with 10 bits MSB-aligned in 16 bits (P010).
    SemiPlanarMsb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt709,
    Bt601,
    Bt2020,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    Sdr,
    Pq,
    Hlg,
}

/// One decoded picture in CPU memory. Freed when dropped.
pub struct Frame {
    raw: RawFrame,
}

// SAFETY: the frame's buffers are reference counted by FFmpeg and immutable
// once decoded; freeing from another thread is allowed.
unsafe impl Send for Frame {}

impl Frame {
    pub fn layout(&self) -> PlaneLayout {
        match self.raw.layout {
            0 => PlaneLayout::Planar,
            1 => PlaneLayout::SemiPlanar,
            _ => PlaneLayout::SemiPlanarMsb,
        }
    }

    pub fn width(&self) -> u32 {
        self.raw.width as u32
    }

    pub fn height(&self) -> u32 {
        self.raw.height as u32
    }

    /// 8 or 10.
    pub fn bits(&self) -> u32 {
        self.raw.bits as u32
    }

    /// Seconds from the start of the stream, if known.
    pub fn pts(&self) -> Option<f64> {
        (self.raw.pts >= 0.0).then_some(self.raw.pts)
    }

    pub fn matrix(&self) -> Matrix {
        match self.raw.matrix {
            1 => Matrix::Bt601,
            2 => Matrix::Bt2020,
            _ => Matrix::Bt709,
        }
    }

    pub fn full_range(&self) -> bool {
        self.raw.full_range != 0
    }

    pub fn transfer(&self) -> Transfer {
        match self.raw.transfer {
            1 => Transfer::Pq,
            2 => Transfer::Hlg,
            _ => Transfer::Sdr,
        }
    }

    pub fn hardware(&self) -> bool {
        self.raw.hardware != 0
    }

    /// MV-HEVC view id; None for single-view video.
    pub fn view_id(&self) -> Option<u32> {
        u32::try_from(self.raw.view_id).ok()
    }

    /// The eye this view is for, when the stream says.
    pub fn eye(&self) -> Option<Eye> {
        Eye::from_raw(self.raw.eye)
    }

    /// Bytes per sample: 1 for 8-bit, 2 for 10-bit.
    pub fn bytes_per_sample(&self) -> usize {
        if self.raw.bits > 8 { 2 } else { 1 }
    }

    /// Plane dimensions in texels: (width, height, components per texel).
    pub fn plane_size(&self, plane: usize) -> (u32, u32, u32) {
        let (w, h) = (self.width(), self.height());
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        match (self.layout(), plane) {
            (_, 0) => (w, h, 1),
            (PlaneLayout::Planar, _) => (cw, ch, 1),
            _ => (cw, ch, 2),
        }
    }

    pub fn plane_count(&self) -> usize {
        self.raw.plane_count as usize
    }

    /// Row slices of one plane (without padding), top to bottom.
    pub fn rows(&self, plane: usize) -> impl Iterator<Item = &[u8]> {
        let (w, h, c) = self.plane_size(plane);
        let row_bytes = (w * c) as usize * self.bytes_per_sample();
        let stride = self.raw.linesize[plane] as isize;
        let base = self.raw.data[plane];
        (0..h as isize).map(move |y| {
            // SAFETY: FFmpeg guarantees `height` rows of `linesize` bytes, each
            // holding at least `row_bytes` of samples, alive until release.
            unsafe { std::slice::from_raw_parts(base.offset(y * stride), row_bytes) }
        })
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe { jv_frame_release(self.raw.handle) };
    }
}

/// What the view pairer needs to know about a decoded view.
pub trait ViewTagged {
    fn view_id(&self) -> Option<u32>;
    fn eye(&self) -> Option<Eye>;
    fn pts(&self) -> Option<f64>;
    /// Width, height, plane layout and bit depth: views of one picture match.
    fn shape(&self) -> (u32, u32, PlaneLayout, u32);
}

impl ViewTagged for Frame {
    fn view_id(&self) -> Option<u32> {
        Frame::view_id(self)
    }
    fn eye(&self) -> Option<Eye> {
        Frame::eye(self)
    }
    fn pts(&self) -> Option<f64> {
        Frame::pts(self)
    }
    fn shape(&self) -> (u32, u32, PlaneLayout, u32) {
        (self.width(), self.height(), self.layout(), self.bits())
    }
}

/// One moment of video: a single picture, or for spatial (MV-HEVC) video the
/// two views decoded from the same access unit (always the same shape).
pub enum Picture<F = Frame> {
    Mono(F),
    Stereo { left: F, right: F },
}

impl<F: ViewTagged> Picture<F> {
    /// The left view (or the only one): every view has the same shape and time.
    pub fn first(&self) -> &F {
        match self {
            Picture::Mono(frame) => frame,
            Picture::Stereo { left, .. } => left,
        }
    }

    pub fn pts(&self) -> Option<f64> {
        self.first().pts()
    }

    /// The views in eye order (left first); one for mono.
    pub fn views(&self) -> Vec<&F> {
        match self {
            Picture::Mono(frame) => vec![frame],
            Picture::Stereo { left, right } => vec![left, right],
        }
    }
}

/// What the container says about the views, to tell the eyes apart when the
/// frames themselves don't.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewHints {
    /// Eye of the base view and of the second one.
    pub view_eyes: [Option<Eye>; 2],
    pub primary_eye: Option<Eye>,
    /// The container says the views are stored right eye first.
    pub order_reversed: bool,
}

/// Pairs consecutive views of the same access unit (same pts, different
/// view id) into stereo pictures. FFmpeg's MV-HEVC decoder outputs the base
/// layer, then the second, for each picture. A view without its partner (a
/// damaged stream, the end, a seek) becomes a mono picture shown to both
/// eyes, so nothing is dropped.
pub struct ViewPairer<F> {
    hints: ViewHints,
    held: Option<F>,
}

impl<F: ViewTagged> ViewPairer<F> {
    pub fn new(hints: ViewHints) -> Self {
        Self { hints, held: None }
    }

    /// Takes the next decoded view; returns a picture when one is complete.
    /// A returned mono picture may leave `frame` held for its partner.
    pub fn push(&mut self, frame: F) -> Option<Picture<F>> {
        if frame.view_id().is_none() {
            // Single-view video: nothing to pair.
            return match self.held.take() {
                Some(held) => {
                    self.held = Some(frame);
                    Some(Picture::Mono(held))
                }
                None => Some(Picture::Mono(frame)),
            };
        }
        let Some(held) = self.held.take() else {
            self.held = Some(frame);
            return None;
        };
        let same_time = match (held.pts(), frame.pts()) {
            (Some(a), Some(b)) => (a - b).abs() < 1e-6,
            _ => false,
        };
        // Views of different shapes can't share a texture (a damaged stream):
        // they are shown one at a time instead.
        if same_time && held.view_id() != frame.view_id() && held.shape() == frame.shape() {
            let (left, right) = self.eyes(held, frame);
            return Some(Picture::Stereo { left, right });
        }
        // A view without its partner: show it to both eyes, wait with this one.
        self.held = Some(frame);
        Some(Picture::Mono(held))
    }

    /// At the end of the stream: a view still waiting for its partner.
    pub fn flush(&mut self) -> Option<Picture<F>> {
        self.held.take().map(Picture::Mono)
    }

    /// Drops a half-received pair (after a seek).
    pub fn reset(&mut self) {
        self.held = None;
    }

    /// (left, right) for the two views of one picture, `base` being the one
    /// FFmpeg output first (the base layer). Evidence, strongest first: the
    /// frames' own eye tags, the container's view positions, the hero eye,
    /// and finally the stored order (base = left unless marked reversed).
    fn eyes(&self, base: F, second: F) -> (F, F) {
        let base_is_left = match (base.eye(), second.eye()) {
            (Some(Eye::Left), _) | (_, Some(Eye::Right)) => true,
            (Some(Eye::Right), _) | (_, Some(Eye::Left)) => false,
            _ => {
                let fallback = if self.hints.order_reversed {
                    Eye::Right
                } else {
                    Eye::Left
                };
                let base_eye = match self.hints.view_eyes {
                    [Some(eye), _] => Some(eye),
                    [None, Some(Eye::Left)] => Some(Eye::Right),
                    [None, Some(Eye::Right)] => Some(Eye::Left),
                    _ => self.hints.primary_eye,
                }
                .unwrap_or(fallback);
                base_eye == Eye::Left
            }
        };
        if base_is_left {
            (base, second)
        } else {
            (second, base)
        }
    }
}

/// A video decoder owning its media; pull frames with [`VideoDecoder::next_frame`].
pub struct VideoDecoder {
    raw: *mut RawDecoder,
    media: Media,
    stats: DecodeStats,
    pairer: ViewPairer<Frame>,
}

// SAFETY: used from one thread at a time (the decode thread).
unsafe impl Send for VideoDecoder {}

impl Media {
    /// Opens the video decoder; same hardware-first and fallback rules as [`Media::decode`].
    pub fn into_decoder(
        self,
        hw_backend: Option<&str>,
        allow_software: bool,
        decoder_options: &str,
    ) -> anyhow::Result<VideoDecoder> {
        let backend = hw_backend.map(CString::new).transpose()?;
        let options = CString::new(decoder_options)?;
        let mut raw = std::mem::MaybeUninit::<RawStats>::zeroed();
        let decoder = unsafe {
            jv_decoder_open(
                self.raw,
                backend.as_ref().map_or(std::ptr::null(), |b| b.as_ptr()),
                allow_software as c_int,
                options.as_ptr(),
                raw.as_mut_ptr(),
            )
        };
        let s = unsafe { raw.assume_init() };
        if decoder.is_null() {
            bail!("{}", text(&s.error));
        }
        OPEN_DECODERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let hints = self
            .info
            .video
            .as_ref()
            .map_or(ViewHints::default(), |v| ViewHints {
                view_eyes: v.view_eyes,
                primary_eye: v.primary_eye,
                order_reversed: v.stereo_inverted,
            });
        Ok(VideoDecoder {
            raw: decoder,
            pairer: ViewPairer::new(hints),
            media: self,
            stats: DecodeStats {
                frames: 0,
                hardware_frames: 0,
                software_frames: 0,
                elapsed_seconds: 0.0,
                frames_per_second: 0.0,
                decoder: text(&s.decoder),
                hw_backend: optional(&s.hw_backend),
                output_pixel_format: None,
                note: optional(&s.note),
                error: None,
            },
        })
    }
}

impl VideoDecoder {
    pub fn info(&self) -> &MediaInfo {
        self.media.info()
    }

    /// Decoder name, backend and fallback note (counters are not updated).
    pub fn stats(&self) -> &DecodeStats {
        &self.stats
    }

    /// The next frame, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        let mut raw = std::mem::MaybeUninit::<RawFrame>::zeroed();
        match unsafe { jv_decoder_next(self.raw, raw.as_mut_ptr()) } {
            0 => Ok(Some(Frame {
                raw: unsafe { raw.assume_init() },
            })),
            AVERROR_EOF => Ok(None),
            AVERROR_PATCHWELCOME => bail!("Decoder produced an unsupported pixel format"),
            code => bail!("Decoding failed (FFmpeg error {code})"),
        }
    }

    /// The next picture (both views of spatial video together), or `None`
    /// at the end of the stream.
    pub fn next_picture(&mut self) -> anyhow::Result<Option<Picture>> {
        loop {
            match self.next_frame()? {
                Some(frame) => {
                    if let Some(picture) = self.pairer.push(frame) {
                        return Ok(Some(picture));
                    }
                }
                None => return Ok(self.pairer.flush()),
            }
        }
    }

    /// Also decodes the audio track as interleaved f32 at `rate` Hz, `channels`
    /// channels. Returns false when the file has no playable audio.
    pub fn enable_audio(&mut self, rate: u32, channels: u32) -> bool {
        unsafe { jv_decoder_enable_audio(self.raw, rate as c_int, channels as c_int) == 0 }
    }

    /// Takes the audio decoded so far (read alongside video frames), with the
    /// time of its first sample in seconds from the start of the video.
    pub fn take_audio(&mut self, channels: u32) -> Option<(Vec<f32>, Option<f64>)> {
        let frames = unsafe { jv_decoder_audio_available(self.raw) };
        if frames <= 0 {
            return None;
        }
        let mut samples = vec![0f32; frames as usize * channels as usize];
        let mut pts = -1.0;
        let n = unsafe { jv_decoder_audio_read(self.raw, samples.as_mut_ptr(), frames, &mut pts) };
        samples.truncate(n.max(0) as usize * channels as usize);
        Some((samples, (pts >= 0.0).then_some(pts)))
    }

    /// Jumps to the keyframe at or before `seconds`; frames before the target
    /// still arrive and should be skipped by the caller.
    /// Decodes subtitle track `track` (index into `info().subtitles`) alongside
    /// the video, or none. False if it can't be decoded.
    pub fn select_subtitle(&mut self, track: Option<usize>) -> bool {
        let track = track.map_or(-1, |t| t as c_int);
        unsafe { jv_decoder_select_subtitle(self.raw, track) == 0 }
    }

    /// Plays audio track `track` (index into `info().audio_tracks`) instead;
    /// seek afterwards. False if it can't be decoded (the old one stays).
    pub fn select_audio(&mut self, track: usize) -> bool {
        unsafe { jv_decoder_select_audio(self.raw, track as c_int) == 0 }
    }

    /// Subtitle cues decoded since the last call.
    pub fn take_subtitles(&mut self) -> Vec<crate::subtitles::Cue> {
        let mut cues = Vec::new();
        let mut raw = std::mem::MaybeUninit::<RawCue>::zeroed();
        // SAFETY: `raw` is writable; the decoder is open.
        while unsafe { jv_decoder_subtitle_read(self.raw, raw.as_mut_ptr()) } == 1 {
            let cue = unsafe { raw.assume_init_ref() };
            // Take the picture (we own it now), then free the C copy.
            let image = (!cue.rgba.is_null() && cue.width > 0 && cue.height > 0).then(|| {
                let len = cue.width as usize * cue.height as usize * 4;
                // SAFETY: C allocated `width * height * 4` bytes at `rgba`.
                let rgba = unsafe { std::slice::from_raw_parts(cue.rgba, len) }.to_vec();
                std::sync::Arc::new(crate::subtitles::Bitmap {
                    rgba,
                    width: cue.width as u32,
                    height: cue.height as u32,
                    x: cue.x,
                    y: cue.y,
                    frame_width: cue.frame_width.max(1) as u32,
                    frame_height: cue.frame_height.max(1) as u32,
                })
            });
            if !cue.rgba.is_null() {
                unsafe { jv_free(cue.rgba as *mut c_void) };
            }
            let text = crate::subtitles::clean_markup(&text(&cue.text));
            if cue.clear != 0 || !text.is_empty() || image.is_some() {
                // A cue with neither text nor picture erases (see `Cue`).
                cues.push(crate::subtitles::Cue {
                    start: cue.start,
                    end: cue.end,
                    text,
                    image,
                });
            }
        }
        cues
    }

    pub fn seek(&mut self, seconds: f64) -> anyhow::Result<()> {
        self.pairer.reset();
        match unsafe { jv_decoder_seek(self.raw, seconds.max(0.0)) } {
            0 => Ok(()),
            code => bail!("Seek failed (FFmpeg error {code})"),
        }
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        unsafe { jv_decoder_close(self.raw) };
        OPEN_DECODERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Decoders not yet closed. The hardware decoder has buffers for only one 8K
/// stream: opening a new one before the last closed falls back to the CPU.
static OPEN_DECODERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A decoder that didn't close in time is still open: later waits don't
/// wait for it again (it may never close), until it does.
static STUCK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Waits (up to `limit`) until every earlier decoder has closed.
pub fn wait_for_decoders_closed(limit: std::time::Duration) -> bool {
    use std::sync::atomic::Ordering::SeqCst;
    let started = std::time::Instant::now();
    while OPEN_DECODERS.load(SeqCst) > 0 {
        if STUCK.load(SeqCst) || started.elapsed() > limit {
            STUCK.store(true, SeqCst);
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    STUCK.store(false, SeqCst);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_structs_match_c() {
        // A mismatch would silently corrupt memory across the FFI boundary.
        assert_eq!(unsafe { jv_media_info_size() }, size_of::<RawInfo>());
        assert_eq!(unsafe { jv_frame_size() }, size_of::<RawFrame>());
    }

    #[derive(Debug, PartialEq)]
    struct View {
        id: Option<u32>,
        eye: Option<Eye>,
        pts: f64,
    }

    impl ViewTagged for View {
        fn view_id(&self) -> Option<u32> {
            self.id
        }
        fn eye(&self) -> Option<Eye> {
            self.eye
        }
        fn pts(&self) -> Option<f64> {
            Some(self.pts)
        }
        fn shape(&self) -> (u32, u32, PlaneLayout, u32) {
            // Views with id 9 are a different size (a damaged stream).
            let width = if self.id == Some(9) { 3840 } else { 1920 };
            (width, 1080, PlaneLayout::Planar, 8)
        }
    }

    fn view(id: u32, pts: f64) -> View {
        View {
            id: Some(id),
            eye: None,
            pts,
        }
    }

    fn stereo(p: Option<Picture<View>>) -> (u32, u32) {
        match p {
            Some(Picture::Stereo { left, right }) => (left.id.unwrap(), right.id.unwrap()),
            other => panic!(
                "expected a stereo picture, got {:?}",
                other.map(|p| p.views().len())
            ),
        }
    }

    #[test]
    fn pairs_base_then_second_view() {
        let mut pairer = ViewPairer::new(ViewHints::default());
        assert!(pairer.push(view(0, 0.0)).is_none());
        // No hints: the base layer (lower id) is the left eye.
        assert_eq!(stereo(pairer.push(view(1, 0.0))), (0, 1));
        assert!(pairer.push(view(0, 1.0 / 30.0)).is_none());
        assert_eq!(stereo(pairer.push(view(1, 1.0 / 30.0))), (0, 1));
        assert!(pairer.flush().is_none());
    }

    #[test]
    fn frame_eye_tags_win() {
        let mut pairer = ViewPairer::new(ViewHints {
            view_eyes: [Some(Eye::Left), Some(Eye::Right)],
            primary_eye: Some(Eye::Left),
            order_reversed: false,
        });
        let tagged = |id, eye| View {
            id: Some(id),
            eye: Some(eye),
            pts: 0.0,
        };
        pairer.push(tagged(0, Eye::Right));
        assert_eq!(stereo(pairer.push(tagged(1, Eye::Left))), (1, 0));
    }

    #[test]
    fn container_hints_place_the_base_layer() {
        // Hero eye right: the base layer is the right eye.
        let mut pairer = ViewPairer::new(ViewHints {
            primary_eye: Some(Eye::Right),
            ..Default::default()
        });
        pairer.push(view(0, 0.0));
        assert_eq!(stereo(pairer.push(view(1, 0.0))), (1, 0));
        // View positions beat the hero eye.
        let mut pairer = ViewPairer::new(ViewHints {
            view_eyes: [None, Some(Eye::Right)],
            primary_eye: Some(Eye::Right),
            ..Default::default()
        });
        pairer.push(view(0, 0.0));
        assert_eq!(stereo(pairer.push(view(1, 0.0))), (0, 1));
        // The base layer is the first view output, whatever its id.
        let mut pairer = ViewPairer::new(ViewHints::default());
        pairer.push(view(1, 0.0));
        assert_eq!(stereo(pairer.push(view(0, 0.0))), (1, 0));
    }

    #[test]
    fn reversed_order_only_changes_the_last_guess() {
        let reversed = ViewHints {
            order_reversed: true,
            ..Default::default()
        };
        let mut pairer = ViewPairer::new(reversed);
        pairer.push(view(0, 0.0));
        assert_eq!(
            stereo(pairer.push(view(1, 0.0))),
            (1, 0),
            "base is the right eye"
        );
        // Frames that say which eye they are win over the flag.
        let mut pairer = ViewPairer::new(reversed);
        let tagged = |id, eye| View {
            id: Some(id),
            eye: Some(eye),
            pts: 0.0,
        };
        pairer.push(tagged(0, Eye::Left));
        assert_eq!(stereo(pairer.push(tagged(1, Eye::Right))), (0, 1));
    }

    #[test]
    fn views_of_different_shapes_are_not_paired() {
        let mut pairer = ViewPairer::new(ViewHints::default());
        pairer.push(view(0, 0.0));
        assert!(matches!(pairer.push(view(9, 0.0)), Some(Picture::Mono(_))));
        assert!(matches!(pairer.flush(), Some(Picture::Mono(_))));
    }

    #[test]
    fn a_view_without_its_partner_is_shown_mono() {
        let mut pairer = ViewPairer::new(ViewHints::default());
        pairer.push(view(0, 0.0));
        // The next picture's base layer arrives: the lone view goes out mono.
        match pairer.push(view(0, 0.04)) {
            Some(Picture::Mono(v)) => assert_eq!(v, view(0, 0.0)),
            _ => panic!("expected the lone view as mono"),
        }
        assert_eq!(stereo(pairer.push(view(1, 0.04))), (0, 1));
        // At the end, a view still waiting comes out too.
        pairer.push(view(0, 0.08));
        assert!(matches!(pairer.flush(), Some(Picture::Mono(_))));
        // After a seek, a half pair is dropped.
        pairer.push(view(0, 5.0));
        pairer.reset();
        assert!(pairer.flush().is_none());
    }

    #[test]
    fn single_view_video_passes_straight_through() {
        let mut pairer = ViewPairer::new(ViewHints::default());
        let plain = |pts| View {
            id: None,
            eye: None,
            pts,
        };
        assert!(matches!(pairer.push(plain(0.0)), Some(Picture::Mono(_))));
        assert!(matches!(pairer.push(plain(0.04)), Some(Picture::Mono(_))));
        assert!(pairer.flush().is_none());
    }

    /// Decodes an Apple spatial video sample made with avconvert (see
    /// tools/make-spatial-samples.sh): red left eye, blue right eye.
    #[test]
    fn decodes_both_views_of_spatial_video() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/spatial-red-left.mov");
        let Ok(file) = std::fs::File::open(&path) else {
            eprintln!("skipped: {} not found", path.display());
            return;
        };
        let media = Media::open("spatial-red-left.mov", std::io::BufReader::new(file)).unwrap();
        let video = media.info().video.clone().unwrap();
        assert!(video.multilayer);
        assert_eq!(video.views, 2);
        assert_eq!(video.spherical, Some(SphericalKind::Rectilinear));
        let mut decoder = media.into_decoder(None, true, "").unwrap();
        let mut stereo_pictures = 0;
        for _ in 0..10 {
            let Some(picture) = decoder.next_picture().unwrap() else {
                break;
            };
            if let Picture::Stereo { left, right } = &picture {
                assert_eq!(left.pts(), right.pts());
                // Red is strong in the left view's V (Cr) plane, blue in the right's U (Cb).
                let mean = |f: &Frame, plane: usize| {
                    let rows: Vec<u8> = f.rows(plane).flatten().copied().collect();
                    rows.iter().map(|&b| b as f64).sum::<f64>() / rows.len() as f64
                };
                assert!(
                    mean(left, 2) > mean(right, 2),
                    "left view should be the red one"
                );
                assert!(
                    mean(right, 1) > mean(left, 1),
                    "right view should be the blue one"
                );
                stereo_pictures += 1;
            }
        }
        assert!(
            stereo_pictures >= 8,
            "only {stereo_pictures} stereo pictures"
        );
    }
}
