"""Assemble a local comparison sheet from hand-drawn SVG masters; no dependencies."""
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent
STEMS = ["belly-envelope", "peekaboo", "flap-face", "folded-penguin", "letter-slide", "glyph"]
NAMES = ["Tuxedo post", "Special delivery", "Happy mail", "Paper bird", "First-class slide", "Quiet post"]
DESCRIPTIONS = [
    "A folded white belly turns the tuxedo into an envelope.",
    "A curious little penguin peeks over the rim of an open letter.",
    "The envelope flap becomes a bright-eyed penguin face.",
    "A paper-fold silhouette finds a penguin in the negative space.",
    "A round little courier catches a ride on a tilted letter.",
    "One-color penguin and envelope, distilled for the menu bar.",
]
NOTES = ["The everyday companion", "The warm welcome", "The mail-first mark", "The geometric direction", "The playful direction", "The system companion"]

# Optical variants retain the identity and tile margin, but trade depth and
# secondary folds for wider seams and larger eyes. Geometry is hand authored.
for n, stem in enumerate(STEMS[:5], 1):
    source = (HERE / f"icon-{n}-{stem}.svg").read_text()
    source = re.sub(r"  <defs>.*?</defs>\n", "", source, flags=re.S)
    source = re.sub(r' filter="url\(#[^)]*\)"', "", source)
    source = re.sub(r'  <path d="M316 102[^\n]+\n', "", source)
    for key, color in {"ice": "#dae6ef", "tile": "#182332", "ink": "#142231", "paper": "#ffffff"}.items():
        source = source.replace(f'url(#{key})', color)
    source = source.replace('r="21"', 'r="36"').replace('r="22"', 'r="36"').replace('r="23"', 'r="36"')
    # Wider, deeper beaks remain warm and visible at a true 16px raster.
    beaks = {
        1: ('M480 469Q512 458 544 469L518 503Q512 510 506 503Z', 'M462 474Q512 460 562 474L521 532Q512 544 503 532Z'),
        2: ('M481 441Q512 431 543 441L518 474Q512 482 506 474Z', 'M466 456Q512 442 558 456L520 510Q512 522 504 510Z'),
        3: ('M479 466Q512 455 545 466L518 501Q512 509 506 501Z', 'M462 474Q512 460 562 474L521 532Q512 544 503 532Z'),
        4: ('M479 455H545L512 496Z', 'M464 467H560L512 536Z'),
        5: ('M490 469Q522 458 554 469L528 503Q522 510 516 503Z', 'M472 477Q522 463 572 477L531 535Q522 547 513 535Z'),
    }
    source = source.replace(*beaks[n])
    if n == 1:
        source = re.sub(r'    <path d="M406 747[^\n]+\n', '', source)
        source = re.sub(r'    <path d="M371 697[^\n]+\n', '', source)
        source = source.replace('stroke="#8196a7" stroke-width="17"', 'stroke="#547185" stroke-width="32"')
    elif n == 2:
        source = re.sub(r'    <path d="M265 738[^\n]+\n', '', source)
        source = source.replace('stroke="#a6bece" stroke-width="10"', 'stroke="#6d8a9f" stroke-width="36"')
    elif n == 3:
        source = re.sub(r'    <path d="M250 367[^\n]+\n', '', source)
        source = source.replace('stroke="#aabcca" stroke-width="14"', 'stroke="#96aebf" stroke-width="28"')
    elif n == 4:
        source = re.sub(r'    <path d="M321 391[^\n]+\n', '', source)
        source = re.sub(r'    <path d="M703 391[^\n]+\n', '', source)
        source = source.replace('stroke="#263747" stroke-width="25"', 'stroke="#142231" stroke-width="36"')
    elif n == 5:
        source = re.sub(r'      <path d="M269 751[^\n]+\n', '', source)
        source = re.sub(r'      <path d="M401 558[^\n]+\n', '', source)
        source = source.replace('stroke="#8ca5b9" stroke-width="15"', 'stroke="#648296" stroke-width="32"')
    source = source.replace('</title>', ' — optical size</title>', 1)
    source = source.replace('  <!-- 824px', '  <!-- Flat optical variant for 16–32px. -->\n  <!-- 824px', 1)
    (HERE / f"icon-{n}-small.svg").write_text(source)

