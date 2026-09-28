"""Export the round-3 assets and a 1600 × 1000 @2× librsvg proof.

All writes stay beside this script. Masters are hand-authored; this script
reads them and never rewrites their geometry. Requires rsvg-convert and Pillow.
"""
from base64 import b64encode
from functools import lru_cache
from html import escape
from io import BytesIO
from pathlib import Path
import subprocess

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
ITEMS = [
    dict(id="A", slug="tuxedo-split-a", name="Tuxedo split", tag="RECOMMENDED",
         note="−15° · White flap, curved ink back, a gentle forward point.", shapes=2),
    dict(id="B", slug="tuxedo-split-b", name="Tuxedo split", tag="LIFTED BEAK",
         note="−30° · A steeper fold gives the bird a stronger direction.", shapes=2),
    dict(id="C", slug="tuxedo-split-c", name="Tuxedo split", tag="QUARTER-TURN",
         note="90° · The envelope V turns into a left-facing ink beak.", shapes=2),
    dict(id="D", slug="tuxedo-split-d", name="Tuxedo split", tag="MIRROR TURN",
         note="Mirrored 90° · The same split points the bird to the right.", shapes=2),
    dict(id="04", slug="ice-floe", name="Ice floe", tag="FAVORITE",
         note="Raised slightly for balance; the small-size notch stays open.", shapes=2),
    dict(id="06", slug="cut-pebble", name="Cut pebble", tag="RETAINED",
         note="Round 2 unchanged · Tilted pebble with a broad envelope cut.", shapes=3),
]


def optical(slug):
    candidate = f"{slug}-small.svg"
    return candidate if (HERE / candidate).exists() else f"{slug}.svg"


@lru_cache(maxsize=None)
def raster(filename, size):
    return subprocess.run(
        ["rsvg-convert", "--width", str(size), "--height", str(size),
         str(HERE / filename)], capture_output=True, check=True,
    ).stdout


