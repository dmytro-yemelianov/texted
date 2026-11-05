> Historical design proposal. For the current implementation and measured limits, read [README](../README.md) and [performance notes](performance.md). GPU rendering, mmap and supervision below are proposed work, not implemented guarantees.

# Technical Design Specification: Texted

> **Document Version:** 1.0.0  
> **Target:** Minimal, ultra-performant, QNX-microkernel inspired GUI text editor in Rust  
> **Inspirations:** Windows Notepad (simplicity), Ghostty (latency & GPU rendering), Zed.dev (data structures & concurrency), QNX Neutrino (microkernel & message passing)

---

## 1. System Overview & Philosophy

### 1.1 The "Windows Notepad" Standard
Desktop text editors have bifurcated into two extremes:
1. **Rich Document Editors (e.g. macOS TextEdit):** Encumbered by RTF/rich styling attributes, complex layout engines, and unwanted format conversions.
2. **Heavyweight IDEs / Web-Based Editors (e.g. VS Code, Electron apps):** High cold startup latency (500–2500ms), 150MB+ idle memory footprint, complex configuration matrices, and unpredictable input latency.

**Texted** focuses on the pure Windows Notepad paradigm:
- **Instantaneous utility:** Cold launch in $<15\text{ms}$.
- **Strictly Plain Text:** UTF-8 primary, transparent CRLF/LF handling, optional UTF-16/BOM ingestion.
- **Predictable Keystroke-to-Pixel Response:** Target latency $< 4\text{ms}$ (matching or beating a native terminal like Ghostty).
- **Core Feature Set:** Caret navigation, multi-line selection, undo/redo, search/replace, word wrap toggle, status bar (Line/Col, Encoding, Line Endings, Zoom), and atomic file persistence.

---

## 2. QNX-Inspired Microkernel Architecture

### 2.1 The QNX Neutrino Microkernel Principles
In QNX Neutrino:
- The kernel is ultra-minimal: it handles only thread scheduling, inter-process communication (IPC), interrupt dispatching, and timer management.
- All drivers, filesystems, and display servers run in user space as **Resource Managers**.
- Communication is structured around two primitives:
  1. **Synchronous Request-Reply Rendezvous (`MsgSend`, `MsgReceive`, `MsgReply`):** The sender blocks until the recipient receives the message and issues a reply. This eliminates buffering overhead, avoids queue overflows, and guarantees deterministic flow control.
  2. **Asynchronous Pulses (`MsgSendPulse`):** Fixed-size, non-blocking messages (8-bit code + 64-bit value) for high-priority asynchronous notifications (timer ticks, interrupt signals, input pulses) that never block the sender.

### 2.2 Texted Microkernel Architecture (In-Process Rust Model)

Rather than paying the context-switching penalty of true OS-level processes for a single-binary desktop app, **Texted implements the QNX microkernel paradigm as an in-process, thread-isolated actor microkernel** with memory-safe channels and zero-copy shared memory handles.

```
                      +-----------------------------+
                      |      OS Event Platform      |
                      | (winit / DisplayLink / IME) |
                      +-----------------------------+
                                     |
                          [Raw Input / OS Pulses]
                                     v
                      +-----------------------------+
                      |        INPUT SERVER         |
                      |  - Key & mouse mapping      |
                      |  - IME composition          |
                      +-----------------------------+
                                     |
               Pulse(PULSE_KEY_INPUT, packed_action)
                                     v
+=========================================================================+
|                            TEXTED MICROKERNEL                           |
|                                                                         |
|  [Channels & Connections]      [Pulse Router]      [Shared Memory Arena]|
|  - Channel IDs (ChID)          - Lock-free rings   - Zero-copy Buffer   |
|  - Connection IDs (CoID)       - Priority pulses     View Descriptors   |
|  - Sync Rendezvous Handlers    - Fault Supervisor  - Epoch-based GC     |
+=========================================================================+
      |                       |                      |               |
      | MsgSend/Reply         | Pulse                | Frame Pulse   | MsgSend
      v                       v                      v               v
+--------------+      +----------------+     +--------------+  +-----------+
|    BUFFER    |      |     LAYOUT     |     |    RENDER    |  |    I/O    |
|    SERVER    |      |     SERVER     |     |    SERVER    |  |  SERVER   |
| (SumTree /   |      | (Swash/Cosmic  |     | (wgpu / Metal|  | (memmap2, |
|  PieceTable) |      |  Line Metrics) |     |  Quad Atlas) |  |  atomic)  |
+--------------+      +----------------+     +--------------+  +-----------+
```

