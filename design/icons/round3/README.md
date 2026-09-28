# Penguin · Round 03

Four orientations of the round-2 Tuxedo split, a restrained Ice floe polish,
and Cut pebble carried forward unchanged.

Open [the HTML comparison](index.html) or [the exported sheet](icons-round3-sheet.png).
Both show all six options at **200px, 64px, 32px, and 16px on dark and light**.
The PNG sheet is **3200 × 2000 pixels**, equivalent to 1600 × 1000 at 2×.

## Recommended Tuxedo: A / Forward fold

**A** best balances the two readings: its white belly remains an envelope V,
while the gentle counterclockwise tilt gives the ink region a forward point
and a curved body boundary. It also stays closest to the round-2 favorite.
**B** pushes the direction further. **C** and **D** make the sideways beak
more explicit, but the envelope needs more interpretation.

| Option | Orientation / refinement | Master | Transparent PNG |
| --- | --- | --- | --- |
| A · Forward fold | −15° split; base eased into the lower edge | [SVG](tuxedo-split-a.svg) | [1024px](png/tuxedo-split-a.png) |
| B · Lifted beak | −30° split; lower curve reshaped to avoid a thin corner wedge | [SVG](tuxedo-split-b.svg) | [1024px](png/tuxedo-split-b.png) |
| C · Quarter-turn | +90° clockwise; the ink V points left | [SVG](tuxedo-split-c.svg) | [1024px](png/tuxedo-split-c.png) |
| D · Mirror turn | Horizontal mirror of C; the ink V points right | [SVG](tuxedo-split-d.svg) | [1024px](png/tuxedo-split-d.png) |
| 04 · Ice floe | White shape raised 20px; original geometry preserved | [SVG](ice-floe.svg) | [1024px](png/ice-floe.png) |
| 06 · Cut pebble | Exact copy of the round-2 master | [SVG](cut-pebble.svg) | [1024px](png/cut-pebble.png) |

Angles refer to the **internal split**, around the 512,512 canvas center.
The squircle itself never rotates. A/B retain the rotated fold anchors while
their lower curves are optically adjusted; they are not rigid rotations of
the entire icon. C/D use the quarter-turn and its mirror directly.

## Small-size handling

- **Tuxedo A–D:** use the master at every size. The broad split stays open at
  16px; no separate small SVGs or duplicate PNGs are needed.
- **Ice floe:** use [ice-floe-small.svg](ice-floe-small.svg) at 16–32px
  ([PNG](png/ice-floe-small.png)). It retains round 2's enlarged 128px notch
  and moves the white shape up 24px. The master retains the 92px notch.
- **Cut pebble:** use [cut-pebble-small.svg](cut-pebble-small.svg) at 16–32px
  ([PNG](png/cut-pebble-small.png)). Like the master, it is a byte-for-byte
  copy of round 2, including the wider 88px cut rather than the master's 64px.

[The optical review](review/small-size-review.png) includes actual 1× 32px
and 16px rasterizations, plus 4× nearest-neighbor enlargements. Inspect that
image at 100% to assess low-density displays; the 2× comparison sheet is a
separate high-density proof. Both were visually inspected.

Ice floe's white mass was centered at approximately y=534 in the master and
y=538 in the small file. Raising it 20/24px brings both near y=514, just below
the tile's center. No points, notch widths, or proportions were otherwise changed.

## Asset specifications

- 1024 × 1024 SVG canvases, with the exact round-2 continuous-corner container.
- Container bounds remain x/y **100–924**: the same 824px tile and 100px margins.
- Tuxedo A–D and Ice floe: **2 painted paths / 2 colors**, including the tile.
  The Tuxedo clip references the existing tile; it adds no painted shape.
- Cut pebble retains **3 painted paths / 2 colors**, including its stroked cut.
- Only ink **#182B3A** and white **#FFFFFF**. No gradients, shadows, fonts,
  external resources, or raster imagery inside the icon SVGs.
- `png/` contains **eight 1024 × 1024 RGBA exports**: six masters and the two
  optical variants, all rasterized directly with `rsvg-convert` and preserving
  transparent canvas margins.

## Rendering and reproduction

The exported comparison is an **librsvg composition**, not a Chrome screenshot.
An isolated headless Chrome attempt exited with **SIGABRT (-6)** before writing
an image and returned no diagnostic text; see
[the capture status](review/chrome-render-status.json). The requested fallback
was used. Browser rendering of the HTML has not been visually verified.

The HTML and exported sheet use the same catalog, sizes, arrangement, and
background colors. The sheet's SVG embeds PNG samples rendered by librsvg at
their exact 2× display resolution. The icon masters remain fully vector.

Run from the repository root:

```sh
python3 design/icons/round3/build-sheet.py
```

Requires `rsvg-convert`, Python with Pillow, and macOS system fonts for the
optical review. It writes only under `round3/`, exports PNGs, rebuilds the
HTML and [sheet composition](icons-round3-sheet.svg), and renders the optical
review. It reads the hand-authored masters without overwriting them.

The [orientation study](review/orientation-study.png) and
[refinement proof](review/refinement-proof.png) record the visual passes;
use the root-level masters and final sheet for selection.

Final structural, export, and link checks are recorded in
[validation.json](review/validation.json). No git commands were run.
