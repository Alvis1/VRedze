//! Decides, from stream headers alone, whether a video can play smoothly on
//! this device and explains why not. The result is shown before playback
//! (browser badges) so users never meet an unexplained stutter or black screen.

use crate::media::VideoInfo;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Steam Frame: V4L2 (Qualcomm iris) hardware decoding plus CPU decoding.
    SteamFrame,
    /// Desktop GPU with Vulkan video decoding (development machine).
    Desktop,
    /// Meta Quest 2/3: CPU decoding for now (MediaCodec comes next).
    Quest,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "android") {
            Self::Quest
        } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            Self::SteamFrame
        } else {
            Self::Desktop
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::SteamFrame => "Steam Frame",
            Self::Desktop => "this PC",
            Self::Quest => "Quest",
        }
    }

    /// Why the hardware decoder will not be used for this stream (a full clause
    /// for the user), or `None` if it will. Mirrors the allow-list in
    /// `native/media.c`: anything not positively 8-bit 4:2:0 goes to the CPU,
    /// because 10-bit input crashes the Frame's decoder firmware.
    fn hardware_gap(self, v: &VideoInfo) -> Option<String> {
        let name = self.name();
        if v.multilayer {
            // Hardware decoders output only the base layer: one eye.
            return Some(
                "spatial video has two views and the hardware decoder only outputs one".into(),
            );
        }
        match self {
            Self::SteamFrame => {
                if v.codec == "vp9" {
                    return Some(format!(
                        "VP9 isn't sent to {name}'s hardware video decoder: a second VP9 video crashes it in the current SteamOS"
                    ));
                }
                if !matches!(v.codec.as_str(), "h264" | "hevc") {
                    return Some(format!(
                        "{name}'s hardware video decoder doesn't support {}",
                        codec_label(&v.codec)
                    ));
                }
                if v.bit_depth > 8 {
                    return Some(format!(
                        "{name}'s hardware video decoder only handles 8-bit video in the current SteamOS, and this video is {}-bit",
                        v.bit_depth
                    ));
                }
                if v.bit_depth == 0 && !known_8bit_profile(v) {
                    return Some(
                        "the video's bit depth couldn't be confirmed as 8-bit, so the hardware decoder isn't used, to be safe"
                            .into(),
                    );
                }
                // A profile allowing more than 8 bits may switch to 10-bit
                // mid-stream, which crashes the firmware (mirrors media.c).
                if v.profile.is_some() && !known_8bit_profile(v) {
                    return Some(format!(
                        "the video's profile ({}) allows more than 8 bits, so the hardware decoder isn't used, to be safe",
                        v.profile.as_deref().unwrap_or("")
                    ));
                }
                let pix = v.pixel_format.as_deref().unwrap_or("");
                if pix.contains("422") || pix.contains("444") {
                    return Some(format!(
                        "{name}'s hardware video decoder only handles 4:2:0 color video"
                    ));
                }
                if v.width > 8192 || v.height > 8192 {
                    return Some(format!(
                        "{name}'s hardware video decoder is limited to 8192 pixels per side"
                    ));
                }
                None
            }
            Self::Desktop => {
                (!matches!(v.codec.as_str(), "h264" | "hevc" | "av1" | "vp9")).then(|| {
                    format!(
                        "{name}'s hardware video decoder doesn't support {}",
                        codec_label(&v.codec)
                    )
                })
            }
            // Mirrors mediacodec_unsuitable in native/media.c.
            Self::Quest => {
                if !matches!(v.codec.as_str(), "h264" | "hevc" | "vp9" | "av1") {
                    return Some(format!(
                        "{name}'s hardware video decoder doesn't support {}",
                        codec_label(&v.codec)
                    ));
                }
                // Quest 3 (media_codecs.xml): at most 138240 macroblocks per
                // picture (8192x4320) and 7776000 per second (8K at 56 fps).
                // 7680x7680 360° video is 1.7 times over: the decoder fails to start.
                let blocks = v.width.div_ceil(16) as f64 * v.height.div_ceil(16) as f64;
                if v.width > 8192 || v.height > 8192 || blocks > 138_240.0 {
                    return Some(format!(
                        "{name}'s hardware video decoder takes at most 8192x4320 pixels (or 5760x5760 for square 360° video), and this video is {}x{}",
                        v.width, v.height
                    ));
                }
                let fps = if v.fps > 0.0 { v.fps } else { 30.0 };
                if blocks * fps > 7_776_000.0 {
                    return Some(format!(
                        "{name}'s hardware video decoder can't keep up with {}x{} at {:.0} fps",
                        v.width, v.height, fps
                    ));
                }
                let pix = v.pixel_format.as_deref().unwrap_or("");
                if v.bit_depth != 8 || pix.contains("422") || pix.contains("444") {
                    return Some(
                        "this player hands the hardware decoder 8-bit 4:2:0 video only for now"
                            .into(),
                    );
                }
                if matches!(v.codec.as_str(), "h264" | "hevc")
                    && v.profile.is_some()
                    && !known_8bit_profile(v)
                {
                    return Some(
                        "this player hands the hardware decoder 8-bit video only for now".into(),
                    );
                }
                None
            }
        }
    }

    /// Sustained CPU decode rate in pixels per second, or `None` if unlimited
    /// for practical purposes.
    fn software_rate(self, v: &VideoInfo) -> Option<f64> {
        match self {
            // Measured 2026-09-26 on Steam Frame with the bundled FFmpeg 8.1.3 + NEON
            // patch, over SMB: HEVC10 4K 178 fps / 8K 47.8 fps (≈1.5-1.6 Gpx/s).
            // AV1 (dav1d) 4K 88 / 8K 35 fps with SteamOS FFmpeg. Others are estimates.
            Self::SteamFrame => Some(match v.codec.as_str() {
                "hevc" => 1.5e9,
                "h264" => 1.3e9,
                "av1" => 0.8e9,
                // Not measured on the Frame: FFmpeg's VP9 decoder is about as
                // fast as its H.264 one elsewhere.
                "vp9" => 1.2e9,
                _ => 0.8e9,
            }),
            Self::Desktop => None,
            // Measured 2026-10-04 on Quest 3 (bundled FFmpeg 8.1.3): H.264
            // 7680x7680 at 26 fps (1.55 Gpx/s); MV-HEVC, both views counted,
            // 1080p at 99 and 4K at 28 pictures per second (~0.45 Gpx/s).
            // Others are estimates. Quest 2 is slower.
            Self::Quest => Some(match v.codec.as_str() {
                "hevc" => 0.45e9,
                "h264" => 1.5e9,
                "av1" => 0.6e9,
                "vp9" => 0.9e9,
                _ => 0.6e9,
            }),
        }
    }
}

