# StitchForge v0.10 — embroidery app

Vector art, PDFs, pictures (PNG/JPG, even mockup photos) and any-font text
in; machine-ready DST, a printable operator
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

**Windows:** double-click `Launch StitchForge (Windows).bat`. The first time
it installs Rust (no Visual Studio needed) and builds, which takes about 10–15
minutes on a typical PC; then it opens the app and leaves `StitchForge.exe` in
this folder for next time. If Windows SmartScreen warns about the .bat, click
**More info → Run anyway**.

**Linux:** install Rust from https://rustup.rs, then run `cargo run --release`.

### In the app

**Files.** File menu or the top bar:
- **Open…** (Cmd/Ctrl+O) opens a StitchForge project (`.sfg`), artwork (SVG,
  PDF, AI), a picture (PNG, JPG, BMP, GIF, WebP, TIFF) or a machine file (DST,
  EXP, PES). Dropping a file on the window works too (art or a picture
  dropped onto an open design is added as new layers).
- **Vector PDF / AI files** open straight into layers: fills, strokes, CMYK
  and spot colours (Pantone names become the layer names). A page-sized
  background is left out; live text, gradients and embedded pictures are
  listed under "Check before running". A PDF that only holds a picture goes
  to the trace window.

**Trace a picture** (open a PNG/JPG, or File → Trace a picture…). Instead of
tracing by hand, StitchForge builds the logo for you:
- finds the background (and on a **mockup photo**, the garment too) and crops
  to the logo; drag on the picture to set your own crop;
- picks the real thread colours from the flat areas, so blurry edges and JPEG
  noise don't add extra colours; matches each to the thread chart;
- cleans up specks and slivers too small to sew, snaps circles to true
  circles and straight-edged shapes (bars, stars, block letters) to sharp
  polygons, and fits smooth curves everywhere else;
- stacks colours the way a digitizer would: big fills underneath and running
  under the colours on top (no gaps), outlines on top; letter insides and
  other background-coloured holes are left open;
- each color becomes a layer (pieces that sit apart, like an emblem and a
  line of text, become separate layers) that you can edit like any art.

Set the width it will sew at, the number of colours (automatic or up to 12)
and Clean / Balanced / Fine detail; the result updates as you change them.
Click a colour chip to leave it out, or to stitch a background-coloured area
(white text that should be stitched on a dark shirt).

**Thread weights.** 40 wt is the default. Pick the design's weight next to
Fabric, or give any layer its own (Weight, under its thread): 60 wt for small
lettering, 30 wt for fast coverage, 12 wt for a bold look. Spacing scales with
the weight, a weight change gets its own stop and needle, and "Check before
running" says when strokes are too thin for the weight (and which weight would
work). **Weight guide** compares them side by side: needle size, spacing,
narrowest satin, smallest letters. The operator sheet and stops CSV list each
stop's weight, needle size and thread use; Production shows thread per piece
and pieces per cone.

**Views.** Switch any time with the buttons at the top left of the canvas
(or keys 1, 2, 3): **Vector** shows the artwork itself (shapes and live text
for drawing and editing), **Stitches 3D** shows thread on fabric (satin
columns with their rounded bevel, a split satin as two bevels, tatami's
woven texture, puff raised), **Flat lines** shows plain stitch lines.

**Text spacing.** Text layers stay editable text. Under the font and letter
height: letter spacing, word spacing, line spacing (× normal), alignment
(left, center, right), and **Kerning (letter by letter)** for each pair.

**Satin settings** (per layer, under Stitching): satin underlay (Auto, center
run, edge run, zigzag, edge run + zigzag, double zigzag, none), split satin
over (default 8 mm), short stitches on curves (on by default), satin spacing
and pull compensation under "Stitch settings (this layer)".

**Fill (tatami) settings:** fill underlay (Auto, tatami across, edge run,
edge run + tatami, double tatami, none), row spacing, stitch length, needle
pattern (½, ⅓, ¼, ⅕ offset), underlay spacing and inset, pull compensation.
Auto adds an edge run and a second underlay layer on caps.

**3D puff** (per layer): satin sewn over foam. StitchForge sews a contour
first, then stops (the operator sheet says "place 3 mm puff foam"), then a
tack-down run, tight satin (0.2 mm spacing, no split under 12 mm) and end
caps that wrap the foam at every open end. Puff always sews after the flat
parts. Foam 2, 3 or 6 mm. Columns under 3 mm are flagged.

**Cap sizes.** Sewing on Cap: choose Adult (4 × 2.25 in max) or Youth
(4 × 2 in max); "Fit cap" scales the design to the largest size that fits,
and larger designs are flagged.

**Measure (D).** Drag across a column, letter stroke or gap: shows mm and
inches and which thread weights it suits as a satin column and as a letter
height. Shift snaps to 15°.
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

