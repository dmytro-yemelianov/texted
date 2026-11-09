# Texted handover — 2026-11-09

Start with [README](README.md), the [hardening results](docs/performance-hardening-results.md), and the [spec/plan](docs/performance-hardening-plan.md). The original design in `HANDOVER.md` and `docs/design-spec.md` is historical: the current buffer is Ropey, the GUI renderer is wgpu/Metal, and the GUI transport is asynchronous.

## Current work and delivery

The native-file, GPU and hardening stack ([PRs 1–6](https://github.com/dmytro-yemelianov/texted/pulls?q=is%3Apr+is%3Amerged)) is merged into `main`. The v0.2.0 preview packages this integrated source. See [release delivery](docs/releases.md) and the [release page](https://github.com/dmytro-yemelianov/texted/releases/tag/v0.2.0) for public assets; the tag workflow gates publication on ARM/Intel checks and universal-package verification. PRs #7–#16 followed as one stacked chain and are merged into `main` with merge commits; [v0.3.0](https://github.com/dmytro-yemelianov/texted/releases/tag/v0.3.0) packages that source (see the last section).

A0–A11 are implemented. The [results](docs/performance-hardening-results.md) record reference-machine CPU percentiles, deterministic work/memory/ordering gates and native acceptance evidence. Historical feature branches remain available for tracing the staged changes; new work starts from `main`.

## Appearance work

[PR #7](https://github.com/dmytro-yemelianov/texted/pull/7) is merged into `main` and adds static theme palettes and editor typography only. `appearance.rs` owns versioned, validated values and optional color overrides; `appearance_ui.rs` owns the modal Style panel; `preferences.rs` coalesces atomic writes off the UI thread. `graphics.rs` loads selectable system faces once at startup and uses a separate fixed UI face. Every font/size/spacing change updates the shared viewport metrics; pixel mode uses integer cells with a Unicode fallback. The default settings file is under Application Support/Texted; use `TEXTED_SETTINGS_PATH` for isolated UI tests. The final feature commit is `549d312`; the merge commit is `bcd3a77`, with the same source tree. ARM/Intel CI, universal packaging, 95 tests and review passed. Native checks confirmed corrected Ukrainian pixel glyphs, punctuation, Style/zoom controls and persistence across restart without changing document bytes. No shader effects were added. These changes are in development builds; the existing `v0.2.0` preview predates PR #7. See README for controls and config.

## Native menus

The `feat/native-menus` branch adds macOS application menus (Texted, File, Edit, Search, View, Window) and a document context menu (right-click / Shift+F10). `menus.rs` owns native items and their enable/check state; callbacks send `AppEvent::Menu` into the existing typed intent path. `context_menu.rs` owns selection hit rules and the pending invocation value. The shell waits for accepted commands before testing the selection or positioning the caret, and cancels a popup request if newer input, document identity or modal state makes it obsolete. Published snapshots now include undo/redo availability. Clipboard and file barriers remain authoritative. winit's default menu and automatic window tabbing are disabled because this is a single-document editor. Local validation: 99 Rust tests, Clippy, formatting, two packaging tests and the explicit Metal test passed. Native checks covered both popup entry points, selection preservation, context deletion and undo, history enablement, Style, single-dispatch Cmd+L and saving an exact UTF-8 fixture. These changes are development-only; no release tag was created.

## Platform framebuffers

`feat/platform-framebuffer` builds on the native-menu branch and adds real fixed-resolution GPU canvases. Style's Screen/Resolution row defaults to Auto by theme, with Native and explicit platform presets. `framebuffer.rs` owns bounded resolutions and presentation/input transforms; `presentation.rs` caches one GPU texture and presents exact texels, with aspect-fit nearest-texel scaling and letterboxing. The last successful frame's transform governs clicks, drags, pixel-wheel events and context-menu positions. Compact Style/search/status controls fit the smallest 256×192 canvas. Existing version-1 preferences load with Auto and separate 8 px pixel typography; native typography stays intact. See [framebuffer details and primary references](docs/framebuffer.md). Local validation passed 105 Rust tests, Clippy, formatting, two packaging tests and both explicit Metal tests. Native checks confirmed C64/ZX raster changes, Native bypass, compact controls, pointer mapping, restart persistence and unchanged document bytes. No release tag is created.

## Style dropdowns

`feat/style-dropdowns` builds on the framebuffer branch. All five Style selectors now open lists: theme, resolution, font, size and spacing. `dropdown.rs` owns bounded popup geometry, disabled-item navigation and fractional wheel scrolling; `appearance_ui.rs` owns typed choices and commits. Click/Enter/Space opens a list; Up/Down, Home/End and Page Up/Down navigate; Enter commits; Escape/outside clicks dismiss the list first. Hover/browsing does not change settings, choosing the current theme preserves custom colors, and disabled fonts cannot be applied. Popup drawing and hit testing share geometry, including at 256×192. Closing Style, keyboard file actions and zoom/reset dismiss stale popup state. Validation: 111 Rust tests, formatting and Clippy passed. Native verification covered full theme/resolution lists, direct mouse selection, keyboard selection/cancellation, and the complete ten-theme list at 256×192; document bytes remained unchanged.

## Window-filling pixel scaling

`feat/aspect-fit-scaling` builds on the dropdown branch and removes whole-number scale quantization. Every fixed framebuffer fills one physical window dimension while retaining the original aspect ratio (rounded by less than one pixel on the other dimension). The shared mapping still governs rendering, pointer/drag input, wheel deltas and context-menu placement. Source resolution stays fixed; only presentation changes during resize. The Metal readback case now checks a 3×3 source at a non-integer 32/3 scale, including exact texels and black letterboxing. Validation passed 112 Rust tests, Clippy, formatting and both explicit Metal checks. Native preview confirmed the VGA canvas fills the window height with only aspect-ratio side bars, follows Zoom/restore resizing and retains correct Style-button hit testing.

## Ukrainian bitmap fonts

`feat/ukrainian-bitmap-fonts` builds on aspect-fit scaling. All custom UI and editor text now use bundled native bitmap strikes: IBM BIOS 8×8, EGA 8×14, VGA 8×16, Spleen 6×12 and Spleen 8×16. System-font loading, outline rasterization, thresholded Unicode fallback, `ab_glyph` and `font8x8` are removed. Native defaults to VGA 16 px; fixed framebuffers default to BIOS 8 px. Sizes are whole multiples of the selected native height, bounded by 64 px. Version-1 system-font family values migrate through serde aliases; native/framebuffer settings stay independent. Missing scalars show a visible box.

`bitmap_fonts.rs` exposes sorted, embedded native masks. `assets/fonts` includes unchanged pinned OTB/JSON sources, original-row checksums, glyph-level modification records and full licenses. Existing glyphs remain unchanged. IBM Unicode Plus sources already cover Ukrainian; Spleen adds Є/є from mirrored native Э/э, and missing punctuation/currency uses each face's existing strokes. See the [catalog review and pixel sheets](docs/bitmap-font-review.md). The related local Nasinnia and pixel-ssh checkouts were read without changes. Optional offline regeneration needs `fonttools==4.60.2`; runtime and ordinary CI do not. Packages include and verify the complete font source/license tree.

Validation passed 114 Rust tests, four Python asset/packaging tests, Clippy, formatting, reproducible asset regeneration and both explicit Metal tests. Readback checks now reject intermediate font edge colors. Native acceptance covered the five-face list, VGA/Spleen Ukrainian rendering, BIOS in a 256×192 framebuffer, restart persistence and a byte-identical saved UTF-8 fixture. Use the separate `target/ukrainian-fonts-preview/Texted Ukrainian Fonts.app` and settings for further GUI checks; older running previews may contain user work. No release tag is created.

## Component map

| Component | Owner / invariant |
|---|---|
| `main.rs`, `app.rs` | Bootstrap and native adaptation/scheduling; no editing algorithms or blocking buffer replies |
| `dropdown.rs` | Bounded list geometry/navigation shared by native-sized and compact Style controls |
| `menus.rs`, `context_menu.rs` | Native actions/state and selection-aware popup invocation; shared intent dispatch |
| `input.rs`, `commands.rs` | Physical-key shortcuts and typed intents, windowless |
| `editing.rs`, `selection.rs`, `mouse.rs` | Canonical editor/drag transitions and selection values; shell mouse state tracks only gestures |
| `buffer_service.rs` | Sole live-buffer owner; bounded admission, completion watermark, published snapshots and fenced bulk commits |
| `search.rs`, `search_ui.rs` | Bounded workers/checkpoints/coverage plus UI fields and paste stamps |
| `clipboard.rs`, `transfers.rs` | Snapshot extraction and child I/O off UI; invocation barrier, write-before-cut and context-fenced completion |
| `documents.rs`, `file_flow.rs` | Native async sheets, background file I/O, saved-root dirty state, command/clipboard barriers |
| `viewport.rs`, `texted-layout` | Visible geometry, scrolling, caret, compact search coverage and pure highlight clipping |
| `framebuffer.rs`, `presentation.rs` | Fixed source canvases, bounded GPU targets and matching presentation/input transforms |
| `bitmap_fonts.rs`, `graphics.rs`, `editor_view.rs`, `gpu.rs` | Embedded native bitmap masks, integer pixel scaling, immutable drawing preparation, bounded caches and ordered Metal submission |
| `texted-buffer` | Persistent rope, byte-safe coordinates/history, resumable scanner, streaming replacement planner, local word traversal |
| `texted-io` | Streaming UTF-8/BOM/CRLF load and atomic save |
| `texted-kernel` | Reusable request/reply primitives retained for tests/benchmarks; not the GUI transport |

## Contracts to preserve

The UI never owns a mutable live buffer. Read the published immutable snapshot; enqueue typed commands. A successful submit means acceptance, with a later completion. Busy/closed admission returns ownership, and the UI reports rejection. The admission limit is 64 undrained commands. File actions wait for the completion prefix, including an earlier worker that finishes after later ordinary edits.

Bulk plans are isolated snapshots, commit once, undo once, and validate document ID, revision, cursor and selection. Ordinary typing may invalidate a plan; report the conflict and keep the newer edit. Escape cancels pending bulk/clipboard work. A committed operation cannot subsequently become cancelled.

Search owns at most one active job, one latest pending job and one result; query limit is 64 KiB, auxiliary budget 8 MiB, scan cancellation quantum 4 KiB. Do not restore a full-match vector in GUI paths. `find_matches` remains a compatibility/reference helper. Coverage stores viewport cells, not all document offsets. Checkpoints retain non-overlap phase.

Clipboard extraction/transfer runs off-thread, with a five-second deadline and 128 MiB capacity. Cut deletes only after write success and a matching invocation context. Paste fences both document context and search-field serial. Save/close must wait for clipboard continuations as well as buffer commands. Subprocess failures/timeouts must leave the document intact and reap the child.

The saved rope root defines clean state. Cursor motion cannot dirty a file. Saving an older snapshot cannot clean later edits. Loading a document clears history and changes identity. Preserve BOM, LF/CRLF and trailing newlines across saves.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
python3 -m unittest discover -s scripts -p 'test_*.py'
cargo test --release -p texted-app --lib metal_ -- --ignored
cargo run --locked --release --bin bench -- --hardening
python3 scripts/package_macos.py
```

The reference machine is Apple M5 / macOS 27.2 / Rust 1.88.0. Benchmark data are CPU times, not input-to-display latency; search-owned storage excludes shared rope roots and RSS. See [results](docs/performance-hardening-results.md) for sizes, sample counts and percentiles. Existing GPU preview instances can contain user work: use a separate bundle ID and generated temporary documents for native checks.

Known limits: scalar-character cells rather than grapheme/complex-script shaping; no IME composition; whole-file rope loading rather than mmap; history bounded by transaction count. Very long contiguous words and exact search counts necessarily scan their input. No additional features are part of the completed hardening scope.

## Full Verba library and tabbed editor (2026-11-08)

`feat/verba-font-library` builds on `feat/ukrainian-bitmap-fonts`. It adds the complete local Verba catalog (2,266 entries; 1,993 usable Unicode faces; 273 disabled with reasons), actual saved glyph edits, multi-byte bitmap rows, bounded decoded/thumbnail caches and stable font IDs in preferences. See [sync/data contract](docs/verba-font-library.md). The importer uses Verba's normalized `exportRecord`; it neither modifies corpus drafts nor applies unselected recipe candidates. Corpus binaries are local Application Support data, never Git assets. User-owned untracked `docs/specs/` is unrelated and must stay untouched.

The old information strip is replaced by an actionable word/selection count, on-demand details card, hover help, scale and Appearance. Statistics scan cancellable immutable snapshots off-thread, keyed by tab and buffer document identity. Appearance is draggable, shows bitmap specimens and typed font search, previews arrow navigation instantly, commits on Enter/click and rolls back on Escape. Uncommitted previews are not persisted. System/Pixel/Large pixel cursor choices follow text, buttons and drag handles and restore the OS cursor on exit/focus loss.

The editor now owns per-tab sessions with independent buffers/history/search/cursor/viewport/file state. Native Finder opens all supplied paths. The optional clickable F1–F12 command panel, tab navigation and 10% nearest-texel text scale are described in [tab behavior](docs/tabbed-editor.md) and README. This supersedes the earlier single-document, F3 Diagnostics, integer-only zoom and no-preview descriptions above. Existing released v0.2.0 assets remain unchanged; this is development work.

Final local checks: 144 Rust tests, four Python asset/packaging tests, formatting, strict Clippy and both explicit Metal readback tests passed. The separate release picker CPU profile is opt-in/ignored during ordinary tests. Replacing bit-at-a-time CRC32 validation with a lookup table reduced the measured cold catalog/picker initialization from ~543 ms to ~167 ms on Apple M5; warm visible picker command preparation was ~11.3 µs. `/Applications/Texted.app` is updated as an arm64 development build, with the prior installed app backed up under ignored `target/installed-backups/`. Earlier preview processes with unsaved fixture edits were left running. The final F-panel is one line with short contextual labels and physical keyboard modifier layers only; there are no modifier controls in the panel.


## Unified command panel and pointer feedback

Follow-up work removes the separate footer entirely. Ctrl opens scale/statistics values inside the existing F-key placeholders (F7/F8 ±10%, F9 current/reset scale, F10 cursor position/details, F11 wrap state/toggle, F12 word/selection count/details). The default layer stays command-focused. `PanelHelp` in `status_ui.rs` paints only delayed help and the details overlay, in scene coordinates. `editor_view` and `Viewport` no longer reserve status height. Context values use cached asynchronous counts; no document scans occur on hover or paint.

F-buttons highlight on hover and invocation, including physical keyboard holds and a 160 ms flash for brief calls. Focus loss/release clears held state. Cursor selection now offers eleven styles: System, Small/regular/Large pixel, Dark/Amber/Green/Cyan pixel, Classic arrow, Crosshair and Ring. The latter two retain contextual text/drag shapes. Native QA confirmed Ctrl values, a 10% scale step, details, hover/help, the complete cursor list and Ring rendering. Local validation: 149 Rust tests and strict Clippy/formatting passed; the installed arm64 development app was updated and a new isolated preview was opened without closing prior unsaved windows.

## macOS exit deadlock fix (2026-11-08)

The reported exit hang was reproduced and sampled. Main was joining `DocumentStats`; its wake callback or final `Window` release synchronously dispatched to the main queue. winit 0.29's `maybe_queue_on_main` actually waits on main. Reordering the window alone passed idle exit but failed the save/exit race. The complete fix sends all service, file-I/O and dialog-future notifications through `AppEvent::Wake`/`EventLoopProxy`. Only the main handler calls `request_redraw`; callbacks hold no `Arc<Window>`. The window also drops last. Dialogs reveal/restore/focus their parent before awaiting a choice. See [exit lifecycle](docs/exit-lifecycle.md).

Two additional tests cover background native-future wake without UI access and service teardown with queued events left undispatched, including durable preference flushing. The full suite now has 151 passing Rust tests. Native validation covers empty/two-tab quit and a previously hanging dirty Save/quit with Unicode preserved. Install/launch tests should use Launch Services for the real app, and separately identified isolated bundles for QA; always verify process exit rather than only window disappearance. Recovered prior fixture edits are under ignored `target/exit-recovery/`.


## Shared cell-grid transport (2026-11-08)

`feat/terminal-cell-grid` builds on `feat/verba-font-library`. Both executables now bootstrap the same library `App`: `texted --terminal` and `texted-tui` use a crossterm transport, `Host::Cells`, and an ANSI diff renderer, without creating winit/NSApplication/GPU. `graphics::DrawList` preserves source characters/colors/borders/cursor alongside native GPU commands. `cell_grid.rs` exports version-1 bounded frames and safely writes single-cell terminal characters. `--dump-grid COLSxROWS [FILE ...]` exports the actual common UI as JSON. See [contract and validation](docs/terminal-cell-grid.md).

`dialog_ui.rs` replaces regular OS file/error/Save-Discard-Cancel sheets with one cell-aligned in-app controller for both backends. It uses asynchronous bounded tickets, off-thread directory/path inspection, existing EditorModel path editing and context-fenced clipboard completion. Existing FileFlow acceptance/save/close barriers remain authoritative. Context menus also render through the common scene; the macOS application menu remains a native adapter. Cell-only font/scale/cursor settings are visibly terminal-managed. Terminal preferences are separate unless TEXTED_SETTINGS_PATH is supplied. Background workers still capture only Wake, never a native Window; preserve the macOS exit fix. GPU tests now live in the library target (`--lib metal_`). User-owned untracked `docs/specs/` remains untouched.

Validation for the shared transport: 161 ordinary Rust tests, strict Clippy/formatting, four Python regressions, both explicit Metal readbacks and the release PTY smoke test passed. Native QA verified Save As with menu-driven path selection, cancellation of dirty quit, and durable Save/quit with actual process exit. Local universal ZIP/DMG packaging verifies both executable slices, resources, load commands, cell-grid output, extraction and checksums. Development artifacts are under ignored `dist/terminal-cell-grid-preview/`; published releases remain unchanged.


## Terminal bitmap framebuffer (2026-11-08)

`feat/terminal-graphics` builds on `feat/terminal-cell-grid`. `--render=auto|kitty|ansi` selects negotiated Kitty image tiles or ANSI; both run the same App and UI. `raster.rs` composites the native DrawList with one-bit masks and nearest-neighbor presentation. `kitty.rs` sends only changed, compressed 8×4-cell tiles and tracks owned image IDs for cleanup. `terminal_probe.rs` negotiates graphics and exact pixel geometry. `terminal_input.rs` owns the TTY byte stream (crossterm event reads are removed), quarantines replies/control strings, preserves bracketed paste and supports extended key/focus/mouse events. Pixel mouse mode 1016 is queried and enabled only when supported; cell reporting remains the fallback.

Graphics unlocks the existing full font picker, 10% zoom, spacing, palettes and reference framebuffers. The pointer remains terminal-managed. Graphics and ANSI preference files are separate and honor TEXTED_SETTINGS_PATH. Frames have a 16MP/32,768-tile budget; invalid resize suspends the view while preserving editor state. Source workers still capture only Wake, never native Window; no GPU/native event loop is initialized for terminal graphics. See [architecture, protocol contract and measured CPU/bandwidth limits](docs/terminal-graphics.md).

Validation includes the ordinary workspace suite/strict Clippy, ANSI fallback PTY, a Kitty peer that decodes tiles and checks font choice, independent zoom, Unicode save, both mouse paths, resize/suspend/resume, image cleanup and termios, forced unsupported-peer rejection, both Metal checks, and native Ghostty bitmap/font/zoom previews. The real desktop automation driver's synthetic clicks reported terminal coordinate 1,1; keyboard-driven native font/zoom checks and protocol-level mouse tests provide separate evidence. QA files/logs are local ignored target data and no user clipboard or corpus drafts are changed. User-owned untracked docs/specs/ remains untouched.

Final local validation: 172 ordinary Rust tests, strict Clippy/formatting, four Python asset/packaging regressions, ANSI PTY fallback, both graphics mouse paths with resize/suspend/resume, unsupported-peer rejection, both Metal readbacks and the opt-in CPU/transfer profile passed. Universal ZIP/DMG packaging and extracted executable verification passed under ignored dist/terminal-graphics-preview/.


## Daily editing and Visuals (2026-11-09)

[PR #16](https://github.com/dmytro-yemelianov/texted/pull/16) (`feat/daily-editing`, merged as `3faed91`) closes the stacked chain #8–#16. It starts with review fixes (`8f0c6a0`): saving through a symlink writes its target, `pbcopy`/`pbpaste` run with a UTF-8 locale, mixed and CR+CRLF endings save back byte for byte, the TUI panic hook restores only its own terminal, short frames no longer underflow, system quit requests answer through `NSTerminateLater`, and a cancelled block edit rolls back.

`texted-io` decodes UTF-8, UTF-16 with BOM, Windows-1251 and KOI8-U as a stream through `encoding_rs`, refuses binary input and saves in the loaded `TextEncoding`; an unrepresentable character stops the save with its line and code point. `DiskStamp` (mtime, length, inode) is taken at load and save. `documents.rs` checks it on focus and tab switch: an unchanged clean document rereads and keeps its caret line, a changed one is noted once, and Save asks Overwrite / Reload / Cancel. `session_store.rs` keeps tabs, drafts, caret, encoding and stamps in `session.json` beside the preferences, locked to one instance, so window quit no longer asks; Cmd+W, the terminal backends, a second instance and a failed session write still ask.

Search adds simple case folding (Cyrillic included) on F5 and whole words on F6 while searching; Replace follows both, and the case-sensitive path keeps the byte matcher. `texted-layout::WrapRows` is the single word-wrap row model for drawing, caret, clicks, highlights and scroll-to-caret; lines over 64 Ki characters keep cell wrapping. Line tools (Cmd+Shift+D, Cmd+Option+Up/Down, Trim Trailing Spaces) are one undo step each; Cmd+J opens `:line[:column]`.

Appearance has a Visuals page. `visuals.rs` owns machines (ZX Spectrum, Atari ST, Mac System 2.0, Game Boy with their render defaults, FPS and chrome), Pixel SSH looks (Pixel SSH, CRT, Arcade, Bloom, Glitch) and seventeen effect rows: ShaderWallpaper's brightness and flicker, Pixel SSH's CRT pass (curvature, chromatic aberration, afterglow, scanlines, pixel grid, jitter, antenna hum, noise snow) and ditherGL's bloom, vignette and six pixel separation modes. Settings carry `effects_model`; files saved for the earlier wallpaper shader (model 1) convert on load. `presentation.wgsl` runs the pass in font-pixel units on Native and fixed screens, skipping it entirely when effects are neutral. `ScreenMapping` carries the same bend/unbend, phosphor and theme background for the shader and for pointer input, and wraps effect time every 600 s so shader hashes keep f32 precision. Amber, Green CRT, Mac and Game Boy are monochrome tubes without chromatic aberration. Pixel SSH's mouse magnet and ditherGL's VHS/tape/frame-history effects are intentionally absent.

Validation: 252 Rust tests (6 opt-in ignored), strict Clippy/formatting, four Python regressions, four Metal readbacks (exact texels with effects off, curved dark corners and per-row beams, the Game Boy matrix gap colour), both PTY peers and the opt-in `scripts/smoke_native_quit.py`. Not covered: restoring an unsaved draft through a real window (headless only), an actual logout, and the line-move shortcuts in terminals without the Kitty keyboard protocol. User-owned untracked `docs/specs/` remains untouched.

## Release v0.3.0

`v0.3.0` is the first release after `v0.2.0` and packages PRs #7–#16: themes and typography, native menus, platform framebuffers, Style dropdowns, aspect-fit scaling, bundled bitmap fonts and the Verba library, tabs and the command panel, the terminal cell grid and Kitty graphics, and #16 above. Next candidates recorded in the known limits: syntax highlighting (none in the tree), grapheme/complex-script shaping, IME composition and memory-mapped loading.

## Lace layer (2026-11-10)

`feat/lace` adds an opt-in Visuals layer that joins the document's own letters with connecting strokes drawn under the text ([design](docs/superpowers/specs/2026-11-09-lace-design.md), [plan](docs/superpowers/plans/2026-11-09-lace.md); the prototype's outputs are the expected answers). The document's glyphs are drawn exactly as before; only stroke sprites are added between the selection highlight and the text.

| Module | Owner / invariant |
|---|---|
| `lace/geom.rs`, `lace/connectors.rs` | Lines, arcs and dots in model units (x-height 8, y up from the glyph's own baseline); where a stroke may continue out of a letter |
| `lace/vectorize.rs` | Bitmap glyph → strokes (Scale2x, thinning, tracing), serif stripping, `FaceMetrics::measure` (its error is the Visuals page's "Unavailable" reason) |
| `lace/ligatures.rs` | Mechanisms (shared stroke, fuse, bar onto stem, hook, arch, rail, swash, stem, tail) with cost and the connector keys they use |
| `lace/table.rs`, `lace/sprite.rs` | Per face, cell and serif setting: `(relation, a, b)` → cheapest entry per connector set, each with a one-font-pixel sprite |
| `lace/layer.rs` | Stable local selection over the visible rows; random draws keyed by document line/column and seed; lists unfilled pairs instead of computing them |
| `lace/service.rs` | One worker, latest request wins: fills requested pairs, publishes an `Arc<Tables>` per `FaceKey`, wakes through the `Wake` closure, drops work for a stale key; `place` reuses the last selection while rows, tables and settings are unchanged |
| `graphics.rs`, `editor_view.rs`, `app.rs` | `FontEngine::face/face_id` and a sprite→glyph cache with its own keys; `paint_lace` places sprites on the text's rounded column x, clipped to the text area, in `bg + (text − bg) × strength`; the app measures the face on every appearance change and draws lace only when it is enabled and the face is available |

Cell-only terminals (ANSI) get no lace; Kitty graphics terminals get it through the shared `DrawList`/raster path. The Tables cell is `[face width, round(line height / font pixel)]` font pixels, so unusual line spacing can leave between-row strokes up to half a font pixel off. Connections to rows outside the drawn rows are not selected, so the first and last visible rows gain their upward/downward strokes only when the neighbour row scrolls in.

Validation: 297 workspace tests (texted-app 239, 6 opt-in ignored), strict Clippy/formatting, four Python regressions and four Metal readbacks. Native check through the Kitty path (a PTY peer reassembling tiles): lace on differs from lace off only in lace-coloured pixels and keeps every text pixel; scrolled frames match the unscrolled one on every row except the top row's strokes from the row above; serifs as strokes are denser; BIOS 8×8 draws lace; ANSI output is byte-identical with lace on and off. The GUI build opened VGA, BIOS and serif settings and quit cleanly through the system quit event.