def build_html():
    cards = []
    for item in ITEMS:
        slug = item["slug"]
        panels = []
        for theme in ("dark", "light"):
            panels.append(f'''<div class="surface {theme}">
              <a class="large" href="{slug}.svg" aria-label="Open {item['id']} {item['name']} SVG">
                <img src="{slug}.svg" width="200" height="200" alt="{item['name']} {item['id']} on {theme}"></a>
              <div class="micro"><span class="theme">{theme.title()}</span>
                <div class="sample"><img src="{optical(slug)}" width="32" height="32" alt="32 pixel sample"><span>32</span></div>
                <div class="sample"><img src="{optical(slug)}" width="16" height="16" alt="16 pixel sample"><span>16</span></div>
              </div></div>''')
        cards.append(f'''<article class="card {'recommended' if item['id'] == 'A' else ''}">
          <header class="card-head"><span class="id">{item['id']}</span>
            <h2><a href="{slug}.svg">{item['name']}</a></h2><span class="tag">{item['tag']}</span></header>
          <div class="surfaces">{''.join(panels)}</div>
          <p class="note">{item['note']}</p><footer class="card-foot">
            <span>{item['shapes']} shapes / 2 colors</span>
            <span><a href="{slug}.svg">SVG ↗</a><span class="sep">·</span><a href="png/{slug}.png">1024 PNG ↗</a></span>
          </footer></article>''')
    docks = []
    for theme in ("dark", "light"):
        icons = ''.join(
            f'''<a class="dock-item" href="{i['slug']}.svg" aria-label="{i['id']} {i['name']}, 64 pixels">
              <img src="{i['slug']}.svg" width="64" height="64" alt=""><span>{i['id']}</span></a>'''
            for i in ITEMS)
        docks.append(f'<div class="dock {theme}"><span class="dock-theme">{theme.title()}</span>{icons}</div>')
    (HERE / 'index.html').write_text(f'''<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Penguin / Round 03 — Bird × envelope</title>
<style>
:root{{color-scheme:dark;--bg:#101416;--card:#191F23;--line:#303A40;--ink:#EAF0F3;--muted:#AAB8C1}}
*{{box-sizing:border-box}}html,body{{margin:0;background:var(--bg);color:var(--ink)}}
body{{font:12px/1.4 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;-webkit-font-smoothing:antialiased}}
a{{color:inherit;text-decoration:none}}a:hover{{text-decoration:underline}}a:focus-visible{{outline:2px solid #AED8E5;outline-offset:3px}}img{{display:block;flex-shrink:0}}
main{{width:1600px;height:1000px;padding:28px 32px 16px;margin:auto}}
.masthead{{height:80px;display:flex;align-items:flex-start;justify-content:space-between}}
.eyebrow{{font:10px/16px ui-monospace,SFMono-Regular,Menlo,monospace;letter-spacing:1.8px;color:#A4BBC9;margin:0 0 4px}}
h1{{font-size:29px;line-height:36px;font-weight:500;letter-spacing:-1px;margin:0}}h1 span{{color:#82949F;font-weight:400}}
.intro{{text-align:right;padding-top:8px}}.intro p{{font-size:12px;margin:0 0 7px}}.intro small{{color:#91A2AE;font-size:10px}}
.grid{{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:16px}}
.card{{height:342px;border:1px solid var(--line);border-radius:15px;background:var(--card);overflow:hidden}}
.card.recommended{{border-color:#6A909E}}.card-head{{height:41px;display:flex;align-items:center;padding:0 16px;gap:10px}}
.id{{font:11px ui-monospace,SFMono-Regular,Menlo,monospace;color:#9AB5C4;width:20px}}
h2{{font-size:15px;font-weight:500;letter-spacing:-.2px;margin:0}}.tag{{margin-left:auto;color:#93A6B3;font-size:8px;letter-spacing:1px}}
.recommended .tag{{color:#BCE3DC;border:1px solid #527D7C;border-radius:5px;padding:3px 7px;letter-spacing:.7px}}
.surfaces{{height:248px;display:grid;grid-template-columns:1fr 1fr;gap:12px;padding:0 11px}}
.surface{{border-radius:9px;overflow:hidden}}.dark{{background:#10181D;color:#A6B8C3}}.light{{background:#EEF2F4;color:#586B77}}
.large{{display:block;width:200px;height:200px;margin:0 auto}}.micro{{height:48px;display:flex;align-items:center;padding:0 14px;gap:16px}}
.theme{{font-size:9px;margin-right:auto}}.sample{{display:flex;align-items:center;gap:5px}}.sample span{{font:9px ui-monospace,SFMono-Regular,Menlo,monospace}}
.note{{height:32px;margin:0;padding:0 15px;display:flex;align-items:center;white-space:nowrap;font-size:11px;color:#C3CFD6}}
.card-foot{{height:20px;padding:0 15px;border-top:1px solid #2B343A;display:flex;align-items:center;justify-content:space-between;font-size:9px;color:#96A9B6}}
.sep{{padding:0 8px;color:#647985}}
.dock-section{{height:126px;margin-top:16px;display:flex;align-items:center;gap:16px}}
.dock-copy{{flex:1;padding-left:4px}}.dock-copy h2{{font-size:17px;margin-bottom:7px}}.dock-copy p{{font-size:10px;color:#9AADB9;margin:0;line-height:18px}}
.dock{{width:596px;height:106px;border:1px solid #35444F;border-radius:16px;display:flex;align-items:center;padding:0 20px 0 16px;gap:20px}}
.dock.light{{border-color:#CFDADF}}.dock-theme{{font-size:9px;width:30px;flex-shrink:0}}.dock-item{{display:flex;flex-direction:column;align-items:center;width:64px;flex-shrink:0;gap:1px}}
.dock-item span{{font:9px/14px ui-monospace,SFMono-Regular,Menlo,monospace}}.bottom{{height:34px;display:flex;align-items:flex-end;justify-content:space-between;font-size:10px;color:#8FA4B2}}
.bottom strong{{font-weight:500;color:#BBD9DB}}
@media(max-width:1599px){{main{{margin:0}}body{{overflow-x:auto}}}}
</style></head><body><main>
<header class="masthead"><div><p class="eyebrow">PENGUIN / ROUND 03</p><h1>Bird × envelope. <span>One quiet gesture.</span></h1></div>
<div class="intro"><p>Four Tuxedo orientations. Two favorites carried forward.</p><small>200 / 64 / 32 / 16px · fixed squircle · 100px transparent margins · ink + white</small></div></header>
<section class="grid" aria-label="Six icon studies on dark and light surfaces">{''.join(cards)}</section>
<section class="dock-section" aria-label="64 pixel Dock comparisons"><div class="dock-copy"><h2>The Dock test.</h2>
<p>64px, same canvas and margins.<br>16–32px: optical files where needed.</p></div>{''.join(docks)}</section>
<footer class="bottom"><span><strong>Recommend A / Forward fold.</strong> The clearest balance of envelope flap and bird gesture.</span>
<span><a href="icons-round3-sheet.png">3200 × 2000 proof ↗</a> · <a href="review/small-size-review.png">1× optical review ↗</a> · <a href="README.md">Asset notes ↗</a></span></footer>
</main></body></html>''')