### 2.3 Microkernel IPC Protocol Definition

#### A. Channels, Connections, and Messages
```rust
use std::sync::atomic::{AtomicU64, Ordering};

pub type ChannelId = u16;
pub type ConnectionId = u16;

pub const CH_BUFFER: ChannelId = 1;
pub const CH_LAYOUT: ChannelId = 2;
pub const CH_RENDER: ChannelId = 3;
pub const CH_INPUT: ChannelId = 4;
pub const CH_IO: ChannelId = 5;

/// Non-blocking, fixed-size Pulse (like QNX pulse_t)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Pulse {
    pub priority: u8,   // 0 = highest, 255 = lowest
    pub code: i8,       // Negative = system pulse, positive = user pulse
    pub value: u64,     // Packed scalar payload or timestamp
}

pub const PULSE_SYS_FRAME_TICK: i8  = -1;
pub const PULSE_SYS_DIRTY_RECT: i8  = -2;
pub const PULSE_KEY_DOWN: i8        = 10;
pub const PULSE_KEY_UP: i8          = 11;
pub const PULSE_MOUSE_MOVE: i8      = 12;
pub const PULSE_BUFFER_MODIFIED: i8 = 20;

/// Synchronous Request-Reply Envelope (MsgSend / MsgReply)
pub struct MessageEnvelope {
    pub client_coid: ConnectionId,
    pub transaction_id: u64,
    pub payload: MessagePayload,
    pub reply_sender: oneshot::Sender<MessagePayload>,
}

pub enum MessagePayload {
    // Buffer operations
    BufferInsert { offset: usize, text: String },
    BufferDelete { range: std::ops::Range<usize> },
    BufferGetSnapshot,
    BufferSnapshotResult(BufferSnapshotHandle),
    
    // Layout operations
    LayoutViewport { width_px: f32, top_line: usize, visible_lines: usize },
    LayoutResult(ViewportGeometry),

    // I/O operations
    OpenFile { path: std::path::PathBuf },
    SaveFile { path: Option<std::path::PathBuf> },
    IoCompleted { result: Result<(), String> },
}
```

#### B. Synchronous Rendezvous Pattern (`MsgSend` / `MsgReply`)
```rust
impl MicrokernelClient {
    /// Synchronous rendezvous: sends a request to target server,
    /// blocks current thread until target server receives and replies.
    pub fn msg_send(&self, target_chid: ChannelId, payload: MessagePayload) -> MessagePayload {
        let (reply_tx, reply_rx) = oneshot::channel();
        let envelope = MessageEnvelope {
            client_coid: self.coid,
            transaction_id: self.next_tx_id(),
            payload,
            reply_sender: reply_tx,
        };
        
        self.channel_map.send(target_chid, envelope);
        reply_rx.recv().expect("Server rendezvous dropped without reply")
    }

    /// Asynchronous Pulse: non-blocking high-priority signal
    pub fn msg_send_pulse(&self, target_chid: ChannelId, pulse: Pulse) {
        self.pulse_ring.send(target_chid, pulse);
    }
}
```

### 2.4 Fault Isolation and Resilience
In traditional monolithic editors, an uncaught panic in font rasterization or GPU command buffer recording terminates the entire application, losing unsaved edits.

In **Texted**:
1. The **Buffer Server** maintains document truth in isolated heap/mmap memory.
2. The **Render Server** and **Layout Server** are isolated worker threads supervised by the Microkernel.
3. If the GPU driver encounters device loss or font shaping panics on an invalid glyph:
   - The supervisor thread catches the panic boundary (`std::panic::catch_unwind`).
   - The Render Server is torn down and re-spawned with fresh surface handles.
   - The new Render Server requests a fresh `BufferSnapshot` from the Buffer Server via `msg_send`.
   - Editing continues uninterrupted with **zero data loss**.

---

## 3. High-Performance Text Buffer (Zed & Ghostty DNA)

