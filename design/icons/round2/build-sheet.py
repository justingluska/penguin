"""Build the local comparison HTML and optically adjusted SVGs. No dependencies."""
from pathlib import Path

HERE = Path(__file__).resolve().parent
CONCEPTS = [
    (1, 'tuxedo-split', 'Tuxedo split', 'A curved back. A belly folded into a V.', '2 shapes / 2 colors'),
    (2, 'flap-dot', 'Flap & dot', 'An envelope V and one warm beak-dot.', '3 shapes / 3 colors'),
    (3, 'letter-gap', 'Letter gap', 'Two letters leave a penguin in the gap.', '3 shapes / 2 colors'),
    (4, 'ice-floe', 'Ice floe', 'A paper floe. A tiny notch of penguin.', '2 shapes / 2 colors'),
    (5, 'postmark-p', 'Postmark P', 'A geometric P with a flap for its counter.', '2 shapes / 2 colors'),
    (6, 'cut-pebble', 'Cut pebble', 'A tilted pebble, opened with one V cut.', '3 shapes / 2 colors'),
    (7, 'stacked-letters', 'Stacked letters', 'Two letters meet in a tuxedo silhouette.', '3 shapes / 3 colors'),
    (8, 'arctic-v', 'Arctic V', 'One white V, suspended over arctic blue.', '2 shapes / 1 gradient'),
]

# Deliberate optical edits, all in the same 1024-unit coordinate space.
OPTICAL = {
    2: [('r="34"', 'r="48"'), ('cy="400"', 'cy="384"'),
        ('M270 366L512 548L754 366V480L512 662L270 480Z',
         'M256 352L512 544L768 352V480L512 672L256 480Z')],
    3: [('M228 354L370 278L500 336C454 346 438 374 438 404C438 434 452 452 464 464C418 510 392 570 392 630C392 698 435 730 500 738V758H228Z',
         'M224 352L368 272L496 328C448 336 432 368 432 400C432 432 448 452 456 464C408 512 384 568 384 624C384 696 432 736 496 744V768H224Z'),
        ('M796 354L654 278L524 336C570 346 586 374 586 396L622 410L580 428C574 445 566 456 560 464C606 510 632 570 632 630C632 698 589 730 524 738V758H796Z',
         'M800 352L656 272L528 328C576 336 592 368 592 392L640 412L584 440L568 464C616 512 640 568 640 624C640 696 592 736 528 744V768H800Z')],
    4: [('L466 434V480C466 519 558 519 558 480V408',
         'L448 439V488C448 540 576 540 576 488V403')],
    5: [('M460 378H638L549 518Z', 'M448 368H656L552 536Z')],
    6: [('stroke-width="64"', 'stroke-width="88"')],
    7: [('M336 272H688V336L512 476L336 336Z',
         'M320 256H704V336L512 480L320 336Z'),
        ('M288 416L512 600L736 416V696Q736 752 680 752H344Q288 752 288 696Z',
         'M272 432L512 624L752 432V704Q752 768 688 768H336Q272 768 272 704Z')],
    8: [('M262 330H378L512 544L646 330H762L512 730Z',
         'M248 320H384L512 528L640 320H776L512 752Z')],
}

def master_path(n, slug):
    return f'icon-{n}-{slug}.svg'

def optical_path(n, slug):
    return f'icon-{n}-small.svg' if n in OPTICAL else master_path(n, slug)