def build_svg_sheet():
    parts = ['<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="1000" viewBox="0 0 1600 1000">',
             '<rect width="1600" height="1000" fill="#101416"/>']

    def box(x, y, w, h, fill, radius=0, border=None):
        stroke = f' stroke="{border}"' if border else ''
        parts.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{fill}"{stroke}/>')

    def text(x, y, value, size=12, color='#EAF0F3', anchor='start', mono=False, weight=400):
        family = 'Menlo, monospace' if mono else 'Helvetica Neue, Helvetica, Arial, sans-serif'
        parts.append(f'<text x="{x}" y="{y}" fill="{color}" font-family="{family}" font-size="{size}" '
                     f'font-weight="{weight}" text-anchor="{anchor}">{escape(value)}</text>')

    def icon(source, x, y, size):
        data = b64encode(raster(source, size * 2)).decode()
        parts.append(f'<image x="{x}" y="{y}" width="{size}" height="{size}" href="data:image/png;base64,{data}"/>')

    text(32, 41, 'PENGUIN / ROUND 03', 10, '#A4BBC9', mono=True)
    parts.append('<text x="32" y="78" font-family="Helvetica Neue, Helvetica, Arial, sans-serif" '
                 'font-size="29"><tspan fill="#EAF0F3" font-weight="500">Bird × envelope.</tspan>'
                 '<tspan dx="8" fill="#82949F">One quiet gesture.</tspan></text>')
    text(1568, 47, 'Four Tuxedo orientations. Two favorites carried forward.', 12, anchor='end')
    text(1568, 69, '200 / 64 / 32 / 16px · fixed squircle · 100px transparent margins · ink + white', 10, '#91A2AE', 'end')

    cw = (1536 - 32) / 3
    pw = (cw - 36) / 2
    for n, item in enumerate(ITEMS):
        x, y = 32 + (n % 3) * (cw + 16), 108 + (n // 3) * 358
        box(x, y, cw, 342, '#191F23', 15, '#6A909E' if n == 0 else '#303A40')
        text(x + 16, y + 26, item['id'], 11, '#9AB5C4', mono=True)
        text(x + 46, y + 27, item['name'], 15, weight=500)
        if n == 0:
            box(x + cw - 120, y + 10, 104, 22, '#191F23', 5, '#527D7C')
        text(x + cw - 23, y + 25, item['tag'], 8, '#BCE3DC' if n == 0 else '#93A6B3', 'end')
        for t, theme in enumerate(('dark', 'light')):
            px, py = x + 12 + t * (pw + 12), y + 42
            color = '#A6B8C3' if t == 0 else '#586B77'
            box(px, py, pw, 248, '#10181D' if t == 0 else '#EEF2F4', 9)
            icon(item['slug'] + '.svg', px + (pw - 200) / 2, py, 200)
            text(px + 14, py + 227, theme.title(), 9, color)
            icon(optical(item['slug']), px + pw - 118, py + 208, 32)
            text(px + pw - 81, py + 227, '32', 9, color, mono=True)
            icon(optical(item['slug']), px + pw - 50, py + 216, 16)
            text(px + pw - 29, py + 227, '16', 9, color, mono=True)
        text(x + 15, y + 311, item['note'], 11, '#C3CFD6')
        box(x + 1, y + 322, cw - 2, 1, '#2B343A')
        text(x + 15, y + 335, f"{item['shapes']} shapes / 2 colors", 9, '#96A9B6')
        text(x + cw - 15, y + 335, 'SVG ↗   ·   1024 PNG ↗', 9, '#96A9B6', 'end')

    text(36, 870, 'The Dock test.', 17, weight=500)
    text(36, 896, '64px, same canvas and margins.', 10, '#9AADB9')
    text(36, 914, '16–32px: optical files where needed.', 10, '#9AADB9')
    for t, theme in enumerate(('dark', 'light')):
        x, y = 360 + t * 612, 834
        color = '#A6B8C3' if t == 0 else '#586B77'
        box(x, y, 596, 106, '#10181D' if t == 0 else '#EEF2F4', 16, '#35444F' if t == 0 else '#CFDADF')
        text(x + 16, y + 57, theme.title(), 9, color)
        for n, item in enumerate(ITEMS):
            ix = x + 66 + n * 84
            icon(item['slug'] + '.svg', ix, y + 13, 64)
            text(ix + 32, y + 92, item['id'], 9, color, 'middle', mono=True)
    text(32, 979, 'Recommend A / Forward fold.', 10, '#BBD9DB', weight=500)
    text(185, 979, 'The clearest balance of envelope flap and bird gesture.', 10, '#8FA4B2')
    text(1568, 979, 'librsvg composition · 1600 × 1000 @2× · ink #182B3A + white', 10, '#8FA4B2', 'end')
    parts.append('</svg>')
    (HERE / 'icons-round3-sheet.svg').write_text('\n'.join(parts))
    (HERE / 'icons-round3-sheet.png').write_bytes(subprocess.run(
            ['rsvg-convert', '--width', '3200', '--height', '2000', str(HERE / 'icons-round3-sheet.svg')],
            capture_output=True, check=True).stdout)


def small_review():
    canvas = Image.new('RGB', (1536, 790), '#101416')
    d = ImageDraw.Draw(canvas)
    font_path = '/System/Library/Fonts/SFNS.ttf'
    heading = ImageFont.truetype(font_path, 20)
    label = ImageFont.truetype(font_path, 12)
    d.text((24, 20), '1× optical review / actual 32 and 16px + 4× nearest-neighbor enlargement', font=heading, fill='#EAF0F3')
    for n, item in enumerate(ITEMS):
        x, y = 24 + (n % 3) * 508, 65 + (n // 3) * 354
        d.text((x, y), f"{item['id']} / {item['name']}", font=heading, fill='#EAF0F3')
        for t, theme in enumerate(('Dark', 'Light')):
            sy = y + 35 + 144 * t
            d.rounded_rectangle((x, sy, x + 484, sy + 132), radius=10, fill='#10181D' if t == 0 else '#EEF2F4')
            d.text((x + 12, sy + 10), theme + '  /  32 · 16', font=label, fill='#8BA0AE' if t == 0 else '#586B77')
            for size, px in ((32, 16), (16, 72)):
                bitmap = Image.open(BytesIO(raster(optical(item['slug']), size))).convert('RGBA')
                canvas.paste(bitmap, (x + px, sy + 76 - size // 2), bitmap)
            for size, px in ((32, 164), (16, 362)):
                bitmap = Image.open(BytesIO(raster(optical(item['slug']), size))).convert('RGBA')
                bitmap = bitmap.resize((size * 4, size * 4), Image.Resampling.NEAREST)
                canvas.paste(bitmap, (x + px, sy + 67 - size * 2), bitmap)
    (HERE / 'review').mkdir(exist_ok=True)
    canvas.save(HERE / 'review' / 'small-size-review.png')


def main():
    (HERE / 'png').mkdir(exist_ok=True)
    for item in ITEMS:
        for source in dict.fromkeys((item['slug'] + '.svg', optical(item['slug']))):
            (HERE / 'png' / source.replace('.svg', '.png')).write_bytes(raster(source, 1024))
    build_html()
    build_svg_sheet()
    small_review()
    print('Exported 8 transparent 1024px PNGs; HTML; 3200 × 2000 librsvg sheet; 1× review.')


if __name__ == '__main__':
    main()
