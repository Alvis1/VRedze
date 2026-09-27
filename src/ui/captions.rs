//! Subtitle captions: text in a see-through box, drawn at the bottom of a
//! transparent canvas that is shown over the lower part of the picture.

use super::canvas::{Canvas, Fonts};
use crate::subtitles::{Bitmap, Caption};

pub const WIDTH: u32 = 1600;
pub const HEIGHT: u32 = 320;
const SIZE: f32 = 50.0;
const LINE: f32 = 64.0;
const PAD: f32 = 22.0;

/// At most this many lines are shown (long cues are rare; the rest is cut).
const MAX_LINES: usize = 4;

/// The caption layer spans this share of the screen's width.
pub const SCREEN_SHARE: f32 = 0.9;

pub fn render(caption: &Caption, fonts: &mut Fonts) -> Canvas {
    match (&caption.image, &caption.text) {
        (Some(image), _) => render_bitmap(image),
        (None, Some(text)) => render_text(text, fonts),
        (None, None) => Canvas::new(WIDTH, HEIGHT),
    }
}

/// A picture subtitle at the size it has on screen: its frame spans the
/// screen's width, so the canvas (`SCREEN_SHARE` of it) shows it enlarged.
/// It sits at the bottom, horizontally where the frame puts it.
fn render_bitmap(image: &Bitmap) -> Canvas {
    let mut canvas = Canvas::new(WIDTH, HEIGHT);
    let (fw, fh) = (image.frame_width as f32, image.frame_height as f32);
    let mut sx = WIDTH as f32 / (SCREEN_SHARE * fw);
    // DVD frames (720 wide) have wide pixels: they are shown at 16:9.
    let pixel_aspect = if image.frame_width == 720 && matches!(image.frame_height, 480 | 576) {
        (16.0 / 9.0) / (fw / fh)
    } else {
        1.0
    };
    let mut sy = sx / pixel_aspect;
    // Too big for the layer (rare): shrink evenly.
    let shrink = (HEIGHT as f32 / (image.height as f32 * sy))
        .min(WIDTH as f32 / (image.width as f32 * sx))
        .min(1.0);
    sx *= shrink;
    sy *= shrink;
    let (dw, dh) = (image.width as f32 * sx, image.height as f32 * sy);
    // Frame x → canvas x; the canvas starts (1 - SCREEN_SHARE)/2 into the frame.
    let left = ((image.x as f32 - fw * (1.0 - SCREEN_SHARE) / 2.0) * sx)
        .clamp(0.0, (WIDTH as f32 - dw).max(0.0));
    let top = HEIGHT as f32 - dh;
    let (x0, y0) = (left.floor() as i32, top.floor().max(0.0) as i32);
    let (x1, y1) = ((left + dw).ceil().min(WIDTH as f32) as i32, HEIGHT as i32);
    let (w, h) = (image.width as i32, image.height as i32);
    let texel = |x: i32, y: i32, c: usize| -> f32 {
        let (x, y) = (x.clamp(0, w - 1), y.clamp(0, h - 1));
        image.rgba[((y * w + x) * 4) as usize + c] as f32
    };
    for py in y0..y1 {
        for px in x0..x1 {
            // Bilinear sample at the pixel centre.
            let u = (px as f32 + 0.5 - left) / sx - 0.5;
            let v = (py as f32 + 0.5 - top) / sy - 0.5;
            if u < -0.5 || v < -0.5 || u > w as f32 - 0.5 || v > h as f32 - 0.5 {
                continue;
            }
            let (ux, vy) = (u.floor(), v.floor());
            let (fx, fy) = (u - ux, v - vy);
            let (ix, iy) = (ux as i32, vy as i32);
            let i = ((py as u32 * WIDTH + px as u32) * 4) as usize;
            for c in 0..4 {
                let top_row = texel(ix, iy, c) * (1.0 - fx) + texel(ix + 1, iy, c) * fx;
                let bottom_row = texel(ix, iy + 1, c) * (1.0 - fx) + texel(ix + 1, iy + 1, c) * fx;
                canvas.pixels[i + c] = (top_row * (1.0 - fy) + bottom_row * fy).round() as u8;
            }
        }
    }
    canvas
}

fn render_text(text: &str, fonts: &mut Fonts) -> Canvas {
    let mut canvas = Canvas::new(WIDTH, HEIGHT);
    let max_w = WIDTH as f32 - 4.0 * PAD;
    let mut lines: Vec<String> = text
        .lines()
        .flat_map(|l| fonts.wrap(l, SIZE, max_w))
        .collect();
    lines.truncate(MAX_LINES);
    if lines.is_empty() {
        return canvas;
    }
    let widths: Vec<f32> = lines.iter().map(|l| fonts.measure(l, SIZE)).collect();
    let box_w = widths.iter().copied().fold(0.0, f32::max) + 2.0 * PAD;
    let box_h = lines.len() as f32 * LINE + PAD;
    let (box_x, box_y) = ((WIDTH as f32 - box_w) / 2.0, HEIGHT as f32 - box_h);
    canvas.translucent_rect(box_x, box_y, box_w, box_h, 14.0, [0, 0, 0], 0.62);
    for (i, (line, w)) in lines.iter().zip(&widths).enumerate() {
        let baseline = box_y + PAD / 2.0 + (i as f32 + 1.0) * LINE - 16.0;
        fonts.draw(
            &mut canvas,
            line,
            (WIDTH as f32 - w) / 2.0,
            baseline,
            SIZE,
            [0xf4, 0xf4, 0xf4],
            w + 4.0,
        );
    }
    canvas
}