/// Profiles that are always 8-bit 4:2:0 (used when headers omit the pixel format).
fn known_8bit_profile(v: &VideoInfo) -> bool {
    let profile = v.profile.as_deref().unwrap_or("");
    match v.codec.as_str() {
        "h264" => matches!(
            profile,
            "Baseline" | "Constrained Baseline" | "Main" | "Extended" | "High"
        ),
        "hevc" => matches!(profile, "Main" | "Main Still Picture"),
        "vp9" => profile == "Profile 0",
        _ => false,
    }
}

/// CPU decoding must beat real time by this factor: rendering and audio share the CPU.
const SOFTWARE_HEADROOM: f64 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Hardware decoding: smooth and power-efficient.
    Hardware,
    /// CPU decoding with comfortable headroom; uses more battery.
    Software,
    /// CPU decoding close to its limit; may stutter.
    SoftwareMarginal,
    /// Will not play acceptably; `title`/`detail` say why.
    Unplayable,
}

#[derive(Clone, Debug, Serialize)]
pub struct Assessment {
    pub platform: Platform,
    pub verdict: Verdict,
    /// Short user-facing line, e.g. for a badge tooltip or dialog title.
    pub title: String,
    /// One or two sentences explaining the reason.
    pub detail: Option<String>,
    /// What version of the video would play, when we can tell.
    pub hint: Option<String>,
    /// What to do about it, for a tooltip: the version to convert it to and
    /// how ("What to do: convert it to … tools/fit-for-quest.sh …").
    pub todo: Option<String>,
    /// Just the version to convert it to ("8-bit HEVC at 5760x5760"), for
    /// short notices.
    pub convert_to: Option<String>,
    /// Estimated CPU decode speed relative to real time (software paths only).
    pub software_speed: Option<f64>,
}

fn codec_label(codec: &str) -> String {
    match codec {
        "h264" => "H.264".into(),
        "hevc" => "HEVC".into(),
        "av1" => "AV1".into(),
        "vp9" => "VP9".into(),
        other => other.to_uppercase(),
    }
}

