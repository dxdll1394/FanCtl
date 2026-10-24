#!/usr/bin/env python3
"""Draw the FanCtl app icon (a four-blade fan on a dark rounded square)."""

import sys
import math
from PIL import Image, ImageDraw

SIZE = 1024
img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(img)

# Rounded background with a subtle vertical gradient.
radius = int(SIZE * 0.22)
for y in range(SIZE):
    t = y / SIZE
    r = int(0x1E + t * 0x0C)
    g = int(0x2A + t * 0x10)
    b = int(0x36 + t * 0x14)
    draw.line([(0, y), (SIZE, y)], fill=(r, g, b, 255))
mask = Image.new("L", (SIZE, SIZE), 0)
ImageDraw.Draw(mask).rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius, fill=255)
img.putalpha(mask)

draw = ImageDraw.Draw(img)
cx = cy = SIZE / 2
blades = 4
outer = SIZE * 0.34
inner = SIZE * 0.06

for i in range(blades):
    base = i * (2 * math.pi / blades)
    poly = [(cx, cy)]
    steps = 24
    for s in range(steps + 1):
        a = base + (s / steps) * (math.pi / 2.2)
        wobble = 0.45 + 0.55 * math.sin(s / steps * math.pi)
        poly.append((cx + math.cos(a) * outer * wobble,
                     cy + math.sin(a) * outer * wobble))
    poly.append((cx + math.cos(base + 0.25) * inner,
                 cy + math.sin(base + 0.25) * inner))
    draw.polygon(poly, fill=(0x35, 0xC7, 0x8F, 255))

# Hub.
draw.ellipse([cx - inner, cy - inner, cx + inner, cy + inner],
             fill=(0x16, 0x18, 0x1C, 255))
draw.ellipse([cx - inner * 0.45, cy - inner * 0.45,
              cx + inner * 0.45, cy + inner * 0.45],
             fill=(0x35, 0xC7, 0x8F, 255))

img.save(sys.argv[1])
