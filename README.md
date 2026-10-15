# StitchForge v0.6 — embroidery app (Milestone 2 started: name drops)

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

**Files.** File menu or the top bar:
- **Open…** (Cmd/Ctrl+O) opens a StitchForge project (`.sfg`), SVG artwork,
  or a machine file (DST, EXP, PES). Dropping a file on the window works too
  (an SVG dropped onto an open design is added as new layers).
- **Save** (Cmd/Ctrl+S) / **Save As** (Shift+Cmd/Ctrl+S) keep your layers,
  edits, threads, settings and job info in a `.sfg` file. The file name and a
  • (unsaved changes) show in the window title. New, Open and quitting ask
  before discarding unsaved work.
- **Export…** (Cmd/Ctrl+E): pick DST, PES and/or EXP; the operator sheet,
  stops CSV and preview always come along.
- **Opened stitch files** (DST/EXP/PES) are read-only stitches: pick the
  thread for each stop, play the sew-out, print the sheet, or export to
  another format (e.g. DST → PES).
- **Calibration sampler** (File menu, or `stitchforge sampler`): the Gate 1
  test sew-out. Sew it on your usual fabric with normal backing; the notes on
  its operator sheet say what to check and measure.

**Name drops** (tab at the top). For team and staff orders on multi-head
machines: each drop file sews the logo on every head, then stops before each
name so the operator runs it on one head at a time.
1. In **Design**, add a text layer (Add text…) where the names go. Its font,
   letter height, color and stitching are used for every name.
2. Open **Name drops** and **Load Excel / CSV…** (try
   `examples/team-roster.xlsx`). Tick the name column(s) (e.g. First + Last),
   same line or separate lines, letter case, an optional quantity column.
3. Pick the **name layer** and the **logo layers** (sewn on all heads).
4. Set **heads** and **names per drop** (up to 2 × heads: the second half is a
   second pass on the same heads), logo in every drop, and an optional
   maximum name width (wider names shrink to fit).
5. Fix any name by typing over it in the list. The left panel shows the drop
   as layers: logo, then STOP + name + head for each name; click one to see it.
6. **Export…** writes one machine file per drop, an operator sheet per drop,
   and a printable **run sheet** (plus CSV) listing every stop, which head to
   turn on, the name, and its spreadsheet row.

Letter height means capital-letter height, so every name comes out the same
size whatever its letters.

**Stitch settings in your art app.** Put these in an element's id/name in
Inkscape or Illustrator and StitchForge follows them: `sf-satin`,
`sf-fill`, `sf-run`, `sf-triple`, `sf-angle-45` (any angle), `sf-no-underlay`,
`sf-keep-under`, `sf-width-3` (satin outline width, mm), `sf-miter`,
`sf-round-corner`, `sf-square-corner`, `sf-cap-flat`, `sf-cap-square`,
`sf-cap-round`, `sf-cap-point`.

**Layers (left).** Every shape, outline and text block is a layer. Layers sew
top to bottom.
- Click a layer (or click it on the canvas) to select it.
- ▸ expands a layer to show its pieces (letters, islands) with stitch type,
  angle and stitch count. **Collapse all / Expand all** at the top.
- 👁 hides a layer (hidden layers don't sew).
- **STOP** chip: red = the machine stops / changes color before this layer.
  Click to turn on or off; right-click to go back to automatic. A • means
  you set it.

**Canvas.** Drag a layer to move it (a fill and its outline move together).
Arrow keys nudge 0.5 mm (Shift = 5 mm). Delete removes, Cmd/Ctrl+D
duplicates, Cmd/Ctrl+Z undo, Shift+Cmd/Ctrl+Z redo, Esc deselects.
Drag empty space to pan, scroll or pinch to zoom, F to fit. The white arrow
on a selected layer shows its stitch direction.

**Selected layer (right).** Name; words, font and letter height for text
layers; **W and H** typed in inches (🔒 keeps proportions, 🔓 lets you
stretch); rotation; **thread**: search the chart, or **Custom thread** with
your own color (picker or hex), code and name; **stitch type** (auto, fill,
satin, run outline, triple-run outline); **angle**: type any number (or
0/45/90/135), Auto to let StitchForge choose; **stop before** (auto/on/off,
applies to the whole layer); **underlay** on/off; **under layers above**: Cut
out (skip what's covered) or Keep fill (sew the full shape underneath);
outlines: **satin width** (mm), **corners** (mitered, rounded, square
overlap) and **ends** (flat, square, round, point); filled shapes: **satin
widen** (± mm, for thin or heavy lettering); visible; sew earlier/later; duplicate; delete;
unlink from its partner layer. Number boxes commit on Enter or when you
click away.

**Design.** Overall width or height (typed), flat or cap (center-out), fabric, **Sew in my
layer order**, **Auto-sequence layers** (reorders once: fewer color
changes on flats, center-out on caps), **Automatic stops at color
changes** (turn off to place every stop yourself).

**Bottom bar.** Play the sew-out, **3D / Flat** thread view, show jumps,
fit.

**Export DST + sheet…** writes the DST, operator sheet, stops CSV and
preview into a folder you choose, and opens the sheet.

## Command line

```
# Vector art to stitches, 3.5" wide, flat and cap versions
stitchforge convert logo.svg --width-in 3.5 --profile both \
  --job 10482 --customer "Hillside Rec League" --qty 48 \
  --machine "Head 2" --placement "Left chest" --notes "Match navy"

# Several machine formats at once
stitchforge convert logo.svg --width-in 3.5 --formats dst,pes,exp

# Calibration sampler for a test sew-out
stitchforge sampler --formats dst,pes

# Name drops from a project with a text layer for the name
stitchforge drops team.sfg --data roster.xlsx --columns "First Name,Last Name" \
  --case upper --heads 6 --per-drop 12 --max-width-in 3.5 --formats dst,pes -o drops

# What's in any DST/EXP/PES
stitchforge inspect design.pes

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
