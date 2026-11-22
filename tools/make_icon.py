"""Render the rstrace app icon (assets/icon.png, assets/icon.ico).

Design: a dark tracing frame with reference dots around a white sheet, and an
orange vector curve with node handles. Drawn at 4x and downsampled.
Run: python3 tools/make_icon.py
"""
import math
from pathlib import Path
from PIL import Image, ImageDraw

S = 1024  # working size
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

# Background tile.
d.rounded_rectangle([24, 24, S - 24, S - 24], radius=210, fill=(32, 92, 168, 255))
# Subtle vertical shading.
shade = Image.new("RGBA", (S, S), (0, 0, 0, 0))
sd = ImageDraw.Draw(shade)
for y in range(S):
    a = int(70 * y / S)
    sd.line([(0, y), (S, y)], fill=(0, 20, 60, a))
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).rounded_rectangle([24, 24, S - 24, S - 24], radius=210, fill=255)
img.paste(Image.alpha_composite(img, shade), (0, 0), mask)
d = ImageDraw.Draw(img)

# Frame.
fx0, fy0, fx1, fy1 = 150, 190, S - 150, S - 190
d.rounded_rectangle([fx0, fy0, fx1, fy1], radius=70, fill=(28, 30, 34, 255))
# Reference dots on the frame band.
band = 34
for i in range(9):
    x = fx0 + 80 + i * (fx1 - fx0 - 160) / 8
    r = 13 if i % 3 else 18
    for y in (fy0 + band, fy1 - band):
        d.ellipse([x - r, y - r, x + r, y + r], fill=(235, 238, 240, 255))
for i in range(5):
    y = fy0 + 120 + i * (fy1 - fy0 - 240) / 4
    r = 13 if i % 2 else 17
    for x in (fx0 + band, fx1 - band):
        d.ellipse([x - r, y - r, x + r, y + r], fill=(235, 238, 240, 255))
# Sheet.
sx0, sy0, sx1, sy1 = fx0 + 78, fy0 + 78, fx1 - 78, fy1 - 78
d.rounded_rectangle([sx0, sy0, sx1, sy1], radius=36, fill=(250, 250, 247, 255))

# Vector curve (cubic Bézier) across the sheet.
def bez(p0, p1, p2, p3, t):
    mt = 1 - t
    return tuple(mt**3 * a + 3 * mt * mt * t * b + 3 * mt * t * t * c + t**3 * e for a, b, c, e in zip(p0, p1, p2, p3))

P0, P1, P2, P3 = (sx0 + 40, sy1 - 70), (sx0 + 190, sy0 - 60), (sx1 - 230, sy1 + 90), (sx1 - 40, sy0 + 70)
orange = (255, 128, 20, 255)
# Stamp discs along the curve for a clean round-capped stroke.
for i in range(1601):
    x, y = bez(P0, P1, P2, P3, i / 1600)
    d.ellipse([x - 15, y - 15, x + 15, y + 15], fill=orange)
# Handles at the start node.
handle = (64, 70, 80, 255)
mid = bez(P0, P1, P2, P3, 0.5)
t = 0.5
dx = 3 * ((1 - t) ** 2 * (P1[0] - P0[0]) + 2 * (1 - t) * t * (P2[0] - P1[0]) + t * t * (P3[0] - P2[0]))
dy = 3 * ((1 - t) ** 2 * (P1[1] - P0[1]) + 2 * (1 - t) * t * (P2[1] - P1[1]) + t * t * (P3[1] - P2[1]))
L = math.hypot(dx, dy)
hx, hy = dx / L * 150, dy / L * 150
a, b = (mid[0] - hx, mid[1] - hy), (mid[0] + hx, mid[1] + hy)
d.line([a, b], fill=handle, width=9)
for p in (a, b):
    d.ellipse([p[0] - 17, p[1] - 17, p[0] + 17, p[1] + 17], fill=handle)
n = 30
d.rectangle([mid[0] - n, mid[1] - n, mid[0] + n, mid[1] + n], fill=(255, 255, 255, 255), outline=orange, width=10)

out = Path(__file__).resolve().parent.parent / "assets"
out.mkdir(exist_ok=True)
icon = img.resize((256, 256), Image.LANCZOS)
icon.save(out / "icon.png")
img.resize((512, 512), Image.LANCZOS).save(out / "icon-512.png")
icon.save(out / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
print("wrote", out)
