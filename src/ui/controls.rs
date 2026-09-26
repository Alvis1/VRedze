//! The playback control bar (opened by clicking the video):
//!
//! ```text
//! [⏮][▶][⏭]                 [curve][2D][180° 3D][•••]
//! 12:34 / 1:32:10  ━━━━━━━━━━●──────────────────────────
//! ```
//!
//! Previous/next play the neighbouring videos in the folder; the format
//! buttons fix files whose VR metadata is missing ("•••" has every format and
//! swap eyes). Volume is left to the headset's buttons, and the thumbstick
//! click resets the screen.

use super::canvas::{Canvas, Fonts, Rgb};
use crate::vr::{Projection, Stereo};

pub const WIDTH: u32 = 1200;
pub const HEIGHT: u32 = 240;

const BG: Rgb = [0x15, 0x17, 0x1c];
const BUTTON: Rgb = [0x26, 0x2b, 0x34];
const HOVER: Rgb = [0x35, 0x3d, 0x4a];
const TEXT: Rgb = [0xe8, 0xea, 0xed];
const SUBTLE: Rgb = [0x9a, 0xa0, 0xa6];
const FAINT: Rgb = [0x4a, 0x4f, 0x58];
const ACCENT: Rgb = [0x4f, 0x8c, 0xff];
const TRACK: Rgb = [0x3a, 0x40, 0x4c];

/// Every format, as offered on the "•••" page.
pub const FORMATS: &[(Projection, Stereo)] = &[
    (Projection::Flat, Stereo::Mono),
    (Projection::Flat, Stereo::SideBySide),
    (Projection::Flat, Stereo::TopBottom),
    (Projection::Equirect180, Stereo::SideBySide),
    (Projection::Equirect180, Stereo::TopBottom),
    (Projection::Equirect180, Stereo::Mono),
    (Projection::Fisheye180, Stereo::SideBySide),
    (Projection::Equirect360, Stereo::Mono),
    (Projection::Equirect360, Stereo::TopBottom),
    (Projection::Equirect360, Stereo::SideBySide),
];

/// The two most common formats, one click each.
pub const FLAT: (Projection, Stereo) = (Projection::Flat, Stereo::Mono);
pub const VR180: (Projection, Stereo) = (Projection::Equirect180, Stereo::SideBySide);

fn short_label(projection: Projection, stereo: Stereo) -> String {
    let shape = match projection {
        Projection::Flat => "Flat",
        Projection::Equirect180 => "180°",
        Projection::Equirect360 => "360°",
        Projection::Fisheye180 => "Fisheye",
    };
    let depth = match stereo {
        Stereo::Mono => "2D",
        Stereo::SideBySide => "3D SBS",
        Stereo::TopBottom => "3D TB",
    };
    format!("{shape} {depth}")
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Previous,
    PlayPause,
    Next,
    /// Fraction of the duration.
    Seek(f32),
    Curved,
    /// Flat 2D.
    Flat,
    /// VR180 3D side by side.
    Vr180,
    /// Opens the page with every format.
    More,
    /// A format on the "•••" page (index into [`FORMATS`]).
    Pick(usize),
    SwapEyes,
    /// Back from the "•••" page.
    Back,
    Nothing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub paused: bool,
    pub position: f64,
    pub duration: f64,
    /// None when the curve toggle does not apply (VR180/360).
    pub curved: Option<bool>,
    pub projection: Projection,
    pub stereo: Stereo,
    pub swap_eyes: bool,
    pub has_previous: bool,
    pub has_next: bool,
    /// Showing the "•••" page.
    pub more: bool,
}

type Rect = (f32, f32, f32, f32);

const PREVIOUS: Rect = (24.0, 20.0, 96.0, 96.0);
const PLAY: Rect = (136.0, 20.0, 96.0, 96.0);
const NEXT: Rect = (248.0, 20.0, 96.0, 96.0);
const CURVED: Rect = (656.0, 20.0, 96.0, 96.0);
const FLAT_BUTTON: Rect = (768.0, 20.0, 120.0, 96.0);
const VR180_BUTTON: Rect = (904.0, 20.0, 160.0, 96.0);
const MORE: Rect = (1080.0, 20.0, 96.0, 96.0);
const SEEK: Rect = (330.0, 140.0, 846.0, 80.0);

/// The "•••" page: formats, then swap eyes and back, in a 4-column grid.
fn more_rects() -> Vec<(Hit, Rect)> {
    let (w, h, gap) = (276.0, 60.0, 16.0);
    let mut hits: Vec<Hit> = (0..FORMATS.len()).map(Hit::Pick).collect();
    hits.extend([Hit::SwapEyes, Hit::Back]);
    hits.into_iter()
        .enumerate()
        .map(|(i, hit)| {
            let (col, row) = ((i % 4) as f32, (i / 4) as f32);
            (hit, (24.0 + col * (w + gap), 20.0 + row * (h + 12.0), w, h))
        })
        .collect()
}