### 3.1 The Zed-Inspired `SumTree` / Piece Tree
To handle everything from a 10-line note to a 500MB log file without latency stutter, the buffer engine combines two proven data structures:
1. **Piece Table with Memory-Mapped Base Buffer**:
   - The original file is mapped read-only (`mmap`). No copy into RAM is performed on open.
   - Insertions append to an append-only memory buffer.
2. **Chunked B-Tree (`SumTree`) Index**:
   - Edits are indexed by a 2-3 B-Tree.
   - Each node contains a **Monoid Summary**:
     ```rust
     #[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
     pub struct TextSummary {
         pub bytes: usize,           // Total UTF-8 byte length
         pub utf16_units: usize,     // UTF-16 code units (for IME/OS cursor alignment)
         pub lines: usize,           // Total newline characters ('\n')
         pub longest_line: usize,    // Byte length of longest line in chunk (for scroll bounds)
     }

     impl std::ops::Add for TextSummary {
         type Output = Self;
         fn add(self, rhs: Self) -> Self::Output {
             Self {
                 bytes: self.bytes + rhs.bytes,
                 utf16_units: self.utf16_units + rhs.utf16_units,
                 lines: self.lines + rhs.lines,
                 longest_line: self.longest_line.max(rhs.longest_line),
             }
         }
     }
     ```

### 3.2 Algorithmic Complexities
- **Line-to-Offset Translation:** $O(\log N)$ by traversing the tree using `summary.lines`.
- **Offset-to-Line Translation:** $O(\log N)$ by traversing the tree using `summary.bytes`.
- **Insert / Delete:** $O(\log N)$ with persistent tree structural sharing (Copy-on-Write).
- **Snapshot Creation:** $O(1)$ shallow reference increment (`Arc<Node>`).

---

## 4. Typography & Viewport Layout Server

### 4.1 Incremental Viewport-Bounded Layout
Traditional rich text editors layout the entire document on change. Texted computes layout **only for the visible viewport rows** plus a tiny overscan buffer (e.g., 5 lines above, 5 lines below).

```
Document: 1,000,000 lines
+-----------------------------------+
| Lines 0 .. 494: Unshaped          |
+-----------------------------------+
| Lines 495 .. 499: Overscan Top    |
| Lines 500 .. 540: Visible Viewport|  <-- Shaped & Cached in Atlas
| Lines 541 .. 545: Overscan Bottom |
+-----------------------------------+
| Lines 546 .. 1,000,000: Unshaped  |
+-----------------------------------+
```

### 4.2 Word Wrap with Line Metrics Cache
- When Word Wrap is enabled, physical lines are wrapped into visual sub-lines based on the current viewport pixel width.
- A thread-safe LRU cache stores `(LineIndex, ViewportWidth) -> Vec<VisualWrapBreak>`.
- Scrolling vertically performs binary search over the visible line metrics without reflowing off-screen text.

---

## 5. GPU Rendering Pipeline (Ghostty DNA)

### 5.1 Architecture: Instanced Quad Compositor
Following Ghostty's ultra-efficient terminal pipeline:
- Avoid multi-pass CPU rasterization into software framebuffers.
- All visual elements are rendered as **Instanced Quads** on the GPU:
  1. Solid background / canvas quad.
  2. Text selection highlight quads.
  3. Cursor / caret vertical bar quad.
  4. Glyph quads sampled from a single dynamic glyph atlas texture.

### 5.2 GPU Quad Vertex Layout
```rust
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct QuadInstance {
    pub position: [f32; 2],  // Top-left screen coordinate (pixels)
    pub size: [f32; 2],      // Width and Height (pixels)
    pub uv_min: [f32; 2],    // Texture atlas UV coordinates (0.0 if solid color)
    pub uv_max: [f32; 2],
    pub color: [f32; 4],     // RGBA float color (with premultiplied alpha)
    pub flags: u32,          // 0 = solid color, 1 = textured glyph, 2 = subpixel text
}
```

