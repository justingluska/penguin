"""Make an explicitly labeled vector proof when browser export is unavailable.

This does not render HTML and must not be described as a Chrome screenshot.
All icon pixels are rendered directly from the delivered, hand-drawn SVGs.
"""
import argparse
from html import escape
from pathlib import Path
import re
import subprocess

HERE = Path(__file__).resolve().parent
parser = argparse.ArgumentParser()
parser.add_argument('--output', default='icons-sheet.png')
args = parser.parse_args()
target = HERE / args.output
if target.parent != HERE or target.suffix != '.png':
    parser.error('Output must be a PNG filename directly under design/icons/.')

STEMS = ['belly-envelope', 'peekaboo', 'flap-face', 'folded-penguin', 'letter-slide', 'glyph']
NAMES = ['Tuxedo post', 'Special delivery', 'Happy mail', 'Paper bird', 'First-class slide', 'Quiet post']
LINES = [
    ['A folded white belly turns', 'the tuxedo into an envelope.'],
    ['A curious little penguin peeks', 'over the rim of an open letter.'],
    ['The envelope flap becomes', 'a bright-eyed penguin face.'],
    ['A paper-fold silhouette finds a', 'penguin in the negative space.'],
    ['A round little courier catches', 'a ride on a tilted letter.'],
    ['One-color penguin and envelope,', 'distilled for the menu bar.'],
]
NOTES = ['The everyday companion', 'The warm welcome', 'The mail-first mark', 'The geometric direction', 'The playful direction', 'The system companion']
parts = ['''<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="1000" viewBox="0 0 1600 1000">
<defs><linearGradient id="dock-bg" x2="1" y2="1"><stop stop-color="#223142"/><stop offset="1" stop-color="#58748b"/></linearGradient></defs>
<rect width="1600" height="1000" fill="#090b0e"/>
<g font-family="Helvetica,Arial,sans-serif">''']

def rect(x, y, w, h, radius, fill, stroke='none'):
    parts.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{fill}" stroke="{stroke}"/>')

def label(x, y, value, size=12, color='#a1a4a5', anchor='start', weight=400):
    parts.append(f'<text x="{x}" y="{y}" font-size="{size}" fill="{color}" text-anchor="{anchor}" font-weight="{weight}">{escape(value)}</text>')

sequence = 0
def icon(n, x, y, size, small=False, color='#172332'):
    global sequence
    sequence += 1
    filename = f'icon-{n}-small.svg' if small else ('icon-6-app.svg' if n == 6 else f'icon-{n}-{STEMS[n-1]}.svg')
    source = (HERE / filename).read_text()
    source = re.sub(r'<(?:title|desc)[^>]*>.*?</(?:title|desc)>', '', source)
    source = re.sub(r' aria-labelledby="[^"]*"', '', source)
    source = source.replace('width="1024" height="1024"', f'x="{x}" y="{y}" width="{size}" height="{size}" color="{color}"', 1)
    prefix = f'p{sequence}-'
    source = re.sub(r'id="([^"]+)"', lambda m: f'id="{prefix}{m[1]}"', source)
    source = re.sub(r'url\(#([^)]*)\)', lambda m: f'url(#{prefix}{m[1]})', source)
    parts.append(source)

