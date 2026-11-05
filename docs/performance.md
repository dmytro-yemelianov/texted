# Implementation audit and follow-up — 2026-11-05

The later audit's H1–H5 findings are addressed on `feat/editor-hardening`. Start with the [current results and limits](performance-hardening-results.md), [specification](performance-hardening-spec.md) and [implementation plan](performance-hardening-plan.md). The sections below preserve the earlier audit and delivery history; their test counts and measurements describe those earlier stages.

## Changes

The original `Vec<Piece>` copied every previous piece list into undo history. Repeated typing therefore retained quadratic metadata. A persistent [Ropey tree](https://docs.rs/ropey/1.6.1/ropey/struct.Rope.html) now provides shared roots for snapshots and history. Consecutive single-character input is grouped until navigation, a different operation, a newline or a 750 ms pause. History retains at most 1,000 transactions.

Rendering no longer materializes a whole-document `String` or `Vec<&str>`. Indexed line slices supply the viewport directly, including deep document positions and partial long lines. Coordinates, caret, highlights and mouse hit-testing use the same visible-row ranges. Scrolling records the physical line and wrapped subrow.

Replace All and Replace Next preserve undo and valid UTF-8 cursor boundaries. Background replacements commit only when their base revision and cursor still match. Search results are revision/query checked, and highlights visit visible matches/rows. Search uses streaming KMP across rope chunks.

File loading streams into a rope and normalizes CRLF across input buffer boundaries. Atomic saves preserve BOM, requested line endings, trailing-newline count and existing permissions. File-open failures retain the current document. Open and save errors appear in native dialogs and stderr.

The buffer server now sleeps on its channel, and the GUI waits for input or the next caret deadline instead of polling every 16 ms. Search, replacement computation and saving do not run on the UI thread. Ordered edit requests avoid reordering text pulses around navigation requests.

Removed disconnected GPU/quad code, the unused registry, unused dependencies and search RPCs. Drawing primitives were extracted from the event loop. Removed fake RAM values, CPU-derived "FPS" and unconditional benchmark success claims. Benchmarks use black_box, assert that deletion changes the buffer, and count actual undo operations.

## Measurements

Local release build, same machine, 800×600 layout, short lines, cursor at the start. Before values were single samples; after values are means from 100 iterations after warmup. They demonstrate the change in scaling, not a controlled end-to-end latency comparison.

| Workload | Before | After |
|---|---:|---:|
| 64 MiB snapshot | 7.46 ms | below 0.1 µs timer granularity |
| 64 MiB no-wrap layout at start | 40.65 ms | 3.51 µs |
| 64 MiB no-wrap layout near end | not recorded | 5.01 µs |
| 64 MiB wrapped layout near end | not recorded | 8.95 µs |
| 160k-character line, string coordinate helper | 843 ms | 327 µs |
| 160k-character line, indexed rope lookup | unavailable | 0.13 µs |

Snapshot benchmarks can produce zero-duration samples at this timer resolution. Neither these numbers nor the HUD measure hardware input latency, compositor delay or display scanout.

## Validation and limits

Regression coverage includes Unicode byte offsets, Replace All history, grouped typing, immutable old snapshots, bounded history, background commit conflicts, randomized edits against a String model, cross-chunk search, CRLF/BOM round-trips and wrapped-row caret/click agreement.

The editor now composites text and rectangles on Metal through wgpu and redraws for caret blink. Layout and cache-miss font rasterization remain on the CPU. Plain scalar-character columns are not grapheme/complex-script layout. Loading still parses the complete file into a rope; it is streaming, not lazy mmap. History is bounded by transaction count, not a strict byte quota. The later hardening replaces match-count-sized GUI search storage with bounded coverage/checkpoints, and removes clipboard and buffer reply waits from the UI. Horizontal scrolling in no-wrap mode slices the visible character range. Longest-line measurement runs in a cancellable background job keyed by document revision; only visible line summaries are read on the UI thread. A temporary range based on visible lines is available while the document-wide range is being measured.

Final validation: 29 unit tests pass, formatting passes, Clippy passes with warnings denied, and the release binaries build. Native GUI checks covered wrapped-line caret movement, automatic search selection, Replace All followed by a single Undo, and byte-for-byte BOM/CRLF save preservation. GUI checks used an isolated temporary document.


Horizontal scrolling follow-up: the viewport now shares its first visible character column across rendering, caret, mouse hit-testing and selection/search highlights. Trackpad deltas accumulate below a character cell; Shift+wheel redirects the vertical axis. The scrollbar supports thumb dragging and page clicks, clamps after resizing or deletion, and disappears when wrapping is enabled. Native GUI checks covered horizontal wheel scrolling, Cmd+Right caret tracking, scrollbar dragging, and clicking Ukrainian text after a horizontal offset. Model/layout tests also cover Shift routing, stale width results, short lines and wrap mode ignoring the horizontal offset.

File workflow follow-up (v0.2): native Open and Save As sheets run asynchronously on the main thread; loading and saving run in workers. New, open, drop, close and quit share the Save/Discard/Cancel guard. A successful save records the exact written rope root, so edits made during a save remain dirty and undo can return to a clean saved state. Saves break typing groups; installing a document clears history and increments the revision to reject results from the old document. Pending replacement commits finish before lifecycle actions inspect dirty state.

Validation for the file workflow: 40 Rust tests and two packaging tests pass, with formatting and Clippy warnings denied. Packaged-app GUI checks covered Open, Save As, Cancel retaining dirty edits, Discard creating an empty document, and Save on quit. The quit/save check preserved the temporary document's BOM and CRLF bytes exactly. Universal ZIP/DMG packaging and integrity checks pass locally. Finder Open With and drag/drop still need a manual end-to-end check.

GPU follow-up: replaced softbuffer and the CPU pixel loops with wgpu/Metal. The renderer submits ordered, instanced glyph and rectangle quads, reuses its instance buffer and uploads glyph masks only on atlas misses. Atlas storage grows lazily to at most eight 1024×1024 R8 pages and evicts pages only when they are unused by the current frame. Font rasterization caches at most 4,096 glyphs per font instance; unknown characters share the font's missing-glyph entry. Clipping adjusts both geometry and texture coordinates, including negative glyph bearings. Resize reconfigures the surface; outdated/lost surfaces retry, while fatal device or memory errors enter the native error/guarded-close flow after pending file work finishes.

GPU validation: 45 regular Rust tests and two packaging tests pass. The explicitly invoked hardware test also passes on Metal / Apple M5: pixel readback checks opaque drawing, glyph alpha, clipping and paint order; cache checks cover reuse, the page budget and safe eviction. Shader validation runs in regular tests without requiring a GPU. The hardware test is ignored by default because CI runners may lack Metal access. Packaged-app GUI checks covered Ukrainian text, search selection, wrapping, resizing, the active adapter in F3 and guarded quit. Universal ZIP/DMG packaging passes locally. These checks establish correct GPU use, not a measured speedup or keyboard-to-pixel latency result.
