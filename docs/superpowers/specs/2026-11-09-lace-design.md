# Lace of letters — design

Date: 2026-11-09. Status: reviewed by a builder check; owner answers folded in. Prototype (throwaway, JavaScript, ~1,100 lines) lives outside the repo; numbers below come from it.

## Intent

Texted gets a generated **lace layer**: the **document's own letters**, on Texted's monospaced cell grid, are joined to their neighbours by connecting strokes, so the text reads as threads of one lace. The connecting strokes are drawn **under the document text** as a layer of the Visuals page, next to the existing screen effects; the letters themselves render exactly as today, on top. It is decoration: it must never make the document harder to read or edit.

The owner's framing, in order of how it was given:

1. Every glyph has **connectors**. A straight line can be **extended** past its end; an arc or circle can be **joined by a tangent line**. Example: `А` has two connectors at the apex (the two diagonals extended past their meeting point), one at each end of the crossbar, and one at each foot. `а` has the top arc (tangent host), the stem (two ends) and the bottom arc (two ends).
2. Ligatures are needed for four situations: letters **touching**, letters **in a word**, letters **across a space** (different words), and letters **in neighbouring rows**.
3. The lace is **generated automatically**, with parameters such as spacing and density.
4. The font is **monospaced** and letters sit **on a grid** (the Texted editor grid).
5. A pair of letters gets **at most one ligature**. Each letter gets **at most one ligature per side**: forward, back, up, down.
6. A ligature may remove part of a stroke, but **never deeper than its own attachment point**.
7. Serifs are handled by an explicit mechanism; **by default the calculation ignores serifs**.
8. Between two rows, **at most one connection per pair of neighbouring columns**.
9. It lives as a **Visuals background layer under the document text** (not a change to how the document's glyphs are drawn, not a separate generator).
10. The letters it joins are **the document's own text**.
11. It is **stable**: what joins to what depends only on the nearby text and the seed, so it does not reshuffle while scrolling, resizing or when the line-number gutter widens.
12. **Spacing** sets how many empty cells a connection between words may cross (1–3).

## Goals

- A lace layer, off by default, switchable and tunable on the Appearance → Visuals page, persisted with the other visuals.
- Works with every bitmap face Texted can use: the five bundled faces (`assets/fonts/*.bin`) and Verba library faces (`font_library.rs`), at their native cell size and Texted's integer/10 % scaling.
- Deterministic: same settings, font, cell size and seed give the same lace.
- Cheap enough that it never shows in typing latency.

## Non-goals

- Changing the document's glyphs (they render exactly as today; the lace only adds strokes underneath).
- Lace where there is no text: empty cells past line ends and below the last line stay empty.
- Non-grid layouts (rings, scatter) — explored in the prototype, dropped with the grid decision.
- The ANSI terminal backend (cells cannot draw strokes). Kitty-graphics terminals get it through the shared raster path (see Rendering).
- Outline/vector fonts; Texted has none.

## Anti-goals (failure even if it works)

- The document becomes harder to read: lace competing with text in colour, weight or motion.
- A ligature eating a letter (see rule 6); a letter becoming another letter (`f` read as `l` because its crossbar vanished — happened in the prototype, see Lessons).
- Any per-frame cost proportional to the whole window when nothing changed; any work on the UI thread that scales with font size or window size beyond drawing cached tiles.

## Model (as built in the prototype)

### Strokes

Each glyph becomes a small set of primitives in font-pixel coordinates: **lines** and **circular arcs** (full circles for dots). Bitmap faces are converted once per face:

1. Components of ≤ 4 pixels within a 2×2 box become **dots** (circles r ≈ 0.7 px) — `i`, `j`, `ї` dots.
2. **Serif stripping** (default on, see Serifs).
3. Upscale ×4 with **Scale2x (EPX)** twice: pixel staircases become diagonals; filled square corners stay square.
4. **Zhang–Suen thinning**; graph of skeleton pixels (4-neighbours, plus diagonals not bridged by a 4-neighbour); trace paths between ends and junctions; merge adjacent junction pixels into one centre; prune spurs ≤ 3 upscaled pixels.
5. **Ramer–Douglas–Peucker** (ε = 0.3 font px). Vertices turning ≤ 55° become **tangent fillet arcs**; sharper vertices stay corners.
6. **Bar restore:** thinning erases a 1-pixel bar sticking 1 px out of a stem (the VGA `f` crossbar). Any 1-pixel-high run of ≥ 3 px crossing at most a 2 px stem, not already ≥ 70 % covered by traced strokes, is added back as a line.

Vectorising all 118 letters of VGA 8×16 takes ~0.2 s in JavaScript.

### Connectors

Per glyph, computed from the primitives:

- **Ray**: a line end extended along the line, or an arc end continued along its tangent. Kept when it leaves the glyph without running into its own ink (marching test), and — at a joint — when it makes ≥ 25° (line) / ≥ 60° (arc) with every other stroke leaving that point. This reproduces the owner's `А` (6 rays) and `а` (stem ×2, bowl ends ×2, arcs as hosts) on the line-and-arc model.
- **Host**: every arc/circle; another stroke may arrive tangentially.
- **Latent ray**: a ray blocked only by a dot (`i`, `j` stem tops). It opens when a ligature absorbs the dot.

### Ligature mechanisms

| Situation | Mechanism | Rule |
|---|---|---|
| touching / in word | **fuse** | two bar ends face each other at the same height (x-height zone and above) → one bar |
| | **bar onto stem** | a bar end runs onto a neighbour's stem top (`Th`) |
| | **shared stroke** | coincident strokes when cells touch |
| | **hook / arch** | a free stroke end leaves its arc tangentially and lands on a stem top (or a top serif pointing back) via **two tangent arcs** (biarc); `fi fl fb fh fk` (hook, from ascender height), `ct st sp ст` (arch, from x-height). Landing on a latent ray absorbs the dot (`fi`, `fj`) |
| across a space | **rail** | top bars across the empty cell |
| | **swash** | biarc from a right-going end or stem top to a left-facing end or stem top of the next word's first letter |
| neighbouring rows | **stem** | a downward ray continues straight into an upward ray in the same column |
| | **tail** | biarc from a downward end to an upward end in the row below (same column or diagonal) |

A ligature that reroutes a stroke end may remove only what lies **between its attachment point and that stroke's free end** (plus a dot on the extension of the stem it lands on), walking back over end-to-end strokes but never past a junction, at most 110° of turning and 9 units, and at most a quarter of the letter's ink. Only the absorbed dot of `i` exceeds a quarter (49 % in VGA without serifs). An **additive** mode (remove nothing) exists; in VGA it leaves only bar fusions — hooks and arches need to reroute.

### Grid tables

On a monospaced grid every neighbour is at a fixed offset, so a ligature is a **table entry** `(relation, left glyph, right glyph) → candidates`, computed once per face and cell size:

`h` next cell · `s` across one empty cell · `v` cell below · `dr`/`dl` below-right / below-left.

Each table keeps the cheapest candidate per set of connectors. Applying them to a grid: collect candidates for all occupied cells, sort (in-word first, then mechanism order, then cost), accept greedily with resource keys:

- connector keys (a ray, a terminal, a dot is used once);
- `pair` (one ligature per pair of cells — rule 5);
- side slots `F`/`B` (forward/back) and `U`/`D` (up/down; diagonals count as up/down) — rule 5;
- band slots: between rows `r` and `r+1`, one connection per column pair `(0–1, 2–3, …)` — rule 8;
- a clearance check against the other cells of the 3×3 neighbourhood (a table entry only knows its two cells);
- density: in-word ligatures always; other situations accepted with probability `density` from a seeded generator.

Measured (VGA 8×16, 48×14 cells of mixed Ukrainian/English words, density 1): 479 connections — 213 in-word, 45 across a space, 197 between rows, 24 diagonal — in ~20 ms. Per-direction tables for 26² Latin or 33² Cyrillic lower-case pairs take tens of ms each. Without the column-pair rule there were 275 between-row connections.

Classic pairs reproduced in VGA (serifs stripped and kept): `ff fi fl ft Th ct st sp ст тт пт` (11/11).

### Serifs

Two detectors on the bitmap, before vectorising:

- **foot/head**: the end row of a stem (≤ 2 px wide) sticks out 1–2 px on a side, the stroke does not continue past that row (so crossbars are never taken), and the protruding pixels touch nothing diagonally in the stem's next row (so bowls and shoulders meeting a stem — `а`, `n`, `h` — are not taken);
- **nub**: a 1–2 px vertical tick at the end of a bar ≥ 4 px that touches nothing else (`т`, `T`, `E`).

VGA: serifs found in 51 of 118 letters; connectors per letter 5.4 without serifs vs 6.7 with them. Setting: "count serifs as strokes", default **off**.

## Constraints from Texted

- **UI thread**: never blocks on buffer or file work; background workers notify through `AppEvent::Wake`, never capture a native window (exit-lifecycle fix). Face vectorising and table building belong off the UI thread, like `DocumentStats`.
- **Drawing**: `DrawList` carries rectangles and glyph quads (coverage bitmaps) plus terminal cell commands; there is no line primitive. The Metal path and the Kitty raster path (`raster.rs`) both consume `DrawList`.
- **Fixed framebuffers** draw in font pixels and present with nearest-texel scaling; Native draws at the window's scale. Visuals effects run after composition in `presentation.wgsl`.
- **Settings** live in `appearance.json` via `Visuals` (versioned, normalised on load; `effects_model` migration precedent).
- Fonts: bundled faces are `u32` code point + MSB-left rows; Verba faces may be other cell sizes and multi-byte rows.

## Decided how

- Lace is a **background layer**: drawn after the window clear and before the document text, in the palette's dim colour so text stays on top (exact colour role is the builder's).
- The grid is the **document's own visible cell grid**, wrapped rows included (same face, cell width and row height as the editor). The lace moves with the text when it scrolls.
- Only the **connecting strokes** are drawn (fusions, bridges, stems, tails). The document's glyphs are not trimmed or re-drawn; rule 6 still bounds where a bridge may attach (between its attachment point and the free end of that stroke), and an absorbed dot simply stays visible in the text.
- **Stability**: a connection depends only on the text in a bounded neighbourhood and on the seed; random draws (density) are keyed by document position (line, column) and seed, never by screen position. Scrolling, resizing and gutter changes therefore never reshuffle; an edit changes only connections near it. The global greedy pass of the prototype must become local (the builder chooses how, e.g. fixed-priority local selection), with a test that the same text gives the same connections at different scroll offsets and window widths.
- **Spacing**: the across-a-space relation generalises from one empty cell to 1–3 (`s1`, `s2`, `s3`), default 1.
- Defaults: serifs ignored; between rows one connection per column pair; in-word ligatures always; density for the other situations adjustable.

