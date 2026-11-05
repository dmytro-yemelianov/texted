# Texted: Ultra-Minimal, High-Performance Microkernel Text Editor

## Executive Vision

**Texted** is a distraction-free, ultra-low-latency, minimal GUI text editor written in Rust. It captures the clean, instant, no-nonsense utility of **Windows Notepad** while adopting the cutting-edge performance engineering of **Ghostty** and **Zed**, organized around a **QNX-inspired microkernel architecture**.

---

## Core Philosophy

1. **Notepad Simplicity, Not TextEdit Bloat**:
   - Strictly plain text (UTF-8, LF/CRLF auto-detection, optional BOM preservation).
   - No rich text, no RTF conversions, no hidden document wrappers, no formatting inspectors.
   - Clean UI: text surface, line numbers (toggleable), minimal status bar (Line/Column, Encoding, Line Endings, Zoom), lightweight menu/command palette.

2. **Ghostty & Zed Performance DNA**:
   - **Ghostty-grade Keystroke Latency**: Under 2–4ms end-to-end keystroke-to-pixel display. Direct display-link synchronisation (120Hz/240Hz ProMotion / VRR).
   - **Zed-grade Buffer Architecture**: Chunked B-Tree / Piece Tree (`SumTree`) for $O(\log N)$ edits and instant opening of multi-gigabyte files with virtual memory paging (`mmap`).
   - **GPU-Accelerated Quad Batching**: Dedicated GPU rendering pipeline (`wgpu` / native platform backends) using instanced glyph quads backed by a dynamic LRU texture atlas.

3. **QNX Microkernel Architecture**:
   - The editor core is an ultra-small, deterministic **Microkernel IPC Bus**.
   - Subsystems (Buffer Engine, GPU Renderer, Layout/Typography, Input Dispatcher, File I/O, OS Window Shell) run as isolated **Resource Servers**.
   - Servers communicate strictly via typed message passing:
     - **Synchronous Rendezvous** (`MsgSend` / `MsgReceive` / `MsgReply`)
     - **Asynchronous Pulses** (low-latency 64-bit atomic notifications for frame ticks, raw input, dirty rects)
     - **Zero-Copy Shared Memory Handles** (virtual memory slice descriptors for buffer snapshots)
   - Fault isolation: a GPU crash or font rasterizer panic will not kill the buffer or lose unsaved edits. The Render Server restarts and reconnects seamlessly.

---

## Architectural Blueprint

```
+-------------------------------------------------------------------------+
|                           OS Platform / Shell                           |
|      (winit / raw-window-handle / CVDisplayLink / DXGI / Wayland)       |
+-------------------------------------------------------------------------+
                                    |
                             [Raw OS Events]
                                    v
+-------------------------------------------------------------------------+
|                           INPUT SERVER                                  |
|   - Raw event normalization        - IME composition state              |
|   - Keybinding resolution          - Mouse / scroll gestures            |
+-------------------------------------------------------------------------+
          | (Pulse: PULSE_KEY_INPUT / ActionMsg)
          v
+=========================================================================+
|                         TEXTED MICROKERNEL CORE                         |
|   - Deterministic message router   - Pulse queue (lock-free ring)       |
|   - Service registry & discovery   - Shared Memory Arena allocator      |
|   - Transaction & tick scheduler   - Fault supervisor & restart watcher |
+=========================================================================+
    |                     |                          |                 |
    | MsgSend/Reply       | Zero-Copy Snapshot       | Frame Tick      | Async Load/Save
    v                     v                          v                 v
+---------------+  +------------------+  +--------------------+  +-------------+
| BUFFER SERVER |  |  LAYOUT SERVER   |  |   RENDER SERVER    |  |  IO SERVER  |
| - Piece Tree  |  | - Line wrap      |  | - GPU pipeline     |  | - mmap I/O  |
| - Undo/Redo   |  | - Font shaping   |  | - Instanced quads  |  | - UTF audit |
| - Edit log    |  | - Glyph metric   |  | - Atlas cache      |  | - File watch|
| - Cursor state|  |   cache          |  | - VSync displaylink|  | - Backups   |
+---------------+  +------------------+  +--------------------+  +-------------+
```

---

## Subsystem Breakdown

### 1. The QNX-Inspired Microkernel (`texted-kernel`)
QNX's microkernel design is centered on **Synchronous Message Passing** (`MsgSend`, `MsgReceive`, `MsgReply`), non-blocking **Pulses**, and **Channel/Connection ID** management. In Rust, this is implemented as an ultra-fast in-process message exchange with crossbeam ring-buffers and zero-copy shared memory arenas.

```rust
pub struct KernelMessage {
    pub sender: ServiceId,
    pub msg_type: u16,
    pub payload: Payload,
}

pub enum Payload {
    Inline([u8; 48]),                      // Zero heap allocation for 95% of messages
    SharedSlice(ShmSliceHandle),           // Zero-copy reference to memory arena
    ReplyChannel(oneshot::Sender<Response>),// Synchronous rendezvous channel
}

pub struct Pulse {
    pub code: i8,                          // e.g. PULSE_TICK, PULSE_INPUT, PULSE_DIRTY
    pub value: u64,                        // 64-bit packed scalar (e.g. sequence ID or tick timestamp)
}
```

* **No direct cross-service mutations**: No server holds a mutable reference (`&mut`) to another server's state.
* **Deterministic Event Loop**: The microkernel dispatches pulses with priority over general request/reply messages, guaranteeing predictable input responsiveness.

---

### 2. The Buffer Server (`texted-buffer`)
Inspired by Zed's `SumTree` and Windows Notepad's legacy piece table modernization:

* **Data Structure**: A chunked 2-3 B-Tree of memory-mapped or heap-allocated chunks (Piece Tree / Rope).
* **Metrics Indexed in Internal Nodes**:
  - Byte count (`usize`)
  - UTF-8 char count (`usize`)
  - Newline count (`usize`) for $O(\log N)$ line-to-offset and offset-to-line queries.
* **Zero-Copy Snapshots**: Uses structural sharing (copy-on-write). When the Layout or Render Server requests the current viewport:
  1. The Buffer Server returns an immutable `BufferSnapshot` handle.
  2. The snapshot shares nodes with the live tree; mutations in the Buffer Server fork only the modified path ($O(\log N)$).
* **Transaction & Undo Stack**: Grouped edits with timestamped deltas for instant undo/redo without memory ballooning.

---

### 3. The Layout & Typography Server (`texted-layout`)
* **Text Shaping**: Direct bindings to `swash` or `cosmic-text` for fast, lightweight unicode script resolution, font fallback, and bi-di processing.
* **Viewport-Only Incremental Layout**:
  - Only measures and shapes lines visible in the active viewport (plus a 5-line overscan buffer for smooth scrolling).
  - Word wrap uses an incremental cache keyed by `(BufferVersion, ViewportWidth)`.
* **Spatial Coordinate Mapping**:
  - Provides bi-directional mapping between `(Line, Col)` and `(ScreenX, ScreenY)` for cursor placement and mouse selections.

---

### 4. The Render Server (`texted-render`)
Inspired by Ghostty's low-overhead graphics pipeline:

* **Hardware Abstraction**: `wgpu` (with native Metal/Vulkan/DirectX12 targets) or raw platform renderers.
* **Glyph Texture Atlas**:
  - Dynamic GPU texture atlas (LRU cached).
  - Rasterizes glyphs on CPU or GPU compute once, uploads to atlas, and renders text entirely as instanced quads `(x, y, w, h, u, v, color)`.
* **Single Render Pass Quad Pipeline**:
  - Background quads, selection rectangles, cursor bar, and glyph quads are batched into a single vertex/instance buffer.
  - Draw call count per frame: **1 to 2 draw calls max**.
* **Display Sync**:
  - Frame delivery synchronized to OS refresh rate via `CVDisplayLink` (macOS), `CreateWaitableSwapChain` (Windows), or Wayland frame callbacks.
  - Zero idle rendering: If no pulse flags the display as dirty, the render loop consumes 0% CPU and 0% GPU.

---

### 5. The Input & Keybinding Server (`texted-input`)
* **Normalized Key Action Protocol**:
  - Maps physical keys, modifiers, and OS compose sequences into high-level editor intents (`Action::Insert(char)`, `Action::DeleteForward`, `Action::MoveCursor(Direction, Granularity)`).
* **IME Support**:
  - Seamless inline composition with pre-edit strings and candidate window positioning.
* **Fast Path Dispatch**:
  - Keystrokes are bundled into high-priority pulses dispatched directly to the Buffer Server via the Microkernel, triggering speculative local layout updates.

---

### 6. The I/O Server (`texted-io`)
* **Memory-Mapped Loading**: Large files (> 10 MB) are opened via `memmap2`. Initial load is instant ($< 5\text{ms}$) because the OS loads pages on demand.
* **Encoding Detection**: Fast SIMD-accelerated UTF-8 validation (`simdutf8`).
* **Safe Atomic Saves**: Writes to a hidden temporary sibling file and performs an atomic rename to prevent corruption upon power loss.
* **External Watcher**: Uses OS filesystem events (`kqueue`, `ReadDirectoryChangesW`, `inotify`) to detect disk modifications and prompt for reload.

---

## Latency & Performance Budget

| Metric | Target | Traditional Editor (e.g. Electron) | Texted Architecture |
|---|---|---|---|
| **Cold Startup Time** | $< 15\text{ ms}$ | $500 - 2000\text{ ms}$ | Native binary, lazy glyph init, zero GC overhead |
| **Keystroke-to-Pixel Latency** | $< 4\text{ ms}$ | $30 - 80\text{ ms}$ | Direct pulse queue + instanced GPU quad pipeline |
| **Memory Footprint (Idle)** | $< 12\text{ MB}$ | $150 - 400\text{ MB}$ | No browser runtime, flat array buffers, shared atlas |
| **Large File Opening (1 GB)** | Instant ($< 10\text{ ms}$) | Out of memory / freeze | Memory-mapped Piece Tree virtual memory paging |
| **CPU Usage (Idle)** | $0.0\%$ | $1 - 3\%$ | Event-driven wait-on-pulse, no polling loops |

---

## Workspace Structure

```
texted/
├── Cargo.toml                  # Workspace root
├── docs/
│   ├── design-spec.md          # Complete architectural specification & protocol
│   └── microkernel-ipc.md      # QNX-style message passing specifications
├── crates/
│   ├── texted-kernel/          # Core microkernel IPC, scheduler, & shared memory arena
│   ├── texted-buffer/          # Piece Tree, SumTree buffer, snapshot & undo system
│   ├── texted-layout/          # Typography shaper, line-metrics, wrap & coordinate maps
│   ├── texted-render/          # GPU compositor, glyph atlas, quad batching, displaylink
│   ├── texted-input/           # Keybindings, IME handling, raw OS input normalization
│   ├── texted-io/              # Memory-mapped file I/O, UTF-8 validator, atomic persistence
│   └── texted-app/             # Main application shell, server bootstrapper & supervisor
└── tests/
    └── integration/            # Multi-server stress tests and latency benchmarks
```
