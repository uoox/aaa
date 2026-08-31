#!/usr/bin/env python3
"""Draws the aaa-ui mark and exports every asset the two clients need.

The mark is the prompt caret from the aaa menu — the ❯ that marks the
selected row — followed by three session lines in the status colours the
whole product is built around: amber waiting for input, green running,
grey idle. So the icon says what the app is for, and the three lines
double as the three a's.

Everything is drawn from paths rather than a font, so the shape is
identical wherever it is rendered and does not depend on what is
installed. Run: python3 brand/logo.py
"""

import os
import subprocess
import sys

from PIL import Image, ImageDraw, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "out")

BG_TOP = (20, 26, 33)      # #141a21
BG_BOTTOM = (10, 14, 18)   # #0a0e12  terminal ground
CARET = (83, 198, 221)     # #53c6dd  cyan, the accent
AMBER = (227, 180, 92)     # #e3b45c  waiting for input
GREEN = (94, 203, 143)     # #5ecb8f  running
GREY = (95, 109, 124)      # #5f6d7c  idle
EDGE = (54, 67, 79)        # #36434f  hairline

S = 1024                   # master canvas; every export is downsampled
SS = 4                     # supersample factor for clean diagonals


def rounded_mask(size, radius, supersample=SS):
    """Antialiased rounded-square mask, drawn large and shrunk down."""
    big = Image.new("L", (size * supersample, size * supersample), 0)
    ImageDraw.Draw(big).rounded_rectangle(
        [0, 0, size * supersample - 1, size * supersample - 1],
        radius=radius * supersample,
        fill=255,
    )
    return big.resize((size, size), Image.LANCZOS)


def vertical_gradient(size, top, bottom):
    grad = Image.new("RGB", (1, size))
    px = grad.load()
    for y in range(size):
        t = y / max(1, size - 1)
        px[0, y] = tuple(round(a + (b - a) * t) for a, b in zip(top, bottom))
    return grad.resize((size, size), Image.BICUBIC)


def draw_mark(size, inset_ratio=0.0):
    """The full icon at `size` px. inset_ratio shrinks the art inside the
    canvas, which is what macOS wants (its icons float in their square)."""
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))

    box = round(size * (1 - inset_ratio * 2))
    radius = round(box * 0.225)  # squircle-ish, in line with modern macOS

    plate = vertical_gradient(box, BG_TOP, BG_BOTTOM).convert("RGBA")
    plate.putalpha(rounded_mask(box, radius))

    # A hairline lifts the plate off dark wallpapers.
    ring = Image.new("RGBA", (box, box), (0, 0, 0, 0))
    ImageDraw.Draw(ring).rounded_rectangle(
        [1, 1, box - 2, box - 2], radius=radius, outline=EDGE + (150,),
        width=max(1, round(box * 0.006)),
    )
    plate.alpha_composite(ring)

    art = Image.new("RGBA", (box, box), (0, 0, 0, 0))
    d = ImageDraw.Draw(art)

    u = box / 100.0            # one design unit == 1% of the plate
    stroke = round(u * 8.5)
    cap = stroke // 2

    # ❯ caret: two strokes meeting at a point, left of centre. cx and the
    # line origin below are set so the caret's left tip and the longest
    # line's right end sit the same distance from the plate edge.
    cx, cy = u * 34, u * 50
    reach_x, reach_y = u * 15, u * 17
    d.line([(cx - reach_x, cy - reach_y), (cx, cy)], fill=CARET, width=stroke)
    d.line([(cx, cy), (cx - reach_x, cy + reach_y)], fill=CARET, width=stroke)
    for pt in ((cx - reach_x, cy - reach_y), (cx, cy), (cx - reach_x, cy + reach_y)):
        d.ellipse([pt[0] - cap, pt[1] - cap, pt[0] + cap, pt[1] + cap], fill=CARET)

    # Three session lines: amber waiting, green running, grey idle. Ragged
    # right edge so they read as a list, not as an equals sign.
    x0 = u * 54
    for i, (colour, length) in enumerate(
        ((AMBER, u * 27), (GREEN, u * 21), (GREY, u * 15))
    ):
        y = cy + (i - 1) * u * 17
        d.line([(x0, y), (x0 + length, y)], fill=colour, width=stroke)
        for px_ in (x0, x0 + length):
            d.ellipse([px_ - cap, y - cap, px_ + cap, y + cap], fill=colour)

    # CRT-ish bloom: the art blurred underneath itself.
    glow = art.filter(ImageFilter.GaussianBlur(radius=box * 0.022))
    glow.putalpha(glow.getchannel("A").point(lambda a: round(a * 0.55)))

    plate.alpha_composite(glow)
    plate.alpha_composite(art)

    off = round(size * inset_ratio)
    canvas.alpha_composite(plate, (off, off))
    return canvas


