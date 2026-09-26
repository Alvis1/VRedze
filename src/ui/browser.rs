//! The headset browser panel: a list of servers, shares, folders or videos,
//! drawn into a canvas that is shown on an OpenXR quad layer.

use super::canvas::{Canvas, Fonts, Rgb};
use crate::playability::Verdict;

pub const WIDTH: u32 = 1600;
pub const HEIGHT: u32 = 1000;
const HEADER: f32 = 120.0;
const FOOTER: f32 = 64.0;
const ROW: f32 = 88.0;
const PAD: f32 = 32.0;

const BG: Rgb = [0x15, 0x17, 0x1c];
const ROW_BG: Rgb = [0x1d, 0x21, 0x28];
const HOVER: Rgb = [0x2c, 0x33, 0x40];
const TEXT: Rgb = [0xe8, 0xea, 0xed];
const SUBTLE: Rgb = [0x9a, 0xa0, 0xa6];
const ACCENT: Rgb = [0x4f, 0x8c, 0xff];
const GREEN: Rgb = [0x34, 0xc7, 0x59];
const YELLOW: Rgb = [0xff, 0xcc, 0x00];
const ORANGE: Rgb = [0xff, 0x95, 0x00];
const RED: Rgb = [0xff, 0x45, 0x3a];
const GREY: Rgb = [0x5f, 0x63, 0x68];

#[derive(Clone, Debug, PartialEq)]
pub enum Icon {
    Server,
    Share,
    Folder,
    /// A video; `None` while its playability is still being checked.
    Video(Option<Verdict>),
    /// A file that could not be read.
    Broken,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub icon: Icon,
    pub label: String,
    pub detail: String,
    pub right: String,
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub title: String,
    pub body: Vec<String>,
    pub button: String,
}

#[derive(Clone, Debug, Default)]
pub struct View {
    pub title: String,
    pub rows: Vec<Row>,
    /// Shown instead of rows (loading, errors, empty folders).
    pub status: Option<String>,
    pub dialog: Option<Dialog>,
    /// First visible row (fractional while scrolling).
    pub scroll: f32,
    pub footer: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Row(usize),
    DialogButton,
    Nothing,
}

pub fn visible_rows() -> f32 {
    (HEIGHT as f32 - HEADER - FOOTER) / ROW
}

impl View {
    pub fn max_scroll(&self) -> f32 {
        (self.rows.len() as f32 - visible_rows()).max(0.0)
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }
}

fn dialog_button_rect() -> (f32, f32, f32, f32) {
    let (w, h) = (280.0, 80.0);
    (WIDTH as f32 / 2.0 - w / 2.0, HEIGHT as f32 - 250.0, w, h)
}