fn resolution_label(v: &VideoInfo) -> String {
    match v.width.max(v.height) {
        w if w >= 7680 => "8K".into(),
        w if w >= 5760 => format!("{:.1}K", w as f64 / 1024.0),
        w if w >= 3840 => "4K".into(),
        _ => format!("{}p", v.height),
    }
}

fn describe(v: &VideoInfo) -> String {
    let depth = if v.bit_depth > 0 {
        format!(" {}-bit", v.bit_depth)
    } else {
        String::new()
    };
    format!(
        "{} {:.0} fps{} {}",
        resolution_label(v),
        v.fps.round(),
        depth,
        codec_label(&v.codec)
    )
}

pub fn assess(platform: Platform, video: Option<&VideoInfo>) -> Assessment {
    let mut a = judge(platform, video);
    if let Some(v) = video
        && let Some((target, how)) = conversion(platform, v, a.verdict)
    {
        let lead = match a.verdict {
            Verdict::Hardware => "",
            Verdict::Software => "Optional, for less battery use:",
            Verdict::SoftwareMarginal => "If it stutters:",
            Verdict::Unplayable => "What to do:",
        };
        a.todo = Some(format!("{lead} convert it to {target}. {how}"));
        a.convert_to = Some(target);
    }
    a
}

/// The verdict and its explanation, without the advice.
fn judge(platform: Platform, video: Option<&VideoInfo>) -> Assessment {
    let Some(v) = video else {
        return Assessment {
            platform,
            verdict: Verdict::Unplayable,
            title: "No video track".into(),
            detail: Some("This file contains no video stream.".into()),
            hint: None,
            todo: None,
            convert_to: None,
            software_speed: None,
        };
    };
    // Two spatial views share one texture, at most 16384 px wide on these GPUs.
    let texture_width = v.width as u64 * v.views.max(1) as u64;
    if texture_width > 16384 || v.height > 16384 {
        return Assessment {
            platform,
            verdict: Verdict::Unplayable,
            title: format!("Too large to show ({})", describe(v)),
            detail: Some(format!(
                "{}×{} is beyond what the headset's GPU can hold as one picture.",
                texture_width, v.height
            )),
            hint: hint(platform, v),
            todo: None,
            convert_to: None,
            software_speed: None,
        };
    }
    let gap = platform.hardware_gap(v);
    if gap.is_none() {
        return Assessment {
            platform,
            verdict: Verdict::Hardware,
            title: format!("Plays with hardware decoding ({})", describe(v)),
            detail: None,
            hint: None,
            todo: None,
            convert_to: None,
            software_speed: None,
        };
    }
    let gap = gap.unwrap_or_default();
    // Unknown frame rate: assume 60 fps, the worst common case for VR.
    let fps = if v.fps > 0.0 { v.fps } else { 60.0 };
    // Spatial video decodes both views.
    let needed = v.width as f64 * v.height as f64 * fps * v.views.max(1) as f64;
    let speed = platform.software_rate(v).map(|rate| {
        if needed > 0.0 {
            rate / needed
        } else {
            f64::INFINITY
        }
    });
    let name = platform.name();
    let why_hw = capitalize(&gap);
    match speed {
        None => Assessment {
            platform,
            verdict: Verdict::Software,
            title: format!("Plays with CPU decoding ({})", describe(v)),
            detail: Some(format!("{why_hw}. It is decoded on the CPU instead.")),
            hint: None,
            todo: None,
            convert_to: None,
            software_speed: None,
        },
        Some(s) if s >= SOFTWARE_HEADROOM => Assessment {
            platform,
            verdict: Verdict::Software,
            title: format!("Plays with CPU decoding ({})", describe(v)),
            detail: Some(format!(
                "{why_hw}. It is decoded on the CPU instead: playback is smooth but uses more battery."
            )),
            hint: None,
            todo: None,
            convert_to: None,
            software_speed: Some(s),
        },
        Some(s) if s >= 1.0 => Assessment {
            platform,
            verdict: Verdict::SoftwareMarginal,
            title: format!("May stutter ({})", describe(v)),
            detail: Some(format!(
                "{why_hw}. The CPU can only just keep up with this video, so playback may stutter."
            )),
            hint: hint(platform, v),
            todo: None,
            convert_to: None,
            software_speed: Some(s),
        },
        Some(s) => Assessment {
            platform,
            verdict: Verdict::Unplayable,
            title: format!("Can't play smoothly on {name} ({})", describe(v)),
            detail: Some(format!(
                "{why_hw}. Decoding it on the CPU instead reaches only about {:.0}% of the speed needed, so playback would stutter badly.",
                (s * 100.0).floor()
            )),
            hint: hint(platform, v),
            todo: None,
            convert_to: None,
            software_speed: Some(s),
        },
    }
}

