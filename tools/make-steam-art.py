#!/usr/bin/env python3
"""Steam library artwork for the headset's non-Steam entry (Pillow, macOS fonts).

Writes assets/steam/: capsule.png (600x900 library card), header.png (920x430),
hero.png (1920x620 banner) and logo.png (transparent title, shown on the hero).
scripts/install-frame.sh copies them into Steam's grid folder.
Usage: tools/make-steam-art.py ["App Name"]
"""
import math
import os
import sys

from PIL import Image, ImageDraw, ImageFilter, ImageFont

NAME = sys.argv[1] if len(sys.argv) > 1 else "Just Video"
TAGLINE = "3D  ·  180°  ·  360°  ·  Spatial"
OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "steam")
FONT = "/System/Library/Fonts/Avenir Next.ttc"  # index 2: Demi Bold, 0: Bold
TOP, BOTTOM = (18, 24, 44), (40, 18, 64)  # deep blue to violet
LEFT_EYE, RIGHT_EYE = (255, 92, 108), (64, 196, 255)  # anaglyph red / cyan


def font(size, bold=True):
    try:
        return ImageFont.truetype(FONT, size, index=0 if bold else 2)
    except OSError:
        return ImageFont.truetype("/System/Library/Fonts/Supplemental/Arial Bold.ttf", size)


def background(w, h):
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        t = y / max(h - 1, 1)
        row = tuple(int(TOP[i] + (BOTTOM[i] - TOP[i]) * t) for i in range(3))
        for x in range(w):
            px[x, y] = row
    # A soft glow behind the emblem.
    glow = Image.new("L", (w, h), 0)
    ImageDraw.Draw(glow).ellipse((w * 0.15, h * 0.05, w * 0.85, h * 0.75), fill=90)
    glow = glow.filter(ImageFilter.GaussianBlur(min(w, h) * 0.12))
    light = Image.new("RGB", (w, h), (90, 70, 160))
    return Image.composite(light, img, glow)


def emblem(size):
    """Two overlapping lenses (left red, right cyan) with a play triangle."""
    s = size
    layer = Image.new("RGBA", (s * 2, s), (0, 0, 0, 0))
    r = int(s * 0.42)
    cy = s // 2
    for cx, color in ((int(s * 0.72), LEFT_EYE), (int(s * 1.28), RIGHT_EYE)):
        lens = Image.new("RGBA", layer.size, (0, 0, 0, 0))
        d = ImageDraw.Draw(lens)
        d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=color + (200,))
        d.ellipse((cx - r * 0.86, cy - r * 0.86, cx + r * 0.86, cy + r * 0.86), fill=color + (120,))
        layer = Image.alpha_composite(layer, lens)
    d = ImageDraw.Draw(layer)
    tri = r * 0.62
    cx = s
    d.polygon(
        [(cx - tri * 0.45, cy - tri * 0.6), (cx - tri * 0.45, cy + tri * 0.6), (cx + tri * 0.62, cy)],
        fill=(255, 255, 255, 255),
    )
    return layer


def centered_text(draw, xy_center, text, fnt, fill, shadow=True):
    box = draw.textbbox((0, 0), text, font=fnt)
    w, h = box[2] - box[0], box[3] - box[1]
    x, y = xy_center[0] - w / 2 - box[0], xy_center[1] - h / 2 - box[1]
    if shadow:
        draw.text((x + 3, y + 4), text, font=fnt, fill=(0, 0, 0, 150))
    draw.text((x, y), text, font=fnt, fill=fill)


def fit_font(draw, text, max_width, start):
    size = start
    while size > 10:
        f = font(size)
        box = draw.textbbox((0, 0), text, font=f)
        if box[2] - box[0] <= max_width:
            return f
        size -= 2
    return font(size)


def card(w, h, emblem_frac, title_frac, tagline=True):
    img = background(w, h).convert("RGBA")
    e = emblem(int(min(w * 0.42, h * emblem_frac)))
    img.alpha_composite(e, (w // 2 - e.width // 2, int(h * 0.30 - e.height / 2)))
    d = ImageDraw.Draw(img)
    title = fit_font(d, NAME, w * 0.86, int(h * title_frac))
    centered_text(d, (w / 2, h * 0.62), NAME, title, (255, 255, 255, 255))
    if tagline:
        tag = fit_font(d, TAGLINE, w * 0.8, int(h * title_frac * 0.32))
        centered_text(d, (w / 2, h * 0.76), TAGLINE, tag, (200, 205, 235, 255), shadow=False)
    return img.convert("RGB")


def main():
    os.makedirs(OUT, exist_ok=True)
    card(600, 900, 0.36, 0.11).save(os.path.join(OUT, "capsule.png"))
    # Header: emblem left, text right, apart.
    w, h = 920, 430
    img = background(w, h).convert("RGBA")
    e = emblem(int(h * 0.45))
    img.alpha_composite(e, (int(w * 0.03), h // 2 - e.height // 2))
    d = ImageDraw.Draw(img)
    title = fit_font(d, NAME, w * 0.44, int(h * 0.2))
    centered_text(d, (w * 0.73, h * 0.44), NAME, title, (255, 255, 255, 255))
    tag = fit_font(d, TAGLINE, w * 0.44, int(h * 0.075))
    centered_text(d, (w * 0.73, h * 0.64), TAGLINE, tag, (200, 205, 235, 255), shadow=False)
    img.convert("RGB").save(os.path.join(OUT, "header.png"))
    # Hero: Steam draws the logo over its lower left, so the emblem goes right.
    w, h = 1920, 620
    img = background(w, h).convert("RGBA")
    e = emblem(int(h * 0.6))
    img.alpha_composite(e, (int(w * 0.96) - e.width, h // 2 - e.height // 2))
    img.convert("RGB").save(os.path.join(OUT, "hero.png"))
    # Logo: title on transparency (Steam draws it over the hero).
    logo = Image.new("RGBA", (1280, 360), (0, 0, 0, 0))
    d = ImageDraw.Draw(logo)
    centered_text(d, (640, 150), NAME, fit_font(d, NAME, 1180, 200), (255, 255, 255, 255))
    centered_text(d, (640, 300), TAGLINE, fit_font(d, TAGLINE, 1100, 60), (215, 220, 245, 255), shadow=False)
    logo.save(os.path.join(OUT, "logo.png"))
    print("Wrote", ", ".join(sorted(os.listdir(OUT))), "to", os.path.normpath(OUT))


if __name__ == "__main__":
    main()
