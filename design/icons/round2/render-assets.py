"""Rasterize SVGs with librsvg; compose an explicitly labeled fallback proof."""
import argparse
from io import BytesIO
from pathlib import Path
import runpy
import subprocess

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
catalog = runpy.run_path(str(HERE / 'build-sheet.py'))
CONCEPTS = catalog['CONCEPTS']
master_path = catalog['master_path']
optical_path = catalog['optical_path']
FONT = '/System/Library/Fonts/SFNS.ttf'
MONO = '/System/Library/Fonts/SFNSMono.ttf'
SCALE = 2

def raster(filename, size):
    result = subprocess.run(['rsvg-convert', '--width', str(size), '--height', str(size),
                             str(HERE / filename)], check=True, capture_output=True)
    return Image.open(BytesIO(result.stdout)).convert('RGBA')

def font(size, mono=False):
    return ImageFont.truetype(MONO if mono else FONT, round(size * SCALE))

def proof(output):
    canvas = Image.new('RGB', (3200, 2000), '#101113')
    d = ImageDraw.Draw(canvas)

    def box(x, y, w, h, color, radius=0, outline=None):
        bounds = tuple(round(v * SCALE) for v in (x, y, x+w, y+h))
        d.rounded_rectangle(bounds, radius=round(radius*SCALE), fill=color,
                            outline=outline, width=SCALE)

    def text(x, y, value, size=12, color='#EDEEF0', mono=False, anchor='lt'):
        d.text((round(x*SCALE), round(y*SCALE)), value, font=font(size, mono),
               fill=color, anchor=anchor)

    def icon(source, x, y, size):
        bitmap = raster(source, size*SCALE)
        canvas.paste(bitmap, (round(x*SCALE), round(y*SCALE)), bitmap)

    text(28, 26, 'PENGUIN / ROUND 02', 10, '#97ADBD', mono=True)
    text(28, 44, 'Less detail.', 27)
    text(164, 44, 'More signal.', 27, '#747A83')
    text(1572, 30, 'Eight abstract studies for a quieter inbox.', 12, '#C0C4CB', anchor='rt')
    text(1572, 50, '1024 SVG masters · 100px margin · 2–3 shapes · 256 / 64 / 32 / 16px', 11, '#777E87', anchor='rt')

    for n, slug, name, description, spec in CONCEPTS:
        col, row = (n-1) % 4, (n-1) // 4
        x, y = 28 + col*391.5, 94 + row*378
        box(x, y, 377.5, 364, '#191B1E', 16, '#2A2D31')
        text(x+17, y+17, f'0{n}', 10, '#777E87', mono=True)
        text(x+42, y+14, name, 15)
        if n in [2, 6]:
            box(x+324, y+11, 36, 19, '#191B1E', 5, '#435967')
            text(x+342, y+16, 'Pick', 9, '#BAD2E2', anchor='mt')
        icon(master_path(n, slug), x+1, y+42, 256)
        mx, my = x+263, y+73
        for theme, oy, bg, outline, fg in [('Dark',0,'#111214','#303339','#969CA5'),
                                           ('Light',104,'#F3F3F5','#E0E1E4','#6A7079')]:
            box(mx, my+oy, 104, 95, bg, 9, outline)
            text(mx+9, my+oy+11, theme, 9, fg)
            icon(optical_path(n, slug), mx+10, my+oy+35, 32)
            icon(optical_path(n, slug), mx+74, my+oy+51, 16)
            text(mx+26, my+oy+72, '32', 9, fg, mono=True, anchor='mt')
            text(mx+82, my+oy+72, '16', 9, fg, mono=True, anchor='mt')
        text(x+17, y+308, description, 11, '#B0B5BC')
        d.line((round((x+1)*SCALE), round((y+330)*SCALE),
                round((x+376.5)*SCALE), round((y+330)*SCALE)), fill='#25282C', width=SCALE)
        text(x+17, y+342, spec, 9, '#7F8893')
        text(x+360, y+342, '512 PNG ↗', 9, '#ABB7C3', anchor='rt')

    box(28, 850, 1544.5+7.5, 102, '#263540', 16, '#37424B')
    text(50, 882, 'Small by design.', 15)
    text(50, 905, '64px in the Dock · consistent canvas margins', 10, '#AFBFCB')
    dx = 466
    box(dx, 858, 642, 86, '#34444F', 20, '#596975')
    for n, slug, name, description, spec in CONCEPTS:
        ix = dx+16+(n-1)*78
        icon(master_path(n, slug), ix, 862, 64)
        text(ix+32, 929, f'0{n}', 9, '#AFC0CE', mono=True, anchor='mt')
    for i, color in enumerate(['#182B3A', '#FFFFFF', '#C4DAE8', '#F4AA64']):
        box(1484+i*18, 870, 12, 12, color, 6)
    text(1550, 892, 'Ink + white + ice or orange', 10, '#AFC0CD', anchor='rt')
    text(1550, 908, 'One accent per concept', 10, '#AFC0CD', anchor='rt')
    text(28, 966, 'Shortlist: 06 Cut pebble + 02 Flap & dot · penguin character, reduced to geometry.', 10, '#B8C5D0')
    text(1572, 966, 'Librsvg composite · 1600 × 1000 @2× · Chrome capture unavailable', 10, '#7E8893', anchor='rt')
    output.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(output)
    print(f'Proof: {output.relative_to(HERE)} (3200 × 2000, librsvg + Pillow)')

def small_review(output):
    canvas = Image.new('RGB', (1600, 780), '#101113')
    d = ImageDraw.Draw(canvas)
    heading = ImageFont.truetype(FONT, 21)
    label = ImageFont.truetype(FONT, 12)
    d.text((24, 20), 'Optical review / actual 1× pixels + 4× nearest-neighbor enlargement', fill='#EDEEF0', font=heading)
    for n, slug, name, description, spec in CONCEPTS:
        col, row = (n-1) % 4, (n-1) // 4
        x, y = 24+col*394, 65+row*350
        d.text((x, y), f'0{n} / {name}', font=heading, fill='#EDEEF0')
        for theme, oy, bg, fg in [('Dark',35,'#111214','#9FA3A9'), ('Light',178,'#F3F3F5','#6A7079')]:
            d.rounded_rectangle((x,y+oy,x+370,y+oy+130),radius=10,fill=bg)
            d.text((x+12,y+oy+10), theme+'   32 / 16',font=label,fill=fg)
            for size, xpos in [(32,x+12),(16,x+62)]:
                bitmap = raster(optical_path(n, slug),size)
                canvas.paste(bitmap,(xpos,y+oy+60-size//2),bitmap)
            for size, xpos in [(32,x+105),(16,x+268)]:
                bitmap = raster(optical_path(n, slug),size).resize((size*4,size*4),Image.Resampling.NEAREST)
                canvas.paste(bitmap,(xpos,y+oy+65-size*2),bitmap)
    output.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(output)
    print(f'Optical review: {output.relative_to(HERE)}')

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', default='icons-round2-sheet.png')
    parser.add_argument('--small-output', default='review/small-size-review.png')
    args = parser.parse_args()
    paths = [(HERE / s).resolve() for s in [args.output, args.small_output]]
    if any(not p.is_relative_to(HERE) or p.suffix != '.png' for p in paths):
        parser.error('All outputs must be PNGs within round2/.')
    (HERE / 'png').mkdir(exist_ok=True)
    for n, slug, *_ in CONCEPTS:
        raster(master_path(n, slug), 512).save(HERE / 'png' / f'icon-{n}.png')
    proof(paths[0])
    small_review(paths[1])