def build():
    for n, slug, name, description, spec in CONCEPTS:
        if n not in OPTICAL:
            continue
        source = (HERE / master_path(n, slug)).read_text()
        for before, after in OPTICAL[n]:
            if before not in source:
                raise ValueError(f'Optical source missing in icon {n}: {before}')
            source = source.replace(before, after)
        source = source.replace('</title>', ' — optical 16–32px</title>')
        source = source.replace('  <desc', '  <!-- Enlarged counters / gaps for small display sizes. -->\n  <desc')
        (HERE / optical_path(n, slug)).write_text(source)

    cards, dock = [], []
    for n, slug, name, description, spec in CONCEPTS:
        source = master_path(n, slug)
        small = optical_path(n, slug)
        micros = ''.join(f'''<div class="micro {theme}"><span class="theme-label">{theme.title()}</span>
            <div class="samples"><div><img src="{small}" width="32" height="32" alt="{name}, 32 pixels"><small>32</small></div>
            <div><img src="{small}" width="16" height="16" alt="{name}, 16 pixels"><small>16</small></div></div></div>'''
            for theme in ['dark', 'light'])
        badge = '<span class="pick">Pick</span>' if n in [2, 6] else ''
        cards.append(f'''<article class="concept"><header class="card-heading"><span class="number">0{n}</span><h2><a href="{source}">{name}</a></h2>{badge}</header>
          <div class="study"><a href="{source}" class="hero" aria-label="Open {name} SVG"><img src="{source}" width="256" height="256" alt="{name}"></a>
          <div class="size-checks">{micros}</div></div>
          <p class="description">{description}</p>
          <footer class="card-footer"><span>{spec}</span><a href="png/icon-{n}.png">512 PNG <span aria-hidden="true">↗</span></a></footer></article>''')
        dock.append(f'<a class="dock-item" href="{source}" aria-label="{name}"><img src="{source}" width="64" height="64" alt=""><span>0{n}</span></a>')

    css = '''
    :root{color-scheme:dark;--bg:#101113;--panel:#191B1E;--line:#2A2D31;--text:#EDEEF0;--muted:#9FA3A9;--dim:#777E87}
    *{box-sizing:border-box}html,body{margin:0;background:var(--bg);color:var(--text)}body{font:12px/1.45 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;-webkit-font-smoothing:antialiased}a{color:inherit;text-decoration:none}a:focus-visible{outline:2px solid #91C9EE;outline-offset:4px}img{display:block;flex-shrink:0}
    main{width:1600px;height:1000px;padding:24px 28px 18px;margin:0 auto}.masthead{height:70px;display:flex;justify-content:space-between;align-items:flex-start}.kicker{font-size:10px;letter-spacing:1.8px;color:#97ADBD;margin:0 0 4px}.masthead h1{font-size:27px;font-weight:500;letter-spacing:-.9px;margin:0}.masthead h1 span{color:#747A83;font-weight:400}.masthead aside{text-align:right;padding-top:5px}.masthead aside p{margin:0 0 4px;color:#C0C4CB;font-size:12px}.masthead aside span{color:var(--dim);font-size:11px}
    .grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:14px}.concept{height:364px;border:1px solid var(--line);border-radius:16px;overflow:hidden;background:var(--panel)}.card-heading{height:41px;padding:0 17px;display:flex;align-items:center;gap:10px}.number{color:var(--dim);font:10px ui-monospace,SFMono-Regular,Menlo,monospace}h2{font-size:15px;font-weight:500;letter-spacing:-.25px;margin:0}.pick{margin-left:auto;padding:2px 7px;border:1px solid #435967;border-radius:5px;color:#BAD2E2;font-size:9px}.study{height:256px;display:flex;align-items:center;padding:0 9px 0 0;gap:6px}.hero{width:256px;height:256px;flex-shrink:0}.size-checks{flex:1;min-width:0;display:grid;gap:9px}.micro{height:95px;border-radius:9px;padding:9px 8px 5px;border:1px solid #303339}.micro.dark{background:#111214}.micro.light{background:#F3F3F5;border-color:#E0E1E4;color:#363B42}.theme-label{font-size:9px;color:#969CA5}.light .theme-label{color:#6A7079}.samples{display:flex;align-items:flex-end;justify-content:space-between;gap:5px;height:55px}.samples>div{display:flex;flex-direction:column;align-items:center;justify-content:flex-end;gap:3px}.samples small{font:9px ui-monospace,SFMono-Regular,Menlo,monospace;color:#818892}.light small{color:#757C86}.description{height:31px;padding:0 17px;margin:0;display:flex;align-items:center;font-size:11px;color:#B0B5BC;white-space:nowrap}.card-footer{height:34px;padding:0 17px;border-top:1px solid #25282C;display:flex;justify-content:space-between;align-items:center;font-size:9px;color:#7F8893}.card-footer a{color:#ABB7C3}
    .dock-section{height:102px;margin-top:14px;border:1px solid #37424B;border-radius:16px;background:#263540;display:flex;align-items:center;justify-content:space-between;padding:0 22px}.dock-copy h2{font-size:15px;margin-bottom:3px}.dock-copy p{font-size:10px;color:#AFBFCB;margin:0}.dock{height:86px;display:flex;gap:14px;align-items:center;padding:4px 16px 1px;background:#CADCE316;border:1px solid #D1E0ED35;border-radius:20px}.dock-item{display:flex;flex-direction:column;align-items:center;gap:0}.dock-item span{font:9px/13px ui-monospace,SFMono-Regular,Menlo,monospace;color:#AFC0CE}.dock-side{width:265px;font-size:10px;color:#AFC0CD;text-align:right}.palette{display:flex;justify-content:flex-end;gap:6px;margin-bottom:7px}.palette i{width:12px;height:12px;border:1px solid #ffffff20;border-radius:50%}.bottom{height:30px;display:flex;justify-content:space-between;align-items:center;font-size:10px;color:#7E8893}.bottom strong{color:#B8C5D0;font-weight:400}
    @media(max-width:900px){main{margin:0}body{overflow-x:auto}}
    '''
    html = f'''<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Penguin / Round 02 — Abstract app icons</title><style>{css}</style></head>
<body><main><header class="masthead"><div><p class="kicker">PENGUIN / ROUND 02</p><h1>Less detail. <span>More signal.</span></h1></div><aside><p>Eight abstract studies for a quieter inbox.</p><span>1024 SVG masters · 100px margin · 2–3 shapes · 256 / 64 / 32 / 16px</span></aside></header>
<section class="grid" aria-label="Eight abstract app icon concepts">{''.join(cards)}</section>
<section class="dock-section" aria-label="64 pixel Dock comparison"><div class="dock-copy"><h2>Small by design.</h2><p>64px in the Dock · consistent canvas margins</p></div><div class="dock">{''.join(dock)}</div><div class="dock-side"><div class="palette"><i style="background:#182B3A"></i><i style="background:#FFFFFF"></i><i style="background:#C4DAE8"></i><i style="background:#F4AA64"></i></div>Ink + white + ice or orange<br>One accent per concept</div></section>
<footer class="bottom"><span><strong>Shortlist: 06 Cut pebble + 02 Flap &amp; dot</strong> · penguin character, reduced to geometry.</span><span>32 / 16 use optical variants where needed · select a title for the SVG</span></footer></main></body></html>'''
    (HERE / 'index.html').write_text(html)
    print('Wrote comparison HTML and seven optical variants.')

if __name__ == '__main__':
    build()
