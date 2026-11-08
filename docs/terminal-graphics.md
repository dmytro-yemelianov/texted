# Bitmap fonts through terminal graphics

The terminal can render Texted's own bitmap fonts, scale and fixed-resolution profiles without changing its configured font. `--render=auto` selects Kitty graphics after a successful protocol query and valid pixel dimensions; otherwise it uses the existing ANSI scene. `--render=kitty` requires confirmation and reports an error if unavailable. `--render=ansi` skips graphics negotiation.

```sh
/Applications/Texted.app/Contents/MacOS/texted-tui --render=auto file.txt
/Applications/Texted.app/Contents/MacOS/texted-tui --render=kitty file.txt
# Equivalent entry point:
/Applications/Texted.app/Contents/MacOS/texted --terminal --render=kitty file.txt
```

F9 opens the same Appearance panel as the window backend. Editor font exposes the full local Verba catalog with bitmap previews and disabled entries. Arrow navigation previews; Enter commits; Escape restores an uncommitted choice. Ctrl+= / Ctrl+- change text scale by 10%, Ctrl+0 resets it. Resolution, theme and line-height choices work too. The mouse pointer remains terminal-managed. Font changes affect the editor's own column/row geometry, independently of terminal character-cell dimensions.

## Rendering and transfer

`raster.rs` consumes the same clipped `graphics::DrawList` as Metal. It composites cached one-bit glyph masks and ordered rectangles into RGB pixels, samples with nearest-neighbor rules, and presents reference framebuffers with the existing aspect-fit/letterbox mapping. It creates no native window or GPU device. Source text, sessions, selection, search, dialogs and menus still belong to the common App.

`kitty.rs` partitions the final image into rectangles of 8 terminal columns × 4 rows, with smaller edge tiles. Only changed tiles are sent. Each uses compressed RGB, direct base64 transmission and chunks of at most 4,096 bytes. Images are placed at cell-aligned origins at their final pixel size, avoiding terminal-side font rasterization and image scaling. This works without a shared filesystem. Full frames are repainted on resize, invalidation and focus return. Image ownership is tracked in a bounded ordered set; updates, resizing, suspension and normal/error/panic cleanup delete only the session's allocated IDs. Output failures invalidate the previous-frame cache.

The transport follows the [Kitty graphics specification](https://sw.kovidgoyal.net/kitty/graphics-protocol/): basic image query, raw RGB with zlib compression, explicit placement size, no cursor movement, suppressed per-image responses and synchronized update brackets. A terminal implementing these features is required; Ghostty 1.4.0 was checked locally. Direct SSH transports need no image files. A multiplexer must forward the protocol; if negotiation is filtered, Auto falls back to ANSI.

## Input and geometry

`terminal_input.rs` is the single owner of Unix TTY input. It incrementally separates graphics replies, pixel-size replies, capability replies, keys, extended keyboard events, mouse events, focus and bracketed paste. Query responses and unknown control strings cannot become document text. Escape disambiguation applies only outside paste. Control strings are bounded; paste uses the existing 128 MiB limit. No crossterm event reader competes for bytes; crossterm still supplies terminal setup and shared event values.

The client prefers exact cell-pixel reports (`CSI 16 t`) over window-size estimates (`CSI 14 t` or ioctl), preventing titlebar/margin values from overriding precise geometry. Window/cell dimensions are rechecked on resize and DPI changes. Pixel mouse reporting (1016) is enabled only after a positive mode-support response; otherwise events use terminal cell centers. Pixel mode can target glyphs smaller than a terminal cell. Physical mouse behavior on terminals without pixel reports retains cell granularity.

Frames are limited to 16 megapixels and 32,768 tiles; the existing terminal cell limit also applies. Windows below 40×24 or exceeding the raster budget suspend display/input, reclaim images and show a resize message. Enlarging/shrinking back into the supported range repaints the preserved editor state. The terminal does not exit and discard edits on an oversized resize.

Terminal graphics preferences use `terminal-graphics.json`; ANSI uses `terminal-appearance.json`; window preferences remain separate. An explicit `TEXTED_SETTINGS_PATH` is honored in all modes. Raw mode, keyboard flags, mouse/paste/focus reporting, colors and alternate-screen state are restored on normal return, initialization/I/O errors and Rust panic unwinding. Forced OS termination cannot run cleanup.

## Verification and measured limits

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --bin texted --bin texted-tui
python3 scripts/smoke_terminal.py --binary target/release/texted-tui
python3 scripts/smoke_terminal_graphics.py --binary target/release/texted-tui
cargo test --locked --release -p texted-app --lib metal_ -- --ignored
cargo test --locked --release -p texted-app --lib profile_terminal_frames -- --ignored --nocapture
```

The graphics PTY peer decodes compressed tiles, selects EGA through the common Appearance picker using both pixel and cell mouse paths, verifies independent 110% zoom, saves Ukrainian text, resizes and suspends/resumes the editor, and checks image reclamation and restored termios. A peer that never confirms graphics must fail cleanly in forced mode. The ANSI PTY scenario also exercises automatic fallback. Unit tests cover byte-fragmented replies, opaque paste (including delayed Escape), bounds, exact bitmap colors, clipping, letterboxing, no-op frames, single-tile updates, chunk decoding and resize cleanup. Native Ghostty QA confirmed VGA → Spleen 6×12 and 110% scale while the terminal font/cell configuration stayed unchanged. GPU readback checks remain in the library target.

On Apple M5, the opt-in release profile measured CPU raster/comparison/compression over 100 caret updates using a cached scene:

| Final size | CPU per frame | First image payload | Caret update payload |
|---|---:|---:|---:|
| 800×480 | ~0.63 ms | 84,688 bytes | ~1,180 bytes |
| 1920×1072 | ~1.19 ms | 220,008 bytes | ~1,180 bytes |

Payload sizes count base64 data, excluding escape headers. Measurements exclude editor layout/input, terminal decoding/GPU work, network transfer and input-to-display latency. The CPU compositor processes the visible frame; it does not imply a full-frame transfer per keystroke. Graphics output is an image: terminal-native text selection/accessibility cannot recover document characters from its pixels. Use Texted's selection, copy and search. The editor's existing scalar-cell/grapheme limitations remain unchanged.
