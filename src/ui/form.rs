//! A small form (text fields) with an on-panel keyboard, drawn into the
//! browser panel and operated by pointing: used to add servers and rename files.

use super::canvas::{Canvas, Fonts, Rgb};

const TEXT: Rgb = [0xe8, 0xea, 0xed];
const SUBTLE: Rgb = [0x9a, 0xa0, 0xa6];
const FIELD: Rgb = [0x1d, 0x21, 0x28];
const FIELD_FOCUS: Rgb = [0x26, 0x2f, 0x40];
const KEY: Rgb = [0x2a, 0x2f, 0x38];
const KEY_HOVER: Rgb = [0x3a, 0x42, 0x50];
const ACCENT: Rgb = [0x4f, 0x8c, 0xff];
const ERROR: Rgb = [0xff, 0x6b, 0x60];

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub label: String,
    pub value: String,
    /// Shown as dots (passwords).
    pub secret: bool,
    pub placeholder: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Lower,
    Upper,
    Symbols,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub title: String,
    pub fields: Vec<Field>,
    pub focused: usize,
    pub layer: Layer,
    pub error: Option<String>,
    pub busy: Option<String>,
    pub submit: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Shift,
    Symbols,
    Backspace,
    Space,
    Cancel,
    Submit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Field(usize),
    Key(Key),
    Nothing,
}

impl Form {
    pub fn new(title: impl Into<String>, fields: Vec<Field>, submit: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            fields,
            focused: 0,
            layer: Layer::Lower,
            error: None,
            busy: None,
            submit: submit.into(),
        }
    }

    pub fn value(&self, i: usize) -> &str {
        &self.fields[i].value
    }

    /// Applies a key; returns Cancel/Submit for the owner to act on.
    pub fn press(&mut self, key: Key) -> Option<Key> {
        self.error = None;
        let field = &mut self.fields[self.focused];
        match key {
            Key::Char(c) => {
                field.value.push(c);
                if self.layer == Layer::Upper {
                    self.layer = Layer::Lower; // one-shot shift
                }
            }
            Key::Space => field.value.push(' '),
            Key::Backspace => {
                field.value.pop();
            }
            Key::Shift => {
                self.layer = if self.layer == Layer::Upper {
                    Layer::Lower
                } else {
                    Layer::Upper
                };
            }
            Key::Symbols => {
                self.layer = if self.layer == Layer::Symbols {
                    Layer::Lower
                } else {
                    Layer::Symbols
                };
            }
            Key::Cancel | Key::Submit => return Some(key),
        }
        None
    }
}

// Layout inside the 1600×1000 browser panel.
const X0: f32 = 32.0;
const FIELD_Y: f32 = 132.0;
const FIELD_H: f32 = 72.0;
const FIELD_GAP: f32 = 10.0;
const KEY_H: f32 = 84.0;
const KEY_GAP: f32 = 10.0;

fn rows(layer: Layer) -> [&'static str; 4] {
    match layer {
        Layer::Lower => ["1234567890", "qwertyuiop", "asdfghjkl@", "zxcvbnm.-_"],
        Layer::Upper => ["1234567890", "QWERTYUIOP", "ASDFGHJKL@", "ZXCVBNM.-_"],
        Layer::Symbols => ["!#$%&*()+=", "\\/:;,'\"?~^", "[]{}<>|`€£", "§°¨´¤½.-_ "],
    }
}

type Rect = (f32, f32, f32, f32);

fn keyboard_top(form: &Form) -> f32 {
    FIELD_Y + form.fields.len() as f32 * (FIELD_H + FIELD_GAP) + 24.0
}

fn field_rect(i: usize) -> Rect {
    (
        X0 + 260.0,
        FIELD_Y + i as f32 * (FIELD_H + FIELD_GAP),
        1600.0 - 2.0 * X0 - 260.0,
        FIELD_H,
    )
}

/// Every key with its rectangle.
fn keys(form: &Form, panel_width: f32) -> Vec<(Key, Rect)> {
    let top = keyboard_top(form);
    let width = panel_width - 2.0 * X0;
    let mut out = Vec::new();
    // Four character rows of 10 keys; the wide keys sit on the sides.
    for (r, chars) in rows(form.layer).iter().enumerate() {
        let y = top + r as f32 * (KEY_H + KEY_GAP);
        let side = 180.0;
        let key_w = (width - 2.0 * (side + KEY_GAP) - 9.0 * KEY_GAP) / 10.0;
        let (left, right) = match r {
            0 => (None, Some(Key::Backspace)),
            2 => (Some(Key::Shift), None),
            3 => (Some(Key::Symbols), None),
            _ => (None, None),
        };
        if let Some(k) = left {
            out.push((k, (X0, y, side, KEY_H)));
        }
        for (i, c) in chars.chars().enumerate() {
            let x = X0 + side + KEY_GAP + i as f32 * (key_w + KEY_GAP);
            out.push((
                if c == ' ' { Key::Space } else { Key::Char(c) },
                (x, y, key_w, KEY_H),
            ));
        }
        if let Some(k) = right {
            out.push((k, (X0 + width - side, y, side, KEY_H)));
        }
    }
    // Bottom row: cancel, space, submit.
    let y = top + 4.0 * (KEY_H + KEY_GAP);
    out.push((Key::Cancel, (X0, y, 300.0, KEY_H)));
    out.push((Key::Space, (X0 + 310.0, y, width - 620.0, KEY_H)));
    out.push((Key::Submit, (X0 + width - 300.0, y, 300.0, KEY_H)));
    out
}

