# Texted

<p align="center"><img src="assets/icon.png" width="144" alt="Texted app icon"></p>

[Download for macOS](https://github.com/dmytro-yemelianov/texted/releases) · [Build status](https://github.com/dmytro-yemelianov/texted/actions/workflows/build.yml) · [Packaging and releases](docs/releases.md)

A minimal Rust plain-text editor with a GPU-rendered GUI and a buffer owned by a separate server thread. The QNX, Ghostty and Zed references in the original design describe inspiration; they are not claims of equivalent architecture or measured display latency.

## Run

```sh
cargo run --release --bin texted -- path/to/file.txt
cargo run --release --bin texted
```

The current GUI uses bundled bitmap fonts and macOS `pbcopy`/`pbpaste`. Running without a path opens an empty document. Open and Save As use native macOS dialogs. Opening another file, creating a document or quitting prompts to save unsaved edits. Files can also be dropped onto the window or opened through Finder's Open With menu; Texted edits one document at a time.

## Implementation

- `texted-buffer`: persistent Ropey tree, indexed byte/character/line conversions, immutable shared snapshots, bounded undo/redo (1,000 transactions), and grouped consecutive character input. Block operations and replacements are individual transactions.
- `texted-kernel`: reusable typed request/reply and notification primitives, retained for their tests and benchmarks. The GUI uses the bounded asynchronous `buffer_service` instead of blocking RPC.
- `texted-layout`: reads only visible rope slices; wrapping, caret placement, hit-testing, and highlight clipping share the same visual-row geometry. The viewport stores a physical top line and a wrapped subrow, so scrolling does not reflow preceding document lines.
- `texted-io`: streaming UTF-8 loading, internal LF normalization, BOM/newline metadata, and buffered atomic saves through a unique sibling file. Saves flush and sync the file and, on Unix, its parent directory. Existing permissions are preserved.
- `texted-app`: winit/wgpu shell using Metal on macOS. `graphics.rs` builds ordered drawing commands; `gpu.rs` draws instanced rectangles and glyphs from a bounded texture atlas. Search has one active job, one latest pending job and a bounded result mailbox; it counts matches without storing every offset. Bulk edits and replacements prepare snapshot plans outside the buffer owner and commit only against the same document, revision, cursor and selection. Native dialogs are asynchronous; file loading and saving run in background workers. The saved rope root tracks unsaved edits, including undo back to the saved state.

Snapshots do not flatten the document. Search streams across rope chunks. Text compositing, backgrounds, selection and the caret render on the GPU. Font outlines are rasterized on the CPU on cache misses and uploaded once per atlas entry. Atlas coverage is limited to eight 1 MiB pages, with eviction between frames; instance buffers are reused. There is no full-window CPU pixel buffer. A Metal-capable Mac is required; startup reports an error if GPU initialization fails.

## Controls

Development builds include native **Texted / File / Edit / Search / View / Window** menus. Right-click the document, or press **Shift+F10**, for editing and Style actions. Right-clicking inside a selection preserves it; clicking outside positions the caret first. Unavailable actions are disabled, and View options reflect the current editor state. Native Window actions include minimize, zoom and fullscreen. The published v0.2.0 preview predates these menus.

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
| Appearance / styles | Cmd+, or the Style button |
| Context menu | Right-click / Shift+F10 |
| Editor font size | Cmd++ / Cmd+- / Cmd+0 (reset size) |
| Diagnostics | F3 |

Physical letter keys keep shortcuts working with Ukrainian keyboard layouts. Wrapping currently breaks at character-cell boundaries. Text uses character columns rather than grapheme clusters or shaped display cells; complex-script shaping and IME composition remain future work. No-wrap mode supports the trackpad horizontal axis, Shift+wheel, scrollbar dragging and page clicks. Navigation and typing keep the caret visible; manual scrolling leaves its document position unchanged.

In Native mode, the status bar counts Unicode scalar values and reports UTF-8 buffer bytes. The byte count excludes the BOM and uses normalized LF, so it can differ from the saved file's size. Encoding and line-ending labels describe the save format.

## Themes and typography

Available in `main` development builds; the published v0.2.0 preview predates this feature.

Appearance offers Modern Dark/Light, VGA, EGA, Atari, Commander, C64, ZX Spectrum, Amber and Green CRT palettes. The theme applies to the document, selection/search highlights, gutter, status, search and appearance panels. Palettes and editor fonts are independent.

Open **Style** in the status bar or press **Cmd+,**. Click a field to open its dropdown list, or use Tab/Shift+Tab to focus a field and Enter/Space to open it. Up/Down selects a list item; Enter applies it. Escape or an outside click dismisses the list without changing the value; a second Escape closes Style. Long lists support wheel scrolling, Page Up/Down and Home/End. Choosing a value applies it immediately; Done closes the panel. Choose IBM BIOS 8×8, IBM EGA 8×14, IBM VGA 8×16, Spleen 6×12 or Spleen 8×16. Every face includes the complete Ukrainian alphabet. Sizes are whole multiples of the native cell height, up to 64 px, with 100–180% line height. Text uses exact one-bit bitmap masks with nearest-texel sampling, including UI labels (VGA 8×16 in Native mode; BIOS 8×8 inside framebuffers). Unsupported characters show an explicit missing-glyph box. See the [font review and glyph sheets](docs/bitmap-font-review.md). Settings do not edit the document or its undo history.

Style also offers a **Screen / Resolution** row. **Auto (theme)** renders retro palettes into a real limited-resolution framebuffer: VGA 640×480, EGA 640×350, Atari/C64 320×200, Commander 720×400, ZX Spectrum 256×192, and monochrome 720×350. Modern themes stay native. Choose **Native** for a full-resolution palette, or select an explicit raster independently of the theme. The complete in-app scene scales to the largest size that fits the window, preserving the source aspect ratio with letterboxing and nearest-texel sampling. Fractional upscale factors are supported. Compact Style/search/status controls remain usable at 256×192. Native and framebuffer typography are saved separately; framebuffers start with IBM BIOS 8×8 at 8 px. See [framebuffer behavior and reference platforms](docs/framebuffer.md).

Preferences save atomically in the background to `~/Library/Application Support/Texted/appearance.json`. The latest accepted settings are flushed on normal exit. An invalid or newer-version file is left intact; the app uses session-only defaults and reports the load error. `TEXTED_SETTINGS_PATH` selects an alternate file for isolated previews.

For custom colors, edit that file while Texted is closed; for example:

```json
{
  "version": 1,
  "theme": "c64",
  "framebuffer": "auto",
  "pixel_typography": { "family": "bios_8x8", "size_px": 8, "line_spacing": 100 },
  "typography": { "family": "spleen_8x16", "size_px": 16, "line_spacing": 120 },
  "colors": { "text": "#E8DCFF", "selection": "#6454A0" }
}
```

Optional `colors` keys are `bg`, `text`, `caret`, `selection`, `block_selection`, `search_match`, `search_active`, `gutter_bg`, `gutter_border`, `gutter_text`, `panel_bg`, `panel_border`, `input_bg`, `input_border`, `input_active`, `button_bg`, `button_border`, `status_bg`, `status_border`, `status_text`, and `status_accent`. Values must be `#RRGGBB`; unknown keys are rejected. Selecting another palette clears color overrides; typography changes retain them. Reset defaults restores the built-in appearance. All five faces are embedded in the executable. Older `courier`, `menlo`, `monaco` and `pixel` preferences migrate to bitmap faces with valid native sizes. Fixed framebuffers use a nearest-texel presentation pass. CRT effects remain a separate future change.

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# Hardware rendering test, explicitly run on a Mac with a Metal GPU:
cargo test -p texted-app metal_ -- --ignored --nocapture
cargo run --release --bin bench
cargo run --release --bin bench -- --hardening
```

The benchmark reports CPU operation times and percentiles, never keyboard-to-pixel latency. F3 reports UI command preparation time, layout time, document bytes, snapshot storage type and the active GPU/backend. These CPU times exclude GPU execution, submission and display scanout; the HUD does not report display FPS or RAM usage.

See [performance notes](docs/performance.md) for the audit, measurements and remaining limits. [The original design](docs/design-spec.md) is historical planning material.

The [hardening results](docs/performance-hardening-results.md) record implementation, benchmarks and acceptance evidence for the [specification](docs/performance-hardening-spec.md) and [staged plan](docs/performance-hardening-plan.md).

`main.rs` bootstraps the app; `app.rs` adapts native events. `input`/`commands` decode intents, `editing`/`selection` own canonical transitions, `search` owns bounded scan jobs, and `editor_view` paints immutable snapshots. `file_flow` and `transfers` enforce save/close and clipboard barriers. Redraw never waits for a buffer reply.

Search queries are limited to 64 KiB and auxiliary search storage to 8 MiB. Generated edit growth and clipboard text are limited to 128 MiB. The buffer accepts at most 64 undrained commands; saturation is visible in the window title. Clipboard operations have a five-second deadline. Escape cancels pending bulk/clipboard work. New edits can invalidate a worker plan; its conflict is shown without overwriting the newer content. Save, New, Open and Close wait for earlier accepted mutations and clipboard continuations.
