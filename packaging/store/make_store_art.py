"""Microsoft Store art for Onyx: wallpaper and demo cover for the screenshots,
then the Store logos (poster, box art, tile icons) and captioned screenshots.

Usage:
  python make_store_art.py prep <assets dir> <out dir>
      wallpaper.png (1920x1080), cover.bgra (raw BGRA for ISLET_DEMO_ART)
  python make_store_art.py finish <assets dir> <out dir>
      logos, plus captioned screenshots from raw-*.png captures in <out dir>
"""
import math
import os
import random
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

mode, assets, out = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(out, exist_ok=True)
W, H = 1920, 1080
BG = (11, 11, 14)


def blobs(w, h, spots, blur):
    """Soft colour glows on near-black."""
    im = Image.new('RGB', (w, h), BG)
    layer = Image.new('RGB', (w, h), (0, 0, 0))
    d = ImageDraw.Draw(layer)
    for (x, y, r, color) in spots:
        d.ellipse((x - r, y - r, x + r, y + r), fill=color)
    layer = layer.filter(ImageFilter.GaussianBlur(blur))
    return Image.blend(im, layer, 0.55)


def grain(im, amount=6):
    rnd = random.Random(7)
    noise = Image.effect_noise(im.size, 40).convert('RGB')
    return Image.blend(im, noise, amount / 255)


def font(size, weight='Semibold'):
    path = r'C:\Windows\Fonts\SegUIVar.ttf'
    f = ImageFont.truetype(path, size)
    try:
        f.set_variation_by_name(weight)
    except Exception:
        pass
    return f


if mode == 'prep':
    # Desktop wallpaper: dark, with a warm glow low on the screen so the top
    # (where the island sits) stays calm.
    wall = blobs(W, H, [
        (380, 980, 520, (200, 92, 40)),
        (1500, 1120, 600, (120, 50, 150)),
        (980, 1300, 500, (40, 60, 160)),
    ], 180)
    grain(wall).save(os.path.join(out, 'wallpaper.png'))

    # Demo cover: an original abstract sunset (not a real album's artwork).
    S = 300
    cover = Image.new('RGB', (S, S))
    px = cover.load()
    for y in range(S):
        t = y / (S - 1)
        for x in range(S):
            r = int(250 * (1 - t) + 70 * t)
            g = int(150 * (1 - t) + 40 * t)
            b = int(90 * (1 - t) + 120 * t)
            px[x, y] = (r, g, b)
    d = ImageDraw.Draw(cover)
    d.ellipse((S * 0.28, S * 0.36, S * 0.72, S * 0.80), fill=(255, 214, 150))
    for i in range(5):
        y = S * 0.62 + i * 14
        d.rectangle((0, y, S, y + 6), fill=(70 + i * 6, 40, 110))
    cover = cover.filter(ImageFilter.GaussianBlur(1.2))
    cover.save(os.path.join(out, 'cover.png'))
    b, g, r = cover.split()[2], cover.split()[1], cover.split()[0]
    bgra = Image.merge('RGBA', (b, g, r, Image.new('L', (S, S), 255)))
    open(os.path.join(out, 'cover.bgra'), 'wb').write(bgra.tobytes())
    print('prep done')
    sys.exit()

# ---- finish: logos --------------------------------------------------------
icon = Image.open(os.path.join(assets, 'onyx.png')).convert('RGBA')


def logo_canvas(w, h, icon_frac, wordmark):
    im = blobs(w, h, [(w * 0.5, h * 0.62, min(w, h) * 0.55, (70, 70, 120))], min(w, h) * 0.22)
    im = grain(im).convert('RGBA')
    side = int(min(w, h) * icon_frac)
    cy = h * (0.42 if wordmark else 0.5)
    im.alpha_composite(icon.resize((side, side), Image.LANCZOS), (int((w - side) / 2), int(cy - side / 2)))
    if wordmark:
        f = font(int(side * 0.36))
        d = ImageDraw.Draw(im)
        tw = d.textlength('Onyx', font=f)
        d.text(((w - tw) / 2, cy + side * 0.62), 'Onyx', font=f, fill=(242, 242, 245))
    return im.convert('RGB')