fn inside((x, y, w, h): Rect, px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

pub fn hit(state: &State, x: f32, y: f32) -> Hit {
    if state.more {
        return more_rects()
            .into_iter()
            .find(|(hit, r)| {
                inside(*r, x, y) && (*hit != Hit::SwapEyes || state.stereo != Stereo::Mono)
            })
            .map_or(Hit::Nothing, |(hit, _)| hit);
    }
    let buttons = [
        (PREVIOUS, Hit::Previous, state.has_previous),
        (PLAY, Hit::PlayPause, true),
        (NEXT, Hit::Next, state.has_next),
        (CURVED, Hit::Curved, state.curved.is_some()),
        (FLAT_BUTTON, Hit::Flat, true),
        (VR180_BUTTON, Hit::Vr180, true),
        (MORE, Hit::More, true),
    ];
    for (rect, hit, enabled) in buttons {
        if enabled && inside(rect, x, y) {
            return hit;
        }
    }
    // The seek bar is easy to hit: its whole row, a little beyond its ends.
    let (sx, sy, sw, sh) = SEEK;
    if state.duration > 0.0
        && y >= sy - 10.0
        && y <= sy + sh + 10.0
        && x >= sx - 20.0
        && x <= sx + sw + 20.0
    {
        return Hit::Seek(((x - sx) / sw).clamp(0.0, 1.0));
    }
    Hit::Nothing
}

pub fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn centered(c: &mut Canvas, fonts: &mut Fonts, label: &str, r: Rect, size: f32, color: Rgb) {
    let w = fonts.measure(label, size);
    fonts.draw(
        c,
        label,
        r.0 + (r.2 - w) / 2.0,
        r.1 + r.3 / 2.0 + size * 0.36,
        size,
        color,
        r.2,
    );
}

/// A right-pointing triangle from vertical strips, `h` tall at its base.
fn triangle(c: &mut Canvas, x: f32, cy: f32, w: f32, h: f32, flip: bool, color: Rgb) {
    let steps = (w * 2.0) as usize;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let half = h / 2.0 * (1.0 - t);
        let px = if flip { x + w - t * w } else { x + t * w };
        c.rect(px, cy - half, 1.5, half * 2.0, 0.0, color);
    }
}

