"""Onyx Island icon: a polished onyx pill on a black tile.

Drawn at 1024px and downsampled; the small sizes get a heavier rim so the
pill still reads on a 16px taskbar slot.
"""
import sys
from PIL import Image, ImageDraw, ImageFilter, ImageChops

OUT = sys.argv[1]
S = 1024


def vgrad(w, h, top, bottom):
    g = Image.new('RGBA', (1, h))
    for y in range(h):
        t = y / max(h - 1, 1)
        g.putpixel((0, y), tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(4)))
    return g.resize((w, h))


def mask_rr(size, box, r):
    m = Image.new('L', size, 0)
    ImageDraw.Draw(m).rounded_rectangle(box, r, fill=255)
    return m


def draw(rim_px, pw=780, ph=280, dot=True):
    img = Image.new('RGBA', (S, S), (0, 0, 0, 0))

    # Tile: near-black with a faint lift at the top.
    tile = vgrad(S, S, (28, 28, 34, 255), (6, 6, 8, 255))
    tile_mask = mask_rr((S, S), (0, 0, S - 1, S - 1), 232)
    img.paste(tile, (0, 0), tile_mask)

    # Tile inner edge: a hairline of light along the top.
    edge = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(edge).rounded_rectangle((3, 3, S - 4, S - 4), 229, outline=(255, 255, 255, 34), width=6)
    edge = ImageChops.multiply(edge, vgrad(S, S, (255, 255, 255, 255), (255, 255, 255, 0)))
    img.alpha_composite(edge)

    # Pill.
    x0, y0 = (S - pw) // 2, (S - ph) // 2 + 10
    box = (x0, y0, x0 + pw, y0 + ph)
    r = ph // 2

    # Soft cool glow under the pill.
    glow = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(glow).rounded_rectangle((x0 + 30, y0 + 40, x0 + pw - 30, y0 + ph + 30), r, fill=(120, 135, 255, 70))
    img.alpha_composite(glow.filter(ImageFilter.GaussianBlur(60)))

    # Rim: bright on top, fading down (a polished edge catching light).
    rim = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(rim).rounded_rectangle(box, r, fill=(255, 255, 255, 255))
    rim = ImageChops.multiply(rim, vgrad(S, S, (255, 255, 255, 255), (255, 255, 255, 255)))
    rim_shade = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    rim_shade.paste(vgrad(pw, ph, (235, 238, 255, 255), (70, 72, 90, 255)), (x0, y0))
    rim_mask = mask_rr((S, S), box, r)
    img.paste(rim_shade, (0, 0), rim_mask)

    # Body: the onyx itself, deep black with a slight sheen.
    ib = (x0 + rim_px, y0 + rim_px, x0 + pw - rim_px, y0 + ph - rim_px)
    ir = r - rim_px
    body = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    body.paste(vgrad(pw, ph, (34, 34, 42, 255), (2, 2, 3, 255)), (x0, y0))
    img.paste(body, (0, 0), mask_rr((S, S), ib, ir))

    # Onyx banding: two faint curved strata through the stone.
    bands = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(bands)
    d.arc((x0 - 200, y0 + 70, x0 + pw + 200, y0 + ph + 520), 200, 340, fill=(255, 255, 255, 30), width=10)
    d.arc((x0 - 260, y0 + 120, x0 + pw + 260, y0 + ph + 620), 200, 340, fill=(255, 255, 255, 16), width=8)
    bands = bands.filter(ImageFilter.GaussianBlur(3))
    clip = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    clip.paste(bands, (0, 0), mask_rr((S, S), ib, ir))
    img.alpha_composite(clip)

    # Specular glint, top left of the stone.
    glint = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(glint).rounded_rectangle((x0 + r * 0.7, y0 + rim_px + ph * 0.09, x0 + pw * 0.42, y0 + rim_px + ph * 0.14), 12, fill=(255, 255, 255, 70))
    img.alpha_composite(glint.filter(ImageFilter.GaussianBlur(8)))

    # Live dot: the island's activity light, right end of the stone.
    if dot:
        cx, cy, dr = x0 + pw - r * 0.78, y0 + ph // 2, ph * (0.1 if pw < 850 else 0.12)
        halo = Image.new('RGBA', (S, S), (0, 0, 0, 0))
        ImageDraw.Draw(halo).ellipse((cx - dr * 2.2, cy - dr * 2.2, cx + dr * 2.2, cy + dr * 2.2), fill=(130, 150, 255, 120))
        img.alpha_composite(halo.filter(ImageFilter.GaussianBlur(dr)))
        ImageDraw.Draw(img).ellipse((cx - dr, cy - dr, cx + dr, cy + dr), fill=(165, 180, 255, 255))

    return img


big = draw(rim_px=14)
small = draw(rim_px=46, pw=900, ph=400)
big.save(OUT + r'\onyx-1024.png')
for size in (256, 128, 64):
    big.resize((size, size), Image.LANCZOS).save(OUT + rf'\onyx-{size}.png')
for size in (48, 32, 24, 16):
    small.resize((size, size), Image.LANCZOS).save(OUT + rf'\onyx-{size}.png')

frames = [big.resize((s, s), Image.LANCZOS) for s in (256, 128, 64)]
frames += [small.resize((s, s), Image.LANCZOS) for s in (48, 40, 32, 24, 20, 16)]
frames[0].save(OUT + r'\onyx.ico', format='ICO', sizes=[f.size for f in frames], append_images=frames[1:])

# Preview sheet: sizes on dark and light taskbar strips.
sheet = Image.new('RGBA', (900, 420), (245, 245, 247, 255))
ImageDraw.Draw(sheet).rectangle((0, 210, 900, 420), fill=(24, 24, 28, 255))
sheet.alpha_composite(big.resize((180, 180), Image.LANCZOS), (20, 15))
sheet.alpha_composite(big.resize((180, 180), Image.LANCZOS), (20, 225))
x = 240
for s in (64, 48, 32, 24, 16):
    src = big if s >= 64 else small
    icon = src.resize((s, s), Image.LANCZOS)
    sheet.alpha_composite(icon, (x, 105 - s // 2))
    sheet.alpha_composite(icon, (x, 315 - s // 2))
    x += s + 60
sheet.save(OUT + r'\preview.png')
print('ok')
