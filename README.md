# Texted

<p align="center"><img src="assets/icon.png" width="144" alt="Texted app icon"></p>

[Download for macOS](https://github.com/dmytro-yemelianov/texted/releases) · [Build status](https://github.com/dmytro-yemelianov/texted/actions/workflows/build.yml) · [Packaging and releases](docs/releases.md)

A minimal Rust plain-text editor with a GPU-rendered GUI and a buffer owned by a separate server thread. The QNX, Ghostty and Zed references in the original design describe inspiration; they are not claims of equivalent architecture or measured display latency.

## Run

```sh
cargo run --release --bin texted -- path/to/file.txt
cargo run --release --bin texted
```

The current GUI uses macOS fonts and `pbcopy`/`pbpaste`. Running without a path opens an empty document. Open and Save As use native macOS dialogs. Opening another file, creating a document or quitting prompts to save unsaved edits. Files can also be dropped onto the window or opened through Finder's Open With menu; Texted edits one document at a time.

## Implementation

- `texted-buffer`: persistent Ropey tree, indexed byte/character/line conversions, immutable shared snapshots, bounded undo/redo (1,000 transactions), and grouped consecutive character input. Block operations and replacements are individual transactions.
- `texted-kernel`: typed blocking request/reply channels and a separate FIFO notification channel preferred over requests. The buffer server blocks while idle. Document edits use one ordered request channel.
- `texted-layout`: reads only visible rope slices; wrapping, caret placement, hit-testing, and highlight clipping share the same visual-row geometry. The viewport stores a physical top line and a wrapped subrow, so scrolling does not reflow preceding document lines.
- `texted-io`: streaming UTF-8 loading, internal LF normalization, BOM/newline metadata, and buffered atomic saves through a unique sibling file. Saves flush and sync the file and, on Unix, its parent directory. Existing permissions are preserved.
- `texted-app`: winit/wgpu shell using Metal on macOS. `graphics.rs` builds ordered drawing commands; `gpu.rs` draws instanced rectangles and glyphs from a bounded texture atlas. Search and replacements run on snapshots in background threads. Replacement commits reject a changed document/cursor. Native dialogs are asynchronous; file loading and saving run in background workers. The saved rope root tracks unsaved edits, including undo back to the saved state.

Snapshots do not flatten the document. Search streams across rope chunks. Text compositing, backgrounds, selection and the caret render on the GPU. Font outlines are rasterized on the CPU on cache misses and uploaded once per atlas entry. Atlas coverage is limited to eight 1 MiB pages, with eviction between frames; instance buffers are reused. There is no full-window CPU pixel buffer. A Metal-capable Mac is required; startup reports an error if GPU initialization fails.

## Controls

| Action | Shortcut |
|---|---|
| Copy / cut / paste | Cmd+C / Cmd+X / Cmd+V |
| Undo / redo | Cmd+Z / Cmd+Shift+Z |
| New / open | Cmd+N / Cmd+O |
| Save | Cmd+S |
| Save As | Cmd+Shift+S |
| Close / quit | Cmd+W / Cmd+Q |
| Find / replace | Cmd+F / Cmd+R |
| Next / previous match | Cmd+G / Cmd+Shift+G |
| Toggle wrapping | Option+Z |
| Horizontal scrolling (wrap off) | Trackpad / Shift+wheel / scrollbar |
| Block selection | Option+drag or Cmd+B |
| Toggle line numbers | Cmd+L |
| Diagnostics | F3 |

Physical letter keys keep shortcuts working with Ukrainian keyboard layouts. Wrapping currently breaks at character-cell boundaries. Text uses character columns rather than grapheme clusters or shaped display cells; complex-script shaping and IME composition remain future work. No-wrap mode supports the trackpad horizontal axis, Shift+wheel, scrollbar dragging and page clicks. Navigation and typing keep the caret visible; manual scrolling leaves its document position unchanged.

The status bar counts Unicode scalar values and reports UTF-8 buffer bytes. The byte count excludes the BOM and uses normalized LF, so it can differ from the saved file's size. Encoding and line-ending labels describe the save format.

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# Hardware rendering test, explicitly run on a Mac with a Metal GPU:
cargo test -p texted-app metal_renders_clipping_alpha_and_paint_order -- --ignored --nocapture
cargo run --release --bin bench
```

The benchmark reports CPU operation times and percentiles, never keyboard-to-pixel latency. F3 reports UI command preparation time, layout time, document bytes, snapshot storage type and the active GPU/backend. These CPU times exclude GPU execution, submission and display scanout; the HUD does not report display FPS or RAM usage.

See [performance notes](docs/performance.md) for the audit, measurements and remaining limits. [The original design](docs/design-spec.md) is historical planning material.

The next hardening work is defined in the [performance and architecture specification](docs/performance-hardening-spec.md) and its [staged implementation plan](docs/performance-hardening-plan.md).