fn inside((x, y, w, h): Rect, px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

pub fn hit(form: &Form, panel_width: f32, x: f32, y: f32) -> Hit {
    if form.busy.is_some() {
        return Hit::Nothing;
    }
    for i in 0..form.fields.len() {
        if inside(field_rect(i), x, y) {
            return Hit::Field(i);
        }
    }
    keys(form, panel_width)
        .into_iter()
        .find(|(_, r)| inside(*r, x, y))
        .map_or(Hit::Nothing, |(k, _)| Hit::Key(k))
}

fn key_label(key: Key, form: &Form) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Shift => {
            if form.layer == Layer::Upper {
                "SHIFT".into()
            } else {
                "Shift".into()
            }
        }
        Key::Symbols => {
            if form.layer == Layer::Symbols {
                "abc".into()
            } else {
                "#+=".into()
            }
        }
        Key::Backspace => "Delete".into(),
        Key::Space => "space".into(),
        Key::Cancel => "Cancel".into(),
        Key::Submit => form.submit.clone(),
    }
}

pub fn render(canvas: &mut Canvas, fonts: &mut Fonts, form: &Form, hover: Hit) {
    let w = canvas.width as f32;
    for (i, field) in form.fields.iter().enumerate() {
        let (fx, fy, fw, fh) = field_rect(i);
        fonts.draw(canvas, &field.label, X0, fy + 47.0, 30.0, SUBTLE, 250.0);
        let focused = i == form.focused;
        let hovered = hover == Hit::Field(i);
        canvas.rect(
            fx,
            fy,
            fw,
            fh,
            12.0,
            if focused {
                FIELD_FOCUS
            } else if hovered {
                KEY_HOVER
            } else {
                FIELD
            },
        );
        if focused {
            canvas.rect(fx, fy + fh - 4.0, fw, 4.0, 2.0, ACCENT);
        }
        let shown = if field.secret {
            "•".repeat(field.value.chars().count())
        } else {
            field.value.clone()
        };
        if shown.is_empty() {
            fonts.draw(
                canvas,
                &field.placeholder,
                fx + 20.0,
                fy + 48.0,
                32.0,
                [0x5f, 0x63, 0x68],
                fw - 40.0,
            );
        } else {
            // Keep the end (where typing happens) visible.
            let mut text = shown.clone();
            while fonts.measure(&text, 34.0) > fw - 60.0 && text.chars().count() > 1 {
                text.remove(0);
            }
            let width = fonts.draw(canvas, &text, fx + 20.0, fy + 48.0, 34.0, TEXT, fw - 40.0);
            if focused {
                canvas.rect(fx + 24.0 + width, fy + 16.0, 3.0, fh - 32.0, 1.0, ACCENT);
            }
        }
    }
    let status_y = keyboard_top(form) - 6.0;
    if let Some(busy) = &form.busy {
        fonts.draw(canvas, busy, X0, status_y, 26.0, SUBTLE, w - 2.0 * X0);
    } else if let Some(error) = &form.error {
        fonts.draw(canvas, error, X0, status_y, 26.0, ERROR, w - 2.0 * X0);
    }
    for (key, (x, y, kw, kh)) in keys(form, w) {
        let hovered = hover == Hit::Key(key);
        let color = match key {
            Key::Submit => {
                if hovered {
                    [0x6b, 0xa0, 0xff]
                } else {
                    ACCENT
                }
            }
            _ if hovered => KEY_HOVER,
            _ => KEY,
        };
        canvas.rect(x, y, kw, kh, 12.0, color);
        let label = key_label(key, form);
        let size = if matches!(key, Key::Char(_)) {
            40.0
        } else {
            32.0
        };
        let lw = fonts.measure(&label, size);
        fonts.draw(
            canvas,
            &label,
            x + (kw - lw) / 2.0,
            y + kh / 2.0 + size * 0.36,
            size,
            TEXT,
            kw,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Form {
        Form::new(
            "Add server",
            vec![
                Field {
                    label: "Address".into(),
                    value: String::new(),
                    secret: false,
                    placeholder: String::new(),
                },
                Field {
                    label: "Password".into(),
                    value: String::new(),
                    secret: true,
                    placeholder: String::new(),
                },
            ],
            "Save",
        )
    }

    #[test]
    fn typing_shift_and_backspace() {
        let mut f = form();
        f.press(Key::Shift);
        f.press(Key::Char('N'));
        f.press(Key::Char('a'));
        f.press(Key::Char('s'));
        f.press(Key::Backspace);
        assert_eq!(f.value(0), "Na");
        assert_eq!(f.layer, Layer::Lower, "shift is one-shot");
        assert_eq!(f.press(Key::Submit), Some(Key::Submit));
    }

    #[test]
    fn every_key_is_hittable_and_distinct() {
        let f = form();
        for (key, (x, y, w, h)) in keys(&f, 1600.0) {
            assert_eq!(
                hit(&f, 1600.0, x + w / 2.0, y + h / 2.0),
                Hit::Key(key),
                "{key:?}"
            );
            assert!(y + h <= 1000.0, "{key:?} below the panel");
        }
    }
}