/// Suggests a variant of the same video that would play well.
fn hint(platform: Platform, v: &VideoInfo) -> Option<String> {
    let mut options = Vec::new();
    let eight_bit = VideoInfo {
        bit_depth: 8,
        codec: if v.codec == "av1" {
            "hevc".into()
        } else {
            v.codec.clone()
        },
        ..v.clone()
    };
    if platform.hardware_gap(&eight_bit).is_none() {
        options.push(format!(
            "an 8-bit {} version",
            codec_label(&eight_bit.codec)
        ));
    }
    // Quest: the largest size its hardware decoder takes, same shape.
    let blocks = v.width.div_ceil(16) as f64 * v.height.div_ceil(16) as f64;
    if platform == Platform::Quest && blocks > 138_240.0 {
        // The largest common width that fits, the height in proportion.
        let most = v.width as f64 * (138_240.0 / blocks).sqrt();
        let width = [7680, 6144, 5760, 5120, 4096, 3840, 2880, 1920]
            .into_iter()
            .find(|&w| w as f64 <= most)
            .unwrap_or(1920);
        let height =
            ((v.height as f64 * width as f64 / v.width as f64 / 16.0).floor() * 16.0) as u32;
        let smaller = VideoInfo {
            width,
            height,
            bit_depth: 8,
            codec: "hevc".into(),
            profile: Some("Main".into()),
            ..v.clone()
        };
        if platform.hardware_gap(&smaller).is_none() {
            options.push(format!(
                "an 8-bit HEVC version at {}x{}",
                smaller.width, smaller.height
            ));
        }
    }
    if v.width.max(v.height) > 4096 {
        let scale = 4096.0 / v.width.max(v.height) as f64;
        let smaller = VideoInfo {
            width: (v.width as f64 * scale) as u32,
            height: (v.height as f64 * scale) as u32,
            ..v.clone()
        };
        if matches!(
            judge(platform, Some(&smaller)).verdict,
            Verdict::Hardware | Verdict::Software
        ) {
            options.push("a 4K version".into());
        }
    }
    (!options.is_empty()).then(|| format!("{} of this video would play.", join_or(&options)))
}

/// For a video that doesn't use the hardware decoder: the first version of
/// it that would (target), and how to make it. Spatial video becomes side by
/// side; others become 8-bit HEVC at the same size, the largest size the
/// Quest's decoder takes, 30 fps, or 4K.
fn conversion(platform: Platform, v: &VideoInfo, verdict: Verdict) -> Option<(String, String)> {
    if verdict == Verdict::Hardware {
        return None;
    }
    let hevc = |width: u32, height: u32, fps: f64| VideoInfo {
        codec: "hevc".into(),
        profile: Some("Main".into()),
        pixel_format: Some("yuv420p".into()),
        bit_depth: 8,
        width,
        height,
        fps,
        multilayer: false,
        views: 1,
        ..v.clone()
    };
    let plays = |c: &VideoInfo| judge(platform, Some(c)).verdict == Verdict::Hardware;
    // The largest common width the Quest's decoder takes at this shape.
    let quest_fit = |w: u32, h: u32| -> (u32, u32) {
        let blocks = w.div_ceil(16) as f64 * h.div_ceil(16) as f64;
        if blocks <= 138_240.0 && w <= 8192 {
            return (w, h);
        }
        let most = (w as f64 * (138_240.0 / blocks).sqrt()).min(8192.0);
        let fit = [7680, 6144, 5760, 5120, 4096, 3840, 2880, 1920]
            .into_iter()
            .find(|&c| c as f64 <= most)
            .unwrap_or(1920);
        (
            fit,
            ((h as f64 * fit as f64 / w as f64 / 16.0).floor() * 16.0) as u32,
        )
    };
    const FIT: &str = "tools/fit-for-quest.sh does this on a Mac, or use HandBrake (H.265, 8-bit).";
    if v.multilayer || v.views >= 2 {
        // Both eyes side by side in one picture, which hardware decoders play.
        let (w, h) = quest_fit(v.width * 2, v.height);
        let sbs = hevc(w, h, v.fps);
        if !plays(&sbs) {
            return None;
        }
        let how = if w == v.width * 2 {
            "tools/spatial2sbs.sh does this on a Mac.".to_string()
        } else {
            format!("tools/spatial2sbs.sh --max-width {w} does this on a Mac.")
        };
        return Some((format!("side-by-side 3D ({w}x{h})"), how));
    }
    let same = hevc(v.width, v.height, v.fps);
    if plays(&same) {
        return Some(("8-bit HEVC at the same size".into(), FIT.into()));
    }
    let (w, h) = quest_fit(v.width, v.height);
    if plays(&hevc(w, h, v.fps)) {
        return Some((format!("8-bit HEVC at {w}x{h}"), FIT.into()));
    }
    if v.fps > 31.0 && plays(&hevc(w, h, 30.0)) {
        return Some((
            format!("8-bit HEVC at {w}x{h} and 30 fps"),
            "HandBrake does this (H.265, 8-bit, frame rate 30).".into(),
        ));
    }
    let side = v.width.max(v.height);
    if side > 3840 {
        let (w, h) = (
            (v.width as u64 * 3840 / side as u64) as u32 / 16 * 16,
            (v.height as u64 * 3840 / side as u64) as u32 / 16 * 16,
        );
        if plays(&hevc(w, h, v.fps.min(60.0))) {
            return Some((
                format!("8-bit HEVC at {w}x{h}"),
                "HandBrake does this (H.265, 8-bit, and that size).".into(),
            ));
        }
    }
    None
}