## Calls I made (check these)

- Rendering: rasterise each cell's lace into a coverage bitmap in **font pixels** (pixel-art strokes, matching Texted's bitmap look), cached per (face, cell size, glyph, neighbourhood signature), drawn as glyph quads. This avoids a new GPU primitive and gives Kitty terminals the lace for free.
- The lace is **static** (it changes only with the text, the settings, the seed, the face or the cell size), not animated.
- Off by default; `effects_model`-style versioning for the new settings.

## Reference prototype and exact definitions

The prototype is in [`2026-11-09-lace-prototype/`](2026-11-09-lace-prototype/) (`core.js` model and connectors, `font.js` model alphabet, `lig.js` mechanisms, `grid.js` tables and grid application, `vectorize.js` bitmap → strokes and serifs; `gridtest.js` prints tables and classic pairs, run with `node gridtest.js`). **Port it; its outputs are the expected answers.** Definitions where the text above is loose:

- **Units**: model units, where the x-height is 8. A bitmap face is scaled by `k = 8 / (baseline − xTopRow − 0.5)` (VGA: `k ≈ 1.23`, so one font pixel ≈ 1.23 units). "9 units", "≥ 6.0 high", tolerances are all in these units, measured from the glyph's own baseline. Thresholds therefore scale with the face.
- **Cost**: bar fuse = gap; bar onto stem = gap + 0.5; shared stroke = 0; bridge (biarc) = length + 0.05 × total turning in degrees.
- **Order**: situation first (in word, across a space, between rows, diagonal), then mechanism (shared stroke; fuse, bar onto stem, hook, arch; rail, swash; stem, tail), then cost.
- **Marching test** (`analyse` in `core.js`): step 0.2 units from 0.4 along the ray until it leaves the glyph's bounding box + 0.3; reject when within 0.3 of any other own stroke (strokes touching the start point are ignored for the first 0.6).
- **Clearance**: bridge points farther than 0.7 from both ends must stay ≥ 0.4 from every stroke of the 3×3 cells around the pair (excluding the rerouted and target strokes).
- **Biarc**: equal tangent lengths (`α` from the standard quadratic; parallel tangents use `α = |v|² / (2 v·t)`); reject if a radius < 1.4 (in word), 1.6 (across a space), 2.0 (between rows), or turning > 240° / 220° / 110°.
- **Model alphabet**: `font.js` — lines and circular arcs, x-height 8, cap 12, descender −4, width 8.
- **Classic pair "reproduced"**: its table entry for `h` contains the mechanism: `ff` fuse · `fi` hook (dot absorbed or kept) · `fl` hook · `ft` fuse or hook · `Th` bar onto stem or fuse · `ct st sp ст` arch · `тт` fuse · `пт` fuse or bar onto stem.