### 5.3 Dynamic LRU Glyph Atlas
- Single 8-bit or 32-bit GPU texture (e.g., 2048x2048 `R8Unorm` or `RGBA8Unorm`).
- When a glyph is requested:
  1. Check Atlas Hash Map: `(FontId, GlyphId, SubpixelBin) -> Option<AtlasRect>`.
  2. If hit: emit `QuadInstance` referencing atlas UVs.
  3. If miss: rasterize glyph via `swash`, blit into atlas shelf allocator, upload to GPU via staging buffer, and cache rectangle.
  4. Atlas rarely invalidates during standard plain-text editing because ASCII and common Unicode glyphs fit easily in cache.

### 5.4 Display Pacing (Sub-4ms Keystroke Latency)
- Ghostty synchronizes rendering directly with display refresh intervals (`CVDisplayLink` on macOS, `DXGI` on Windows).
- **Damage Rect Tracking:**
  - A keystroke only modifies the caret line and status bar.
  - Texted records a `DirtyLineMask`. If only line 42 changed, only the quads for line 42 and the status bar are updated in the instance buffer.
  - If no pulse is received, the render thread yields to sleep, consuming **0% CPU and 0% GPU**.

---

## 6. End-to-End Keystroke Timeline

Target: Keystroke to display photons in $< 4\text{ms}$.

```
Time (ms)  Event Description
0.00 ms    [OS / Hardware]: Physical keypress (e.g. 'a') triggers OS keyboard event.
0.20 ms    [Input Server]: Winit / OS event handler normalizes key; maps to Action::InsertChar('a').
0.35 ms    [Input Server]: Fires MsgSendPulse(CH_BUFFER, PULSE_KEY_INPUT, packed_char).
0.50 ms    [Buffer Server]: Microkernel pulse received; executes Piece Tree insert at caret.
0.70 ms    [Buffer Server]: Updates caret (col += 1). Emits PULSE_BUFFER_MODIFIED to Layout & Render.
0.90 ms    [Layout Server]: Retrieves glyph for 'a' from font shaper cache; updates line metrics.
1.30 ms    [Render Server]: Updates instance quad buffer (single dirty line updated).
1.60 ms    [Render Server]: Submits GPU command buffer via wgpu (single draw call).
2.00 ms    [GPU Hardware]: GPU rasterizes quads to swapchain backbuffer.
display    [VSync / ProMotion]: Frame presented on next display scanout.
```

---

## 7. Concrete Rust Crate Breakdown

The workspace is organized into discrete crates enforcing microkernel boundaries:

| Crate | Responsibility | Dependencies |
|---|---|---|
| `texted-kernel` | Microkernel IPC, rendezvous channels, pulse ring-buffers, supervisor | `crossbeam-channel`, `parking_lot`, `oneshot` |
| `texted-buffer` | SumTree piece table, snapshots, undo/redo transaction log | `smallvec`, `memmap2` |
| `texted-layout` | Viewport shaper, line metrics, wrap breaks, coordinate mapping | `swash` or `cosmic-text` |
| `texted-render` | wgpu pipeline, dynamic glyph atlas, instanced quad batcher | `wgpu`, `bytemuck` |
| `texted-input` | OS keybinding map, IME composition, gesture recognizer | `winit` (events) |
| `texted-io` | Memory-mapped loader, atomic file persistence, disk watcher | `memmap2`, `simdutf8`, `notify` |
| `texted-app` | Entry point, OS window shell, menu bar, supervisor daemon | `winit`, `texted-*` crates |

---

## 8. Implementation Strategy & Next Steps

1. **Phase 1: Microkernel Core & Buffer Engine**
   - Implement `texted-kernel` with synchronous rendezvous and pulse channels.
   - Implement `texted-buffer` with the `SumTree` piece table and line/offset summaries.
   - Validate with multi-megabyte file insert/delete fuzz tests.

2. **Phase 2: GPU Render Engine & Glyph Atlas**
   - Implement `texted-render` with `wgpu` instanced quad renderer.
   - Build the dynamic shelf-allocator glyph atlas backed by `swash`.
   - Measure 120Hz/240Hz frame delivery with zero dirty rendering.

3. **Phase 3: Input, Layout & Notepad Feature Polish**
   - Wire up `winit` in `texted-app` with the `InputServer`.
   - Implement caret blink pacing, rectangular/character selections, drag-select.
   - Add status bar (Line/Col, Encoding, Zoom, Line Ending) and find/replace dialog.
   - Integrate `memmap2` and atomic save in `texted-io`.