label(32, 51, 'Penguin / App icon studies', 27, '#f0f0f0', weight=500)
label(32, 75, 'Fast mail. A little personality. Six ways to bring the penguin and the envelope together.', 12)
label(1568, 43, 'A QUIETER KIND OF EMAIL', 10, '#a9b5c0', 'end')
label(1568, 66, 'Hand-drawn vectors · 1024 × 1024 masters · 824 px app tiles', 11, '#7e8993', 'end')
for n in range(1, 7):
    x = 32 + ((n - 1) % 3) * (1552 / 3)
    y = 100 + ((n - 1) // 3) * 367
    width = 1504 / 3
    rect(x, y, width, 351, 18, '#111419', '#262c33')
    label(x + 19, y + 31, f'0{n}', 11, '#788897')
    label(x + 49, y + 32, NAMES[n-1], 16, '#f0f0f0', weight=500)
    if n in (1, 6):
        rect(x + width - 115, y + 16, 97, 22, 6, '#16222c', '#435768')
        label(x + width - 66, y + 30, 'Recommended' if n == 1 else 'Glyph + app tile', 9, '#c3d9e9', 'middle')
    icon(n, x + 8, y + 52, 256)
    dx = x + 271
    for i, line in enumerate(LINES[n-1]):
        label(dx, y + 81 + i * 18, line, 12, '#b5bdc5')
    for j, mode in enumerate(['Dark', 'Light']):
        my = y + 138 + j * 75
        light = mode == 'Light'
        rect(dx, my, width - 287, 67, 9, '#f5f7f9' if light else '#050709', '#e0e6ec' if light else '#262d36')
        label(dx + 13, my + 35, mode, 10, '#656e76' if light else '#969fa8')
        tint = '#172332' if light else '#f0f4f8'
        icon(n, dx + 75, my + 11, 32, True, tint)
        icon(n, dx + 139, my + 27, 16, True, tint)
        label(dx + 91, my + 57, '32', 9, '#7a8690', 'middle')
        label(dx + 147, my + 57, '16', 9, '#7a8690', 'middle')
    parts.append(f'<path d="M{x} {y+309}H{x+width}" stroke="#1e252d"/>')
    label(x + 19, y + 334, NOTES[n-1], 10, '#87939e')
    label(x + width - 19, y + 334, '256 px / template glyph' if n == 6 else '256 px / optical sizes', 9, '#6f7c88', 'end')

rect(32, 835, 1536, 130, 18, 'url(#dock-bg)', '#33414e')
label(60, 891, 'At home in the Dock.', 15, '#f0f0f0', weight=500)
label(60, 913, 'All six at 64 px, with the same canvas margin', 11, '#c0ccd6')
label(60, 930, 'and a soft, continuous-corner tile.', 11, '#c0ccd6')
rect(445, 848, 745, 104, 23, '#ffffff19', '#e2f1ff50')
rect(464, 877, 48, 48, 12, '#8c9fae', '#c5d3de')
parts.append('<path d="M479 889v11m17-11v11m-16 8q8 7 16 0" stroke="#385066" stroke-width="2.5" fill="none" stroke-linecap="round"/>')
parts.append('<path d="M533 878v52m574-52v52" stroke="#e7f3fc50"/>')
for n in range(1, 7):
    ix = 557 + (n - 1) * 87
    icon(n, ix, 860, 64)
    label(ix + 32, 940, f'0{n}', 9, '#d6e0e8', 'middle')
rect(1124, 877, 48, 48, 12, '#b8c8d4', '#d4e1eb')
parts.append('<path d="M1136 894h24m-24 9h24m-24 9h24" stroke="#8398aa" stroke-width="3"/>')
for i, fill in enumerate(['#142231', '#ffffff', '#f5ad4f', '#c8dae8']):
    parts.append(f'<circle cx="{1466+i*22}" cy="884" r="6.5" fill="{fill}" stroke="#ffffff40"/>')
label(1540, 913, 'Ink / white / warm amber / ice', 10, '#c7d4df', 'end')
label(1540, 930, '01–05 color · 06 currentColor glyph', 10, '#c7d4df', 'end')
label(34, 985, 'Recommended: 01 Tuxedo post — a natural bridge from the existing face to a distinctive mail app.', 10, '#aebcca')
label(1566, 985, 'Vector proof · 1600 × 1000 @2x · Chrome export blocked in this session', 10, '#74818e', 'end')
parts.append('</g></svg>')
proof = HERE / 'icons-proof.svg'
proof.write_text('\n'.join(parts))
subprocess.run(['rsvg-convert', '--width=3200', '--height=2000', '-o', str(target), str(proof)], check=True)
print(f'Vector proof: {target.name} (3200×2000). This is not an HTML screenshot.')