## Settings (v1) — my call

On/off · density (situations other than in-word) · strength (opacity of the lace colour) · spacing (empty cells a connection between words may cross, 1–3, default 1) · count serifs as strokes (off) · reshuffle (new seed). Additive mode is not needed as a setting: the document's glyphs are never trimmed.

## Faces — my call

Quality bar: the five bundled faces look like the VGA prototype and pass the classic-pair and invariant tests. Verba faces: no crashes and zero rule violations; a face without the needed letters, or with cells outside 6–32 px wide and 8–64 px high, shows the lace as unavailable on the Visuals page with the reason.

## Left to the builder

- Module layout (suggested: `lace/` with vectorise, connectors, ligatures, tables, raster), Rust data types, caching keys and eviction, worker scheduling.
- Stroke rasterisation details (line/arc stepping, stroke weight of one font pixel).
- Tests: the prototype's checks become unit tests — `А` six rays and `а` per the owner's description on the model alphabet; 11/11 classic pairs on VGA; rule 5/6/8 invariants on a filled grid (zero violations); serif detector cases (`f` crossbar kept; `а`, `n`, `h` shoulders kept; `т` nubs removed); determinism by seed.

## Lessons from the prototype

- Thresholds in absolute coordinates broke every row but the first; all height and direction tests must be in the glyph's own frame relative to its baseline.
- Arc trimming without a limit erased the bowl of `е`; the attachment-point rule and quarter-ink cap fixed it.
- Thinning erased the VGA `f` crossbar; bar restore fixed it.
- Parallel-tangent bridges (S-curves) need the special case of the biarc formula.
