# StitchForge test pack

Original test designs and pictures (no client artwork) that each push one part of the
engine. Lettering is already converted to outlines, so every file opens the
same on any computer. Fonts used to draw them: TeX Gyre Heros, Schola, Bonum,
Chorus and Adventor (free, GUST font license).

| File | What it tests | What a pass looks like |
| --- | --- | --- |
| 01-badge-layers.svg | Many colors, overlapping layers, tiny shapes (sun rays, stars), a stroke over fills, text on a banner | Lower layers don't show through; no double-stitched overlaps; tiny "EST. 1998" period is skipped with a warning; about 10,000 stitches |
| 02-small-lettering.svg | Lettering at 0.25", 4 mm and 3 mm cap height, sans and serif, digits | Every letter is there and readable in 3D view; 3 mm rows may warn about narrow columns |
| 03-lettering-styles.svg | Chunky serif, script, geometric sans, italic serif | Satin turns with each stroke; no missing strokes (check the n, d and h); script joins look smooth |
| 04-holes-islands.svg | Letters with holes, a donut, a paw print (separate pieces), an even-odd star and target rings | Holes stay open; each paw pad sews as its own piece; rings don't fill in |
| 05-outlines-corners.svg | Outlines: 3 mm zigzag satin, star, rounded box, open S-curve with round ends, 0.4 mm run, 0.8 mm triple run, heart | Sharp corners are clean (try each corner type); ends follow the cap setting; thin lines become run/triple run |
| 06-fill-patterns.svg | Contour, spiral, radial and tatami fills, plus a panel with its own density and stitch length (from `sf-` tags in the names) | Each shape shows its pattern; the panel is visibly lighter (0.6 mm rows) than the others |
| 07-cap-front.svg | Arched lettering with an outline, straight lettering under it; open with Sewing on: Cap | Sews red, navy, gold: 3 stops, each color center-out from the bottom |
| 08-big-roster.xlsx | 41 rows for name drops: accents (Zoë, José), apostrophes, hyphens, a very long name, stray spaces, a blank row, a duplicate, quantities | 43 names (Qty column on), blank row skipped, long name shrinks to the max width, run sheet lists the right spreadsheet rows |
| 09-jacket-back.svg | A large 7.5" jacket-back design: big fills, a gear with a hole, arched lettering | Builds in a few seconds; about 30,000 stitches; frame-size warning if it's larger than the chosen frame |
| 10-logo-web.png | A clean PNG logo (the kind saved from a website): circles, a star, a rounded banner with white text, small red text | Trace window finds 5 colors (navy, gold, red, white, black); circles come out as perfect circles, the star with 10 sharp points; the white "ACE" is stitched (it sits on the navy) |
| 11-logo-low-quality.jpg | The same logo as a heavily compressed JPEG | Same 5 colors as the PNG, no extra in-between colors, outlines nearly as clean |
| 12-mockup-photo.jpg | The logo on a shaded green T-shirt photo | Garment and studio background removed automatically, crop lands on the logo, 4 colors; drag on the picture to change the crop |
| 13-vector-shield.pdf | A vector PDF: CMYK navy, a Pantone 123 C spot color, a page background, live text | Opens directly as layers (no tracing); the gold layer is named "PANTONE 123 C"; notes say the background was left out and the live text needs outlines |
| 14-picture-in-pdf.pdf | A PDF that only holds a picture | Goes to the trace window instead of opening empty |
| 15-tiny-logo.png | A 210 × 150 px logo traced up to 4" | Still clean curves, no blocky steps; colors right |

Pictures: drag the size, colors and detail in the trace window and watch the
result update; click a color chip to leave it out (or to stitch a
background-colored area). Use the Measure tool (D) on thin parts and try the
lettering at 60 wt.

To report a problem: a screenshot plus one line (file, what you did, what
looked wrong) in the Problems table of the test checklist.