fn join_or(items: &[String]) -> String {
    match items {
        [one] => capitalize(one),
        [first, rest @ ..] => format!("{} or {}", capitalize(first), rest.join(" or ")),
        [] => String::new(),
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(codec: &str, width: u32, height: u32, depth: u32, fps: f64) -> VideoInfo {
        VideoInfo {
            codec: codec.into(),
            profile: None,
            pixel_format: None,
            width,
            height,
            bit_depth: depth,
            fps,
            stereo_mode: None,
            stereo_inverted: false,
            projection: None,
            horizontal_degrees: None,
            ..Default::default()
        }
    }

    fn frame(v: &VideoInfo) -> Assessment {
        assess(Platform::SteamFrame, Some(v))
    }

    #[test]
    fn eight_bit_h264_and_hevc_use_hardware() {
        assert_eq!(
            frame(&video("h264", 1920, 1080, 8, 24.0)).verdict,
            Verdict::Hardware
        );
        assert_eq!(
            frame(&video("hevc", 8192, 4096, 8, 60.0)).verdict,
            Verdict::Hardware
        );
    }

    #[test]
    fn user_library_8k60_hevc10_is_explained() {
        let a = frame(&video("hevc", 8192, 4096, 10, 59.94));
        assert_eq!(a.verdict, Verdict::Unplayable);
        let detail = a.detail.unwrap();
        assert!(a.title.contains("8K 60 fps 10-bit HEVC"), "{}", a.title);
        assert!(detail.contains("only handles 8-bit video"), "{detail}");
        // 1.5e9 / (8192*4096*59.94) ≈ 0.746 → "74%" (measured: 0.8×)
        assert!(detail.contains("74%"), "{detail}");
        let hint = a.hint.unwrap();
        assert!(hint.contains("8-bit HEVC") && hint.contains("4K"), "{hint}");
    }

    #[test]
    fn four_k_ten_bit_and_av1_fall_back_to_cpu() {
        let hevc = frame(&video("hevc", 3840, 2160, 10, 60.0));
        assert_eq!(hevc.verdict, Verdict::Software);
        assert!(hevc.detail.unwrap().contains("battery"));
        assert_eq!(
            frame(&video("av1", 3840, 2160, 10, 30.0)).verdict,
            Verdict::Software
        );
        // AV1 8K60: 0.8e9 / 2.0e9 → unplayable, suggests HEVC 8-bit.
        let av1 = frame(&video("av1", 7680, 3840, 10, 60.0));
        assert_eq!(av1.verdict, Verdict::Unplayable);
        assert!(av1.hint.unwrap().contains("8-bit HEVC"));
    }

    #[test]
    fn near_the_limit_warns() {
        // AV1 5.7K 30 fps: 0.8e9 / (5760*2880*30) ≈ 1.6 → Software; 5.7K 40 fps ≈ 1.2 → marginal.
        assert_eq!(
            frame(&video("av1", 5760, 2880, 10, 30.0)).verdict,
            Verdict::Software
        );
        assert_eq!(
            frame(&video("av1", 5760, 2880, 10, 40.0)).verdict,
            Verdict::SoftwareMarginal
        );
    }

    #[test]
    fn unconfirmed_bit_depth_never_uses_hardware() {
        let mut v = video("hevc", 3840, 2160, 0, 30.0);
        v.profile = Some("Main 10".into());
        let a = frame(&v);
        assert_ne!(a.verdict, Verdict::Hardware);
        assert!(a.detail.unwrap().contains("couldn't be confirmed"));
        v.profile = Some("Main".into());
        assert_eq!(frame(&v).verdict, Verdict::Hardware);
        let mut chroma = video("h264", 1920, 1080, 8, 30.0);
        chroma.pixel_format = Some("yuv422p".into());
        assert_ne!(frame(&chroma).verdict, Verdict::Hardware);
        // 8-bit first pictures, but a profile that allows 10-bit ones later.
        let mut mixed = video("hevc", 3840, 2160, 8, 30.0);
        mixed.profile = Some("Main 10".into());
        assert_ne!(frame(&mixed).verdict, Verdict::Hardware);
    }

    #[test]
    fn quest_limits_and_hint() {
        let mut v = video("h264", 7680, 7680, 8, 30.0);
        v.profile = Some("Main".into());
        v.pixel_format = Some("yuv420p".into());
        let a = assess(Platform::Quest, Some(&v));
        assert_ne!(
            a.verdict,
            Verdict::Hardware,
            "7680x7680 is over 138240 blocks"
        );
        assert!(a.hint.unwrap_or_default().contains("5760x5760"));
        let mut fits = video("hevc", 8192, 4096, 8, 30.0);
        fits.profile = Some("Main".into());
        fits.pixel_format = Some("yuv420p".into());
        assert_eq!(
            assess(Platform::Quest, Some(&fits)).verdict,
            Verdict::Hardware
        );
        fits.fps = 60.0;
        assert_ne!(
            assess(Platform::Quest, Some(&fits)).verdict,
            Verdict::Hardware
        );
    }

    #[test]
    fn spatial_video_never_uses_hardware_and_counts_both_views() {
        let mut v = video("hevc", 2200, 2200, 8, 30.0);
        v.profile = Some("Main".into());
        v.multilayer = true;
        v.views = 2;
        let a = frame(&v);
        assert_eq!(a.verdict, Verdict::Software);
        // Twice the pixels of one view: 2200 x 2200 x 30 x 2.
        let one_view = 1.5e9 / (2200.0 * 2200.0 * 30.0);
        assert!((a.software_speed.unwrap() - one_view / 2.0).abs() < 1e-9);
        // Two 8640-wide views don't fit one texture.
        v.width = 8640;
        v.height = 4320;
        assert_eq!(frame(&v).verdict, Verdict::Unplayable);
    }

    #[test]
    fn advice_says_what_to_convert_to() {
        // The 8K 360° files that the Quest 3 can't decode in hardware.
        let mut v = video("h264", 7680, 7680, 8, 30.0);
        v.profile = Some("High".into());
        v.pixel_format = Some("yuv420p".into());
        let a = assess(Platform::Quest, Some(&v));
        assert_eq!(a.convert_to.as_deref(), Some("8-bit HEVC at 5760x5760"));
        let todo = a.todo.unwrap();
        assert!(
            todo.starts_with("What to do: convert it to 8-bit HEVC at 5760x5760."),
            "{todo}"
        );
        assert!(todo.contains("tools/fit-for-quest.sh"));
        // 10-bit on the Frame: the same size, 8-bit.
        let a = frame(&video("hevc", 8192, 4096, 10, 59.94));
        assert_eq!(a.convert_to.as_deref(), Some("8-bit HEVC at the same size"));
        // Spatial video: side by side, smaller where the Quest needs it.
        let mut s = video("hevc", 1920, 1080, 8, 60.0);
        s.multilayer = true;
        s.views = 2;
        let a = assess(Platform::Quest, Some(&s));
        assert_eq!(a.convert_to.as_deref(), Some("side-by-side 3D (3840x1080)"));
        assert!(
            a.todo
                .unwrap()
                .starts_with("Optional, for less battery use:")
        );
        // Hardware decoding: nothing to do.
        let mut ok = video("hevc", 3840, 2160, 8, 30.0);
        ok.profile = Some("Main".into());
        ok.pixel_format = Some("yuv420p".into());
        assert!(assess(Platform::Quest, Some(&ok)).todo.is_none());
    }

    #[test]
    fn missing_video_is_unplayable() {
        let a = assess(Platform::SteamFrame, None);
        assert_eq!(a.verdict, Verdict::Unplayable);
        assert_eq!(a.title, "No video track");
    }
}