**Drawing (v0.8).** The tool strip on the left draws new art right in
StitchForge, so a logo can be built from scratch or added to:
- **Select (V)**: click, Shift-click to select several, drag to move them.
- **Nodes (N)**: edit anchors and curve handles of the selected shape.
- **Stitches (M)**: edit needle points by hand.
- **Pen (P)**: click for corners, drag for curves; click the first point to
  close (filled shape), double-click or Enter to finish an open path (an
  outline), Backspace removes the last point, Esc cancels.
- **Rectangle (R)** with corner radius, **Ellipse (E)**, **Polygon / star (G)**
  with any number of points, **Line (L)**: drag to draw, Shift to constrain;
  a click alone makes a 20 mm shape.
- **Text (T)**: click where the words go.
- New shapes use the fill and outline colours and outline width shown on the
  right while a drawing tool is active. Points snap to existing anchors, and
  to a 1 mm grid when that box is ticked.

**Path menu** (also on the right when layers are selected): **Union**,
**Subtract** (lowest minus the rest), **Intersect**, **Exclude**, **Combine**,
**Break apart**, **Outline to fill**, **Border…** (a satin outline or a backing
fill grown around the selection or the whole design), **Flip**, and **Align /
Spread** for several layers. Results come back as smooth Bézier paths with
anchors only where the outline really turns. **File → Export artwork as
SVG** saves the vector art for Inkscape, Illustrator or a customer proof.

**Design objects (v0.7).** Every layer keeps two sides that stay in step:
its *shape* (the vector paths, with their anchors and curve handles) and its
*stitches* (the needle points). Both are saved in the `.sfg` file.
- **Edit shape**: drag anchors and handles, double-click the outline to add
  an anchor, Delete removes one. Stitches regenerate when you let go.
- **Edit stitches**: click or box-select needle points, drag or arrow-nudge
  them, Delete removes, double-click a stitch to add a point. Hand-edited
  stitches are kept as they are; moving, resizing or rotating the layer
  moves them too. If you later change the shape or its stitch settings, the
  layer shows **⚠ check** and asks: *Regenerate stitches* or *Keep my stitches*.

**Fill types and per-layer settings (v0.7).** Fills can be **Tatami** (rows),
**Contour** (rings that follow the outline), **Spiral** (one continuous line in
from the edge) or **Radial** (rows from the middle). Each layer can set its own
row/satin spacing, stitch length, pull compensation, underlay spacing and
inset, and tatami needle pattern (½ ⅓ ¼ ⅕) — blank means the fabric's
setting. Tags: `sf-contour`, `sf-spiral`, `sf-radial`, `sf-density-0.4`,
`sf-len-3`, `sf-pull-0.3`.

**Lettering (v0.7): turning satin.** Satin lettering on Auto angle follows each
stroke: the letter is traced to its centre line, split into strokes, and the
satin turns with each curve. Strokes that carry straight through a junction
are one unbroken column, sharp corners (N, M, K) are separate overlapping
columns, serif feet are covered straight across, and the centre-run underlay
doubles as the hidden travel, so letters sew without internal trims. Typing an
angle switches that layer back to one fixed satin angle.

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
**Garment** colour (presets like Black, Navy, Heather gray, or any colour;
saved with the project and used on the operator sheet and previews), fit.
The 3D view draws each stitch as thread lying on fabric: it tapers into the
needle holes, casts a soft shadow and catches the light by its angle, so
tatami shows its woven brick texture and satin its long glossy strands. On a
chosen garment, zooming in shows the twill weave.

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

# A picture, mockup photo or PDF to stitches (auto-traced)
stitchforge convert logo.png --width-in 3.5
stitchforge convert mockup.jpg --width-in 4 --weight 60

# Just the tracing: layered SVG plus a preview with the anchor points marked
stitchforge trace logo.jpg --width-in 3.5 --colors 4 --detail clean -o logo.svg

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
`--weight 12|30|40|60` (thread weight, default 40),
`--width-mm/--height-mm/--width-in/--height-in`, `--threads chart.csv`
(columns `code,name,r,g,b`; file name becomes the brand).

## What the engine does

- **Vector to stitch:** SVG from Illustrator, Inkscape, CorelDRAW, Affinity;
  PDF and PDF-compatible AI files. Curves, transforms, groups, both fill
  rules, gradients (first colour), text.
- **Pictures to stitch:** PNG/JPG/mockup photos auto-traced into stacked,
  stitch-ready colour layers (see Trace a picture).
- **Thread weights:** 40 wt default; 12/30/60 wt scale spacing, size limits,
  needle size and thread-use estimates.
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
- JEF and other formats.
- Tracing: gradients and photos with shading (now: flat colours only),
  reading text in a picture and offering it as editable lettering.
- Desktop UI (Tauri), MCP server for AI agents, job/production database.
- Brand thread charts (load your shop's CSV for now).
