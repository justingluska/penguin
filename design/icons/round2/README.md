# Penguin · Round 02

Eight abstract app-icon studies, informed by the round-one sheet and the dark
inbox, arctic sidebar, and light lavender product screenshots. The emphasis is
bold geometry, generous negative space, and a calm silhouette at small sizes.

Open [index.html](index.html) for the comparison, or
[icons-round2-sheet.png](icons-round2-sheet.png) for the exported proof.

| # | Concept / SVG master | Idea |
| --- | --- | --- |
| 01 | [Tuxedo split](icon-1-tuxedo-split.svg) | One curved boundary; the white belly begins with an envelope V. |
| 02 | [Flap & dot](icon-2-flap-dot.svg) | A white flap and one orange beak-dot. |
| 03 | [Letter gap](icon-3-letter-gap.svg) | Two envelope folds leave the bird in their negative space. |
| 04 | [Ice floe](icon-4-ice-floe.svg) | A white paper floe and a small dark notch. |
| 05 | [Postmark P](icon-5-postmark-p.svg) | A geometric P with a flap-shaped counter. |
| 06 | [Cut pebble](icon-6-cut-pebble.svg) | A tilted egg silhouette split by an envelope V. |
| 07 | [Stacked letters](icon-7-stacked-letters.svg) | Two cards form a folded collar and an envelope belly. |
| 08 | [Arctic V](icon-8-arctic-v.svg) | A crisp white V over a single navy-to-ice gradient. |

## Recommendations

**06 · Cut pebble** is the strongest primary identity: the rounded, tilted form
retains penguin character, while the V supplies the mail cue. It works in two
colors and has a recognizable silhouette without a face.

**02 · Flap & dot** is the most economical alternative. The envelope reads
immediately, and the warm dot gives it a little personality. It is the stronger
choice if clarity at 16px takes priority over a distinctive body silhouette.

## Assets

- Eight handwritten 1024 × 1024 SVG masters. No fonts, visible text, raster
  content, filters, external resources, or generated-image assets in the icons.
- Each uses two or three painted shapes **including its container**, with at
  most three source colors. Only 08 has a gradient; it uses two color stops.
- Shared continuous-corner container: x/y 100–924, leaving the same 100px
  transparent margin as round one. No baked-in shadow or highlight rim.
- Ink `#182B3A`, white `#FFFFFF`, and optional ice `#C4DAE8` or orange `#F4AA64`.
  Orange and ice never appear together in an icon.
- Use masters at 64px and larger. Use `icon-2-small.svg` through
  `icon-8-small.svg` at 16–32px. These enlarge important gaps/counters and
  strengthen small marks. 01 uses its master unchanged at every size.
- `png/icon-1.png` through `png/icon-8.png`: transparent 512 × 512 PNGs,
  rasterized directly from the masters with `rsvg-convert`.
- The HTML shows each concept at 256px, a 64px Dock comparison, and 32px/16px
  samples on both dark and light surfaces. Names link to SVGs; PNG links open
  the individual exports. It uses local files and system fonts only.

## Rendering status

**The exported sheet is a librsvg/Pillow composite, not a Chrome screenshot.**
Headless Chrome was attempted with the requested 1600 × 1000 viewport and
device scale factor 2, but exited with SIGABRT (`-6`) before producing a PNG;
its diagnostic log was empty. The requested fallback was used. The delivered
sheet is exactly **3200 × 2000 pixels**, representing 1600 × 1000 at 2×, and its
footer identifies the renderer. Browser capture of the HTML remains unverified.

The Chrome helper starts an isolated process in the background, polls for a
complete nonempty PNG, and terminates only its own process group after the
write. Its temporary profile stays under this directory. Run from the repo root:

```sh
python3 design/icons/round2/build-sheet.py
python3 design/icons/round2/render-assets.py
```

To capture the HTML where Chrome can launch, preserving the fallback proof:

```sh
python3 design/icons/round2/render-chrome.py --output icons-round2-chrome.png
```

`build-sheet.py` assembles the HTML and the explicit optical adjustments from
the handwritten masters. `render-assets.py` exports the eight PNGs, composes
the fallback sheet, and produces the small-size review. These scripts write
only within `round2/`. The fallback compositor requires Pillow and macOS system
fonts; all icon rasterization uses librsvg.

## Two polish passes

All three full sheets and their actual-size optical reviews were inspected:

1. [Initial proof](review/initial.png): the split looked like a cat, the floe
   sat low, and the opposing cards resembled an hourglass.
2. [First polish](review/polish-1.png): opened the white split to the tile's
   right edge, raised and widened the floe, and rebuilt the letters into a
   folded collar and belly.
3. [Final proof](icons-round2-sheet.png): rounded the negative-space bird's
   body, gave its contour a single angular beak cue, and strengthened 02's
   orange dot for 16px. Updated the shortlist after comparing the Dock renders.

[Final optical review](review/small-size-review.png) shows actual 1× 16px and
32px rasterizations on dark/light surfaces alongside 4× nearest-neighbor
enlargements. Earlier optical reviews are retained in `review/`.

Validation passed for all 15 SVGs: XML, canvas dimensions, allowed elements,
shape/color limits, and gradient count. All eight PNGs are 512px RGBA with the
expected transparent margin; the sheet is 3200 × 2000. All local HTML links
resolve, with eight cards and all 48 requested size/background samples.
