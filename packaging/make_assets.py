"""MSIX logo images from the Onyx icon.

Usage: python make_assets.py <assets dir> <out dir>

Large images come from `onyx.png` (1024px); 48px and under come from the
small frames in `onyx.ico`, which are drawn with a heavier rim to stay
readable. Each logo gets scale variants, and the app icon gets target-size
"unplated" variants so the taskbar and Start show it without a coloured plate.
"""
import os
import sys

from PIL import Image

src, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)

big = Image.open(os.path.join(src, 'onyx.png')).convert('RGBA')
ico = Image.open(os.path.join(src, 'onyx.ico'))
small = {}
for size in ico.info.get('sizes', []):
    ico.size = size
    small[size[0]] = ico.convert('RGBA').copy()


def icon(px):
    """The icon at px x px, from the frame drawn for that size if there is one."""
    if px in small:
        return small[px]
    if px < 64:
        base = min((s for s in small if s >= px), default=None)
        if base:
            return small[base].resize((px, px), Image.LANCZOS)
    return big.resize((px, px), Image.LANCZOS)


def canvas(w, h, icon_px):
    """The icon centred on a transparent w x h image."""
    im = Image.new('RGBA', (w, h), (0, 0, 0, 0))
    im.alpha_composite(icon(icon_px), ((w - icon_px) // 2, (h - icon_px) // 2))
    return im


def save(name, im):
    im.save(os.path.join(out, name))


SCALES = (100, 125, 150, 200, 400)

# The app icon (Start, taskbar, Settings): Square44x44Logo.
for s in SCALES:
    px = round(44 * s / 100)
    save(f'Square44x44Logo.scale-{s}.png', icon(px))
for t in (16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256):
    save(f'Square44x44Logo.targetsize-{t}.png', icon(t))
    save(f'Square44x44Logo.targetsize-{t}_altform-unplated.png', icon(t))
    save(f'Square44x44Logo.targetsize-{t}_altform-lightunplated.png', icon(t))

# Tiles: the icon centred with room around it.
for s in SCALES:
    k = s / 100
    save(f'Square150x150Logo.scale-{s}.png', canvas(round(150 * k), round(150 * k), round(96 * k)))
    save(f'Wide310x150Logo.scale-{s}.png', canvas(round(310 * k), round(150 * k), round(96 * k)))
    save(f'SmallTile.scale-{s}.png', canvas(round(71 * k), round(71 * k), round(50 * k)))
    save(f'LargeTile.scale-{s}.png', canvas(round(310 * k), round(310 * k), round(190 * k)))
    save(f'StoreLogo.scale-{s}.png', icon(round(50 * k)))

print('assets:', len(os.listdir(out)))