pub fn render(state: &State, fonts: &mut Fonts, hover: Hit) -> Canvas {
    let mut c = Canvas::new(WIDTH, HEIGHT);
    c.clear(BG);
    let button = |c: &mut Canvas, r: Rect, hovered: bool, active: bool| {
        let color = if active {
            ACCENT
        } else if hovered {
            HOVER
        } else {
            BUTTON
        };
        c.rect(r.0, r.1, r.2, r.3, 18.0, color);
    };

    if state.more {
        let current = (state.projection, state.stereo);
        for (hit, r) in more_rects() {
            let (label, active) = match hit {
                Hit::Pick(i) => (
                    short_label(FORMATS[i].0, FORMATS[i].1),
                    FORMATS[i] == current,
                ),
                Hit::SwapEyes if state.stereo == Stereo::Mono => continue,
                Hit::SwapEyes => ("Swap eyes".to_string(), state.swap_eyes),
                _ => ("Back".to_string(), false),
            };
            button(&mut c, r, hover == hit, active);
            centered(&mut c, fonts, &label, r, 28.0, TEXT);
        }
        return c;
    }

    // Previous / play-pause / next.
    for (r, hit, enabled) in [
        (PREVIOUS, Hit::Previous, state.has_previous),
        (PLAY, Hit::PlayPause, true),
        (NEXT, Hit::Next, state.has_next),
    ] {
        button(&mut c, r, enabled && hover == hit, false);
        let color = if enabled { TEXT } else { FAINT };
        let (cx, cy) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
        match hit {
            Hit::PlayPause if state.paused => {
                triangle(&mut c, cx - 14.0, cy, 36.0, 48.0, false, color)
            }
            Hit::PlayPause => {
                c.rect(cx - 17.0, cy - 22.0, 12.0, 44.0, 3.0, color);
                c.rect(cx + 5.0, cy - 22.0, 12.0, 44.0, 3.0, color);
            }
            Hit::Previous => {
                c.rect(cx - 20.0, cy - 18.0, 7.0, 36.0, 2.0, color);
                triangle(&mut c, cx - 12.0, cy, 30.0, 36.0, true, color);
            }
            _ => {
                triangle(&mut c, cx - 18.0, cy, 30.0, 36.0, false, color);
                c.rect(cx + 13.0, cy - 18.0, 7.0, 36.0, 2.0, color);
            }
        }
    }

    // Curved screen: an arc seen from above.
    let curved = state.curved.unwrap_or(false);
    button(
        &mut c,
        CURVED,
        state.curved.is_some() && hover == Hit::Curved,
        curved,
    );
    let color = if state.curved.is_some() { TEXT } else { FAINT };
    let (cx, cy) = (CURVED.0 + CURVED.2 / 2.0, CURVED.1 + CURVED.3 / 2.0);
    for i in 0..=40 {
        let t = i as f32 / 20.0 - 1.0;
        c.rect(
            cx + t * 28.0 - 3.0,
            cy + 10.0 - 18.0 * (1.0 - t * t) - 3.0,
            6.0,
            6.0,
            3.0,
            color,
        );
    }

    // Formats.
    let current = (state.projection, state.stereo);
    button(&mut c, FLAT_BUTTON, hover == Hit::Flat, current == FLAT);
    centered(&mut c, fonts, "2D", FLAT_BUTTON, 32.0, TEXT);
    button(&mut c, VR180_BUTTON, hover == Hit::Vr180, current == VR180);
    centered(&mut c, fonts, "180° 3D", VR180_BUTTON, 32.0, TEXT);
    let other = current != FLAT && current != VR180;
    button(&mut c, MORE, hover == Hit::More, other);
    for i in 0..3 {
        c.circle(
            MORE.0 + MORE.2 / 2.0 + (i as f32 - 1.0) * 22.0,
            MORE.1 + MORE.3 / 2.0,
            6.0,
            TEXT,
        );
    }

    // Time and seek bar.
    let time = format!(
        "{} / {}",
        format_time(state.position),
        format_time(state.duration)
    );
    fonts.draw(
        &mut c,
        &time,
        24.0,
        SEEK.1 + SEEK.3 / 2.0 + 12.0,
        34.0,
        TEXT,
        SEEK.0 - 40.0,
    );
    let (sx, sy, sw, sh) = SEEK;
    let track_y = sy + sh / 2.0;
    c.rect(sx, track_y - 6.0, sw, 12.0, 6.0, TRACK);
    let fraction = if state.duration > 0.0 {
        (state.position / state.duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    c.rect(sx, track_y - 6.0, sw * fraction, 12.0, 6.0, ACCENT);
    if let Hit::Seek(f) = hover {
        c.rect(sx + sw * f - 2.0, track_y - 22.0, 4.0, 44.0, 2.0, SUBTLE);
        let label = format_time(f as f64 * state.duration);
        let w = fonts.measure(&label, 24.0);
        let lx = (sx + sw * f - w / 2.0).clamp(sx, sx + sw - w);
        fonts.draw(&mut c, &label, lx, sy + 6.0, 24.0, SUBTLE, w + 4.0);
    }
    c.circle(sx + sw * fraction, track_y, 14.0, TEXT);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            paused: false,
            position: 30.0,
            duration: 120.0,
            curved: Some(false),
            projection: Projection::Flat,
            stereo: Stereo::Mono,
            swap_eyes: false,
            has_previous: true,
            has_next: false,
            more: false,
        }
    }

    fn center(r: Rect) -> (f32, f32) {
        (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
    }

    #[test]
    fn buttons_and_seek_bar() {
        let s = state();
        assert_eq!(hit(&s, SEEK.0, SEEK.1 + 10.0), Hit::Seek(0.0));
        assert_eq!(
            hit(&s, SEEK.0 + SEEK.2 + 5.0, SEEK.1 + 10.0),
            Hit::Seek(1.0)
        );
        for (r, h) in [
            (PREVIOUS, Hit::Previous),
            (PLAY, Hit::PlayPause),
            (NEXT, Hit::Nothing), // no next video
            (CURVED, Hit::Curved),
            (FLAT_BUTTON, Hit::Flat),
            (VR180_BUTTON, Hit::Vr180),
            (MORE, Hit::More),
        ] {
            let (x, y) = center(r);
            assert_eq!(hit(&s, x, y), h);
            assert!(r.0 + r.2 <= WIDTH as f32 && r.1 + r.3 <= HEIGHT as f32);
        }
        let spherical = State { curved: None, ..s };
        let (x, y) = center(CURVED);
        assert_eq!(hit(&spherical, x, y), Hit::Nothing);
    }

    #[test]
    fn more_page_fits_and_hits() {
        let s = State {
            more: true,
            ..state()
        };
        for (h, r) in more_rects() {
            assert!(
                r.0 + r.2 <= WIDTH as f32 && r.1 + r.3 <= HEIGHT as f32,
                "{h:?} outside"
            );
            // Mono video: nothing to swap.
            let expect = if h == Hit::SwapEyes { Hit::Nothing } else { h };
            let (x, y) = center(r);
            assert_eq!(hit(&s, x, y), expect);
        }
    }

    #[test]
    fn times_read_naturally() {
        assert_eq!(format_time(75.0), "1:15");
        assert_eq!(format_time(5530.0), "1:32:10");
    }
}
