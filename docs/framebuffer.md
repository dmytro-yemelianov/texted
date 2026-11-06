# Platform framebuffers

Style's **Screen / Resolution** row selects a fixed GPU canvas. **Auto (theme)** uses the table below. **Native** renders directly at window resolution. Explicit presets work with any palette. The framebuffer controls the document, gutter, selection, search, status and Style panel; macOS menus and file dialogs remain native.

| Auto theme | Canvas | Reference choice |
|---|---|---|
| Modern Dark / Light | Window pixels | Native rendering |
| VGA | 640×480 | VGA graphics raster |
| EGA | 640×350 | EGA high-resolution raster |
| Atari | 320×200 | Atari ST low-resolution color raster |
| Commander | 720×400 | VGA 80-column DOS text raster |
| C64 | 320×200 | VIC-II active display |
| ZX Spectrum | 256×192 | Spectrum active display |
| Amber / Green CRT | 720×350 | MDA-style monochrome text raster |

The raster dimensions follow the original [IBM adapter references](https://bitsavers.org/pdf/ibm/pc/cards/IBM_VGA_XGA_Technical_Reference_Manual_May92.pdf), [Atari ST engineering specification](https://www.temlib.org/AtariForumWiki/index.php/Engineering_Hardware_Specification), [Commodore 64 service manual](https://www.commodore.ca/manuals/funet/cbm/schematics/computers/c64/manual-html/Page_01.html) and [Spectrum display-memory description](https://worldofspectrum.org/ZXBasicManual/zxmanchap24.html). Commander and monochrome themes select representative PC text rasters. Texted retains its own fonts, palettes, editor chrome and character-cell layout. The feature models raster dimensions with square pixels; original ROM fonts, hardware color/attribute restrictions and CRT pixel-aspect correction are outside its scope.

## Presentation and interaction

The scene is laid out and drawn at the selected canvas size. A second GPU pass reads exact source texels with `textureLoad`. Presentation uses the largest whole-number scale fitting the physical window and centers the image on black letterboxing. Windows smaller than the canvas fit the complete image using nearest texels. Resizing the window changes presentation, while document layout stays at the selected resolution.

One shared `ScreenMapping` handles presentation, pointer/drag coordinates, pixel-wheel deltas and native context-menu positioning. Clicks on letterboxing do not select text or activate controls. The last successfully presented mapping remains authoritative until the next successful frame. Changing resolution cancels pending popups and drag gestures, then recomputes viewport/search geometry.

Style and search use shared compact paint/hit geometry in framebuffer mode; every control fits 256×192. The compact status bar shows caret position, wrapping/column-selection mode and canvas dimensions. Cmd+L still hides the gutter when more text columns are needed.

## Settings

Appearance version 1 remains readable. Missing new fields use Auto and an independent Pixel 8x8 / 8 px font. `typography` retains the native-mode font; `pixel_typography` stores the font used inside limited framebuffers. Style edits the currently active font settings. Cmd+0 resets to 8 px in framebuffer mode and 18 px in Native mode.

```json
{
  "version": 1,
  "theme": "c64",
  "framebuffer": "auto",
  "typography": { "family": "menlo", "size_px": 18, "line_spacing": 120 },
  "pixel_typography": { "family": "pixel", "size_px": 8, "line_spacing": 100 }
}
```

Accepted framebuffer values: `auto`, `native`, `vga`, `ega`, `atari_st`, `dos_text`, `c64`, `zx_spectrum`, `mono_text`. All choices save through the existing coalescing atomic preference writer. Resolution/font changes do not modify document contents, undo history or dirty state.

## Resource and validation contracts

`framebuffer.rs` owns presets and pure transforms. `presentation.rs` owns a single cached color target and the presentation pipeline. The largest preset is 640×480: 1,228,800 bytes of RGBA/BGRA texels (driver allocation overhead excluded). Target storage changes only when source dimensions change and is released in Native mode. Production presentation does not read pixels back to the CPU.

Tests cover preset limits, legacy settings and separate typography, integer/fractional presentation/input transforms, letterbox rejection, compact controls and shader validation. The explicit Metal test renders a four-color 2×2 source into a letterboxed 64×32 target and checks every output pixel for its exact expected texel, then changes/releases source targets. Native verification covered C64 and ZX canvases, compact Style/search, Native bypass, transformed context-menu input, persistence after restart and byte-for-byte unchanged UTF-8 save output.

```sh
cargo test --locked --workspace
cargo test --locked --release -p texted-app --bin texted metal_ -- --ignored
```