fn inside((x, y, w, h): (f32, f32, f32, f32), px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

/// What the pointer at canvas pixel (x, y) would activate.
pub fn hit(view: &View, x: f32, y: f32) -> Hit {
    if view.dialog.is_some() {
        return if inside(dialog_button_rect(), x, y) {
            Hit::DialogButton
        } else {
            Hit::Nothing
        };
    }
    if view.status.is_some()
        || y < HEADER
        || y > HEIGHT as f32 - FOOTER
        || x < PAD
        || x > WIDTH as f32 - PAD
    {
        return Hit::Nothing;
    }
    let index = ((y - HEADER) / ROW + view.scroll).floor();
    if index >= 0.0 && (index as usize) < view.rows.len() {
        Hit::Row(index as usize)
    } else {
        Hit::Nothing
    }
}

fn draw_icon(canvas: &mut Canvas, icon: &Icon, cx: f32, cy: f32) {
    match icon {
        Icon::Server => {
            canvas.rect(cx - 22.0, cy - 20.0, 44.0, 16.0, 4.0, ACCENT);
            canvas.rect(cx - 22.0, cy + 2.0, 44.0, 16.0, 4.0, ACCENT);
            canvas.circle(cx + 13.0, cy - 12.0, 3.0, BG);
            canvas.circle(cx + 13.0, cy + 10.0, 3.0, BG);
        }
        Icon::Share | Icon::Folder => {
            let color = if *icon == Icon::Share {
                ACCENT
            } else {
                [0x8a, 0xb4, 0xf8]
            };
            canvas.rect(cx - 24.0, cy - 18.0, 20.0, 10.0, 3.0, color);
            canvas.rect(cx - 24.0, cy - 12.0, 48.0, 32.0, 4.0, color);
        }
        Icon::Video(verdict) => {
            let color = match verdict {
                Some(Verdict::Hardware) => GREEN,
                Some(Verdict::Software) => YELLOW,
                Some(Verdict::SoftwareMarginal) => ORANGE,
                Some(Verdict::Unplayable) => RED,
                None => GREY,
            };
            canvas.circle(cx, cy, 16.0, color);
        }
        Icon::Broken => {
            canvas.circle(cx, cy, 16.0, RED);
            canvas.rect(cx - 9.0, cy - 3.0, 18.0, 6.0, 2.0, BG);
        }
    }
}

/// Renders the panel; `pointer` highlights what it hovers and, with
/// `draw_cursor`, marks its position (previews; the headset has a cursor layer).
pub fn render(
    view: &View,
    fonts: &mut Fonts,
    pointer: Option<(f32, f32)>,
    draw_cursor: bool,
) -> Canvas {
    let cursor = pointer;
    let mut canvas = Canvas::new(WIDTH, HEIGHT);
    canvas.clear(BG);
    let w = WIDTH as f32;
    fonts.draw(
        &mut canvas,
        &view.title,
        PAD,
        78.0,
        46.0,
        TEXT,
        w - 2.0 * PAD,
    );
    canvas.rect(
        PAD,
        HEADER - 8.0,
        w - 2.0 * PAD,
        2.0,
        0.0,
        [0x2a, 0x2f, 0x38],
    );

    let hover = cursor.map(|(x, y)| hit(view, x, y));
    if let Some(status) = &view.status {
        let lines = fonts.wrap(status, 36.0, w - 4.0 * PAD);
        for (i, line) in lines.iter().enumerate() {
            fonts.draw(
                &mut canvas,
                line,
                2.0 * PAD,
                HEADER + 90.0 + i as f32 * 52.0,
                36.0,
                SUBTLE,
                w - 4.0 * PAD,
            );
        }
    } else {
        let first = view.scroll.floor() as usize;
        let offset = (view.scroll - view.scroll.floor()) * ROW;
        let bottom = HEIGHT as f32 - FOOTER;
        for (i, row) in view.rows.iter().enumerate().skip(first) {
            let y = HEADER + (i - first) as f32 * ROW - offset;
            if y > bottom {
                break;
            }
            let hovered = hover == Some(Hit::Row(i));
            canvas.rect(
                PAD,
                y + 4.0,
                w - 2.0 * PAD - 24.0,
                ROW - 8.0,
                14.0,
                if hovered { HOVER } else { ROW_BG },
            );
            draw_icon(&mut canvas, &row.icon, PAD + 48.0, y + ROW / 2.0);
            let right_w = if row.right.is_empty() {
                0.0
            } else {
                fonts.measure(&row.right, 28.0) + 24.0
            };
            let text_w = w - 2.0 * PAD - 24.0 - 100.0 - right_w - 24.0;
            if row.detail.is_empty() {
                fonts.draw(
                    &mut canvas,
                    &row.label,
                    PAD + 96.0,
                    y + 56.0,
                    36.0,
                    TEXT,
                    text_w,
                );
            } else {
                fonts.draw(
                    &mut canvas,
                    &row.label,
                    PAD + 96.0,
                    y + 42.0,
                    34.0,
                    TEXT,
                    text_w,
                );
                fonts.draw(
                    &mut canvas,
                    &row.detail,
                    PAD + 96.0,
                    y + 74.0,
                    24.0,
                    SUBTLE,
                    text_w,
                );
            }
            if right_w > 0.0 {
                fonts.draw(
                    &mut canvas,
                    &row.right,
                    w - PAD - 24.0 - right_w,
                    y + 56.0,
                    28.0,
                    SUBTLE,
                    right_w,
                );
            }
        }
        // Cover rows that scrolled under the header or footer.
        canvas.rect(0.0, 0.0, w, HEADER - 8.0, 0.0, BG);
        fonts.draw(
            &mut canvas,
            &view.title,
            PAD,
            78.0,
            46.0,
            TEXT,
            w - 2.0 * PAD,
        );
        canvas.rect(0.0, bottom, w, FOOTER, 0.0, BG);
        let max = view.max_scroll();
        if max > 0.0 {
            let track = bottom - HEADER;
            let thumb = (visible_rows() / view.rows.len() as f32 * track).max(40.0);
            let top = HEADER + view.scroll / max * (track - thumb);
            canvas.rect(w - PAD - 10.0, HEADER, 8.0, track, 4.0, [0x2a, 0x2f, 0x38]);
            canvas.rect(w - PAD - 10.0, top, 8.0, thumb, 4.0, SUBTLE);
        }
    }
    fonts.draw(
        &mut canvas,
        &view.footer,
        PAD,
        HEIGHT as f32 - 22.0,
        26.0,
        SUBTLE,
        w - 2.0 * PAD,
    );

    if let Some(dialog) = &view.dialog {
        // Dim the list behind the dialog.
        for px in canvas.pixels.chunks_exact_mut(4) {
            for c in &mut px[..3] {
                *c /= 3;
            }
        }
        let (dx, dy, dw, dh) = (160.0, 120.0, w - 320.0, HEIGHT as f32 - 240.0);
        canvas.rect(dx, dy, dw, dh, 24.0, [0x22, 0x26, 0x2e]);
        fonts.draw(
            &mut canvas,
            &dialog.title,
            dx + 48.0,
            dy + 84.0,
            42.0,
            TEXT,
            dw - 96.0,
        );
        let mut y = dy + 150.0;
        for paragraph in &dialog.body {
            for line in fonts.wrap(paragraph, 30.0, dw - 96.0) {
                fonts.draw(&mut canvas, &line, dx + 48.0, y, 30.0, SUBTLE, dw - 96.0);
                y += 44.0;
            }
            y += 16.0;
        }
        let rect = dialog_button_rect();
        let hovered = hover == Some(Hit::DialogButton);
        canvas.rect(
            rect.0,
            rect.1,
            rect.2,
            rect.3,
            16.0,
            if hovered { [0x6b, 0xa0, 0xff] } else { ACCENT },
        );
        let label_w = fonts.measure(&dialog.button, 34.0);
        fonts.draw(
            &mut canvas,
            &dialog.button,
            rect.0 + (rect.2 - label_w) / 2.0,
            rect.1 + 53.0,
            34.0,
            TEXT,
            rect.2,
        );
    }

    if let Some((x, y)) = cursor.filter(|_| draw_cursor) {
        canvas.circle(x, y, 12.0, [0xff, 0xff, 0xff]);
        canvas.circle(x, y, 8.0, ACCENT);
    }
    canvas
}

/// Human-readable size, e.g. "4.7 GB".
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_follows_scroll_and_dialog() {
        let mut view = View {
            rows: (0..30)
                .map(|i| Row {
                    icon: Icon::Folder,
                    label: format!("{i}"),
                    detail: String::new(),
                    right: String::new(),
                })
                .collect(),
            ..Default::default()
        };
        assert_eq!(hit(&view, 400.0, HEADER + 10.0), Hit::Row(0));
        view.scroll = 5.0;
        assert_eq!(hit(&view, 400.0, HEADER + ROW + 10.0), Hit::Row(6));
        assert_eq!(hit(&view, 400.0, 10.0), Hit::Nothing);
        view.dialog = Some(Dialog {
            title: String::new(),
            body: vec![],
            button: "OK".into(),
        });
        let (x, y, w, h) = dialog_button_rect();
        assert_eq!(hit(&view, x + w / 2.0, y + h / 2.0), Hit::DialogButton);
        assert_eq!(hit(&view, 400.0, HEADER + 10.0), Hit::Nothing);
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(5_097_390_883), "5.1 GB");
    }
}