glyph = (HERE / "icon-6-glyph.svg").read_text()
glyph_body = re.sub(r'^.*?<!--', '<!--', glyph, count=1, flags=re.S).rsplit('</svg>', 1)[0]
small_glyph = glyph.replace('r="29"', 'r="35"').replace('M477 466H547L512 513Z', 'M468 467H556L512 523Z')
small_glyph = small_glyph.replace('</title>', ' — optical size</title>', 1)
(HERE / 'icon-6-small.svg').write_text(small_glyph)
tile_svg = '''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024" role="img" aria-labelledby="title desc">
  <title id="title">Penguin — Quiet post app icon</title>
  <desc id="desc">The one-color Penguin glyph in its optional macOS app tile.</desc>
  <defs>
    <linearGradient id="ice" x1="280" y1="100" x2="740" y2="924" gradientUnits="userSpaceOnUse"><stop stop-color="#f8fafc"/><stop offset="1" stop-color="#d7e2eb"/></linearGradient>
    <filter id="tile-shadow" x="0" y="0" width="1024" height="1024" filterUnits="userSpaceOnUse"><feDropShadow dx="0" dy="16" stdDeviation="15" flood-color="#030912" flood-opacity=".19"/></filter>
  </defs>
  <path d="M316 100H708C852 100 924 172 924 316V708C924 852 852 924 708 924H316C172 924 100 852 100 708V316C100 172 172 100 316 100Z" fill="url(#ice)" filter="url(#tile-shadow)"/>
  <path d="M316 102H708C850 102 922 174 922 316V708C922 850 850 922 708 922H316C174 922 102 850 102 708V316C102 174 174 102 316 102Z" fill="none" stroke="#fff" stroke-opacity=".65" stroke-width="2"/>
  <g color="#172332" transform="translate(143.36 143.36) scale(.72)">
''' + glyph_body + '  </g>\n</svg>\n'
(HERE / "icon-6-app.svg").write_text(tile_svg)

def icon(n, size, small=False):
    if n == 6 and small:
        # Inline SVG deliberately demonstrates actual currentColor inheritance.
        svg = small_glyph.replace('width="1024" height="1024"', f'width="{size}" height="{size}"')
        svg = re.sub(r'aria-labelledby="title desc"', 'aria-label="Quiet post glyph"', svg)
        svg = re.sub(r'  <(?:title|desc).*?</(?:title|desc)>\n', '', svg)
        return svg
    filename = f'icon-{n}-small.svg' if small else (f'icon-{n}-{STEMS[n-1]}.svg' if n < 6 else 'icon-6-app.svg')
    return f'<img src="{filename}" width="{size}" height="{size}" alt="{NAMES[n-1]}" />'

cards = []
for n in range(1, 7):
    tags = '<span class="tag">Recommended</span>' if n == 1 else ('<span class="tag neutral">Glyph + app tile</span>' if n == 6 else '')
    cards.append(f'''<article class="concept">
      <header class="card-heading"><div><span class="number">0{n}</span><a href="icon-{n}-{STEMS[n-1]}.svg">{NAMES[n-1]}</a></div>{tags}</header>
      <div class="study">
        <a class="hero" href="{'icon-6-app.svg' if n == 6 else f'icon-{n}-{STEMS[n-1]}.svg'}" aria-label="Open 1024 pixel app icon">{icon(n, 256)}</a>
        <div class="details"><p>{DESCRIPTIONS[n-1]}</p>
          <div class="micro dark"><span>Dark</span><div>{icon(n,32,True)}<small>32</small></div><div>{icon(n,16,True)}<small>16</small></div></div>
          <div class="micro light"><span>Light</span><div>{icon(n,32,True)}<small>32</small></div><div>{icon(n,16,True)}<small>16</small></div></div>
        </div>
      </div>
      <footer class="card-footer"><span>{NOTES[n-1]}</span><span>256 px / {'template glyph' if n == 6 else 'optical sizes'}</span></footer>
    </article>''')