logo_canvas(1440, 2160, 0.42, True).save(os.path.join(out, 'poster-1440x2160.png'))
logo_canvas(2160, 2160, 0.42, True).save(os.path.join(out, 'boxart-2160x2160.png'))
for s in (300, 150, 71):
    icon.resize((s, s), Image.LANCZOS).save(os.path.join(out, f'tile-{s}x{s}.png'))

# ---- finish: screenshots ---------------------------------------------------
# Captures from the app's demo mode (see packaging/store/README.md):
#   strip-*.png      the top-centre 1100x260 of the screen, island over the
#                    wallpaper, taken at x=410
#   window-settings  the Settings window alone (PrintWindow)
# Only the island's band of each strip is used.
STRIP_X, BAND = 410, 150
wall = Image.open(os.path.join(out, 'wallpaper.png')).convert('RGBA')


def island_band(name):
    return Image.open(os.path.join(out, name)).convert('RGBA').crop((0, 0, 1100, BAND))


def caption(im, title, sub):
    d = ImageDraw.Draw(im)
    d.text((96, H - 214), title, font=font(64), fill=(245, 245, 247))
    d.text((98, H - 126), sub, font=font(30, 'Regular'), fill=(206, 206, 214))


def shadow(im, box, radius=10, blur=40, alpha=170):
    layer = Image.new('RGBA', im.size, (0, 0, 0, 0))
    x0, y0, x1, y1 = box
    ImageDraw.Draw(layer).rounded_rectangle((x0, y0 + 24, x1, y1 + 24), radius, fill=(0, 0, 0, alpha))
    im.alpha_composite(layer.filter(ImageFilter.GaussianBlur(blur)))


# 1. The open player at the top of the screen.
shot = wall.copy()
shot.alpha_composite(island_band('strip-player.png'), (STRIP_X, 0))
caption(shot, 'Your music, at the top of your screen.', 'Hover the island to open the player. Move away and it folds back.')
shot.convert('RGB').save(os.path.join(out, 'screenshot-1-player.png'))

# 2. Settings, with the island playing up top.
shot = wall.copy()
shot.alpha_composite(island_band('strip-compact.png'), (STRIP_X, 0))
win = Image.open(os.path.join(out, 'window-settings.png')).convert('RGBA')
# The "Your island" preview is a live child window the capture leaves out;
# show the same compact island there, centred where the app draws it.
compact = Image.open(os.path.join(out, 'strip-compact.png')).convert('RGBA')
# Find the resting island where it was drawn (whatever differs from the
# wallpaper behind it), and cut out just the pill shape, so none of the
# wallpaper comes along.
behind = wall.crop((STRIP_X, 0, STRIP_X + 1100, 260)).convert('RGB')
diff = ImageChops.difference(compact.convert('RGB').crop((0, 0, 1100, 80)), behind.crop((0, 0, 1100, 80)))
box = diff.convert('L').point(lambda v: 255 if v > 18 else 0).getbbox()
pill = compact.crop(box)
pill_mask = Image.new('L', pill.size, 0)
ImageDraw.Draw(pill_mask).rounded_rectangle((0, 0, pill.width - 1, pill.height - 1), pill.height // 2, fill=255)
win.paste(pill, (603 - pill.width // 2, 160 - pill.height // 2), pill_mask)
wx, wy = W - win.width - 72, 132
shadow(shot, (wx, wy, wx + win.width, wy + win.height))
mask = Image.new('L', win.size, 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, win.width - 1, win.height - 1), 8, fill=255)
shot.paste(win, (wx, wy), mask)
caption(shot, 'Make it yours.', 'Every setting saves as you go.')
shot.convert('RGB').save(os.path.join(out, 'screenshot-2-settings.png'))
print('screenshots written')
