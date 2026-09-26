//! The playback control bar (opened with A): play/pause, time and seek bar,
//! curved-screen toggle, reset position, and volume as equal-size bars.

use super::canvas::{Canvas, Fonts, Rgb};

pub const WIDTH: u32 = 1200;
pub const HEIGHT: u32 = 260;
pub const VOLUME_STEPS: u32 = 10;

const BG: Rgb = [0x15, 0x17, 0x1c];
const BUTTON: Rgb = [0x26, 0x2b, 0x34];
const HOVER: Rgb = [0x35, 0x3d, 0x4a];
const TEXT: Rgb = [0xe8, 0xea, 0xed];
const SUBTLE: Rgb = [0x9a, 0xa0, 0xa6];
const ACCENT: Rgb = [0x4f, 0x8c, 0xff];
const TRACK: Rgb = [0x3a, 0x40, 0x4c];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    PlayPause,
    /// Fraction of the duration.
    Seek(f32),
    Curved,
    Reset,
    /// 0 = mute, 1..=VOLUME_STEPS = that many bars.
    Volume(u32),
    Nothing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub paused: bool,
    pub position: f64,
    pub duration: f64,
    /// None when the curve toggle does not apply (VR180/360).
    pub curved: Option<bool>,
    /// 0..=VOLUME_STEPS; None without an audio track.
    pub volume: Option<u32>,
}

type Rect = (f32, f32, f32, f32);

const PLAY: Rect = (24.0, 20.0, 96.0, 96.0);
const SEEK: Rect = (430.0, 20.0, 746.0, 96.0);
const CURVED: Rect = (24.0, 144.0, 250.0, 92.0);
const RESET: Rect = (294.0, 144.0, 270.0, 92.0);
const SPEAKER: Rect = (600.0, 144.0, 76.0, 92.0);
const BARS_X: f32 = 690.0;
const BAR_W: f32 = 38.0;
const BAR_GAP: f32 = 11.0;

fn bar_rect(i: u32) -> Rect {
    (BARS_X + i as f32 * (BAR_W + BAR_GAP), 144.0, BAR_W, 92.0)
}

