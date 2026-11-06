# Texted handover — 2026-11-06

Start with [README](README.md), the [hardening results](docs/performance-hardening-results.md), and the [spec/plan](docs/performance-hardening-plan.md). The original design in `HANDOVER.md` and `docs/design-spec.md` is historical: the current buffer is Ropey, the GUI renderer is wgpu/Metal, and the GUI transport is asynchronous.

## Current work and delivery

The native-file, GPU and hardening stack ([PRs 1–6](https://github.com/dmytro-yemelianov/texted/pulls?q=is%3Apr+is%3Amerged)) is merged into `main`. The v0.2.0 preview packages this integrated source. See [release delivery](docs/releases.md) and the [release page](https://github.com/dmytro-yemelianov/texted/releases/tag/v0.2.0) for public assets; the tag workflow gates publication on ARM/Intel checks and universal-package verification.

A0–A11 are implemented. The [results](docs/performance-hardening-results.md) record reference-machine CPU percentiles, deterministic work/memory/ordering gates and native acceptance evidence. Historical feature branches remain available for tracing the staged changes; new work starts from `main`.

## Appearance work

[PR #7](https://github.com/dmytro-yemelianov/texted/pull/7) is merged into `main` and adds static theme palettes and editor typography only. `appearance.rs` owns versioned, validated values and optional color overrides; `appearance_ui.rs` owns the modal Style panel; `preferences.rs` coalesces atomic writes off the UI thread. `graphics.rs` loads selectable system faces once at startup and uses a separate fixed UI face. Every font/size/spacing change updates the shared viewport metrics; pixel mode uses integer cells with a Unicode fallback. The default settings file is under Application Support/Texted; use `TEXTED_SETTINGS_PATH` for isolated UI tests. The final feature commit is `549d312`; the merge commit is `bcd3a77`, with the same source tree. ARM/Intel CI, universal packaging, 95 tests and review passed. Native checks confirmed corrected Ukrainian pixel glyphs, punctuation, Style/zoom controls and persistence across restart without changing document bytes. No shader effects were added. These changes are in development builds; the existing `v0.2.0` preview predates PR #7. See README for controls and config.

## Native menus

The `feat/native-menus` branch adds macOS application menus (Texted, File, Edit, Search, View, Window) and a document context menu (right-click / Shift+F10). `menus.rs` owns native items and their enable/check state; callbacks send `AppEvent::Menu` into the existing typed intent path. `context_menu.rs` owns selection hit rules and the pending invocation value. The shell waits for accepted commands before testing the selection or positioning the caret, and cancels a popup request if newer input, document identity or modal state makes it obsolete. Published snapshots now include undo/redo availability. Clipboard and file barriers remain authoritative. winit's default menu and automatic window tabbing are disabled because this is a single-document editor. Local validation: 99 Rust tests, Clippy, formatting, two packaging tests and the explicit Metal test passed. Native checks covered both popup entry points, selection preservation, context deletion and undo, history enablement, Style, single-dispatch Cmd+L and saving an exact UTF-8 fixture. These changes are development-only; no release tag was created.

## Platform framebuffers

`feat/platform-framebuffer` builds on the native-menu branch and adds real fixed-resolution GPU canvases. Style's Screen/Resolution row defaults to Auto by theme, with Native and explicit platform presets. `framebuffer.rs` owns bounded resolutions and presentation/input transforms; `presentation.rs` caches one GPU texture and presents exact texels, with integer scaling and letterboxing. The last successful frame's transform governs clicks, drags, pixel-wheel events and context-menu positions. Compact Style/search/status controls fit the smallest 256×192 canvas. Existing version-1 preferences load with Auto and separate 8 px pixel typography; native typography stays intact. See [framebuffer details and primary references](docs/framebuffer.md). Local validation passed 105 Rust tests, Clippy, formatting, two packaging tests and both explicit Metal tests. Native checks confirmed C64/ZX raster changes, Native bypass, compact controls, pointer mapping, restart persistence and unchanged document bytes. No release tag is created.

## Component map

| Component | Owner / invariant |
|---|---|
| `main.rs`, `app.rs` | Bootstrap and native adaptation/scheduling; no editing algorithms or blocking buffer replies |
| `menus.rs`, `context_menu.rs` | Native actions/state and selection-aware popup invocation; shared intent dispatch |
| `input.rs`, `commands.rs` | Physical-key shortcuts and typed intents, windowless |
| `editing.rs`, `selection.rs`, `mouse.rs` | Canonical editor/drag transitions and selection values; shell mouse state tracks only gestures |
| `buffer_service.rs` | Sole live-buffer owner; bounded admission, completion watermark, published snapshots and fenced bulk commits |
| `search.rs`, `search_ui.rs` | Bounded workers/checkpoints/coverage plus UI fields and paste stamps |
| `clipboard.rs`, `transfers.rs` | Snapshot extraction and child I/O off UI; invocation barrier, write-before-cut and context-fenced completion |
| `documents.rs`, `file_flow.rs` | Native async sheets, background file I/O, saved-root dirty state, command/clipboard barriers |
| `viewport.rs`, `texted-layout` | Visible geometry, scrolling, caret, compact search coverage and pure highlight clipping |
| `framebuffer.rs`, `presentation.rs` | Fixed source canvases, bounded GPU targets and matching presentation/input transforms |
| `editor_view.rs`, `graphics.rs`, `gpu.rs` | Immutable drawing-command preparation, bounded glyph caches, ordered Metal submission |
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
cargo test --release -p texted-app --bin texted metal_ -- --ignored
cargo run --locked --release --bin bench -- --hardening
python3 scripts/package_macos.py
```

The reference machine is Apple M5 / macOS 27.2 / Rust 1.88.0. Benchmark data are CPU times, not input-to-display latency; search-owned storage excludes shared rope roots and RSS. See [results](docs/performance-hardening-results.md) for sizes, sample counts and percentiles. Existing GPU preview instances can contain user work: use a separate bundle ID and generated temporary documents for native checks.

Known limits: scalar-character cells rather than grapheme/complex-script shaping; no IME composition; whole-file rope loading rather than mmap; history bounded by transaction count. Very long contiguous words and exact search counts necessarily scan their input. No additional features are part of the completed hardening scope.
