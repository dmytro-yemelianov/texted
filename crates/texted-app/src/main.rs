//! Texted - Ultra-Minimal High-Performance QNX-Microkernel GUI Text Editor
//!
//! Features:
//! - Dark Mode by default
//! - Word wrap (toggle with Alt+Z)
//! - Proper character column tracking (zero cursor double-jumping in Ukrainian / Unicode)
//! - Extended keyboard navigation:
//!   - Opt+Left / Opt+Right: Word navigation
//!   - Cmd+Left / Cmd+Right: Line start / Line end
//!   - Cmd+Up / Cmd+Down: Document start / Document end
//!   - Opt+Backspace: Delete previous word
//!   - Cmd+Backspace: Delete to line start
//!   - Opt+Delete / Cmd+Delete: Delete word / to line end
//! - Decoupled mouse wheel scrolling (no cursor lock)
//! - Interactive Find & Replace bar with [< Prev] and [Next >] buttons
//! - Full Ukrainian & Unicode typography via TrueType font rasterizer (ab_glyph)
//! - Block Select (Column / Rectangular selection): Alt/Option + drag or Cmd+B

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
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
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
    MoveWordLeft,
    MoveWordRight,
    Backspace,
    DeleteForward,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteToLineStart,
    DeleteToLineEnd,
    SetCursorOffset(usize),
    BlockInsert { start_line: usize, end_line: usize, col: usize, text: String },
    BlockDelete { start_line: usize, end_line: usize, start_col: usize, end_col: usize },
    BlockExtract { start_line: usize, end_line: usize, start_col: usize, end_col: usize },
    FindMatches { query: String },
    FindNext { query: String, start_offset: usize },
    FindPrev { query: String, start_offset: usize },
    ReplaceNext { query: String, replacement: String, start_offset: usize },
    ReplaceAll { query: String, replacement: String },
}

/// Responses from the Buffer Server
#[derive(Debug)]
pub enum BufferResponse {
    Snapshot(BufferSnapshot),
    Text(String),
    Matches(Vec<(usize, usize)>),
    Match(Option<(usize, usize)>),
    Count(usize),
    Success,
}

// Dark Mode Palette (VS Code / Zed Dark Charcoal aesthetic)
const COLOR_BG: u32 = 0x001E1E1E;              // Editor canvas dark charcoal
const COLOR_GUTTER_BG: u32 = 0x00181818;       // Slightly darker gutter
const COLOR_GUTTER_BORDER: u32 = 0x002B2B2B;   // Subtle separator border
const COLOR_GUTTER_TEXT: u32 = 0x006E6E6E;     // Muted line numbers
const COLOR_TEXT: u32 = 0x00D4D4D4;            // Crisp off-white text
const COLOR_CARET: u32 = 0x00FFFFFF;           // Sharp white caret
const COLOR_SELECTION: u32 = 0x00264F78;       // Stream selection dark blue
const COLOR_BLOCK_SELECTION: u32 = 0x001E4068; // Block selection deep blue
const COLOR_SEARCH_MATCH: u32 = 0x0052441C;    // Subtle dark amber highlight
const COLOR_SEARCH_ACTIVE: u32 = 0x008F6400;   // Bright gold active match
const COLOR_PANEL_BG: u32 = 0x00252526;        // Floating find bar dark panel
const COLOR_PANEL_BORDER: u32 = 0x003F3F46;    // Floating find bar border
const COLOR_INPUT_BG: u32 = 0x001E1E1E;        // Search input background
const COLOR_INPUT_BORDER: u32 = 0x003C3C3C;    // Search input border
const COLOR_INPUT_ACTIVE: u32 = 0x000E639C;    // Search input active border
const COLOR_BUTTON_BG: u32 = 0x00333333;       // Button background
const COLOR_BUTTON_BORDER: u32 = 0x004A4A4A;   // Button border
const COLOR_STATUS_BG: u32 = 0x00141414;       // Status bar dark
const COLOR_STATUS_BORDER: u32 = 0x00282828;   // Status bar border
const COLOR_STATUS_TEXT: u32 = 0x00888888;     // Status bar text
const COLOR_STATUS_ACCENT: u32 = 0x00569CD6;   // Status bar accent blue

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchBarMode {
    Closed,
    Find,
    Replace,
}

