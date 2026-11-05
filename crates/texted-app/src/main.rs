//! Texted - Ultra-Minimal High-Performance QNX-Microkernel GUI Text Editor
//!
//! Features:
//! - QNX-inspired Microkernel IPC bus
//! - Full Ukrainian & Unicode typography via TrueType font rasterizer (ab_glyph)
//! - Complete Mouse interactions: single click, double click word, triple click line, drag
//! - Block Select (Column / Rectangular selection): Alt/Option + drag or keyboard toggle
//! - Multi-line block typing, block deletion, block copy/cut/paste
//! - Zero-copy Piece Table buffer engine

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use font8x8::UnicodeFonts;
use texted_buffer::{BufferSnapshot, TextBuffer};
use texted_io::{IoManager, LineEnding};
use texted_kernel::{ChannelEvent, ChannelPair, Pulse, CH_BUFFER, PULSE_KEY_INPUT};
use texted_layout::{LayoutEngine, ViewportConfig};
use winit::event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::WindowBuilder;

/// Messages sent to the Buffer Server
#[derive(Debug)]
pub enum BufferRequest {
    Insert { offset: usize, text: String },
    DeleteRange { start: usize, end: usize },
    GetSnapshot,
    Undo,
    Redo,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveHome,
    MoveEnd,
    Backspace,
    DeleteForward,
    SetCursorOffset(usize),
    BlockInsert { start_line: usize, end_line: usize, col: usize, text: String },
    BlockDelete { start_line: usize, end_line: usize, start_col: usize, end_col: usize },
    BlockExtract { start_line: usize, end_line: usize, start_col: usize, end_col: usize },
}

/// Responses from the Buffer Server
#[derive(Debug)]
pub enum BufferResponse {
    Snapshot(BufferSnapshot),
    Text(String),
    Success,
}

// Colors in 0x00RRGGBB format
const COLOR_BG: u32 = 0x00FFFFFF;              // Clean Notepad white
const COLOR_GUTTER_BG: u32 = 0x00F5F5F5;       // Soft grey gutter
const COLOR_GUTTER_BORDER: u32 = 0x00E2E2E2;   // Subtle separator
const COLOR_GUTTER_TEXT: u32 = 0x00777777;     // Grey line numbers
const COLOR_TEXT: u32 = 0x00111111;            // Deep charcoal text
const COLOR_CARET: u32 = 0x00000000;           // Solid black caret
const COLOR_SELECTION: u32 = 0x00CCE2FF;       // Soft Windows selection blue
const COLOR_BLOCK_SELECTION: u32 = 0x00BBDDFF; // Rectangular block select blue
const COLOR_STATUS_BG: u32 = 0x00F0F0F0;       // Status bar background
const COLOR_STATUS_BORDER: u32 = 0x00D0D0D0;   // Status bar border
const COLOR_STATUS_TEXT: u32 = 0x00333333;     // Status bar text
const COLOR_STATUS_ACCENT: u32 = 0x000055AA;   // Highlighted status text

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    None,
    Stream, // Standard stream selection
    Block,  // Rectangular / column selection
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionState {
    pub mode: SelectionMode,
    pub anchor_line: usize,
    pub anchor_col: usize,
    pub active_line: usize,
    pub active_col: usize,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            mode: SelectionMode::None,
            anchor_line: 0,
            anchor_col: 0,
            active_line: 0,
            active_col: 0,
        }
    }
}

// Alpha blend pixel over background
#[inline(always)]
fn blend_pixel(dst: &mut u32, fg_rgb: u32, alpha: u8) {
    if alpha == 0 {
        return;
    }
    if alpha == 255 {
        *dst = fg_rgb;
        return;
    }
    let a = alpha as u32;
    let inv_a = 255 - a;

    let fr = (fg_rgb >> 16) & 0xFF;
    let fg = (fg_rgb >> 8) & 0xFF;
    let fb = fg_rgb & 0xFF;

    let dr = (*dst >> 16) & 0xFF;
    let dg = (*dst >> 8) & 0xFF;
    let db = *dst & 0xFF;

    let r = (fr * a + dr * inv_a) / 255;
    let g = (fg * a + dg * inv_a) / 255;
    let b = (fb * a + db * inv_a) / 255;

    *dst = (r << 16) | (g << 8) | b;
}

#[derive(Clone)]
struct CachedGlyph {
    width: usize,
    height: usize,
    x_offset: i32,
    y_offset: i32,
    coverage: Vec<u8>,
}

pub struct FontEngine {
    font: Option<FontArc>,
    scale_px: f32,
    pub char_width_px: f32,
    pub line_height_px: f32,
    pub ascent_px: f32,
    cache: HashMap<char, CachedGlyph>,
}

