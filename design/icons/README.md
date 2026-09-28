# Penguin app icon studies

Six hand-drawn vector directions informed by `../README.md`, the semantic colors
in `../penguin.css`, the dark inbox screenshot, and the light command-palette
screenshot. Recommended pairing: **01 for the app; 06 for the sidebar/menu bar**.

| Concept | Master | Idea |
| --- | --- | --- |
| 01 · Tuxedo post | `icon-1-belly-envelope.svg` | A penguin's white bib folds into an envelope. Closest to the existing identity. |
| 02 · Special delivery | `icon-2-peekaboo.svg` | A penguin peeks from an open envelope, flippers resting on its rim. |
| 03 · Happy mail | `icon-3-flap-face.svg` | The envelope flap itself becomes a penguin face, on an ink tile. |
| 04 · Paper bird | `icon-4-folded-penguin.svg` | A faceted paper silhouette with a continuous white counterform and envelope fold. |
| 05 · First-class slide | `icon-5-letter-slide.svg` | A round, waving penguin rides a tilted letter. |
| 06 · Quiet post | `icon-6-glyph.svg` | One-color penguin and envelope with genuine transparent cutouts. |

## Assets and sizing

- All icon SVGs have a 1024 × 1024 canvas and hand-authored geometry. No raster
  images, font dependencies, visible text, scripts, or external resources.
- App tiles occupy x/y 100–924: 824 × 824 with 100px canvas margins. Soft gradients,
  continuous corners, a fine rim and a restrained shadow supply the depth.
- Use the color masters at 64px and above. `icon-1-small.svg` through
  `icon-5-small.svg` are optical versions for 16–32px: larger eyes and beaks,
  stronger envelope seams, flat colors and fewer secondary folds.
- `icon-6-glyph.svg` and `icon-6-small.svg` are transparent `currentColor` assets.
  Inline them to inherit CSS `color`, or use their alpha as a native macOS template
  image. An external HTML `<img>` does not inherit its parent's `currentColor`.
- `icon-6-app.svg` supplies the optional macOS tile for the sixth direction;
  the glyph itself deliberately stays unboxed and single-color.
- Shared palette: deep ink, white, warm amber `#f5ad4f`, and pale ice.

## Comparison and export status

`index.html` is a self-contained local comparison page: 256px masters, a 64px Dock,
and 32/16px optical variants in both light and dark contexts. It needs no network
requests. The system font is used only in the comparison page and proof captions.

**`icons-sheet.png` is a vector-rendered proof, not a Chrome screenshot of the
HTML.** It measures 3200 × 2000 pixels (1600 × 1000 at 2×). Its labeled source is
`icons-proof.svg`. Headless Chrome exited with SIGABRT (exit -6) before producing
a PNG or diagnostic output. The connected Chrome browser separately rejected the
local file URL under its URL security policy. That restriction was not bypassed.
The HTML layout still needs a successful browser capture.

The supplied Chrome exporter uses a dedicated temporary profile under this
directory. It starts Chrome in the background, polls for a complete, nonempty PNG,
checks 3200 × 2000 dimensions, and terminates only its own process group. It
preserves the existing deliverable if Chrome fails. To reproduce on a machine
where headless Chrome can launch, from the repo root:

```sh
python3 design/icons/build-sheet.py
python3 design/icons/render-sheet.py
```

`build-sheet.py` assembles the HTML and derives the explicitly tuned optical
variants from the SVG masters; edit the masters and/or these optical adjustments
there. `render-vector-proof.py` separately builds the labeled non-browser proof:

```sh
python3 design/icons/render-vector-proof.py
```

## Visual review

Three complete vector proofs were rendered and inspected, with two refinement
passes after the first render:

1. `review/pass-1.png`: initial six directions. Identified a flat cutoff inside
   the open envelope, an overly angular origami face, and weak small-size details.
2. `review/pass-2.png`: extended the peeking body into the envelope, softened the
   paper bird and joined its face/belly counterform, and refined the belly folds.
3. `icons-sheet.png`: defined the open-envelope rim, enlarged the optical eyes and
   beaks, strengthened key seams, and tightened the sheet to the requested canvas.

`review/small-size-review.png` shows actual 1× 16px and 32px rasterizations next to
3× nearest-neighbor enlargements. Every delivered icon passed XML validation,
canvas checks, and a standalone 64px SVG render. Browser screenshot verification
remains outstanding; the proof review does not claim to replace that check.