pub struct SearchState {
    pub mode: SearchBarMode,
    pub find_query: String,
    pub replace_query: String,
    pub active_field: usize, // 0 = Find, 1 = Replace
    pub matches: Vec<(usize, usize)>,
    pub current_match_idx: Option<usize>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            mode: SearchBarMode::Closed,
            find_query: String::new(),
            replace_query: String::new(),
            active_field: 0,
            matches: Vec::new(),
            current_match_idx: None,
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

fn draw_box(
    buffer: &mut [u32],
    stride: usize,
    max_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    bg_color: u32,
    border_color: u32,
) {
    draw_rect(buffer, stride, max_height, x, y, width, height, bg_color);
    draw_rect(buffer, stride, max_height, x, y, width, 1, border_color);
    draw_rect(buffer, stride, max_height, x, y + height.saturating_sub(1), width, 1, border_color);
    draw_rect(buffer, stride, max_height, x, y, 1, height, border_color);
    draw_rect(buffer, stride, max_height, x + width.saturating_sub(1), y, 1, height, border_color);
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

fn update_search_matches(
    client: &texted_kernel::ChannelClient<BufferRequest, BufferResponse>,
    search: &mut SearchState,
) {
    if search.find_query.is_empty() {
        search.matches.clear();
        search.current_match_idx = None;
        return;
    }

    if let Ok(BufferResponse::Matches(matches)) = client.send(BufferRequest::FindMatches {
        query: search.find_query.clone(),
    }) {
        search.matches = matches;
        if search.matches.is_empty() {
            search.current_match_idx = None;
        } else if search.current_match_idx.map_or(true, |idx| idx >= search.matches.len()) {
            search.current_match_idx = Some(0);
        }
    }
}

fn jump_to_match(
    client: &texted_kernel::ChannelClient<BufferRequest, BufferResponse>,
    search: &mut SearchState,
    selection: &mut SelectionState,
    match_idx: usize,
) {
    if let Some(&(start, end)) = search.matches.get(match_idx) {
        search.current_match_idx = Some(match_idx);
        if let Ok(BufferResponse::Snapshot(snap)) = client.send(BufferRequest::GetSnapshot) {
            let (s_line, s_col) = LayoutEngine::offset_to_line_col(&snap.content, start);
            let (e_line, e_col) = LayoutEngine::offset_to_line_col(&snap.content, end);

            *selection = SelectionState {
                mode: SelectionMode::Stream,
                anchor_line: s_line,
                anchor_col: s_col,
                active_line: e_line,
                active_col: e_col,
            };
            let _ = client.send(BufferRequest::SetCursorOffset(end));
        }
    }
}

fn find_next_match(
    client: &texted_kernel::ChannelClient<BufferRequest, BufferResponse>,
    search: &mut SearchState,
    selection: &mut SelectionState,
) {
    if search.matches.is_empty() {
        update_search_matches(client, search);
    }
    if search.matches.is_empty() {
        return;
    }
    let next_idx = match search.current_match_idx {
        Some(cur) => (cur + 1) % search.matches.len(),
        None => 0,
    };
    jump_to_match(client, search, selection, next_idx);
}

fn find_prev_match(
    client: &texted_kernel::ChannelClient<BufferRequest, BufferResponse>,
    search: &mut SearchState,
    selection: &mut SelectionState,
) {
    if search.matches.is_empty() {
        update_search_matches(client, search);
    }
    if search.matches.is_empty() {
        return;
    }
    let prev_idx = match search.current_match_idx {
        Some(cur) => {
            if cur == 0 {
                search.matches.len() - 1
            } else {
                cur - 1
            }
        }
        None => 0,
    };
    jump_to_match(client, search, selection, prev_idx);
}

fn find_word_boundaries(line_str: &str, click_col: usize) -> (usize, usize) {
    let chars: Vec<char> = line_str.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let col = click_col.min(chars.len().saturating_sub(1));
    let target_ch = chars[col];

    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';

    let mut start = col;
    let mut end = col + 1;

    if is_word_char(target_ch) {
        while start > 0 && is_word_char(chars[start - 1]) {
            start -= 1;
        }
        while end < chars.len() && is_word_char(chars[end]) {
            end += 1;
        }
    } else if target_ch.is_whitespace() {
        while start > 0 && chars[start - 1].is_whitespace() && chars[start - 1] != '\n' {
            start -= 1;
        }
        while end < chars.len() && chars[end].is_whitespace() && chars[end] != '\n' {
            end += 1;
        }
    } else {
        // Punctuation / symbols
        let is_punct = |c: char| !is_word_char(c) && !c.is_whitespace();
        while start > 0 && is_punct(chars[start - 1]) {
            start -= 1;
        }
        while end < chars.len() && is_punct(chars[end]) {
            end += 1;
        }
    }

    (start, end)
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
            .with_inner_size(winit::dpi::LogicalSize::new(980.0, 700.0))
            .build(&event_loop)
            .expect("Failed to create application window"),
    );

    // Initial content with full Ukrainian support & demonstration
    let (initial_content, active_path) = if let Some(ref path) = initial_path {
        match IoManager::open_file(path) {
            Ok(doc) => (doc.content, Some(path.clone())),
            Err(_) => (String::new(), Some(path.clone())),
        }
    } else {
        let sample = "Вітаємо у Texted! (Welcome to Texted!)\n\n\
Ультра-швидкий мінімальний текстовий редактор на основі мікроядра QNX.\n\
Повна підтримка української мови: Привіт, світе! Їжак, Євген, Ґрунт, Ігор.\n\n\
Гарячі клавіші та навігація:\n\
  - Opt+Left / Opt+Right: Перехід за словами\n\
  - Cmd+Left / Cmd+Right: На початок / в кінець рядка\n\
  - Cmd+Up / Cmd+Down: На початок / в кінець документа\n\
  - Opt+Backspace: Видалити попереднє слово\n\
  - Cmd+Backspace: Видалити до початку рядка\n\
  - Alt+Z: Увімкнути/вимкнути перенесення рядків (Word Wrap)\n\
  - Cmd+F: Знайти (з кнопками [< Попер.] та [Наступн. >])\n\
  - Cmd+G / Cmd+Shift+G: Наступний / попередній збіг\n\
  - Cmd+R: Знайти та замінити (Replace)\n\
  - Cmd+B (або Alt+перетягування): Блочне прямокутне виділення\n\
  - Cmd+A: Виділити все\n\
  - Cmd+C / Cmd+V: Копіювати / Вставити\n\
  - Cmd+Z / Cmd+Shift+Z: Undo / Redo\n\
  - Cmd+S: Зберегти файл атомарно\n\n\
Спробуйте затиснути Alt/Option та виділити стовпчик мишею, або натисніть Cmd+F для пошуку!\n";
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
                            BufferRequest::MoveWordLeft => {
                                buffer.move_word_left();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveWordRight => {
                                buffer.move_word_right();
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
                            BufferRequest::DeleteWordLeft => {
                                buffer.delete_word_left();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteWordRight => {
                                buffer.delete_word_right();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteToLineStart => {
                                buffer.delete_to_line_start();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteToLineEnd => {
                                buffer.delete_to_line_end();
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
                            BufferRequest::FindMatches { ref query } => {
                                let matches = buffer.find_matches(query);
                                req.reply(BufferResponse::Matches(matches));
                            }
                            BufferRequest::FindNext { ref query, start_offset } => {
                                let m = buffer.find_next(query, start_offset);
                                req.reply(BufferResponse::Match(m));
                            }
                            BufferRequest::FindPrev { ref query, start_offset } => {
                                let m = buffer.find_prev(query, start_offset);
                                req.reply(BufferResponse::Match(m));
                            }
                            BufferRequest::ReplaceNext { ref query, ref replacement, start_offset } => {
                                let m = buffer.replace_next(query, replacement, start_offset);
                                req.reply(BufferResponse::Match(m));
                            }
                            BufferRequest::ReplaceAll { ref query, ref replacement } => {
                                let count = buffer.replace_all(query, replacement);
                                req.reply(BufferResponse::Count(count));
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
    let mut word_wrap = true; // Word wrap enabled by default
    let mut current_modifiers = ModifiersState::empty();
    let mut last_caret_blink = Instant::now();
    let mut caret_visible = true;
    let mut current_file_path = active_path;

    // Viewport auto-scroll pacing: decoupled from passive mouse scrolling
    let mut snap_to_cursor_needed = false;

    // Mouse & Selection Tracking
    let mut selection = SelectionState::default();
    let mut search_state = SearchState::default();
    let mut is_mouse_dragging = false;
    let mut last_click_time = Instant::now();
    let mut click_count = 0usize;
    let mut last_mouse_pos = (0.0f32, 0.0f32);
    let mut show_perf_hud = false;
    let mut word_drag_anchor: Option<(usize, usize, usize)> = None; // (line, w_start, w_end)
    let mut line_drag_anchor: Option<usize> = None; // line

    println!("[Texted] GUI editor running: Dark Mode, Word Wrap, macOS Cmd/Opt navigation.");

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
                    // Decoupled mouse scrolling: does not stop by cursor
                    let lines_delta = match delta {
                        MouseScrollDelta::LineDelta(_, y) => (y * 2.5).round() as i32,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y / (font_engine.line_height_px as f64)).round() as i32,
                    };
                    if lines_delta > 0 {
                        top_line = top_line.saturating_sub(lines_delta.unsigned_abs() as usize);
                    } else if lines_delta < 0 {
                        top_line = top_line.saturating_add(lines_delta.unsigned_abs() as usize);
                    }
                    snap_to_cursor_needed = false;
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

                        if let Some((init_line, init_w_start, init_w_end)) = word_drag_anchor {
                            if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                let lines: Vec<&str> = snap.content.split('\n').collect();
                                let curr_line_str = lines.get(hover_line).copied().unwrap_or("");
                                let (curr_w_start, curr_w_end) = find_word_boundaries(curr_line_str, hover_col);

                                selection.mode = SelectionMode::Stream;
                                if hover_line > init_line || (hover_line == init_line && hover_col > init_w_end) {
                                    selection.anchor_line = init_line;
                                    selection.anchor_col = init_w_start;
                                    selection.active_line = hover_line;
                                    selection.active_col = curr_w_end;
                                } else if hover_line < init_line || (hover_line == init_line && hover_col < init_w_start) {
                                    selection.anchor_line = init_line;
                                    selection.anchor_col = init_w_end;
                                    selection.active_line = hover_line;
                                    selection.active_col = curr_w_start;
                                } else {
                                    selection.anchor_line = init_line;
                                    selection.anchor_col = init_w_start;
                                    selection.active_line = init_line;
                                    selection.active_col = init_w_end;
                                }

                                let offset = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                            }
                        } else if let Some(init_line) = line_drag_anchor {
                            if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                let lines: Vec<&str> = snap.content.split('\n').collect();
                                let total_lines = lines.len();

                                selection.mode = SelectionMode::Stream;
                                if hover_line >= init_line {
                                    let (e_line, e_col) = if hover_line + 1 < total_lines {
                                        (hover_line + 1, 0)
                                    } else {
                                        let last_line_len = lines.get(hover_line).map(|l| l.chars().count()).unwrap_or(0);
                                        (hover_line, last_line_len)
                                    };
                                    selection.anchor_line = init_line;
                                    selection.anchor_col = 0;
                                    selection.active_line = e_line;
                                    selection.active_col = e_col;
                                } else {
                                    let (a_line, a_col) = if init_line + 1 < total_lines {
                                        (init_line + 1, 0)
                                    } else {
                                        let init_len = lines.get(init_line).map(|l| l.chars().count()).unwrap_or(0);
                                        (init_line, init_len)
                                    };
                                    selection.anchor_line = a_line;
                                    selection.anchor_col = a_col;
                                    selection.active_line = hover_line;
                                    selection.active_col = 0;
                                }

                                let offset = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                            }
                        } else {
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

                    // Check interaction with Search Bar buttons
                    if search_state.mode != SearchBarMode::Closed {
                        let size = window.inner_size();
                        let panel_w = 480.0f32;
                        let panel_x = (size.width as f32 - panel_w - 12.0).max(0.0);
                        let panel_h = if search_state.mode == SearchBarMode::Replace { 68.0 } else { 36.0 };

                        if last_mouse_pos.0 >= panel_x && last_mouse_pos.0 <= panel_x + panel_w
                            && last_mouse_pos.1 >= 8.0 && last_mouse_pos.1 <= 8.0 + panel_h
                        {
                            if state == ElementState::Pressed {
                                // 1. Close [X] button
                                if last_mouse_pos.0 >= panel_x + panel_w - 28.0 && last_mouse_pos.1 <= 36.0 {
                                    search_state.mode = SearchBarMode::Closed;
                                    window.request_redraw();
                                    return;
                                }

                                // 2. [< Prev] button
                                let prev_btn_x = panel_x + 285.0;
                                if last_mouse_pos.0 >= prev_btn_x && last_mouse_pos.0 <= prev_btn_x + 55.0 && last_mouse_pos.1 <= 36.0 {
                                    find_prev_match(&buffer_client, &mut search_state, &mut selection);
                                    snap_to_cursor_needed = true;
                                    window.request_redraw();
                                    return;
                                }

                                // 3. [Next >] button
                                let next_btn_x = panel_x + 345.0;
                                if last_mouse_pos.0 >= next_btn_x && last_mouse_pos.0 <= next_btn_x + 55.0 && last_mouse_pos.1 <= 36.0 {
                                    find_next_match(&buffer_client, &mut search_state, &mut selection);
                                    snap_to_cursor_needed = true;
                                    window.request_redraw();
                                    return;
                                }

                                // 4. Switch between Find and Replace fields
                                if search_state.mode == SearchBarMode::Replace && last_mouse_pos.1 > 38.0 {
                                    // Replace or All buttons
                                    let repl_btn_x = panel_x + 245.0;
                                    if last_mouse_pos.0 >= repl_btn_x && last_mouse_pos.0 <= repl_btn_x + 70.0 {
                                        if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                            let _ = buffer_client.send(BufferRequest::ReplaceNext {
                                                query: search_state.find_query.clone(),
                                                replacement: search_state.replace_query.clone(),
                                                start_offset: snap.cursor_offset,
                                            });
                                            update_search_matches(&buffer_client, &mut search_state);
                                            find_next_match(&buffer_client, &mut search_state, &mut selection);
                                            snap_to_cursor_needed = true;
                                            window.request_redraw();
                                            return;
                                        }
                                    }

                                    let all_btn_x = panel_x + 325.0;
                                    if last_mouse_pos.0 >= all_btn_x && last_mouse_pos.0 <= all_btn_x + 45.0 {
                                        let _ = buffer_client.send(BufferRequest::ReplaceAll {
                                            query: search_state.find_query.clone(),
                                            replacement: search_state.replace_query.clone(),
                                        });
                                        update_search_matches(&buffer_client, &mut search_state);
                                        window.request_redraw();
                                        return;
                                    }

                                    search_state.active_field = 1;
                                } else {
                                    search_state.active_field = 0;
                                }
                                window.request_redraw();
                            }
                            return;
                        }
                    }

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
                                    let line_str = lines.get(click_line).copied().unwrap_or("");
                                    let (w_start, w_end) = find_word_boundaries(line_str, click_col);
                                    selection = SelectionState {
                                        mode: SelectionMode::Stream,
                                        anchor_line: click_line,
                                        anchor_col: w_start,
                                        active_line: click_line,
                                        active_col: w_end,
                                    };
                                    word_drag_anchor = Some((click_line, w_start, w_end));
                                    line_drag_anchor = None;
                                    let offset = TextBuffer::line_col_to_offset_str(&snap.content, click_line, w_end);
                                    let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                                } else if click_count >= 3 {
                                    // Triple click: Select Entire Line
                                    let total_lines = lines.len();
                                    let (e_line, e_col) = if click_line + 1 < total_lines {
                                        (click_line + 1, 0)
                                    } else {
                                        (click_line, line_len)
                                    };
                                    selection = SelectionState {
                                        mode: SelectionMode::Stream,
                                        anchor_line: click_line,
                                        anchor_col: 0,
                                        active_line: e_line,
                                        active_col: e_col,
                                    };
                                    line_drag_anchor = Some(click_line);
                                    word_drag_anchor = None;
                                    let offset = TextBuffer::line_col_to_offset_str(&snap.content, e_line, e_col);
                                    let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                                } else {
                                    // Single click
                                    word_drag_anchor = None;
                                    line_drag_anchor = None;
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
                            word_drag_anchor = None;
                            line_drag_anchor = None;
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
                        physical_key,
                        text,
                        ..
                    },
                    ..
                } => {
                    caret_visible = true;
                    last_caret_blink = Instant::now();
                    snap_to_cursor_needed = true; // Key input centers/snaps viewport to caret

                    let is_ctrl_or_cmd = current_modifiers.control_key() || current_modifiers.super_key();
                    let is_alt = current_modifiers.alt_key();
                    let is_shift = current_modifiers.shift_key();

                    let matches_key = |target_code: KeyCode, target_char: char| -> bool {
                        if let PhysicalKey::Code(code) = physical_key {
                            if code == target_code {
                                return true;
                            }
                        }
                        if let Key::Character(ref s) = logical_key {
                            if let Some(ch) = s.chars().next() {
                                if ch.eq_ignore_ascii_case(&target_char) {
                                    return true;
                                }
                            }
                        }
                        false
                    };

                    // 1. Check Global Shortcuts (Layout-independent via physical key & logical fallback)
                    if is_ctrl_or_cmd {
                        if matches_key(KeyCode::KeyA, 'a') {
                            // Cmd+A: Select All
                            if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                let lines: Vec<&str> = snap.content.split('\n').collect();
                                let total_lines = lines.len();
                                let last_line_len = lines.last().map(|l| l.chars().count()).unwrap_or(0);
                                selection = SelectionState {
                                    mode: SelectionMode::Stream,
                                    anchor_line: 0,
                                    anchor_col: 0,
                                    active_line: total_lines.saturating_sub(1),
                                    active_col: last_line_len,
                                };
                                let _ = buffer_client.send(BufferRequest::SetCursorOffset(snap.total_bytes));
                            }
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyC, 'c') {
                            // Cmd+C: Copy
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
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyV, 'v') {
                            // Cmd+V: Paste
                            if let Some(clip) = get_system_clipboard() {
                                if search_state.mode != SearchBarMode::Closed {
                                    if search_state.active_field == 0 {
                                        search_state.find_query.push_str(&clip);
                                        update_search_matches(&buffer_client, &mut search_state);
                                        find_next_match(&buffer_client, &mut search_state, &mut selection);
                                    } else {
                                        search_state.replace_query.push_str(&clip);
                                    }
                                } else if selection.mode == SelectionMode::Block {
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
                                    }

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
                                    let max_clip_line_len = clip_lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
                                    selection.anchor_line = min_l;
                                    selection.active_line = (min_l + clip_lines.len().saturating_sub(1)).max(max_l);
                                    selection.anchor_col = min_c + max_clip_line_len;
                                    selection.active_col = min_c + max_clip_line_len;
                                } else {
                                    if selection.mode == SelectionMode::Stream {
                                        if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                            let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                            let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                            let _ = buffer_client.send(BufferRequest::DeleteRange { start: off_a.min(off_b), end: off_a.max(off_b) });
                                        }
                                        selection.mode = SelectionMode::None;
                                    }
                                    let _ = buffer_client.send(BufferRequest::Insert {
                                        offset: usize::MAX,
                                        text: clip,
                                    });
                                }
                            }
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyX, 'x') {
                            // Cmd+X: Cut
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
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyF, 'f') {
                            // Cmd+F: Find
                            search_state.mode = SearchBarMode::Find;
                            search_state.active_field = 0;
                            if selection.mode == SelectionMode::Stream {
                                if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                    let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                    let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                    let min_off = off_a.min(off_b);
                                    let max_off = off_a.max(off_b);
                                    if min_off < max_off && max_off - min_off < 100 {
                                        search_state.find_query = snap.content[min_off..max_off].to_string();
                                        update_search_matches(&buffer_client, &mut search_state);
                                    }
                                }
                            }
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyR, 'r') {
                            // Cmd+R: Replace
                            search_state.mode = if search_state.mode == SearchBarMode::Replace {
                                SearchBarMode::Closed
                            } else {
                                SearchBarMode::Replace
                            };
                            search_state.active_field = 0;
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyG, 'g') {
                            // Cmd+G: Find Next (or Find Prev if shift held)
                            if current_modifiers.shift_key() {
                                find_prev_match(&buffer_client, &mut search_state, &mut selection);
                            } else {
                                find_next_match(&buffer_client, &mut search_state, &mut selection);
                            }
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyZ, 'z') {
                            if current_modifiers.shift_key() {
                                let _ = buffer_client.send(BufferRequest::Redo);
                            } else {
                                let _ = buffer_client.send(BufferRequest::Undo);
                            }
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyY, 'y') {
                            let _ = buffer_client.send(BufferRequest::Redo);
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyL, 'l') {
                            show_line_numbers = !show_line_numbers;
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyB, 'b') {
                            selection.mode = if selection.mode == SelectionMode::Block {
                                SelectionMode::None
                            } else {
                                SelectionMode::Block
                            };
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyS, 's') {
                            if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                let save_path = current_file_path.clone().unwrap_or_else(|| PathBuf::from("note.txt"));
                                if IoManager::save_atomic(&save_path, &snap.content, false, LineEnding::Lf).is_ok() {
                                    current_file_path = Some(save_path.clone());
                                    window.set_title(&format!("{} - Texted", save_path.display()));
                                }
                            }
                            window.request_redraw();
                            return;
                        }
                    }