impl FontEngine {
    pub fn new(scale_px: f32) -> Self {
        let candidate_paths = [
            "/System/Library/Fonts/Supplemental/Courier New.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Menlo.ttc",
        ];

        let mut loaded_font = None;
        for path in candidate_paths {
            if let Ok(bytes) = fs::read(path) {
                if let Ok(font) = FontArc::try_from_vec(bytes) {
                    loaded_font = Some(font);
                    break;
                }
            }
        }

        let (char_width_px, line_height_px, ascent_px) = if let Some(ref font) = loaded_font {
            let scaled = font.as_scaled(PxScale::from(scale_px));
            let h_advance = scaled.h_advance(scaled.glyph_id('M'));
            let ascent = scaled.ascent();
            let descent = scaled.descent();
            let line_gap = scaled.line_gap();
            let line_h = (ascent - descent + line_gap).ceil().max(scale_px * 1.25);
            (h_advance.round().max(8.0), line_h, ascent)
        } else {
            // Fallback metrics
            (scale_px * 0.6, scale_px * 1.3, scale_px)
        };

        Self {
            font: loaded_font,
            scale_px,
            char_width_px,
            line_height_px,
            ascent_px,
            cache: HashMap::new(),
        }
    }

    pub fn draw_char(
        &mut self,
        buffer: &mut [u32],
        stride: usize,
        max_height: usize,
        x: usize,
        y: usize,
        ch: char,
        color: u32,
    ) {
        if let Some(ref font) = self.font {
            let scale_px = self.scale_px;
            let ascent_px = self.ascent_px;

            let glyph_data = self.cache.entry(ch).or_insert_with(|| {
                let scaled = font.as_scaled(PxScale::from(scale_px));
                let glyph = scaled.scaled_glyph(ch);
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    let width = bounds.width().ceil() as usize;
                    let height = bounds.height().ceil() as usize;
                    let mut cov = vec![0u8; width * height];
                    outlined.draw(|gx, gy, coverage| {
                        if (gx as usize) < width && (gy as usize) < height {
                            cov[gy as usize * width + gx as usize] =
                                (coverage * 255.0).round().min(255.0) as u8;
                        }
                    });
                    CachedGlyph {
                        width,
                        height,
                        x_offset: bounds.min.x.round() as i32,
                        y_offset: (bounds.min.y + ascent_px).round() as i32,
                        coverage: cov,
                    }
                } else {
                    CachedGlyph {
                        width: 0,
                        height: 0,
                        x_offset: 0,
                        y_offset: 0,
                        coverage: Vec::new(),
                    }
                }
            });

            if glyph_data.width > 0 && glyph_data.height > 0 {
                let base_x = x as i32 + glyph_data.x_offset;
                let base_y = y as i32 + glyph_data.y_offset;

                for row in 0..glyph_data.height {
                    let py = base_y + row as i32;
                    if py < 0 || py as usize >= max_height {
                        continue;
                    }
                    let row_offset = (py as usize) * stride;

                    for col in 0..glyph_data.width {
                        let px = base_x + col as i32;
                        if px < 0 || px as usize >= stride {
                            continue;
                        }
                        let alpha = glyph_data.coverage[row * glyph_data.width + col];
                        blend_pixel(&mut buffer[row_offset + px as usize], color, alpha);
                    }
                }
                return;
            }
        }

        // Fallback to font8x8 if TrueType glyph is unavailable
        let glyph = font8x8::BASIC_FONTS
            .get(ch)
            .or_else(|| font8x8::LATIN_FONTS.get(ch))
            .or_else(|| font8x8::BLOCK_FONTS.get(ch))
            .or_else(|| font8x8::BOX_FONTS.get(ch));

        if let Some(glyph_data) = glyph {
            let scale = (self.scale_px / 9.0).round().max(1.0) as usize;
            for (row_idx, &row_byte) in glyph_data.iter().enumerate() {
                for col_idx in 0..8 {
                    if (row_byte & (1 << col_idx)) != 0 {
                        let px = x + col_idx * scale;
                        let py = y + row_idx * scale;
                        draw_rect(buffer, stride, max_height, px, py, scale, scale, color);
                    }
                }
            }
        }
    }

    pub fn draw_string(
        &mut self,
        buffer: &mut [u32],
        stride: usize,
        max_height: usize,
        x: usize,
        y: usize,
        text: &str,
        color: u32,
    ) {
        let mut cur_x = x as f32;
        let char_advance = self.char_width_px;

        for ch in text.chars() {
            if cur_x + char_advance > stride as f32 {
                break;
            }
            self.draw_char(buffer, stride, max_height, cur_x.round() as usize, y, ch, color);
            cur_x += char_advance;
        }
    }
}

fn draw_rect(
    buffer: &mut [u32],
    stride: usize,
    max_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: u32,
) {
    let x_end = (x + width).min(stride);
    let y_end = (y + height).min(max_height);
    for row in y..y_end {
        let row_offset = row * stride;
        for col in x..x_end {
            buffer[row_offset + col] = color;
        }
    }
}

