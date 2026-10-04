//! Video playback state: a decode thread feeding a small queue, and frame
//! selection by timestamp against the runtime's predicted display time.

use super::renderer::EyeParams;
use crate::media::{Picture, VideoDecoder};
use crate::subtitles::Cues;
use crate::vr::{Layout, Projection, Stereo};
use openxr as xr;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Presentation settings shared by all videos.
#[derive(Clone)]
pub struct ViewOptions {
    /// Fisheye lens field of view in degrees.
    pub fisheye_fov: f32,
    /// Flat screen width and distance in metres.
    pub screen_width: f32,
    pub screen_distance: f32,
    /// Shader debug view: 0 off, 1 projection UV, 2 raw YUV samples.
    pub debug_view: u32,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            fisheye_fov: 180.0,
            screen_width: 3.2,
            screen_distance: 3.0,
            debug_view: 0,
        }
    }
}

/// Where the video is placed around the viewer (changed by dragging).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Rotation around the viewer: yaw (left/right) and pitch (up/down), radians.
    pub yaw: f32,
    pub pitch: f32,
    /// Flat/curved screen distance in metres.
    pub distance: f32,
    pub curved: bool,
    /// Size multiplier: screen width for flat/curved, magnification for VR180/360.
    pub zoom: f32,
}

impl Placement {
    pub fn new(options: &ViewOptions) -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            distance: options.screen_distance,
            curved: false,
            zoom: 1.0,
        }
    }

    /// Rotation matrix (columns) of the placement: yaw about +Y, then pitch about +X.
    pub fn rotation(&self) -> [[f32; 3]; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        // R = Ry(yaw) * Rx(pitch)
        [
            [cy, 0.0, -sy],
            [sy * sp, cp, cy * sp],
            [sy * cp, -sp, cy * cp],
        ]
    }
}

/// Command-line playback extras (testing aids).
#[derive(Clone, Default)]
pub struct PlayOptions {
    /// Write the left eye as PNG once playback reaches this media time.
    pub screenshot: Option<(PathBuf, f64)>,
    /// Stop at this media time (seconds).
    pub duration: Option<f64>,
    /// Frames before this media time (after a seek to the previous keyframe) are skipped.
    pub start: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| (0..3).map(|k| m[k][i] * v[k]).sum())
    }

    #[test]
    fn placement_turns_the_screen_direction() {
        let mut p = Placement {
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
            distance: 3.0,
            curved: false,
            zoom: 1.0,
        };
        // Yaw 90° (left) moves the screen's forward (-Z) to -X.
        let f = apply(p.rotation(), [0.0, 0.0, -1.0]);
        assert!((f[0] + 1.0).abs() < 1e-5 && f[2].abs() < 1e-5, "{f:?}");
        p.yaw = 0.0;
        p.pitch = 0.3;
        let f = apply(p.rotation(), [0.0, 0.0, -1.0]);
        assert!(f[1] > 0.2, "pitch up raises the screen: {f:?}");
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct PlayStats {
    pub displayed_frames: u64,
    pub uploaded_frames: u64,
    pub skipped_frames: u64,
    pub xr_frames: u64,
    /// XR frames the runtime asked us to render (false while not visible).
    pub rendered_xr_frames: u64,
    pub media_seconds: f64,
    pub screenshot: Option<String>,
    /// Time spent copying pictures to the GPU staging buffer, total and worst.
    pub upload_seconds: f64,
    pub upload_max_seconds: f64,
    /// XR frames that came later than one display period after the last.
    pub late_xr_frames: u64,
}

impl PlayStats {
    /// One line for the log when a video stops.
    pub fn summary(&self) -> String {
        let uploads = self.uploaded_frames.max(1) as f64;
        format!(
            "{} pictures shown, {} skipped, {} XR frames late; upload {:.1} ms on average, {:.1} ms at worst",
            self.displayed_frames,
            self.skipped_frames,
            self.late_xr_frames,
            self.upload_seconds / uploads * 1e3,
            self.upload_max_seconds * 1e3,
        )
    }
}

enum Decoded {
    Frame(u64, Picture),
    End(u64),
    Failed(u64, String),
}

struct AudioChunk {
    generation: u64,
    /// Time of the first sample, seconds from the start of the video.
    pts: f64,
    samples: Vec<f32>,
}

/// State shared with the audio thread.
struct AudioShared {
    generation: AtomicU64,
    paused: AtomicBool,
    /// Volume 0..=1 as f32 bits.
    volume: AtomicU32,
    /// Media time heard at an instant, for the current generation.
    clock: Mutex<Option<(u64, f64, Instant)>>,
}

impl AudioShared {
    fn heard_now(&self) -> Option<(u64, f64)> {
        let clock = *self.clock.lock().expect("audio clock");
        clock.map(|(generation, position, at)| (generation, position + at.elapsed().as_secs_f64()))
    }
}

fn spawn_audio(
    name: String,
    chunks: mpsc::Receiver<AudioChunk>,
    shared: Arc<AudioShared>,
    stop: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            let mut output = match crate::audio::Output::open(&name) {
                Ok(output) => Some(output),
                Err(e) => {
                    eprintln!("Audio disabled: {e:#}");
                    None
                }
            };
            let channels = crate::audio::CHANNELS as usize;
            let rate = crate::audio::RATE as f64;
            let mut played_generation = 0;
            while !stop.load(Ordering::Relaxed) {
                let chunk = match chunks.recv_timeout(Duration::from_millis(50)) {
                    Ok(chunk) => chunk,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };
                let Some(out) = output.as_mut() else { continue };
                if chunk.generation != shared.generation.load(Ordering::Relaxed) {
                    continue; // from before a seek
                }
                if chunk.generation != played_generation {
                    out.flush();
                    played_generation = chunk.generation;
                }
                // ~10 ms slices keep pause and seek responsive.
                let slice = (rate as usize / 100) * channels;
                let mut offset = 0;
                while offset < chunk.samples.len() {
                    if stop.load(Ordering::Relaxed)
                        || chunk.generation != shared.generation.load(Ordering::Relaxed)
                    {
                        break;
                    }
                    if shared.paused.load(Ordering::Relaxed) {
                        *shared.clock.lock().expect("audio clock") = None;
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    let end = (offset + slice).min(chunk.samples.len());
                    let level = f32::from_bits(shared.volume.load(Ordering::Relaxed));
                    // Perceptual curve: bar steps sound evenly spaced.
                    let gain = level * level;
                    let scaled: Vec<f32> = chunk.samples[offset..end]
                        .iter()
                        .map(|s| s * gain)
                        .collect();
                    if let Err(e) = out.write(&scaled) {
                        eprintln!("{e:#}");
                        return;
                    }
                    offset = end;
                    let written_until = chunk.pts + (offset / channels) as f64 / rate;
                    let heard = written_until - out.latency();
                    *shared.clock.lock().expect("audio clock") =
                        Some((chunk.generation, heard, Instant::now()));
                }
            }
        })
        .expect("spawn audio thread");
}

