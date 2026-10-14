# StitchForge v0.2 — embroidery app (review build)

Vector art and any-font text in; machine-ready DST, a printable operator
spec sheet, and a stitch preview out. Written in Rust, runs on Windows,
macOS and Linux from the same code.

Built clean-room from the StitchForge spec, public format documentation
and general embroidery practice. No Wilcom or Pulse code, files, fonts or
screens were used or examined.

## Open the app

**Mac:** double-click `Launch StitchForge.command`. The first time, it installs
what it needs and builds (a few minutes), then opens the app and leaves a
`StitchForge.app` in this folder. After that, just double-click
`StitchForge.app` (drag it to the Dock if you like).

If macOS says the launcher "can't be opened", right-click it → Open → Open.

**Windows / Linux:** install Rust from https://rustup.rs, then in this folder
run `cargo run --release`.

### In the app

- **Open SVG…**, drag an SVG onto the window, **Text…** (any installed font),
  or **Sample logo**.
- Right panel: width, **Flat goods / Cap (center-out)**, fabric, production
  numbers, color stops, warnings, job fields for the operator sheet.
- **Pro** (top right) adds stitch settings: density, stitch length, pull
  compensation, underlay, thread chart CSV, and jump/travel display.
- Left panel: sewing sequence grouped by needle; click an object to highlight it.
- Bottom: **Play sew-out** steps through the stitches in sewing order
  (Space = play/pause, F = fit, scroll/pinch = zoom, drag = pan,
  double-click = fit).
- **Export DST + sheet…** writes the DST, operator sheet, stops CSV and
  preview into a folder you choose, and opens the sheet.

## Command line

```
# Vector art to stitches, 3.5" wide, flat and cap versions
stitchforge convert logo.svg --width-in 3.5 --profile both \
  --job 10482 --customer "Hillside Rec League" --qty 48 \
  --machine "Head 2" --placement "Left chest" --notes "Match navy"

# Any installed font, no embroidery-font format
stitchforge text "Sunny Hill" --font Poppins --height-mm 20 --color "#1b2d5c"
stitchforge text "Team Name" --font-file MyFont.otf --profile cap

stitchforge fonts              # list installed fonts
stitchforge inspect out/x.dst  # read back a DST
```

Each design writes to `out/`:

| File | What it is |
| --- | --- |
| `name.dst` | Tajima DST for the machine |
| `name.sheet.html` | Operator spec sheet: open, press Print (or save as PDF) |
| `name.stops.csv` | Colour stops: needle, thread code, stitches |
| `name.preview.png` / `.svg` | Thread-style render of the stitches |
| `name.job.json` | Full summary for scripts and AI agents |

Add `--json` to print the summary instead of text, so an agent can drive it.

Options: `--profile flat|cap|both`, `--fabric pique|tshirt|fleece|twill|cap|towel`,
`--width-mm/--height-mm/--width-in/--height-in`, `--threads chart.csv`
(columns `code,name,r,g,b`; file name becomes the brand).

## What the engine does

- **Vector to stitch:** SVG from Illustrator, Inkscape, CorelDRAW, Affinity.
  Curves, transforms, groups, both fill rules, gradients (first colour), text.
- **Stitch type by width:** narrow shapes become satin, wide become tatami
  fill, strokes become satin columns, triple runs or runs.
- **Lettering:** each letter is its own object; satin angle chosen per
  letter, and crossbars and arch tops are re-sewn across the stroke.
- **Underlay, pull compensation, split satins, tie-ins and tie-offs.**
- **Knockout:** lower layers skip what upper layers cover, with a 0.5 mm
  underlap, so no double-stitching.
- **Hidden travel:** the needle travels under coming stitches or upper
  layers instead of trimming, where it won't show.
- **Garment profiles:** Flat keeps layering and merges colours. Cap sews
  from bottom-centre outward, fills bottom to top, double underlay,
  cap-frame size check.
- **Thread matching** by perceptual colour distance, with loose-match warnings.
- **Warnings:** short stitches, frame limits, tiny shapes, narrow columns.

## Verified

- 9 unit tests (geometry, knockout, islands, resampling, DST encoding of
  every possible move).
- DST output cross-checked with the independent open-source reader
  pyembroidery: stitch, colour-change and trim counts match exactly.

## Not yet (next steps)

- Stitch on real fabric and tune recipes — that is Gate 1 in the spec.
- Satin that turns smoothly along curves (now: up to two angles per letter).
- PES/EXP/JEF and other formats; editable native file; raster tracing.
- Desktop UI (Tauri), MCP server for AI agents, job/production database.
- Brand thread charts (load your shop's CSV for now).