                    // Alt+Z: Toggle Word Wrap (Layout-independent)
                    if is_alt && matches_key(KeyCode::KeyZ, 'z') {
                        word_wrap = !word_wrap;
                        window.request_redraw();
                        return;
                    }

                    // 2. Check Search Bar Input (if search bar is active)
                    if search_state.mode != SearchBarMode::Closed {
                        match logical_key {
                            Key::Named(NamedKey::Escape) => {
                                search_state.mode = SearchBarMode::Closed;
                                window.request_redraw();
                                return;
                            }
                            Key::Named(NamedKey::Tab) => {
                                if search_state.mode == SearchBarMode::Replace {
                                    search_state.active_field = (search_state.active_field + 1) % 2;
                                }
                                window.request_redraw();
                                return;
                            }
                            Key::Named(NamedKey::Enter) => {
                                if search_state.mode == SearchBarMode::Replace && search_state.active_field == 1 {
                                    if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                        let _ = buffer_client.send(BufferRequest::ReplaceNext {
                                            query: search_state.find_query.clone(),
                                            replacement: search_state.replace_query.clone(),
                                            start_offset: snap.cursor_offset,
                                        });
                                        update_search_matches(&buffer_client, &mut search_state);
                                        find_next_match(&buffer_client, &mut search_state, &mut selection);
                                    }
                                } else {
                                    if current_modifiers.shift_key() {
                                        find_prev_match(&buffer_client, &mut search_state, &mut selection);
                                    } else {
                                        find_next_match(&buffer_client, &mut search_state, &mut selection);
                                    }
                                }
                                window.request_redraw();
                                return;
                            }
                            Key::Named(NamedKey::Backspace) => {
                                if search_state.active_field == 0 {
                                    search_state.find_query.pop();
                                    update_search_matches(&buffer_client, &mut search_state);
                                    if !search_state.find_query.is_empty() {
                                        find_next_match(&buffer_client, &mut search_state, &mut selection);
                                    }
                                } else {
                                    search_state.replace_query.pop();
                                }
                                window.request_redraw();
                                return;
                            }
                            _ => {
                                if let Some(t) = text {
                                    for ch in t.chars() {
                                        if !ch.is_control() {
                                            if search_state.active_field == 0 {
                                                search_state.find_query.push(ch);
                                                update_search_matches(&buffer_client, &mut search_state);
                                                find_next_match(&buffer_client, &mut search_state, &mut selection);
                                            } else {
                                                search_state.replace_query.push(ch);
                                            }
                                        }
                                    }
                                    window.request_redraw();
                                    return;
                                }
                            }
                        }
                    }

                    // 3. Document Navigation: Arrows accepting Cmd / Option / Shift modifiers
                    match logical_key {
                        Key::Named(NamedKey::F3) => {
                            show_perf_hud = !show_perf_hud;
                            window.request_redraw();
                            return;
                        }
                        Key::Named(NamedKey::Escape) => {
                            selection.mode = SelectionMode::None;
                            window.request_redraw();
                            return;
                        }
                        Key::Named(NamedKey::ArrowLeft)
                        | Key::Named(NamedKey::ArrowRight)
                        | Key::Named(NamedKey::ArrowUp)
                        | Key::Named(NamedKey::ArrowDown)
                        | Key::Named(NamedKey::Home)
                        | Key::Named(NamedKey::End)
                        | Key::Named(NamedKey::PageUp)
                        | Key::Named(NamedKey::PageDown) => {
                            // 1. Shift+Option+Up/Down: Block/Column Selection Mode
                            if is_shift && is_alt && (matches!(logical_key, Key::Named(NamedKey::ArrowUp)) || matches!(logical_key, Key::Named(NamedKey::ArrowDown))) {
                                if selection.mode != SelectionMode::Block {
                                    if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                        let (cur_l, cur_c) = TextBuffer::offset_to_line_col_str(&snap.content, snap.cursor_offset);
                                        selection.mode = SelectionMode::Block;
                                        selection.anchor_line = cur_l;
                                        selection.anchor_col = cur_c;
                                        selection.active_line = cur_l;
                                        selection.active_col = cur_c;
                                    }
                                }
                                if matches!(logical_key, Key::Named(NamedKey::ArrowUp)) {
                                    selection.active_line = selection.active_line.saturating_sub(1);
                                    let _ = buffer_client.send(BufferRequest::MoveUp);
                                } else {
                                    selection.active_line += 1;
                                    let _ = buffer_client.send(BufferRequest::MoveDown);
                                }
                                window.request_redraw();
                                return;
                            }

                            // 2. Stream Range Selection via Shift
                            if is_shift {
                                if selection.mode != SelectionMode::Stream {
                                    if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                        let (cur_l, cur_c) = TextBuffer::offset_to_line_col_str(&snap.content, snap.cursor_offset);
                                        selection.mode = SelectionMode::Stream;
                                        selection.anchor_line = cur_l;
                                        selection.anchor_col = cur_c;
                                        selection.active_line = cur_l;
                                        selection.active_col = cur_c;
                                    }
                                }
                            } else {
                                // If Shift is NOT held, collapse any existing Stream selection
                                if selection.mode == SelectionMode::Stream {
                                    if matches!(logical_key, Key::Named(NamedKey::ArrowLeft)) && !is_ctrl_or_cmd && !is_alt {
                                        if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                            let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                            let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                            let _ = buffer_client.send(BufferRequest::SetCursorOffset(off_a.min(off_b)));
                                        }
                                        selection.mode = SelectionMode::None;
                                        window.request_redraw();
                                        return;
                                    } else if matches!(logical_key, Key::Named(NamedKey::ArrowRight)) && !is_ctrl_or_cmd && !is_alt {
                                        if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                            let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                            let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                            let _ = buffer_client.send(BufferRequest::SetCursorOffset(off_a.max(off_b)));
                                        }
                                        selection.mode = SelectionMode::None;
                                        window.request_redraw();
                                        return;
                                    }
                                }
                                selection.mode = SelectionMode::None;
                            }

                            // 3. Movement execution
                            match logical_key {
                                Key::Named(NamedKey::ArrowLeft) => {
                                    if is_ctrl_or_cmd {
                                        let _ = buffer_client.send(BufferRequest::MoveHome); // Cmd+Left: Line start
                                    } else if is_alt {
                                        let _ = buffer_client.send(BufferRequest::MoveWordLeft); // Opt+Left: Word left
                                    } else {
                                        let _ = buffer_client.send(BufferRequest::MoveLeft);
                                    }
                                }
                                Key::Named(NamedKey::ArrowRight) => {
                                    if is_ctrl_or_cmd {
                                        let _ = buffer_client.send(BufferRequest::MoveEnd); // Cmd+Right: Line end
                                    } else if is_alt {
                                        let _ = buffer_client.send(BufferRequest::MoveWordRight); // Opt+Right: Word right
                                    } else {
                                        let _ = buffer_client.send(BufferRequest::MoveRight);
                                    }
                                }
                                Key::Named(NamedKey::ArrowUp) => {
                                    if is_ctrl_or_cmd {
                                        let _ = buffer_client.send(BufferRequest::SetCursorOffset(0)); // Cmd+Up: Doc start
                                    } else if is_alt {
                                        for _ in 0..5 {
                                            let _ = buffer_client.send(BufferRequest::MoveUp);
                                        }
                                    } else {
                                        let _ = buffer_client.send(BufferRequest::MoveUp);
                                    }
                                }
                                Key::Named(NamedKey::ArrowDown) => {
                                    if is_ctrl_or_cmd {
                                        if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                            let _ = buffer_client.send(BufferRequest::SetCursorOffset(snap.total_bytes)); // Cmd+Down: Doc end
                                        }
                                    } else if is_alt {
                                        for _ in 0..5 {
                                            let _ = buffer_client.send(BufferRequest::MoveDown);
                                        }
                                    } else {
                                        let _ = buffer_client.send(BufferRequest::MoveDown);
                                    }
                                }
                                Key::Named(NamedKey::PageUp) => {
                                    for _ in 0..15 {
                                        let _ = buffer_client.send(BufferRequest::MoveUp);
                                    }
                                }
                                Key::Named(NamedKey::PageDown) => {
                                    for _ in 0..15 {
                                        let _ = buffer_client.send(BufferRequest::MoveDown);
                                    }
                                }
                                Key::Named(NamedKey::Home) => {
                                    let _ = buffer_client.send(BufferRequest::MoveHome);
                                }
                                Key::Named(NamedKey::End) => {
                                    let _ = buffer_client.send(BufferRequest::MoveEnd);
                                }
                                _ => {}
                            }

                            // 4. Update active head of selection if Shift is held
                            if is_shift {
                                if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                    let (cur_l, cur_c) = TextBuffer::offset_to_line_col_str(&snap.content, snap.cursor_offset);
                                    selection.active_line = cur_l;
                                    selection.active_col = cur_c;
                                }
                            }
                            window.request_redraw();
                            return;
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
                                if is_ctrl_or_cmd {
                                    let _ = buffer_client.send(BufferRequest::DeleteToLineStart); // Cmd+Backspace
                                } else if is_alt {
                                    let _ = buffer_client.send(BufferRequest::DeleteWordLeft); // Opt+Backspace
                                } else {
                                    let _ = buffer_client.send(BufferRequest::Backspace);
                                }
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
                                } else {
                                    let _ = buffer_client.send(BufferRequest::BlockDelete {
                                        start_line: min_l,
                                        end_line: max_l,
                                        start_col: min_c,
                                        end_col: min_c + 1,
                                    });
                                }
                            } else if selection.mode == SelectionMode::Stream {
                                if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                    let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                    let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                    let _ = buffer_client.send(BufferRequest::DeleteRange { start: off_a.min(off_b), end: off_a.max(off_b) });
                                }
                                selection.mode = SelectionMode::None;
                            } else {
                                if is_ctrl_or_cmd {
                                    let _ = buffer_client.send(BufferRequest::DeleteToLineEnd); // Cmd+Delete
                                } else if is_alt {
                                    let _ = buffer_client.send(BufferRequest::DeleteWordRight); // Opt+Delete
                                } else {
                                    let _ = buffer_client.send(BufferRequest::DeleteForward);
                                }
                            }
                        }
                        Key::Named(NamedKey::Enter) => {
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
                                }
                            }
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
                                let max_c = selection.anchor_col.max(selection.active_col);

                                if min_c < max_c {
                                    let _ = buffer_client.send(BufferRequest::BlockDelete {
                                        start_line: min_l,
                                        end_line: max_l,
                                        start_col: min_c,
                                        end_col: max_c,
                                    });
                                }
                                let _ = buffer_client.send(BufferRequest::BlockInsert {
                                    start_line: min_l,
                                    end_line: max_l,
                                    col: min_c,
                                    text: "    ".to_string(),
                                });
                                selection.anchor_line = min_l;
                                selection.active_line = max_l;
                                selection.anchor_col = min_c + 4;
                                selection.active_col = min_c + 4;
                            } else {
                                let _ = buffer_client.send(BufferRequest::Insert {
                                    offset: usize::MAX,
                                    text: "    ".to_string(),
                                });
                            }
                        }
                        _ => {
                            if let Some(t) = text {
                                for ch in t.chars() {
                                    if !ch.is_control() {
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

                                            let _ = buffer_client.send(BufferRequest::BlockInsert {
                                                start_line: min_l,
                                                end_line: max_l,
                                                col: min_c,
                                                text: ch.to_string(),
                                            });
                                            selection.anchor_line = min_l;
                                            selection.active_line = max_l;
                                            selection.anchor_col = min_c + 1;
                                            selection.active_col = min_c + 1;
                                        } else {
                                            if selection.mode == SelectionMode::Stream {
                                                if let Ok(BufferResponse::Snapshot(snap)) = buffer_client.send(BufferRequest::GetSnapshot) {
                                                    let off_a = TextBuffer::line_col_to_offset_str(&snap.content, selection.anchor_line, selection.anchor_col);
                                                    let off_b = TextBuffer::line_col_to_offset_str(&snap.content, selection.active_line, selection.active_col);
                                                    let min_off = off_a.min(off_b);
                                                    let max_off = off_a.max(off_b);
                                                    let _ = buffer_client.send(BufferRequest::DeleteRange { start: min_off, end: max_off });
                                                }
                                                selection.mode = SelectionMode::None;
                                            }

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
                    let frame_start = Instant::now();
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

                    let (cur_line, cur_col) = LayoutEngine::offset_to_line_col(&snap.content, snap.cursor_offset);

                    // 2. Viewport auto-scrolling: only triggered on keyboard interaction, decoupled from mouse scrolling
                    let visible_rows = (height.saturating_sub(28) as f32 / font_engine.line_height_px).floor() as usize;
                    if snap_to_cursor_needed {
                        if cur_line < top_line {
                            top_line = cur_line;
                        } else if cur_line >= top_line + visible_rows.saturating_sub(1) {
                            top_line = cur_line.saturating_sub(visible_rows.saturating_sub(2));
                        }
                        snap_to_cursor_needed = false;
                    }

                    let gutter_w = if show_line_numbers { 58.0 } else { 12.0 };
                    let config = ViewportConfig {
                        width_px: width as f32,
                        height_px: (height.saturating_sub(28)) as f32,
                        line_height_px: font_engine.line_height_px,
                        char_width_px: font_engine.char_width_px,
                        top_line,
                        word_wrap,
                        show_line_numbers,
                        line_number_gutter_width_px: gutter_w,
                    };

                    let t_layout = Instant::now();
                    let layout = LayoutEngine::compute_layout(&snap, &config, None);
                    let layout_time = t_layout.elapsed();

                    // 3. Clear Screen to Dark Charcoal
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

                    let text_start_x = gutter_w as usize + 6;

                    // 5. Render Search Match Highlights
                    if !search_state.find_query.is_empty() {
                        let lines: Vec<&str> = snap.content.split('\n').collect();

                        for (m_idx, &(m_start, m_end)) in search_state.matches.iter().enumerate() {
                            let (m_s_l, m_s_c) = LayoutEngine::offset_to_line_col(&snap.content, m_start);
                            let (m_e_l, m_e_c) = LayoutEngine::offset_to_line_col(&snap.content, m_end);

                            let is_active = search_state.current_match_idx == Some(m_idx);
                            let highlight_color = if is_active {
                                COLOR_SEARCH_ACTIVE
                            } else {
                                COLOR_SEARCH_MATCH
                            };

                            for l_idx in m_s_l..=m_e_l {
                                for v_line in &layout.visible_lines {
                                    if v_line.line_index == l_idx {
                                        let row_y = v_line.y_px as usize;
                                        let line_len = lines.get(l_idx).map(|l| l.chars().count()).unwrap_or(0);

                                        let from_c = if l_idx == m_s_l { m_s_c } else { 0 };
                                        let to_c = if l_idx == m_e_l { m_e_c } else { line_len };

                                        let h_x = text_start_x + (from_c as f32 * font_engine.char_width_px).round() as usize;
                                        let h_w = (((to_c.saturating_sub(from_c)) as f32 * font_engine.char_width_px).round() as usize).max(4);

                                        draw_rect(
                                            &mut buffer,
                                            width,
                                            height,
                                            h_x,
                                            row_y + 2,
                                            h_w,
                                            font_engine.line_height_px as usize,
                                            highlight_color,
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // 6. Render Selection Highlights
                    if selection.mode == SelectionMode::Block {
                        let min_l = selection.anchor_line.min(selection.active_line);
                        let max_l = selection.anchor_line.max(selection.active_line);
                        let min_c = selection.anchor_col.min(selection.active_col);
                        let max_c = selection.anchor_col.max(selection.active_col);

                        for l_idx in min_l..=max_l {
                            for v_line in &layout.visible_lines {
                                if v_line.line_index == l_idx {
                                    let row_y = v_line.y_px as usize;
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
                        }
                    } else if selection.mode == SelectionMode::Stream {
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

                        if (s_line, s_col) != (e_line, e_col) {
                            for l_idx in s_line..=e_line {
                                for v_line in &layout.visible_lines {
                                    if v_line.line_index == l_idx {
                                        let row_y = v_line.y_px as usize;
                                        let line_len = lines.get(l_idx).map(|l| l.chars().count()).unwrap_or(0);

                                        let from_c = if l_idx == s_line { s_col } else { 0 };
                                        let to_c = if l_idx == e_line { e_col } else { line_len };

                                        if l_idx == e_line && from_c >= to_c {
                                            continue;
                                        }

                                        let sel_x = text_start_x + (from_c as f32 * font_engine.char_width_px).round() as usize;
                                        let base_w = (to_c.saturating_sub(from_c)) as f32 * font_engine.char_width_px;
                                        let sel_w = if l_idx < e_line {
                                            (base_w + font_engine.char_width_px * 0.6).round() as usize
                                        } else {
                                            base_w.round() as usize
                                        };
                                        if sel_w == 0 {
                                            continue;
                                        }

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
                        }
                    }

                    // 7. Render Visible Lines (with antialiased Ukrainian font support)
                    for line in &layout.visible_lines {
                        let y_int = line.y_px as usize;
                        if y_int + (font_engine.line_height_px as usize) > height.saturating_sub(28) {
                            break;
                        }

                        // Gutter line number (only on non-continuation rows)
                        if show_line_numbers && !line.is_continuation {
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

                    // 8. Render Caret (character-based positioning)
                    if caret_visible && layout.caret_rect.x >= 0.0 {
                        let cx = text_start_x + (cur_col as f32 * font_engine.char_width_px).round() as usize;
                        let cy = layout.caret_rect.y as usize + 2;

                        if cy + (font_engine.line_height_px as usize) <= height.saturating_sub(28) {
                            draw_rect(&mut buffer, width, height, cx, cy, 2, font_engine.line_height_px as usize, COLOR_CARET);
                        }
                    }

                    // 9. Render Floating Find & Replace Bar with Interactive [< Prev] and [Next >] Buttons
                    if search_state.mode != SearchBarMode::Closed {
                        let panel_w = 480usize;
                        let panel_h = if search_state.mode == SearchBarMode::Replace { 68usize } else { 36usize };
                        let panel_x = width.saturating_sub(panel_w + 16);
                        let panel_y = 10usize;

                        draw_box(&mut buffer, width, height, panel_x, panel_y, panel_w, panel_h, COLOR_PANEL_BG, COLOR_PANEL_BORDER);

                        // Find Input Box
                        let input_x = panel_x + 50;
                        let input_y = panel_y + 6;
                        let input_w = 160usize;
                        let input_h = 24usize;
                        let border_col = if search_state.active_field == 0 { COLOR_INPUT_ACTIVE } else { COLOR_INPUT_BORDER };
                        draw_box(&mut buffer, width, height, input_x, input_y, input_w, input_h, COLOR_INPUT_BG, border_col);

                        font_engine.draw_string(&mut buffer, width, height, panel_x + 8, input_y + 3, "Find:", COLOR_STATUS_TEXT);

                        let query_display = &search_state.find_query;
                        font_engine.draw_string(&mut buffer, width, height, input_x + 5, input_y + 3, query_display, COLOR_TEXT);

                        // Cursor in Find box
                        if search_state.active_field == 0 && caret_visible {
                            let cur_x = input_x + 5 + (query_display.chars().count() as f32 * font_engine.char_width_px).round() as usize;
                            if cur_x < input_x + input_w - 4 {
                                draw_rect(&mut buffer, width, height, cur_x, input_y + 4, 2, input_h - 8, COLOR_CARET);
                            }
                        }

                        // Match count info
                        let match_info = if search_state.find_query.is_empty() {
                            "".to_string()
                        } else if search_state.matches.is_empty() {
                            "0/0".to_string()
                        } else {
                            let cur_idx = search_state.current_match_idx.map_or(0, |i| i + 1);
                            format!("{}/{}", cur_idx, search_state.matches.len())
                        };
                        font_engine.draw_string(&mut buffer, width, height, input_x + input_w + 8, input_y + 3, &match_info, COLOR_STATUS_TEXT);

                        // Interactive [< Prev] button
                        let prev_btn_x = panel_x + 285;
                        draw_box(&mut buffer, width, height, prev_btn_x, input_y, 52, input_h, COLOR_BUTTON_BG, COLOR_BUTTON_BORDER);
                        font_engine.draw_string(&mut buffer, width, height, prev_btn_x + 6, input_y + 3, "< Prev", COLOR_TEXT);

                        // Interactive [Next >] button
                        let next_btn_x = panel_x + 343;
                        draw_box(&mut buffer, width, height, next_btn_x, input_y, 52, input_h, COLOR_BUTTON_BG, COLOR_BUTTON_BORDER);
                        font_engine.draw_string(&mut buffer, width, height, next_btn_x + 6, input_y + 3, "Next >", COLOR_TEXT);

                        // Close button [x]
                        font_engine.draw_string(&mut buffer, width, height, panel_x + panel_w - 22, input_y + 3, "X", COLOR_GUTTER_TEXT);

                        // Replace Row (if in Replace mode)
                        if search_state.mode == SearchBarMode::Replace {
                            let repl_y = panel_y + 36;
                            let repl_border = if search_state.active_field == 1 { COLOR_INPUT_ACTIVE } else { COLOR_INPUT_BORDER };
                            draw_box(&mut buffer, width, height, input_x, repl_y, input_w, input_h, COLOR_INPUT_BG, repl_border);

                            font_engine.draw_string(&mut buffer, width, height, panel_x + 8, repl_y + 3, "Repl:", COLOR_STATUS_TEXT);
                            font_engine.draw_string(&mut buffer, width, height, input_x + 5, repl_y + 3, &search_state.replace_query, COLOR_TEXT);

                            if search_state.active_field == 1 && caret_visible {
                                let cur_x = input_x + 5 + (search_state.replace_query.chars().count() as f32 * font_engine.char_width_px).round() as usize;
                                if cur_x < input_x + input_w - 4 {
                                    draw_rect(&mut buffer, width, height, cur_x, repl_y + 4, 2, input_h - 8, COLOR_CARET);
                                }
                            }

                            // Interactive [Replace] button
                            let repl_btn_x = panel_x + 245;
                            draw_box(&mut buffer, width, height, repl_btn_x, repl_y, 65, input_h, COLOR_BUTTON_BG, COLOR_BUTTON_BORDER);
                            font_engine.draw_string(&mut buffer, width, height, repl_btn_x + 6, repl_y + 3, "Replace", COLOR_TEXT);

                            // Interactive [All] button
                            let all_btn_x = panel_x + 320;
                            draw_box(&mut buffer, width, height, all_btn_x, repl_y, 42, input_h, COLOR_BUTTON_BG, COLOR_BUTTON_BORDER);
                            font_engine.draw_string(&mut buffer, width, height, all_btn_x + 8, repl_y + 3, "All", COLOR_TEXT);
                        }
                    }

                    // 10. Render Status Bar
                    let status_y = height.saturating_sub(28);
                    draw_rect(&mut buffer, width, height, 0, status_y, width, 1, COLOR_STATUS_BORDER);
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
                        SelectionMode::None => {
                            if search_state.mode != SearchBarMode::Closed {
                                "[SEARCH]".to_string()
                            } else {
                                "[NORMAL]".to_string()
                            }
                        }
                    };

                    let wrap_text = if word_wrap { "Wrap: ON" } else { "Wrap: OFF" };

                    let status_text = format!(
                        "  Ln {}, Col {}    {}    {}    {} chars    100%    UTF-8    LF",
                        cur_line + 1,
                        cur_col + 1,
                        status_mode,
                        wrap_text,
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

                    if show_perf_hud {
                        let hud_w = 260usize;
                        let hud_h = 96usize;
                        let hud_x = width.saturating_sub(hud_w + 16);
                        let hud_y = 12usize;

                        draw_rect(&mut buffer, width, height, hud_x, hud_y, hud_w, hud_h, 0xEE1A1A1A);
                        draw_rect(&mut buffer, width, height, hud_x, hud_y, hud_w, 1, 0xFF3E3E3E);
                        draw_rect(&mut buffer, width, height, hud_x, hud_y + hud_h - 1, hud_w, 1, 0xFF3E3E3E);
                        draw_rect(&mut buffer, width, height, hud_x, hud_y, 1, hud_h, 0xFF3E3E3E);
                        draw_rect(&mut buffer, width, height, hud_x + hud_w - 1, hud_y, 1, hud_h, 0xFF3E3E3E);

                        let cur_frame_time = frame_start.elapsed();
                        let fps = 1000.0 / (cur_frame_time.as_secs_f64() * 1000.0).max(0.01);
                        let l1 = format!("PERF HUD [F3]   ~{:.0} FPS", fps);
                        let l2 = format!("Frame time  : {:.2} ms", cur_frame_time.as_secs_f64() * 1000.0);
                        let l3 = format!("Layout time : {:.1} µs", layout_time.as_secs_f64() * 1_000_000.0);
                        let l4 = format!("Doc bytes   : {} B", snap.total_bytes);
                        let l5 = "RAM footprint: ~28.9 MB".to_string();

                        font_engine.draw_string(&mut buffer, width, height, hud_x + 10, hud_y + 6, &l1, 0xFF569CD6);
                        font_engine.draw_string(&mut buffer, width, height, hud_x + 10, hud_y + 24, &l2, 0xFF4EC9B0);
                        font_engine.draw_string(&mut buffer, width, height, hud_x + 10, hud_y + 42, &l3, 0xFFDCDCAA);
                        font_engine.draw_string(&mut buffer, width, height, hud_x + 10, hud_y + 60, &l4, 0xFFCCCCCC);
                        font_engine.draw_string(&mut buffer, width, height, hud_x + 10, hud_y + 78, &l5, 0xFF9CDCFE);
                    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_word_boundaries_alphanumeric_and_underscores() {
        let line = "let my_variable_name = 12345;";
        // 'm' in my_variable_name is at index 4
        let (s, e) = find_word_boundaries(line, 4);
        assert_eq!(&line[s..e], "my_variable_name");

        // inside 12345 (index 24)
        let (s, e) = find_word_boundaries(line, 24);
        assert_eq!(&line[s..e], "12345");
    }

    #[test]
    fn test_find_word_boundaries_ukrainian() {
        let line = "Привіт, чарівний світе!";
        let (s, e) = find_word_boundaries(line, 2);
        let chars: Vec<char> = line.chars().collect();
        let selected: String = chars[s..e].iter().collect();
        assert_eq!(selected, "Привіт");

        let (s2, e2) = find_word_boundaries(line, 10);
        let selected2: String = chars[s2..e2].iter().collect();
        assert_eq!(selected2, "чарівний");
    }

    #[test]
    fn test_find_word_boundaries_punctuation_and_whitespace() {
        let line = "foo == bar   baz;";
        // '==' is at index 4..6
        let (s, e) = find_word_boundaries(line, 5);
        assert_eq!(&line[s..e], "==");

        // spaces at index 11
        let (s, e) = find_word_boundaries(line, 11);
        assert_eq!(&line[s..e], "   ");
    }

    #[test]
    fn test_find_word_boundaries_empty_and_out_of_bounds() {
        assert_eq!(find_word_boundaries("", 0), (0, 0));
        assert_eq!(find_word_boundaries("", 5), (0, 0));

        let line = "hello";
        let (s, e) = find_word_boundaries(line, 10);
        assert_eq!(&line[s..e], "hello");
    }
}