dock = ''.join(f'<div class="dock-item">{icon(n,64)}<span>0{n}</span></div>' for n in range(1,7))
html = '''<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Penguin — App icon studies</title>
<style>
:root{color-scheme:dark;--bg:#090b0e;--panel:#111419;--border:#262c33;--text:#f0f0f0;--muted:#a1a4a5;--dim:#7e8993}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:13px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;-webkit-font-smoothing:antialiased}a{color:inherit;text-decoration:none}a:focus-visible{outline:2px solid #70b8ff;outline-offset:4px;border-radius:6px}img,svg{display:block;flex-shrink:0}main{width:1600px;min-height:1000px;padding:27px 32px 10px;margin:auto}.masthead{height:73px;display:flex;align-items:flex-start;justify-content:space-between}.masthead h1{font-size:27px;font-weight:500;letter-spacing:-.9px;margin:0 0 3px}.masthead p{margin:0;color:var(--muted);font-size:12px}.masthead aside{text-align:right;padding-top:5px}.eyebrow{font-size:10px;letter-spacing:1.7px;text-transform:uppercase;color:#a9b5c0;margin-bottom:7px}.spec{color:var(--dim);font-size:11px}.grid{display:grid;grid-template-columns:repeat(3,1fr);gap:16px}.concept{height:351px;border:1px solid var(--border);border-radius:18px;background:var(--panel);overflow:hidden}.card-heading{height:52px;display:flex;align-items:center;justify-content:space-between;padding:0 19px}.card-heading>div{display:flex;align-items:center;gap:11px}.card-heading a{font-size:16px;letter-spacing:-.3px;font-weight:500}.number{font:11px ui-monospace,SFMono-Regular,Menlo,monospace;color:#788897}.tag{font-size:9px;border:1px solid #435768;border-radius:6px;color:#c3d9e9;padding:3px 7px;white-space:nowrap}.tag.neutral{border-color:#343c45;color:#a6b0ba}.study{display:grid;grid-template-columns:256px 1fr;gap:7px;padding:0 16px 0 8px;height:256px}.hero{width:256px;height:256px}.details{padding:13px 0 0 0}.details p{height:65px;margin:0 0 8px;color:#b5bdc5;font-size:12px;line-height:1.5;max-width:181px}.micro{height:67px;border:1px solid #262d36;border-radius:9px;margin-bottom:8px;display:flex;align-items:center;justify-content:space-between;padding:7px 13px;gap:7px}.micro>span{font-size:10px;width:34px}.micro>div{height:50px;width:37px;display:flex;flex-direction:column;align-items:center;justify-content:flex-end;gap:0}.micro small{font:9px/15px ui-monospace,SFMono-Regular,Menlo,monospace;opacity:.6}.dark{background:#050709;color:#f0f4f8}.dark>span{color:#969fa8}.light{background:#f5f7f9;color:#172332;border-color:#e0e6ec}.light>span{color:#656e76}.card-footer{height:42px;padding:0 19px;display:flex;align-items:center;justify-content:space-between;color:#87939e;font-size:10px;border-top:1px solid #1e252d}.card-footer span:last-child{font-size:9px;color:#6f7c88}.dock-section{height:130px;margin-top:17px;border:1px solid #33414e;border-radius:18px;background:radial-gradient(ellipse at 75% 140%,#617d92 0,transparent 65%),linear-gradient(125deg,#223142,#3a5267);display:flex;align-items:center;justify-content:space-between;padding:0 28px;overflow:hidden}.dock-copy{width:300px}.dock-copy h2{font-size:15px;letter-spacing:-.2px;font-weight:500;margin:0 0 5px}.dock-copy p{font-size:11px;color:#c0ccd6;line-height:1.55;margin:0;max-width:260px}.dock{height:104px;display:flex;align-items:center;gap:16px;padding:10px 20px 5px;background:#dce9f320;border:1px solid #e2f1ff3d;border-radius:23px;box-shadow:0 13px 24px #06152626,inset 0 1px #f3f8ff1a}.dock-item{height:89px;display:flex;align-items:center;flex-direction:column}.dock-item span{font:9px/18px ui-monospace,SFMono-Regular,Menlo,monospace;color:#d6e0e8}.dock-rule{height:52px;width:1px;background:#e7f3fc30;margin:0 0 16px}.neighbor{height:48px;width:48px;margin-bottom:19px;border-radius:12px;background:linear-gradient(150deg,#adbfcf,#708699);border:1px solid #d4e1eb44;position:relative;opacity:.75}.neighbor.finder:before{content:"";position:absolute;inset:13px 11px;border-left:3px solid #364e63;border-right:3px solid #364e63}.neighbor.finder:after{content:"";position:absolute;width:22px;height:10px;border-bottom:2px solid #364e63;border-radius:0 0 50% 50%;bottom:9px;left:12px}.neighbor.stack{background:linear-gradient(#a9bac9 0 22%,#dbe3e9 22%)}.neighbor.stack:before{content:"";position:absolute;left:11px;right:11px;top:23px;height:3px;background:#8398aa;box-shadow:0 8px #8398aa}.palette{display:flex;gap:7px;margin:0 0 8px;justify-content:flex-end}.palette i{height:13px;width:13px;border:1px solid #ffffff25;border-radius:50%}.dock-spec{width:210px;text-align:right;font-size:10px;color:#c7d4df}.bottom{display:flex;justify-content:space-between;padding:10px 2px 0;color:#74818e;font-size:10px}.bottom strong{color:#aebcca;font-weight:400}@media(max-width:1100px){main{width:100%;padding:24px}.grid{grid-template-columns:repeat(2,minmax(450px,1fr))}.dock-section{gap:24px;overflow-x:auto}.dock-spec{display:none}.masthead{min-width:900px}.grid,.dock-section,.bottom{min-width:920px}}
</style></head><body><main>
<header class="masthead"><div><h1>Penguin <span style="color:#697681;font-weight:400">/</span> App icon studies</h1><p>Fast mail. A little personality. Six ways to bring the penguin and the envelope together.</p></div><aside><div class="eyebrow">A quieter kind of email</div><div class="spec">Hand-drawn vectors · 1024 × 1024 masters · 824 px app tiles</div></aside></header>
<section class="grid" aria-label="Six app icon concepts">''' + '\n'.join(cards) + '''</section>
<section class="dock-section" aria-label="macOS dock comparison"><div class="dock-copy"><h2>At home in the Dock.</h2><p>All six at 64 px, with the same canvas margin and a soft, continuous-corner tile.</p></div><div class="dock"><span class="neighbor finder" aria-hidden="true"></span><span class="dock-rule"></span>''' + dock + '''<span class="dock-rule"></span><span class="neighbor stack" aria-hidden="true"></span></div><div class="dock-spec"><div class="palette"><i style="background:#142231"></i><i style="background:#fff"></i><i style="background:#f5ad4f"></i><i style="background:#c8dae8"></i></div>Ink / white / warm amber / ice<br>01–05 color · 06 currentColor glyph</div></section>
<footer class="bottom"><span><strong>Recommended: 01 Tuxedo post</strong> — the closest bridge from the existing face to a distinctive mail app.</span><span>32 / 16 px use optical variants · 06 shows the unboxed glyph · click a name for its SVG</span></footer>
</main></body></html>
'''
(HERE / 'index.html').write_text(html)
print('Wrote six optical variants, the glyph app tile, and index.html.')