def draw_android_foreground(size):
    """Adaptive-icon foreground: art only, no plate, kept inside the 66/108
    safe zone so no launcher mask can clip it."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    art = draw_mark(size, inset_ratio=0.0)
    # strip the plate by redrawing art alone at the safe-zone scale
    safe = round(size * 66 / 108)
    inner = Image.new("RGBA", (safe, safe), (0, 0, 0, 0))
    d = ImageDraw.Draw(inner)
    u = safe / 100.0
    stroke = round(u * 10)
    cap = stroke // 2
    cx, cy = u * 30, u * 50
    rx, ry = u * 16, u * 18
    d.line([(cx - rx, cy - ry), (cx, cy)], fill=CARET, width=stroke)
    d.line([(cx, cy), (cx - rx, cy + ry)], fill=CARET, width=stroke)
    for pt in ((cx - rx, cy - ry), (cx, cy), (cx - rx, cy + ry)):
        d.ellipse([pt[0] - cap, pt[1] - cap, pt[0] + cap, pt[1] + cap], fill=CARET)
    x0 = u * 52
    for i, (colour, length) in enumerate(
        ((AMBER, u * 30), (GREEN, u * 23), (GREY, u * 16))
    ):
        y = cy + (i - 1) * u * 18
        d.line([(x0, y), (x0 + length, y)], fill=colour, width=stroke)
        for px_ in (x0, x0 + length):
            d.ellipse([px_ - cap, y - cap, px_ + cap, y + cap], fill=colour)
    glow = inner.filter(ImageFilter.GaussianBlur(radius=safe * 0.03))
    glow.putalpha(glow.getchannel("A").point(lambda a: round(a * 0.5)))
    pad = (size - safe) // 2
    img.alpha_composite(glow, (pad, pad))
    img.alpha_composite(inner, (pad, pad))
    del art
    return img


def draw_android_background(size):
    return vertical_gradient(size, BG_TOP, BG_BOTTOM).convert("RGBA")


def main():
    os.makedirs(OUT, exist_ok=True)
    master = draw_mark(S, inset_ratio=0.0)
    master.save(os.path.join(OUT, "logo-1024.png"))
    draw_mark(512, inset_ratio=0.0).save(os.path.join(OUT, "logo-512.png"))

    # ---- macOS .icns (art inset ~9%, the platform convention) ----
    iconset = os.path.join(OUT, "aaa-ui.iconset")
    os.makedirs(iconset, exist_ok=True)
    mac_master = draw_mark(1024, inset_ratio=0.09)
    for px, name in (
        (16, "icon_16x16.png"), (32, "icon_16x16@2x.png"),
        (32, "icon_32x32.png"), (64, "icon_32x32@2x.png"),
        (128, "icon_128x128.png"), (256, "icon_128x128@2x.png"),
        (256, "icon_256x256.png"), (512, "icon_256x256@2x.png"),
        (512, "icon_512x512.png"), (1024, "icon_512x512@2x.png"),
    ):
        mac_master.resize((px, px), Image.LANCZOS).save(os.path.join(iconset, name))
    icns = os.path.join(OUT, "aaa-ui.icns")
    try:
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", icns], check=True)
        print("wrote", icns)
    except Exception as exc:  # noqa: BLE001
        print("iconutil failed:", exc, file=sys.stderr)

    # ---- Android adaptive icon layers ----
    for folder, px in (
        ("mipmap-mdpi", 108), ("mipmap-hdpi", 162), ("mipmap-xhdpi", 216),
        ("mipmap-xxhdpi", 324), ("mipmap-xxxhdpi", 432),
    ):
        dest = os.path.join(OUT, "android", folder)
        os.makedirs(dest, exist_ok=True)
        draw_android_foreground(px).save(os.path.join(dest, "ic_launcher_foreground.png"))
        draw_android_background(px).save(os.path.join(dest, "ic_launcher_background.png"))
        # legacy square/round launcher icon
        draw_mark(px, inset_ratio=0.0).save(os.path.join(dest, "ic_launcher.png"))
    print("wrote", os.path.join(OUT, "android"))
    print("wrote", os.path.join(OUT, "logo-1024.png"))


if __name__ == "__main__":
    main()
