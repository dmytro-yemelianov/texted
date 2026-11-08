# Shared cell scene and terminal delivery

Both backends run the same library `App`, sessions, input commands, editor view, tabs, command panel, Appearance, search, details and dialog controllers. There is no separate terminal widget implementation.

`graphics::DrawList` retains source Unicode scalars, colors, bounds, borders and cursor operations before native glyph rasterization. The window backend renders its existing one-bit masks with wgpu/Metal. `Renderer::Cells` projects that scene into a bounded `CellGrid`; `AnsiWriter` sends only changed cells and cursor state. Glyphs are not reconstructed from screenshots or bitmap masks. Terminal startup creates no winit window, NSApplication event loop or GPU device. Both executables currently share the same compiled dependency set.

## Run

```sh
cargo run --locked --release --bin texted-tui -- path/to/file.txt
cargo run --locked --release --bin texted -- --terminal first.txt second.txt
# Installed development package:
/Applications/Texted.app/Contents/MacOS/texted-tui path/to/file.txt
```

A terminal of at least 40 columns × 24 rows is required. Resizing below this temporarily pauses input and asks to enlarge the terminal; resizing back repaints the full scene. Normal exit, initialization failure, I/O errors and Rust panic unwinding restore raw mode, mouse capture, bracketed paste, line wrapping, colors and the alternate screen. SIGKILL cannot run cleanup.

F2 saves; F3 finds; F4 replaces; F5 opens; F6 creates a tab; F7/F8 switch tabs; F9 opens Appearance; F10 closes a tab. Ctrl+Q quits. Ctrl/Alt/Shift change the same contextual F-key actions as in the window. Ctrl+letter shortcuts use the existing decoder; standard Ukrainian/Latin key positions are mapped when legacy terminals supply characters. Mouse clicks, selection drags, wheel scrolling, context menus and bracketed Unicode paste feed the existing handlers.

The transport requests the extended keyboard protocol for modifier/release events. Supporting terminals can update modifier layers while keys are held; legacy terminals show a short modifier preview on received combinations and then clear it. Some terminal/OS shortcuts are intercepted before reaching the application. Copy/paste through the application uses the existing macOS clipboard backend; terminal-emulator paste also works through bracketed paste.

Theme colors apply live. Font, text scale, line spacing, framebuffer resolution and pointer shape are controlled by a plain terminal's emulator; the shared Appearance panel marks these settings accordingly and omits unavailable scale commands. Window font specimens, the full Verba bitmap library and GPU effects remain available in the window backend. Terminal preferences use `terminal-appearance.json` beside window preferences; an explicit `TEXTED_SETTINGS_PATH` overrides either path for isolated runs.

## One set of dialogs

`dialog_ui.rs` paints the same draggable Open/Save file picker, overwrite confirmation, error and Save/Discard/Cancel forms in both modes. The field uses `EditorModel` editing, selection, undo and clipboard fences. Enter accepts; Escape cancels; Tab changes focus; arrows browse directories or confirmation choices. F2 accepts the focused action, F3/F4 navigate, F10 cancels. Directory inspection runs off-thread, limits visible entries to 1,000, and accepts a full path for entries outside that list. A changed path invalidates a pending submission's inspection result. Existing FileFlow edit/clipboard/save/close barriers remain authoritative.

`context_menu.rs` preserves selection/caret barriers and now draws and hit-tests its popup through the shared scene. Arrow keys navigate, Enter invokes, Escape dismisses. The macOS application menu remains a native adapter to the same intents, including field editing while a dialog is active.

## Cell-grid interchange

```sh
./target/release/texted --dump-grid 80x24 README.md > frame.json
./target/release/texted-tui --dump-grid 100x30 first.txt second.txt > frame.json
```

This loads the requested files through the common asynchronous file flow and emits the actual active UI frame, without entering raw mode. Initial files open in separate tabs; the last requested tab is active. Errors/timeouts produce a nonzero exit. JSON contains:

- `version: 1`, `columns`, `rows`;
- row-major `cells`, each with one `symbol`, `foreground` and `background` as `0xRRGGBB` integers;
- nullable `cursor: {column, row}`, using zero-based coordinates.

Frames are limited to 1,048,576 cells and validated before ANSI output. Characters and colors survive scene translation and clipping; opaque modal fills erase covered text. Thin native decoration never erases an entire character. Modal/search layers explicitly own cursor visibility. ANSI output treats document text as data: controls cannot inject terminal commands.

The editor currently uses one Unicode scalar per cell in both modes. Plain terminals give some scalars zero or two columns, so ANSI output substitutes a visible U+FFFD for non-single-width scalars; the grid JSON and document bytes preserve the original characters. Ordinary Ukrainian letters occupy one cell. Grapheme shaping and double-width terminal layout are outside this change.

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --bin texted --bin texted-tui
python3 scripts/smoke_terminal.py --binary target/release/texted-tui
cargo test --locked --release -p texted-app --lib metal_ -- --ignored
python3 -m unittest discover -s scripts -p 'test_*.py'
```

The PTY test edits Ukrainian text, saves an existing file, creates a second tab, cancels dirty close, saves through the common path form, opens the common context menu, resizes and quits. A parent process independently reads restored termios after the editor exits. Native GUI acceptance also covers Save As, Select All in the path field, Cancel on quit, Save on quit and actual process exit. CI runs the PTY test on both macOS architectures. Packages include and verify universal GUI and terminal executables, including a cell-grid smoke check after ZIP extraction.