// System clipboard helpers via macOS pbcopy / pbpaste
fn copy_to_system_clipboard(text: &str) {
    if let Ok(mut child) = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

fn get_system_clipboard() -> Option<String> {
    if let Ok(output) = Command::new("pbpaste").output() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("Failed to initialize winit EventLoop");

    let initial_path = std::env::args().nth(1).map(PathBuf::from);
    let window_title = match &initial_path {
        Some(p) => format!("{} - Texted", p.file_name().and_then(|n| n.to_str()).unwrap_or("document.txt")),
        None => "Untitled - Texted".to_string(),
    };

    let window = Rc::new(
        WindowBuilder::new()
            .with_title(&window_title)
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 680.0))
            .build(&event_loop)
            .expect("Failed to create application window"),
    );

    // Initial content with full Ukrainian support & block select demonstration
    let (initial_content, active_path) = if let Some(ref path) = initial_path {
        match IoManager::open_file(path) {
            Ok(doc) => (doc.content, Some(path.clone())),
            Err(_) => (String::new(), Some(path.clone())),
        }
    } else {
        let sample = "Вітаємо у Texted! (Welcome to Texted!)\n\n\
Ультра-швидкий мінімальний текстовий редактор на основі мікроядра QNX.\n\
Повна підтримка української мови: Привіт, світе! Їжак, Євген, Ґрунт, Ігор.\n\n\
Взаємодія з мишею та виділення:\n\
  - Одинарний клік: встановити курсор\n\
  - Подвійний клік: виділити слово\n\
  - Потрійний клік: виділити весь рядок\n\
  - Звичайне перетягування: виділити текст потоком\n\
  - Alt/Option + перетягування: БЛОЧНЕ (стовпчикове / прямокутне) виділення!\n\n\
Блочне редагування (Block Select):\n\
  - Стовпчик 1: [x] Завдання 1\n\
  - Стовпчик 2: [x] Завдання 2\n\
  - Стовпчик 3: [x] Завдання 3\n\n\
Спробуйте затиснути Alt/Option та виділити стовпчик \"[x]\", а потім натиснути клавішу чи Backspace!\n\
Гарячі клавіші: Cmd+Z (Undo), Cmd+Y (Redo), Cmd+S (Зберегти), Cmd+L (Нумерація)\n";
        (sample.to_string(), None)
    };

    let running = Arc::new(AtomicBool::new(true));

    // 1. Initialize Microkernel Channels (QNX ChannelCreate)
    let buffer_channel: ChannelPair<BufferRequest, BufferResponse> = ChannelPair::new(CH_BUFFER);
    let buffer_client = buffer_channel.connect();

    // 2. Spawn Isolated Buffer Server Thread (handles Piece Table)
    let b_running = running.clone();
    let init_text = initial_content.clone();
    let buffer_thread = thread::Builder::new()
        .name("server-buffer".to_string())
        .spawn(move || {
            let mut buffer = TextBuffer::from_string(init_text);
            let server = buffer_channel.server;

            while b_running.load(Ordering::Relaxed) {
                if let Some(event) = server.try_receive() {
                    match event {
                        ChannelEvent::Pulse(pulse) => {
                            if pulse.code == PULSE_KEY_INPUT {
                                // Full Unicode 32-bit scalar decoding (Ukrainian / Cyrillic safe)
                                if let Some(ch) = char::from_u32(pulse.value as u32) {
                                    let offset = buffer.cursor_offset();
                                    buffer.insert(offset, &ch.to_string());
                                }
                            }
                        }
                        ChannelEvent::Message(req) => match req.payload {
                            BufferRequest::Insert { offset, ref text } => {
                                let insert_pos = if offset == usize::MAX {
                                    buffer.cursor_offset()
                                } else {
                                    offset
                                };
                                buffer.insert(insert_pos, text);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteRange { start, end } => {
                                buffer.delete(start, end);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::GetSnapshot => {
                                let snap = buffer.create_snapshot();
                                req.reply(BufferResponse::Snapshot(snap));
                            }
                            BufferRequest::Undo => {
                                buffer.undo();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::Redo => {
                                buffer.redo();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveLeft => {
                                buffer.move_cursor_left();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveRight => {
                                buffer.move_cursor_right();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveUp => {
                                buffer.move_cursor_up();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveDown => {
                                buffer.move_cursor_down();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveHome => {
                                buffer.move_cursor_home();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveEnd => {
                                buffer.move_cursor_end();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::Backspace => {
                                buffer.backspace();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteForward => {
                                buffer.delete_forward();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::SetCursorOffset(offset) => {
                                buffer.set_cursor_offset(offset);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::BlockInsert { start_line, end_line, col, ref text } => {
                                buffer.block_insert(start_line, end_line, col, text);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::BlockDelete { start_line, end_line, start_col, end_col } => {
                                buffer.block_delete(start_line, end_line, start_col, end_col);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::BlockExtract { start_line, end_line, start_col, end_col } => {
                                let extracted = buffer.block_extract(start_line, end_line, start_col, end_col);
                                req.reply(BufferResponse::Text(extracted));
                            }
                        },
                    }
                } else {
                    thread::sleep(Duration::from_micros(500));
                }
            }
        })
        .expect("Failed to spawn buffer server");

    // 3. Initialize Display & Typography
    let context = softbuffer::Context::new(window.clone()).expect("Failed to create softbuffer context");
    let mut surface = softbuffer::Surface::new(&context, window.clone()).expect("Failed to create surface");

    // Typography Engine (18px crisp scalable font)
    let mut font_engine = FontEngine::new(18.0);
    let mut top_line = 0usize;
    let mut show_line_numbers = true;
    let mut current_modifiers = ModifiersState::empty();
    let mut last_caret_blink = Instant::now();
    let mut caret_visible = true;
    let mut current_file_path = active_path;

    // Mouse & Selection Tracking
    let mut selection = SelectionState::default();
    let mut is_mouse_dragging = false;
    let mut last_click_time = Instant::now();
    let mut click_count = 0usize;
    let mut last_mouse_pos = (0.0f32, 0.0f32);

    println!("[Texted] GUI editor running with Ukrainian & Block Select support.");

    let _ = event_loop.run(move |event, elwt| {
        elwt.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));

        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => {
                    running.store(false, Ordering::Relaxed);
                    elwt.exit();
                }
                WindowEvent::ModifiersChanged(mods) => {
                    current_modifiers = mods.state();
                }
                WindowEvent::Resized(_) => {
                    window.request_redraw();
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let lines_delta = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as i32,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y / (font_engine.line_height_px as f64)) as i32,
                    };
                    if lines_delta > 0 {
                        top_line = top_line.saturating_sub(lines_delta.unsigned_abs() as usize);
                    } else if lines_delta < 0 {
                        top_line = top_line.saturating_add(lines_delta.unsigned_abs() as usize);
                    }
                    window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    last_mouse_pos = (position.x as f32, position.y as f32);

                    if is_mouse_dragging {
                        let gutter_w = if show_line_numbers { 58.0 } else { 12.0 };
                        let rel_y = (last_mouse_pos.1 - 4.0).max(0.0);
                        let rel_x = (last_mouse_pos.0 - gutter_w - 6.0).max(0.0);

                        let hover_line = (rel_y / font_engine.line_height_px).floor() as usize + top_line;
                        let hover_col = (rel_x / font_engine.char_width_px).round() as usize;

                        selection.active_line = hover_line;
                        selection.active_col = hover_col;

                        // Alt / Option key triggers Block Select!
                        if current_modifiers.alt_key() {
                            selection.mode = SelectionMode::Block;
                        } else if selection.mode == SelectionMode::None {
                            selection.mode = SelectionMode::Stream;
                        }

                        // Position cursor at active position
                        if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                            let offset = TextBuffer::line_col_to_offset_str(&snap.content, hover_line, hover_col);
                            let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                        }

                        caret_visible = true;
                        last_caret_blink = Instant::now();
                        window.request_redraw();
                    }
                }
                WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                    let gutter_w = if show_line_numbers { 58.0 } else { 12.0 };
                    let rel_y = (last_mouse_pos.1 - 4.0).max(0.0);
                    let rel_x = (last_mouse_pos.0 - gutter_w - 6.0).max(0.0);

                    let click_line = (rel_y / font_engine.line_height_px).floor() as usize + top_line;
                    let click_col = (rel_x / font_engine.char_width_px).round() as usize;

                    match state {
                        ElementState::Pressed => {
                            is_mouse_dragging = true;

                            let now = Instant::now();
                            if now.duration_since(last_click_time) < Duration::from_millis(350) {
                                click_count += 1;
                            } else {
                                click_count = 1;
                            }
                            last_click_time = now;

                            if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                let lines: Vec<&str> = snap.content.split('\n').collect();
                                let line_len = lines.get(click_line).map(|l| l.chars().count()).unwrap_or(0);

                                if click_count == 2 {
                                    // Double click: Select Word
                                    if let Some(line_str) = lines.get(click_line) {
                                        let chars: Vec<char> = line_str.chars().collect();
                                        let mut w_start = click_col.min(chars.len());
                                        let mut w_end = w_start;

                                        while w_start > 0 && chars[w_start - 1].is_alphanumeric() {
                                            w_start -= 1;
                                        }
                                        while w_end < chars.len() && chars[w_end].is_alphanumeric() {
                                            w_end += 1;
                                        }

                                        selection = SelectionState {
                                            mode: SelectionMode::Stream,
                                            anchor_line: click_line,
                                            anchor_col: w_start,
                                            active_line: click_line,
                                            active_col: w_end,
                                        };
                                        let offset = TextBuffer::line_col_to_offset_str(&snap.content, click_line, w_end);
                                        let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                                    }
                                } else if click_count >= 3 {
                                    // Triple click: Select Entire Line
                                    selection = SelectionState {
                                        mode: SelectionMode::Stream,
                                        anchor_line: click_line,
                                        anchor_col: 0,
                                        active_line: click_line,
                                        active_col: line_len,
                                    };
                                    let offset = TextBuffer::line_col_to_offset_str(&snap.content, click_line, line_len);
                                    let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                                } else {
                                    // Single click
                                    let is_block = current_modifiers.alt_key();
                                    selection = SelectionState {
                                        mode: if is_block { SelectionMode::Block } else { SelectionMode::None },
                                        anchor_line: click_line,
                                        anchor_col: click_col,
                                        active_line: click_line,
                                        active_col: click_col,
                                    };
                                    let offset = TextBuffer::line_col_to_offset_str(&snap.content, click_line, click_col);
                                    let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                                }
                            }

                            caret_visible = true;
                            last_caret_blink = Instant::now();
                            window.request_redraw();
                        }
                        ElementState::Released => {
                            is_mouse_dragging = false;
                            if selection.anchor_line == selection.active_line
                                && selection.anchor_col == selection.active_col
                                && click_count == 1
                            {
                                selection.mode = SelectionMode::None;
                            }
                            window.request_redraw();
                        }
                    }
                }
                WindowEvent::KeyboardInput {
                    event: KeyEvent {
                        state: ElementState::Pressed,
                        logical_key,
                        text,
                        ..
                    },
                    ..
                } => {
                    caret_visible = true;
                    last_caret_blink = Instant::now();
                    let is_ctrl_or_cmd = current_modifiers.control_key() || current_modifiers.super_key();

                    match logical_key {
                        Key::Named(NamedKey::ArrowLeft) => {
                            selection.mode = SelectionMode::None;
                            let _ = buffer_client.send(BufferRequest::MoveLeft);
                        }
                        Key::Named(NamedKey::ArrowRight) => {
                            selection.mode = SelectionMode::None;
                            let _ = buffer_client.send(BufferRequest::MoveRight);
                        }
                        Key::Named(NamedKey::ArrowUp) => {
                            selection.mode = SelectionMode::None;
                            let _ = buffer_client.send(BufferRequest::MoveUp);
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            selection.mode = SelectionMode::None;
                            let _ = buffer_client.send(BufferRequest::MoveDown);
                        }
                        Key::Named(NamedKey::Home) => {
                            selection.mode = SelectionMode::None;
                            let _ = buffer_client.send(BufferRequest::MoveHome);
                        }
                        Key::Named(NamedKey::End) => {
                            selection.mode = SelectionMode::None;
                            let _ = buffer_client.send(BufferRequest::MoveEnd);
                        }
                        Key::Named(NamedKey::Backspace) => {
                            if selection.mode == SelectionMode::Block {
                                let min_l = selection.anchor_line.min(selection.active_line);
                                let max_l = selection.anchor_line.max(selection.active_line);
                                let min_c = selection.anchor_col.min(selection.active_col);
                                let max_c = selection.anchor_col.max(selection.active_col);

                                if min_c < max_c {
                                    let _ = buffer_client.send(BufferRequest::BlockDelete {
                                        start_line: min_l,
                                        end_line: max_l,
                                        start_col: min_c,
                                        end_col: max_c,
                                    });
                                    selection.anchor_col = min_c;
                                    selection.active_col = min_c;
                                } else if min_c > 0 {
                                    let _ = buffer_client.send(BufferRequest::BlockDelete {
                                        start_line: min_l,
                                        end_line: max_l,
                                        start_col: min_c - 1,
                                        end_col: min_c,
                                    });
                                    selection.anchor_col = min_c - 1;
                                    selection.active_col = min_c - 1;
                                }
                            } else if selection.mode == SelectionMode::Stream {
                                if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                    let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                    let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                    let min_off = off_a.min(off_b);
                                    let max_off = off_a.max(off_b);
                                    let _ = buffer_client.send(BufferRequest::DeleteRange { start: min_off, end: max_off });
                                }
                                selection.mode = SelectionMode::None;
                            } else {
                                let _ = buffer_client.send(BufferRequest::Backspace);
                            }
                        }
                        Key::Named(NamedKey::Delete) => {
                            if selection.mode == SelectionMode::Block {
                                let min_l = selection.anchor_line.min(selection.active_line);
                                let max_l = selection.anchor_line.max(selection.active_line);
                                let min_c = selection.anchor_col.min(selection.active_col);
                                let max_c = selection.anchor_col.max(selection.active_col);

                                if min_c < max_c {
                                    let _ = buffer_client.send(BufferRequest::BlockDelete {
                                        start_line: min_l,
                                        end_line: max_l,
                                        start_col: min_c,
                                        end_col: max_c,
                                    });
                                    selection.anchor_col = min_c;
                                    selection.active_col = min_c;
                                }
                            } else if selection.mode == SelectionMode::Stream {
                                if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                    let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                    let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                    let _ = buffer_client.send(BufferRequest::DeleteRange { start: off_a.min(off_b), end: off_a.max(off_b) });
                                }
                                selection.mode = SelectionMode::None;
                            } else {
                                let _ = buffer_client.send(BufferRequest::DeleteForward);
                            }
                        }
                        Key::Named(NamedKey::Enter) => {
                            selection.mode = SelectionMode::None;
                            let _ = buffer_client.send(BufferRequest::Insert {
                                offset: usize::MAX,
                                text: "\n".to_string(),
                            });
                        }
                        Key::Named(NamedKey::Tab) => {
                            if selection.mode == SelectionMode::Block {
                                let min_l = selection.anchor_line.min(selection.active_line);
                                let max_l = selection.anchor_line.max(selection.active_line);
                                let min_c = selection.anchor_col.min(selection.active_col);
                                let _ = buffer_client.send(BufferRequest::BlockInsert {
                                    start_line: min_l,
                                    end_line: max_l,
                                    col: min_c,
                                    text: "    ".to_string(),
                                });
                                selection.anchor_col += 4;
                                selection.active_col += 4;
                            } else {
                                let _ = buffer_client.send(BufferRequest::Insert {
                                    offset: usize::MAX,
                                    text: "    ".to_string(),
                                });
                            }
                        }
                        Key::Character(ref s) if is_ctrl_or_cmd => {
                            match s.to_lowercase().as_str() {
                                "z" => {
                                    if current_modifiers.shift_key() {
                                        let _ = buffer_client.send(BufferRequest::Redo);
                                    } else {
                                        let _ = buffer_client.send(BufferRequest::Undo);
                                    }
                                }
                                "y" => {
                                    let _ = buffer_client.send(BufferRequest::Redo);
                                }
                                "l" => {
                                    show_line_numbers = !show_line_numbers;
                                }
                                "b" => {
                                    // Keyboard shortcut to toggle Block Selection mode
                                    selection.mode = if selection.mode == SelectionMode::Block {
                                        SelectionMode::None
                                    } else {
                                        SelectionMode::Block
                                    };
                                }
                                "c" => {
                                    // Copy to system clipboard
                                    if selection.mode == SelectionMode::Block {
                                        let min_l = selection.anchor_line.min(selection.active_line);
                                        let max_l = selection.anchor_line.max(selection.active_line);
                                        let min_c = selection.anchor_col.min(selection.active_col);
                                        let max_c = selection.anchor_col.max(selection.active_col);
                                        if let Ok(BufferResponse::Text(extracted)) = buffer_client.send(BufferRequest::BlockExtract {
                                            start_line: min_l,
                                            end_line: max_l,
                                            start_col: min_c,
                                            end_col: max_c,
                                        }) {
                                            copy_to_system_clipboard(&extracted);
                                        }
                                    } else if selection.mode == SelectionMode::Stream {
                                        if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                            let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                            let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                            let min_off = off_a.min(off_b);
                                            let max_off = off_a.max(off_b);
                                            if min_off < max_off && max_off <= snap.content.len() {
                                                copy_to_system_clipboard(&snap.content[min_off..max_off]);
                                            }
                                        }
                                    }
                                }
                                "x" => {
                                    // Cut to system clipboard
                                    if selection.mode == SelectionMode::Block {
                                        let min_l = selection.anchor_line.min(selection.active_line);
                                        let max_l = selection.anchor_line.max(selection.active_line);
                                        let min_c = selection.anchor_col.min(selection.active_col);
                                        let max_c = selection.anchor_col.max(selection.active_col);
                                        if let Ok(BufferResponse::Text(extracted)) = buffer_client.send(BufferRequest::BlockExtract {
                                            start_line: min_l,
                                            end_line: max_l,
                                            start_col: min_c,
                                            end_col: max_c,
                                        }) {
                                            copy_to_system_clipboard(&extracted);
                                            let _ = buffer_client.send(BufferRequest::BlockDelete {
                                                start_line: min_l,
                                                end_line: max_l,
                                                start_col: min_c,
                                                end_col: max_c,
                                            });
                                        }
                                        selection.mode = SelectionMode::None;
                                    } else if selection.mode == SelectionMode::Stream {
                                        if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                            let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                            let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                            let min_off = off_a.min(off_b);
                                            let max_off = off_a.max(off_b);
                                            if min_off < max_off && max_off <= snap.content.len() {
                                                copy_to_system_clipboard(&snap.content[min_off..max_off]);
                                                let _ = buffer_client.send(BufferRequest::DeleteRange { start: min_off, end: max_off });
                                            }
                                        }
                                        selection.mode = SelectionMode::None;
                                    }
                                }
                                "v" => {
                                    // Paste from system clipboard
                                    if let Some(clip) = get_system_clipboard() {
                                        if selection.mode == SelectionMode::Block {
                                            let min_l = selection.anchor_line.min(selection.active_line);
                                            let min_c = selection.anchor_col.min(selection.active_col);
                                            let clip_lines: Vec<&str> = clip.split('\n').collect();
                                            for (idx, line_text) in clip_lines.iter().enumerate() {
                                                let target_l = min_l + idx;
                                                let _ = buffer_client.send(BufferRequest::BlockInsert {
                                                    start_line: target_l,
                                                    end_line: target_l,
                                                    col: min_c,
                                                    text: line_text.to_string(),
                                                });
                                            }
                                        } else {
                                            let _ = buffer_client.send(BufferRequest::Insert {
                                                offset: usize::MAX,
                                                text: clip,
                                            });
                                        }
                                    }
                                }
                                "s" => {
                                    // Save file atomically
                                    if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                        let save_path = current_file_path.clone().unwrap_or_else(|| PathBuf::from("note.txt"));
                                        if IoManager::save_atomic(&save_path, &snap.content, false, LineEnding::Lf).is_ok() {
                                            current_file_path = Some(save_path.clone());
                                            window.set_title(&format!("{} - Texted", save_path.display()));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        _ => {
                            if let Some(t) = text {
                                for ch in t.chars() {
                                    if !ch.is_control() {
                                        if selection.mode == SelectionMode::Block {
                                            // Block typing: type simultaneously into all selected lines at col!
                                            let min_l = selection.anchor_line.min(selection.active_line);
                                            let max_l = selection.anchor_line.max(selection.active_line);
                                            let min_c = selection.anchor_col.min(selection.active_col);

                                            let _ = buffer_client.send(BufferRequest::BlockInsert {
                                                start_line: min_l,
                                                end_line: max_l,
                                                col: min_c,
                                                text: ch.to_string(),
                                            });
                                            selection.anchor_col = min_c + 1;
                                            selection.active_col = min_c + 1;
                                        } else {
                                            if selection.mode == SelectionMode::Stream {
                                                // Replace selected text
                                                if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                                    let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                                    let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                                    let min_off = off_a.min(off_b);
                                                    let max_off = off_a.max(off_b);
                                                    let _ = buffer_client.send(BufferRequest::DeleteRange { start: min_off, end: max_off });
                                                }
                                                selection.mode = SelectionMode::None;
                                            }

                                            // High-priority Unicode pulse dispatch (supports Ukrainian Cyrillic)
                                            let pulse = Pulse::new(0, PULSE_KEY_INPUT, ch as u32 as u64);
                                            let _ = buffer_client.send_pulse(pulse);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    window.request_redraw();
                }
                WindowEvent::RedrawRequested => {
                    let size = window.inner_size();
                    if size.width == 0 || size.height == 0 {
                        return;
                    }

                    let width = size.width as usize;
                    let height = size.height as usize;

                    let _ = surface.resize(
                        NonZeroU32::new(size.width).unwrap(),
                        NonZeroU32::new(size.height).unwrap(),
                    );

                    let mut buffer = match surface.buffer_mut() {
                        Ok(b) => b,
                        Err(_) => return,
                    };

                    // Blinking cursor pacing
                    if last_caret_blink.elapsed() >= Duration::from_millis(500) {
                        caret_visible = !caret_visible;
                        last_caret_blink = Instant::now();
                    }

                    // 1. Fetch Snapshot from Buffer Server via Microkernel
                    let snap = match buffer_client.send(BufferRequest::GetSnapshot) {
                        Ok(BufferResponse::Snapshot(s)) => s,
                        _ => return,
                    };

                    // Auto-scroll viewport if cursor moved out of visible view
                    let temp_layout = LayoutEngine::compute_layout(&snap, &ViewportConfig::default(), None);
                    let (cur_line, cur_col) = (temp_layout.cursor_line, temp_layout.cursor_col);

                    let visible_rows = (height.saturating_sub(28) as f32 / font_engine.line_height_px).floor() as usize;
                    if cur_line < top_line {
                        top_line = cur_line;
                    } else if cur_line >= top_line + visible_rows.saturating_sub(1) {
                        top_line = cur_line.saturating_sub(visible_rows.saturating_sub(2));
                    }

                    // 2. Viewport metrics
                    let gutter_w = if show_line_numbers { 58.0 } else { 12.0 };
                    let config = ViewportConfig {
                        width_px: width as f32,
                        height_px: (height.saturating_sub(28)) as f32,
                        line_height_px: font_engine.line_height_px,
                        char_width_px: font_engine.char_width_px,
                        top_line,
                        word_wrap: false,
                        show_line_numbers,
                        line_number_gutter_width_px: gutter_w,
                    };

                    let layout = LayoutEngine::compute_layout(&snap, &config, None);

                    // 3. Clear Screen to White
                    buffer.fill(COLOR_BG);

                    // 4. Render Gutter (Line numbers)
                    if show_line_numbers {
                        let gutter_width_int = gutter_w as usize;
                        draw_rect(
                            &mut buffer,
                            width,
                            height,
                            0,
                            0,
                            gutter_width_int,
                            height.saturating_sub(28),
                            COLOR_GUTTER_BG,
                        );
                        // Vertical separator line
                        draw_rect(
                            &mut buffer,
                            width,
                            height,
                            gutter_width_int - 1,
                            0,
                            1,
                            height.saturating_sub(28),
                            COLOR_GUTTER_BORDER,
                        );
                    }

                    // 5. Render Selection Highlights
                    let text_start_x = gutter_w as usize + 6;

                    if selection.mode == SelectionMode::Block {
                        // Render 2D Rectangular Column Selection
                        let min_l = selection.anchor_line.min(selection.active_line);
                        let max_l = selection.anchor_line.max(selection.active_line);
                        let min_c = selection.anchor_col.min(selection.active_col);
                        let max_c = selection.anchor_col.max(selection.active_col);

                        for l_idx in min_l..=max_l {
                            if l_idx >= top_line && l_idx < top_line + visible_rows + 2 {
                                let row_y = ((l_idx - top_line) as f32 * font_engine.line_height_px) as usize;
                                let sel_x = text_start_x + (min_c as f32 * font_engine.char_width_px).round() as usize;
                                let sel_w = (((max_c - min_c) as f32 * font_engine.char_width_px).round() as usize).max(3);

                                draw_rect(
                                    &mut buffer,
                                    width,
                                    height,
                                    sel_x,
                                    row_y + 2,
                                    sel_w,
                                    font_engine.line_height_px as usize,
                                    COLOR_BLOCK_SELECTION,
                                );
                            }
                        }
                    } else if selection.mode == SelectionMode::Stream {
                        // Render Stream Selection
                        let (s_line, s_col) = if (selection.anchor_line, selection.anchor_col) <= (selection.active_line, selection.active_col) {
                            (selection.anchor_line, selection.anchor_col)
                        } else {
                            (selection.active_line, selection.active_col)
                        };
                        let (e_line, e_col) = if (selection.anchor_line, selection.anchor_col) <= (selection.active_line, selection.active_col) {
                            (selection.active_line, selection.active_col)
                        } else {
                            (selection.anchor_line, selection.anchor_col)
                        };

                        let lines: Vec<&str> = snap.content.split('\n').collect();

                        for l_idx in s_line..=e_line {
                            if l_idx >= top_line && l_idx < top_line + visible_rows + 2 {
                                let row_y = ((l_idx - top_line) as f32 * font_engine.line_height_px) as usize;
                                let line_len = lines.get(l_idx).map(|l| l.chars().count()).unwrap_or(0);

                                let from_c = if l_idx == s_line { s_col } else { 0 };
                                let to_c = if l_idx == e_line { e_col } else { line_len };

                                let sel_x = text_start_x + (from_c as f32 * font_engine.char_width_px).round() as usize;
                                let sel_w = (((to_c.saturating_sub(from_c)) as f32 * font_engine.char_width_px).round() as usize).max(4);

                                draw_rect(
                                    &mut buffer,
                                    width,
                                    height,
                                    sel_x,
                                    row_y + 2,
                                    sel_w,
                                    font_engine.line_height_px as usize,
                                    COLOR_SELECTION,
                                );
                            }
                        }
                    }

                    // 6. Render Visible Lines (with antialiased Ukrainian font support)
                    for line in &layout.visible_lines {
                        let y_int = line.y_px as usize;
                        if y_int + (font_engine.line_height_px as usize) > height.saturating_sub(28) {
                            break;
                        }

                        // Gutter line number
                        if show_line_numbers {
                            let num_str = format!("{:>3}", line.line_index + 1);
                            font_engine.draw_string(
                                &mut buffer,
                                width,
                                height,
                                6,
                                y_int + 2,
                                &num_str,
                                COLOR_GUTTER_TEXT,
                            );
                        }

                        // Text content
                        font_engine.draw_string(
                            &mut buffer,
                            width,
                            height,
                            text_start_x,
                            y_int + 2,
                            &line.text,
                            COLOR_TEXT,
                        );
                    }

                    // 7. Render Caret
                    if caret_visible && layout.caret_rect.x >= 0.0 {
                        let cx = text_start_x + (cur_col as f32 * font_engine.char_width_px).round() as usize;
                        let cy = if cur_line >= top_line {
                            ((cur_line - top_line) as f32 * font_engine.line_height_px) as usize + 2
                        } else {
                            0
                        };

                        if cy + (font_engine.line_height_px as usize) <= height.saturating_sub(28) {
                            draw_rect(&mut buffer, width, height, cx, cy, 2, font_engine.line_height_px as usize, COLOR_CARET);
                        }
                    }

                    // 8. Render Windows Notepad Style Status Bar
                    let status_y = height.saturating_sub(28);
                    // Top separator border
                    draw_rect(&mut buffer, width, height, 0, status_y, width, 1, COLOR_STATUS_BORDER);
                    // Background
                    draw_rect(&mut buffer, width, height, 0, status_y + 1, width, 27, COLOR_STATUS_BG);

                    let status_mode = match selection.mode {
                        SelectionMode::Block => {
                            let min_l = selection.anchor_line.min(selection.active_line) + 1;
                            let max_l = selection.anchor_line.max(selection.active_line) + 1;
                            let min_c = selection.anchor_col.min(selection.active_col) + 1;
                            let max_c = selection.anchor_col.max(selection.active_col) + 1;
                            format!("[BLOCK SELECT: R{}-R{}, C{}-C{}]", min_l, max_l, min_c, max_c)
                        }
                        SelectionMode::Stream => "[SELECT]".to_string(),
                        SelectionMode::None => "[NORMAL]".to_string(),
                    };

                    let status_text = format!(
                        "  Ln {}, Col {}    {}    {} chars    100%    Windows (CRLF) / Unix (LF)    UTF-8",
                        cur_line + 1,
                        cur_col + 1,
                        status_mode,
                        snap.total_bytes
                    );

                    let status_color = if selection.mode == SelectionMode::Block {
                        COLOR_STATUS_ACCENT
                    } else {
                        COLOR_STATUS_TEXT
                    };

                    font_engine.draw_string(
                        &mut buffer,
                        width,
                        height,
                        8,
                        status_y + 5,
                        &status_text,
                        status_color,
                    );

                    let _ = buffer.present();
                }
                _ => {}
            },
            Event::AboutToWait => {
                if last_caret_blink.elapsed() >= Duration::from_millis(500) {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    });

    buffer_thread.join().unwrap();
}