struct DecodeThread {
    frames: mpsc::Receiver<Decoded>,
    control: mpsc::Sender<(u64, f64)>,
    requested: Arc<AtomicU64>,
    /// Which embedded subtitle track to decode (None: none).
    subtitle_track: mpsc::Sender<Option<usize>>,
    /// Which audio track to play (a seek follows).
    audio_track: mpsc::Sender<usize>,
}

fn spawn_decoder(
    mut decoder: VideoDecoder,
    start: f64,
    audio: Option<mpsc::Sender<AudioChunk>>,
    stop: Arc<AtomicBool>,
    embedded_cues: Arc<Mutex<Cues>>,
) -> DecodeThread {
    let (subtitle_track, subtitle_changes) = mpsc::channel::<Option<usize>>();
    let (audio_track, audio_changes) = mpsc::channel::<usize>();
    // A few frames of slack absorb decode jitter; more would only cost memory.
    // A hardware (V4L2) decoder's pictures live in its few capture buffers,
    // and it stalls when we hold too many of them (it needs about four free
    // for reference pictures). The shown picture is released once uploaded,
    // so with two queued at most three (next, queued) are held while it decodes.
    let slack = if decoder.stats().hw_backend.is_some() {
        2
    } else {
        4
    };
    let (tx, frames) = mpsc::sync_channel(slack);
    let (control, commands) = mpsc::channel::<(u64, f64)>();
    let requested = Arc::new(AtomicU64::new(0));
    let pending = requested.clone();
    std::thread::Builder::new()
        .name("decode".into())
        .spawn(move || {
            let (mut generation, mut start) = (0u64, start);
            // Resuming: jump to the keyframe before the resume point instead
            // of decoding everything before it.
            if start > 0.0
                && let Err(e) = decoder.seek(start)
            {
                eprintln!("Seek to {start:.1}s failed: {e:#}");
            }
            'decode: loop {
                // Before seeks: the seek that follows a switch restarts the new track.
                while let Ok(track) = audio_changes.try_recv() {
                    if !decoder.select_audio(track) {
                        eprintln!("Audio: can't decode track {track}");
                    }
                }
                // Only the latest of several seeks matters (scrubbing sends many).
                let mut seek = None;
                while let Ok(command) = commands.try_recv() {
                    seek = Some(command);
                }
                if let Some((g, t)) = seek {
                    if let Err(e) = decoder.seek(t) {
                        eprintln!("Seek to {t:.1}s failed: {e:#}");
                    }
                    (generation, start) = (g, t);
                }
                while let Ok(track) = subtitle_changes.try_recv() {
                    embedded_cues.lock().expect("cues").clear();
                    if !decoder.select_subtitle(track) {
                        eprintln!("Subtitles: can't decode track {track:?}");
                    }
                }
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                // Spatial video: both views of a picture travel together.
                let message = match decoder.next_picture() {
                    Ok(Some(picture)) => Decoded::Frame(generation, picture),
                    Ok(None) => Decoded::End(generation),
                    Err(e) => Decoded::Failed(generation, format!("{e:#}")),
                };
                let cues = decoder.take_subtitles();
                if !cues.is_empty() {
                    let mut shared = embedded_cues.lock().expect("cues");
                    for cue in cues {
                        shared.insert(cue);
                    }
                }
                if let Some(audio) = &audio {
                    while let Some((samples, pts)) = decoder.take_audio(crate::audio::CHANNELS) {
                        let pts = pts.unwrap_or(start);
                        // After a seek, skip sound from before the target.
                        let skip = (((start - pts) * crate::audio::RATE as f64).max(0.0) as usize
                            * crate::audio::CHANNELS as usize)
                            .min(samples.len());
                        if skip < samples.len() {
                            let pts = pts
                                + (skip / crate::audio::CHANNELS as usize) as f64
                                    / crate::audio::RATE as f64;
                            let _ = audio.send(AudioChunk {
                                generation,
                                pts,
                                samples: samples[skip..].to_vec(),
                            });
                        }
                    }
                }
                // Seeking lands on the keyframe before `start`; decode through to it.
                if let Decoded::Frame(_, frame) = &message
                    && frame.pts().is_some_and(|t| t < start)
                {
                    continue;
                }
                // A seek arrived while this was decoding: drop it, so no picture
                // of the old position keeps its decoder session alive.
                if pending.load(Ordering::Relaxed) != generation {
                    continue;
                }
                let finished = !matches!(message, Decoded::Frame(..));
                let mut message = Some(message);
                while let Some(m) = message.take() {
                    match tx.try_send(m) {
                        Ok(()) => {}
                        Err(mpsc::TrySendError::Full(m)) => {
                            if stop.load(Ordering::Relaxed) {
                                return;
                            }
                            if pending.load(Ordering::Relaxed) != generation {
                                continue 'decode; // a seek is waiting; drop this frame
                            }
                            message = Some(m);
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => return,
                    }
                }
                if finished {
                    // Wait for a seek (e.g. back from the end) or shutdown.
                    while pending.load(Ordering::Relaxed) == generation {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            }
        })
        .expect("spawn decode thread");
    DecodeThread {
        frames,
        control,
        requested,
        subtitle_track,
        audio_track,
    }
}

/// "Korean · Surround", "English 5.1"…
fn audio_label(index: usize, t: &crate::media::AudioTrackInfo) -> String {
    let mut label = t
        .language
        .as_deref()
        .map(language_name)
        .unwrap_or_else(|| format!("Track {}", index + 1));
    match t.title.as_deref() {
        Some(title) if !title.eq_ignore_ascii_case(&label) => label = format!("{label} · {title}"),
        _ => {
            let channels = match t.channels {
                1 => "mono".to_string(),
                2 => "stereo".to_string(),
                6 => "5.1".to_string(),
                8 => "7.1".to_string(),
                0 => String::new(),
                n => format!("{n} ch"),
            };
            if !channels.is_empty() {
                label = format!("{label} {channels}");
            }
        }
    }
    label
}

/// A subtitle track the viewer can pick.
enum SubtitleSource {
    /// Index into the file's subtitle streams; cues arrive while decoding.
    Embedded(usize),
    /// A .srt file next to the video.
    External(Cues),
}

struct SubtitleTrack {
    label: String,
    source: SubtitleSource,
}

/// "English", "Svenska"… for common ISO 639-2 codes; the code otherwise.
fn language_name(code: &str) -> String {
    let name = match code.to_ascii_lowercase().as_str() {
        "eng" | "en" => "English",
        "swe" | "sv" => "Swedish",
        "nor" | "nob" | "no" => "Norwegian",
        "dan" | "da" => "Danish",
        "fin" | "fi" => "Finnish",
        "ger" | "deu" | "de" => "German",
        "fre" | "fra" | "fr" => "French",
        "spa" | "es" => "Spanish",
        "ita" | "it" => "Italian",
        "por" | "pt" => "Portuguese",
        "dut" | "nld" | "nl" => "Dutch",
        "pol" | "pl" => "Polish",
        "rus" | "ru" => "Russian",
        "jpn" | "ja" => "Japanese",
        "kor" | "ko" => "Korean",
        "chi" | "zho" | "zh" => "Chinese",
        "ara" | "ar" => "Arabic",
        "est" | "et" => "Estonian",
        "lav" | "lv" => "Latvian",
        "lit" | "lt" => "Lithuanian",
        "ukr" | "uk" => "Ukrainian",
        "hin" | "hi" => "Hindi",
        "tam" | "ta" => "Tamil",
        "tel" | "te" => "Telugu",
        "cze" | "ces" | "cs" => "Czech",
        "slo" | "slk" | "sk" => "Slovak",
        "slv" | "sl" => "Slovenian",
        "hun" | "hu" => "Hungarian",
        "rum" | "ron" | "ro" => "Romanian",
        "bul" | "bg" => "Bulgarian",
        "hrv" | "hr" => "Croatian",
        "srp" | "sr" => "Serbian",
        "gre" | "ell" | "el" => "Greek",
        "tur" | "tr" => "Turkish",
        "heb" | "he" => "Hebrew",
        "tha" | "th" => "Thai",
        "vie" | "vi" => "Vietnamese",
        "ind" | "id" => "Indonesian",
        "may" | "msa" | "ms" => "Malay",
        "ice" | "isl" | "is" => "Icelandic",
        "cat" | "ca" => "Catalan",
        "per" | "fas" | "fa" => "Persian",
        _ => return code.to_string(),
    };
    name.to_string()
}

pub struct Playback {
    pub layout: Layout,
    /// Picture corrections and rotation.
    pub image: crate::config::ImageAdjust,
    /// The track CC turns back on.
    last_subtitle: usize,
    subtitle_tracks: Vec<SubtitleTrack>,
    subtitle: Option<usize>,
    embedded_cues: Arc<Mutex<Cues>>,
    /// Briefly shown instead of subtitles ("Subtitles: English"), until then.
    subtitle_notice: Option<(String, Instant)>,
    audio_labels: Vec<String>,
    audio_track: Option<usize>,
    decode: DecodeThread,
    audio: Option<Arc<AudioShared>>,
    stop: Arc<AtomicBool>,
    generation: u64,
    current: Option<Picture>,
    next: Option<Picture>,
    ended: bool,
    pub error: Option<String>,
    fps: f64,
    /// Coded views of the video: 2 for spatial (MV-HEVC) video.
    pub views: u32,
    /// A picture was shown since the start or the last seek.
    shown_since_seek: bool,
    /// A picture was uploaded and released: it is on screen.
    showing: bool,
    pub duration: f64,
    last_pts: f64,
    /// Display time (ns) corresponding to media time 0.
    clock_start: Option<i64>,
    paused_at: Option<i64>,
    pub stats: PlayStats,
    /// The version of the video that would play well ("8-bit HEVC at
    /// 5760x5760"), for the notice when playback can't keep up.
    pub convert_to: Option<String>,
    slow: SlowWatch,
}

/// Whether decoding keeps up, judged over a few seconds of playback.
#[derive(Default)]
struct SlowWatch {
    since: Option<Instant>,
    /// Pictures due in this stretch, and how many of them came late or were skipped.
    pictures: u32,
    late: u32,
    /// Most were late: the user should hear why (once per video).
    detected: bool,
    told: bool,
}

/// A picture shown this far behind the clock counts as late.
const LATE: f64 = 0.15;
/// Stretches of playback judged at a time.
const SLOW_STRETCH: Duration = Duration::from_secs(3);

impl Playback {
    /// Starts decoding (and sound, when the file has audio) at `start` seconds.
    pub fn start(mut decoder: VideoDecoder, layout: Layout, start: f64, volume: f32) -> Self {
        // Embedded text subtitles; the file's default track is on from the start.
        let embedded: Vec<(usize, &crate::media::SubtitleTrackInfo)> = decoder
            .info()
            .subtitles
            .iter()
            .enumerate()
            .filter(|(_, t)| t.supported)
            .collect();
        let subtitle_tracks: Vec<SubtitleTrack> = embedded
            .iter()
            .map(|(i, t)| {
                let mut label = t
                    .language
                    .as_deref()
                    .map(language_name)
                    .unwrap_or_else(|| format!("Track {}", i + 1));
                if let Some(title) = t
                    .title
                    .as_deref()
                    .filter(|title| !title.eq_ignore_ascii_case(&label))
                {
                    label = format!("{label} ({title})");
                }
                SubtitleTrack {
                    label,
                    source: SubtitleSource::Embedded(*i),
                }
            })
            .collect();
        let subtitle = embedded
            .iter()
            .position(|(_, t)| t.default)
            .or((!embedded.is_empty()).then_some(0));
        if let Some(SubtitleSource::Embedded(track)) = subtitle.map(|i| &subtitle_tracks[i].source)
        {
            decoder.select_subtitle(Some(*track));
        }
        let embedded_cues = Arc::new(Mutex::new(Cues::default()));
        let info = decoder.info();
        let fps = info.video.as_ref().map_or(30.0, |v| v.fps.max(1.0));
        let views = info.video.as_ref().map_or(1, |v| v.views);
        let duration = info.duration_seconds;
        let stop = Arc::new(AtomicBool::new(false));
        let has_audio = decoder.enable_audio(crate::audio::RATE, crate::audio::CHANNELS);
        let audio_labels: Vec<String> = decoder
            .info()
            .audio_tracks
            .iter()
            .enumerate()
            .map(|(i, t)| audio_label(i, t))
            .collect();
        let audio_track = decoder.info().audio_track.filter(|_| has_audio);
        let audio = has_audio.then(|| {
            Arc::new(AudioShared {
                generation: AtomicU64::new(0),
                paused: AtomicBool::new(false),
                volume: AtomicU32::new(volume.clamp(0.0, 1.0).to_bits()),
                clock: Mutex::new(None),
            })
        });
        let audio_tx = audio.as_ref().map(|shared| {
            let (tx, rx) = mpsc::channel();
            spawn_audio("Video".into(), rx, shared.clone(), stop.clone());
            tx
        });
        Self {
            layout,
            image: Default::default(),
            last_subtitle: subtitle.unwrap_or(0),
            decode: spawn_decoder(
                decoder,
                start,
                audio_tx,
                stop.clone(),
                embedded_cues.clone(),
            ),
            subtitle_tracks,
            subtitle,
            embedded_cues,
            subtitle_notice: None,
            audio_labels,
            audio_track,
            audio,
            stop,
            generation: 0,
            current: None,
            next: None,
            ended: false,
            error: None,
            fps,
            views,
            shown_since_seek: false,
            showing: false,
            duration,
            last_pts: start - 1.0 / fps,
            clock_start: None,
            paused_at: None,
            stats: PlayStats::default(),
            convert_to: None,
            slow: SlowWatch::default(),
        }
    }

    /// Adds subtitle files found next to the video; the first one is shown
    /// (a file placed next to a video is usually there to be used).
    pub fn add_external_subtitles(&mut self, files: Vec<crate::library::ExternalSubtitles>) {
        if files.is_empty() {
            return;
        }
        let count = files.len();
        let external = files.into_iter().map(|f| SubtitleTrack {
            label: f.name.clone(),
            source: SubtitleSource::External(Cues::new(f.cues)),
        });
        self.subtitle_tracks.splice(0..0, external);
        self.subtitle = self.subtitle.map(|i| i + count);
        self.select_subtitle(Some(0), false);
    }

    pub fn has_subtitles(&self) -> bool {
        !self.subtitle_tracks.is_empty()
    }

    pub fn subtitles_on(&self) -> bool {
        self.subtitle.is_some()
    }

    /// CC: subtitles off, or back on with the last track shown.
    pub fn toggle_subtitles(&mut self) {
        if self.subtitle_tracks.is_empty() {
            return;
        }
        let next = match self.subtitle {
            Some(_) => None,
            None => Some(self.last_subtitle.min(self.subtitle_tracks.len() - 1)),
        };
        self.select_subtitle(next, true);
    }

    fn select_subtitle(&mut self, index: Option<usize>, announce: bool) {
        if let Some(i) = index {
            self.last_subtitle = i;
        }
        let embedded =
            |i: Option<usize>, tracks: &[SubtitleTrack]| match i.map(|i| &tracks[i].source) {
                Some(SubtitleSource::Embedded(t)) => Some(*t),
                _ => None,
            };
        let (before, after) = (
            embedded(self.subtitle, &self.subtitle_tracks),
            embedded(index, &self.subtitle_tracks),
        );
        if before != after {
            let _ = self.decode.subtitle_track.send(after);
        }
        self.subtitle = index;
        if announce {
            let text = match index {
                Some(i) => format!("Subtitles: {}", self.subtitle_tracks[i].label),
                None => "Subtitles off".to_string(),
            };
            self.notice(text, Duration::from_millis(1800));
        }
    }

    /// Shows `text` where subtitles go for a while.
    pub fn notice(&mut self, text: String, duration: Duration) {
        self.subtitle_notice = Some((text, Instant::now() + duration));
    }

    pub fn audio_labels(&self) -> &[String] {
        &self.audio_labels
    }

    pub fn audio_index(&self) -> Option<usize> {
        self.audio_track
    }

    /// Switches to audio track `index`, continuing from the current picture.
    pub fn set_audio_track(&mut self, index: usize) {
        if self.audio_track.is_none()
            || self.audio_track == Some(index)
            || index >= self.audio_labels.len()
        {
            return;
        }
        let _ = self.decode.audio_track.send(index);
        self.audio_track = Some(index);
        self.notice(
            format!("Audio: {}", self.audio_labels[index]),
            Duration::from_millis(1800),
        );
        self.seek(self.position());
    }

    pub fn subtitle_labels(&self) -> Vec<String> {
        self.subtitle_tracks
            .iter()
            .map(|t| t.label.clone())
            .collect()
    }

    pub fn subtitle_index(&self) -> Option<usize> {
        self.subtitle
    }

    /// Shows track `index` (None: off), with a short notice.
    pub fn set_subtitle(&mut self, index: Option<usize>) {
        if index.is_none_or(|i| i < self.subtitle_tracks.len()) {
            self.select_subtitle(index, true);
        }
    }

    /// Off → each track in turn → off.
    pub fn cycle_subtitles(&mut self) {
        if self.subtitle_tracks.is_empty() {
            return;
        }
        let next = match self.subtitle {
            None => Some(0),
            Some(i) if i + 1 < self.subtitle_tracks.len() => Some(i + 1),
            Some(_) => None,
        };
        self.select_subtitle(next, true);
    }

    /// The subtitle for the frame on screen (or a short notice after switching).
    pub fn caption(&self) -> Option<crate::subtitles::Caption> {
        if let Some((text, until)) = &self.subtitle_notice
            && Instant::now() < *until
        {
            return Some(crate::subtitles::Caption::text(text.clone()));
        }
        let time = self.position();
        match &self.subtitle_tracks.get(self.subtitle?)?.source {
            SubtitleSource::External(cues) => cues.caption(time),
            SubtitleSource::Embedded(_) => self.embedded_cues.lock().expect("cues").caption(time),
        }
    }

    /// The picture was copied to the GPU: let go of it, returning its
    /// decoder buffer (a hardware decoder has few). It stays on screen.
    pub fn uploaded(&mut self) {
        if self.current.take().is_some() {
            self.showing = true;
        }
    }

    /// Whether a picture is on screen.
    pub fn showing(&self) -> bool {
        self.showing || self.current.is_some()
    }

    pub fn current(&self) -> Option<&Picture> {
        self.current.as_ref()
    }

    pub fn paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }

    pub fn set_volume(&self, volume: f32) {
        if let Some(audio) = &self.audio {
            audio
                .volume
                .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// Media time shown at display time `now` (ns), once the clock started.
    pub fn media_time(&self, now: i64) -> Option<f64> {
        let now = self.paused_at.unwrap_or(now);
        self.clock_start.map(|start| (now - start) as f64 / 1e9)
    }

    /// Current position for the UI: the shown frame's time.
    pub fn position(&self) -> f64 {
        self.last_pts.max(0.0)
    }

    pub fn toggle_pause(&mut self, now: i64) {
        match self.paused_at.take() {
            // Shift the clock so playback continues where it stopped.
            Some(at) => {
                if let Some(start) = &mut self.clock_start {
                    *start += now - at;
                }
            }
            None => self.paused_at = Some(now),
        }
        if let Some(audio) = &self.audio {
            audio.paused.store(self.paused(), Ordering::Relaxed);
        }
    }

    /// Ends playback with an error (shown on returning to the browser).
    pub fn fail(&mut self, message: String) {
        eprintln!("Playback stopped: {message}");
        self.error = Some(message);
        self.ended = true;
        self.next = None;
    }

    /// Jumps to `seconds`; the current picture stays until the new one arrives.
    pub fn seek(&mut self, seconds: f64) {
        let target = seconds.clamp(0.0, (self.duration - 0.5).max(0.0));
        self.generation += 1;
        self.decode
            .requested
            .store(self.generation, Ordering::Relaxed);
        if let Some(audio) = &self.audio {
            audio.generation.store(self.generation, Ordering::Relaxed);
            *audio.clock.lock().expect("audio clock") = None;
        }
        // Drop queued pictures of the old position first, so their decoder
        // session can close before the decoder opens a new one.
        while self.decode.frames.try_recv().is_ok() {}
        let _ = self.decode.control.send((self.generation, target));
        self.next = None;
        self.ended = false;
        self.error = None;
        self.clock_start = None;
        self.last_pts = target;
        self.shown_since_seek = false;
        // Pictures right after a seek are late by design.
        self.slow.since = None;
        self.slow.pictures = 0;
        self.slow.late = 0;
        // When paused, playback stays paused and shows the new position
        // (see `advance`).
    }

    /// Moves to the newest decoded frame due at `now`; true if it changed.
    pub fn advance(&mut self, now: i64) -> bool {
        if self.paused() && self.clock_start.is_some() {
            return false;
        }
        self.sync_to_audio(now);
        let mut media_time = self.media_time(now);
        let mut changed = false;
        let skipped_before = self.stats.skipped_frames;
        loop {
            if self.next.is_none() && !self.ended {
                match self.decode.frames.try_recv() {
                    Ok(Decoded::Frame(g, frame)) if g == self.generation => self.next = Some(frame),
                    Ok(Decoded::End(g)) if g == self.generation => self.ended = true,
                    Ok(Decoded::Failed(g, e)) if g == self.generation => {
                        eprintln!("Decoding stopped: {e}");
                        self.error = Some(e);
                        self.ended = true;
                    }
                    Ok(_) => continue, // from before a seek
                    Err(mpsc::TryRecvError::Disconnected) => self.ended = true,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            let Some(candidate) = self.next.as_ref() else {
                break;
            };
            let pts = candidate.pts().unwrap_or(self.last_pts + 1.0 / self.fps);
            if media_time.is_some_and(|t| pts > t) {
                break;
            }
            if self.clock_start.is_none() {
                // The first frame starts the clock: it is due exactly now.
                self.clock_start = Some(now - (pts * 1e9) as i64);
                if let Some(at) = &mut self.paused_at {
                    *at = now;
                }
                media_time = Some(pts);
            }
            if changed {
                self.stats.skipped_frames += 1;
            }
            self.last_pts = pts;
            self.current = self.next.take();
            self.shown_since_seek = true;
            changed = true;
            if self.paused() {
                break; // paused after a seek: show just the target frame
            }
        }
        if let Some(t) = media_time {
            self.stats.media_seconds = t;
            if changed {
                let skipped = (self.stats.skipped_frames - skipped_before) as u32;
                self.watch_speed(t - self.last_pts, skipped);
            }
        }
        changed
    }

    /// Counts pictures that came late (or were skipped to catch up); a
    /// stretch of playback where most did means decoding can't keep up.
    fn watch_speed(&mut self, lag: f64, skipped: u32) {
        let w = &mut self.slow;
        if w.detected {
            return;
        }
        let now = Instant::now();
        let since = *w.since.get_or_insert(now);
        w.pictures += 1 + skipped;
        if lag > LATE || skipped > 0 {
            w.late += 1 + skipped;
        }
        if now.duration_since(since) >= SLOW_STRETCH {
            if w.pictures >= 20 && w.late * 2 > w.pictures {
                w.detected = true;
                eprintln!(
                    "Playing slowly: {} of {} pictures late in {:.0} s",
                    w.late,
                    w.pictures,
                    SLOW_STRETCH.as_secs_f64()
                );
            }
            w.since = Some(now);
            w.pictures = 0;
            w.late = 0;
        }
    }

    /// Once per video, when decoding falls behind: what to tell the user.
    pub fn take_slow_notice(&mut self) -> Option<String> {
        if !self.slow.detected || std::mem::replace(&mut self.slow.told, true) {
            return None;
        }
        Some(match &self.convert_to {
            Some(target) => format!(
                "Playing slowly: this headset can't decode this video in time. Convert it to {target} (point at it in the list for how)."
            ),
            None => "Playing slowly: the video can't be read or decoded in time. If it's on a network share, copy it to the headset.".into(),
        })
    }

    /// Keeps the video clock on the audio clock (what is actually heard).
    fn sync_to_audio(&mut self, now: i64) {
        let (Some(audio), Some(start)) = (&self.audio, self.clock_start.as_mut()) else {
            return;
        };
        if self.paused_at.is_some() {
            return;
        }
        let Some((generation, heard)) = audio.heard_now() else {
            return;
        };
        if generation != self.generation {
            return;
        }
        // Frames are shown at the predicted display time, slightly after now.
        const DISPLAY_LEAD: f64 = 0.02;
        let video = (now - *start) as f64 / 1e9;
        let error = video - (heard + DISPLAY_LEAD);
        let correction = if error.abs() > 0.25 {
            error
        } else {
            error * 0.1
        };
        *start += (correction * 1e9) as i64;
    }

    /// Whether a picture was shown since the start or the last seek.
    pub fn shown_since_seek(&self) -> bool {
        self.shown_since_seek
    }

    /// True once the stream ended and its last frame has been shown for a second.
    pub fn finished(&self, now: i64) -> bool {
        self.shown_last_frame_for(now, 1.0)
    }

    /// True once the stream ended and its last frame has been on screen for
    /// one frame's time: when looping, the video starts over without a pause.
    pub fn reached_end(&self, now: i64) -> bool {
        self.shown_last_frame_for(now, 1.0 / self.fps)
    }

    fn shown_last_frame_for(&self, now: i64, seconds: f64) -> bool {
        self.ended
            && self.next.is_none()
            && self
                .media_time(now)
                .is_none_or(|t| t > self.last_pts + seconds)
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn quat_to_columns(q: xr::Quaternionf) -> [[f32; 4]; 3] {
    let (x, y, z, w) = (q.x, q.y, q.z, q.w);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
            0.0,
        ],
        [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
            0.0,
        ],
        [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
            0.0,
        ],
    ]
}

fn mat_mul_t(a: [[f32; 3]; 3], b: [[f32; 4]; 3]) -> [[f32; 4]; 3] {
    // Aᵀ·B with A, B given as columns.
    let mut out = [[0.0; 4]; 3];
    for (j, col) in b.iter().enumerate() {
        for (i, a_col) in a.iter().enumerate() {
            out[j][i] = (0..3).map(|k| a_col[k] * col[k]).sum();
        }
    }
    out
}

/// Shader parameters for one eye.
pub fn eye_params(
    view: &xr::View,
    eye: usize,
    layout: &Layout,
    tex: (u32, u32),
    options: &ViewOptions,
    placement: &Placement,
    quarter_turns: u8,
) -> EyeParams {
    let (w, h) = (tex.0 as f32, tex.1 as f32);
    // Flat screen aspect from one eye's part of the frame (turned on its side
    // when the picture is rotated a quarter).
    let mut aspect = crate::vr::eye_aspect(tex.0, tex.1, layout);
    if quarter_turns % 2 == 1 {
        aspect = 1.0 / aspect.max(0.01);
    }
    // Express the eye in the placement's frame: the shader's screen/sphere is
    // fixed there, so rotating the placement moves the video around the viewer.
    let placed = placement.rotation();
    let p = view.pose.position;
    let origin = mat_mul_t(placed, [[p.x, p.y, p.z, 0.0], [0.0; 4], [0.0; 4]])[0];
    let curved = layout.projection == Projection::Flat && placement.curved;
    EyeParams {
        rot: mat_mul_t(placed, quat_to_columns(view.pose.orientation)),
        tan: [
            view.fov.angle_left.tan(),
            view.fov.angle_right.tan(),
            view.fov.angle_down.tan(),
            view.fov.angle_up.tan(),
        ],
        origin: [origin[0], origin[1], origin[2], eye as f32],
        mode: [
            match layout.projection {
                Projection::Flat if curved => 4.0,
                Projection::Flat => 0.0,
                Projection::Equirect180 => 1.0,
                Projection::Equirect360 => 2.0,
                Projection::Fisheye180 => 3.0,
            },
            match layout.stereo {
                Stereo::Mono => 0.0,
                // Spatial video: the two views sit side by side in the texture.
                Stereo::SideBySide | Stereo::MultiView => 1.0,
                Stereo::TopBottom => 2.0,
            },
            if layout.swap_eyes { 1.0 } else { 0.0 },
            options.fisheye_fov.to_radians(),
        ],
        screen: [
            options.screen_width * placement.zoom,
            options.screen_width * placement.zoom / aspect.max(0.1),
            placement.distance,
            0.0,
        ],
        tex: [w, h, options.debug_view as f32, placement.zoom],
    }
}
