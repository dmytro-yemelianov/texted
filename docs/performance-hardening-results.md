# Hardening acceptance — 2026-11-05

A0–A11 are implemented and merged into `main`, after the native-file and GPU changes. The review sequence was [viewport highlights](https://github.com/dmytro-yemelianov/texted/pull/3), [bounded search](https://github.com/dmytro-yemelianov/texted/pull/4), [controller extraction](https://github.com/dmytro-yemelianov/texted/pull/5), then [asynchronous integration](https://github.com/dmytro-yemelianov/texted/pull/6). All stages passed ARM/Intel checks and universal delivery before integration. The [v0.2.0 preview](https://github.com/dmytro-yemelianov/texted/releases/tag/v0.2.0) packages the integrated source, with the tag workflow repeating validation before publication.

## Reference run

Implementation `6d862a6` (subsequent `5bd747f` changes only search-panel geometry), Apple M5, macOS 27.2 / 26B5086k, rustc 1.88.0. Release build, three warmups and 30 samples per workload. Fixture generation is outside the timer. The desktop was active; particularly the 64 MiB count has scheduling variance. These are CPU preparation/worker/admission measurements, excluding GPU execution and display latency.

```sh
cargo run --locked --release --bin bench -- --hardening
# Optional frozen legacy highlight/copy comparison:
cargo run --locked --release --bin bench -- --hardening --legacy
```

| Workload | p50 | p95 | p99 |
|---|---:|---:|---:|
| Highlight, column 0 | 13.750 µs | 15.625 µs | 17.000 µs |
| Highlight, column 100,000 | 19.166 µs | 20.042 µs | 22.042 µs |
| Highlight, column 1,000,000 | 17.041 µs | 24.292 µs | 46.500 µs |
| Deep wrapped highlight | 514.334 µs | 570.584 µs | 577.125 µs |
| Exact count, 2,000,000 dense bytes | 4.806 ms | 5.066 ms | 5.229 ms |
| Exact count, 64 MiB dense bytes | 170.874 ms | 270.972 ms | 300.336 ms |
| 10,000-row core block insertion, 2,048 columns | 18.603 ms | 23.270 ms | 24.057 ms |
| 10,000-row core extraction, 32 columns | 6.279 ms | 6.951 ms | 7.071 ms |
| Snapshot read + async command admission during block work | 42 ns | 83 ns | 84 ns |
| 2 MB paste into isolated buffer | 442.542 µs | 509.625 µs | 686.667 µs |
| Short word at start of 10M-character line | 208 ns | 209 ns | 250 ns |
| Short word near end of 10M-character line | 333 ns | 375 ns | 417 ns |

The admission sample includes the published-snapshot clone and enqueueing a prepared typed command, not native key decoding, worker completion or drawing. All 33 timed/warmup admissions observed an active block worker. Ordinary commands remain ordered; intervening input can invalidate its result. The core block timing is useful for throughput and is not a UI-thread stall in the integrated app.

## Acceptance evidence

| Finding | Result and regression evidence |
|---|---|
| H1 | Exactly 81 candidates / row visits for 81 visible matches at all three columns, versus the audited million-match prefix walk (324–370 ms). Deep wrapping, multiline spans, UTF-8 edges and stale revisions are covered. Both p95 highlight cases meet the 1 ms target. |
| H2 | Dense 2 MB search reports 16,627 B auxiliary storage; dense 64 MiB search reports 131,315 B (128.24 KiB), below 8 MiB. The legacy 2 MB offset vector alone retained 33,554,432 B; a dense 64 MiB vector would retain at least 1 GiB. Rope roots and process RSS are separate from these counters. Scanner checkpoints preserve global non-overlap state; cancellation checks every 4,096 bytes, below the 64 KiB gate. Tests cover generation supersession, bounded queues/cache release, exact counts, repeated navigation and worker failure. Replacements stream into a rope, with randomized reference checks, cancellation, growth bounds and one-Undo commits. |
| H3 | The UI reads published snapshots and submits commands without reply waits. A gated worker test proves ordinary input and snapshot reads continue before worker release, completion barriers wait across out-of-order outcomes, and stale roots cannot overwrite new input/documents. The 10,000-row service test commits once and undoes to the original rope root. Clipboard tests cover slow/failing/cancelled backends, one-request admission, late success after cancellation and terminating/reaping an expired pipe child. Context tests reject cut/paste into changed documents. A keyboard Save/Close queue test waits through clipboard capture and its accepted paste continuation; native Save includes prior accepted edits; cancelled Save As retains dirty content and guarded quit. Admission p95 is below 2 ms. |
| H4 | `main.rs` is 59 lines; native `event` + `window_event` routing is under 100 lines. The shell delegates editing, scanning, file lifecycle and painting. The larger `editing`, `search`, `buffer_service` and `editor_view` modules each own their named domain; they are not a relocated event-handler method. Input, selection, scanner and service tests need no native window. The temporary synchronous GUI buffer host and replacement-worker adapter were deleted. |
| H5 | Word selection/dragging uses indexed bidirectional rope traversal, without whole-line `String` or `Vec<char>` copies. The 10M-character fixture inspects two characters at the start and three near the end. Unicode, chunk boundaries, punctuation, whitespace and drag direction/jitter are covered. Both p95 results are below 0.5 ms. |

## Integrated verification

Formatting and Clippy with warnings denied pass. The workspace has 81 passing regular Rust tests, plus the explicitly run Metal hardware test; both Python packaging tests pass. The Metal test verifies clipping, glyph alpha and paint order on Apple M5. The normal suite also validates the shader without requiring hardware. ARM and Intel CI repeat the workspace/packaging checks before producing a universal app, ZIP, DMG, manifest and checksums.

Native checks use only an isolated preview and generated temporary files: clean startup, Unicode paste/text, exact dense search, Next advancing once, Replace All with one Undo, native Open, save ordering, byte-for-byte BOM/CRLF preservation, cancelled Save As, guarded quit, wrapped caret snapping at column 2,000,001, no-wrap caret tracking/scrollbar, word selection, native window zoom/resize, and the Metal adapter in F3. Counts now have room before the panel buttons, and hit tests share their painted coordinates.

## Ownership and remaining limits

- `main` registers startup; `app` adapts events and schedules controllers.
- `input`/`commands` decode user intents; `editing` owns the canonical selection/drag transitions and live editing semantics; `selection` is its value type.
- `buffer_service` owns the live model, a 64-command admission budget, ordered completion watermark, immutable publication and one bulk worker. Plans commit only against their original document/revision/cursor/selection. Conflicts and rejected requests remain visible in the window title.
- `search` owns one active/latest-pending/latest-result scan, bounded checkpoints, viewport coverage and generation fences. `search_ui` owns field focus/stamps; it cannot mutate the buffer directly.
- `transfers` captures clipboard invocation context after prior edits; `clipboard` extracts/transfers off-thread, with a five-second deadline and 128 MiB limit. Cut submits deletion only after successful transfer; paste and cut carry context fences.
- `file_flow` waits for prior commands and clipboard continuations before the existing native document guard or save snapshot. Saves during later editing retain the exact written root, so later changes remain dirty.
- `viewport` owns scrolling/caret pacing; `editor_view` prepares drawing commands from immutable models; the existing GPU renderer submits them.

A long contiguous word still requires scanning its run. Exact search counting scans the document in the background. Search auxiliary limits exclude retained shared rope roots and application/GPU memory. History is bounded by transaction count, not a byte quota. Generated growth and clipboard input are capped at 128 MiB; saturation, conflicts and cancellation are explicit outcomes. The current text model remains Unicode scalar columns without grapheme shaping or IME composition, and file loading still builds the complete rope. The kernel's reusable blocking primitives remain tested/benchmarked but are no longer the GUI transport.