fn inside((x, y, w, h): Rect, px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

pub fn hit(state: &State, x: f32, y: f32) -> Hit {
    if inside(PLAY, x, y) {
        return Hit::PlayPause;
    }
    if inside(SEEK, x, y) && state.duration > 0.0 {
        return Hit::Seek(((x - SEEK.0) / SEEK.2).clamp(0.0, 1.0));
    }
    if state.curved.is_some() && inside(CURVED, x, y) {
        return Hit::Curved;
    }
    if inside(RESET, x, y) {
        return Hit::Reset;
    }
    if state.volume.is_some() {
        if inside(SPEAKER, x, y) {
            return Hit::Volume(0);
        }
        for i in 0..VOLUME_STEPS {
            // Bars touch at their gaps' midpoints so no click falls between them.
            let (bx, by, bw, bh) = bar_rect(i);
            if inside((bx - BAR_GAP / 2.0, by, bw + BAR_GAP, bh), x, y) {
                return Hit::Volume(i + 1);
            }
        }
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

pub fn render(state: &State, fonts: &mut Fonts, hover: Hit) -> Canvas {
    let mut c = Canvas::new(WIDTH, HEIGHT);
    c.clear(BG);
    let button = |c: &mut Canvas, r: Rect, hovered: bool, active: bool| {
        let color = if active { ACCENT } else if hovered { HOVER } else { BUTTON };
        c.rect(r.0, r.1, r.2, r.3, 18.0, color);
    };

    // Play / pause.
    button(&mut c, PLAY, hover == Hit::PlayPause, false);
    let (cx, cy) = (PLAY.0 + PLAY.2 / 2.0, PLAY.1 + PLAY.3 / 2.0);
    if state.paused {
        // Triangle from horizontal strips.
        for i in 0..72 {
            let t = i as f32 / 72.0;
            // Pointing right: full height at the left edge, a point at the right.
            let half = 24.0 * (1.0 - t);
            c.rect(cx - 14.0 + t * 36.0, cy - half, 1.5, half * 2.0, 0.0, TEXT);
        }
    } else {
        c.rect(cx - 17.0, cy - 22.0, 12.0, 44.0, 3.0, TEXT);
        c.rect(cx + 5.0, cy - 22.0, 12.0, 44.0, 3.0, TEXT);
    }

    // Time and seek bar.
    let time = format!("{} / {}", format_time(state.position), format_time(state.duration));
    fonts.draw(&mut c, &time, 140.0, 80.0, 34.0, TEXT, 280.0);
    let (sx, sy, sw, sh) = SEEK;
    let track_y = sy + sh / 2.0;
    c.rect(sx, track_y - 6.0, sw, 12.0, 6.0, TRACK);
    let fraction = if state.duration > 0.0 { (state.position / state.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
    c.rect(sx, track_y - 6.0, sw * fraction, 12.0, 6.0, ACCENT);
    if let Hit::Seek(f) = hover {
        c.rect(sx + sw * f - 2.0, track_y - 22.0, 4.0, 44.0, 2.0, SUBTLE);
        let label = format_time(f as f64 * state.duration);
        let w = fonts.measure(&label, 24.0);
        let lx = (sx + sw * f - w / 2.0).clamp(sx, sx + sw - w);
        fonts.draw(&mut c, &label, lx, sy + 16.0, 24.0, SUBTLE, w + 4.0);
    }
    c.circle(sx + sw * fraction, track_y, 14.0, TEXT);

    // Screen buttons.
    if let Some(curved) = state.curved {
        button(&mut c, CURVED, hover == Hit::Curved, curved);
        let label = if curved { "Curved: on" } else { "Curved: off" };
        let w = fonts.measure(label, 32.0);
        fonts.draw(&mut c, label, CURVED.0 + (CURVED.2 - w) / 2.0, CURVED.1 + 58.0, 32.0, TEXT, CURVED.2);
    }
    button(&mut c, RESET, hover == Hit::Reset, false);
    let w = fonts.measure("Reset position", 32.0);
    fonts.draw(&mut c, "Reset position", RESET.0 + (RESET.2 - w) / 2.0, RESET.1 + 58.0, 32.0, TEXT, RESET.2);

    // Volume: speaker (mute) + equal-size bars.
    if let Some(level) = state.volume {
        button(&mut c, SPEAKER, hover == Hit::Volume(0), false);
        let (px, py) = (SPEAKER.0 + 18.0, SPEAKER.1 + SPEAKER.3 / 2.0);
        c.rect(px, py - 10.0, 14.0, 20.0, 2.0, if level == 0 { SUBTLE } else { TEXT });
        for i in 0..16 {
            let t = i as f32 / 16.0;
            c.rect(px + 12.0 + t * 20.0, py - 10.0 - t * 12.0, 1.5, 20.0 + t * 24.0, 0.0, if level == 0 { SUBTLE } else { TEXT });
        }
        if level == 0 {
            c.rect(px - 4.0, py - 2.0, 44.0, 4.0, 2.0, [0xff, 0x45, 0x3a]);
        }
        for i in 0..VOLUME_STEPS {
            let (bx, by, bw, bh) = bar_rect(i);
            let lit = i < level;
            let hovered = hover == Hit::Volume(i + 1);
            let color = match (lit, hovered) {
                (true, true) => [0x6b, 0xa0, 0xff],
                (true, false) => ACCENT,
                (false, true) => HOVER,
                (false, false) => BUTTON,
            };
            c.rect(bx, by, bw, bh, 8.0, color);
        }
    } else {
        fonts.draw(&mut c, "No sound track", SPEAKER.0, SPEAKER.1 + 58.0, 30.0, SUBTLE, 500.0);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State { paused: false, position: 30.0, duration: 120.0, curved: Some(false), volume: Some(5) }
    }

    #[test]
    fn every_point_along_the_volume_row_hits_a_bar() {
        let s = state();
        let (first, last) = (bar_rect(0), bar_rect(VOLUME_STEPS - 1));
        let y = first.1 + first.3 / 2.0;
        let mut seen = Vec::new();
        let mut x = first.0;
        while x <= last.0 + last.2 {
            match hit(&s, x, y) {
                Hit::Volume(n) if n >= 1 => {
                    if seen.last() != Some(&n) {
                        seen.push(n);
                    }
                }
                other => panic!("gap at x={x}: {other:?}"),
            }
            x += 1.0;
        }
        assert_eq!(seen, (1..=VOLUME_STEPS).collect::<Vec<_>>());
    }

    #[test]
    fn seek_bar_maps_to_fraction() {
        let s = state();
        assert_eq!(hit(&s, SEEK.0, SEEK.1 + 10.0), Hit::Seek(0.0));
        assert_eq!(hit(&s, SEEK.0 + SEEK.2, SEEK.1 + 10.0), Hit::Seek(1.0));
        assert_eq!(hit(&s, PLAY.0 + 5.0, PLAY.1 + 5.0), Hit::PlayPause);
        let spherical = State { curved: None, ..s };
        assert_eq!(hit(&spherical, CURVED.0 + 5.0, CURVED.1 + 5.0), Hit::Nothing);
    }

    #[test]
    fn times_read_naturally() {
        assert_eq!(format_time(75.0), "1:15");
        assert_eq!(format_time(5530.0), "1:32:10");
    }
}
