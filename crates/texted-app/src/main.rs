//! Minimal GPU text editor with an isolated buffer server.
//!
//! Features:
//! - Metal rendering through wgpu, with cached glyph masks and instanced quads
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
//! - Ukrainian and Unicode glyphs via the TrueType rasterizer (ab_glyph)
//! - Block Select (Column / Rectangular selection): Alt/Option + drag or Cmd+B

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
mod documents;
mod gpu;
mod graphics;
mod native_macos;
use documents::{Action as DocumentAction, Documents, Effect as DocumentEffect};
#[derive(Debug, Clone)]
pub struct AppEvent(DocumentAction);
mod horizontal_scroll;
use graphics::{draw_box, draw_rect, DrawList, FontEngine};
use horizontal_scroll::{HorizontalScroll, BAR_HEIGHT};
use std::thread;
use std::time::{Duration, Instant};

use texted_buffer::{BufferSnapshot, TextBuffer};
use texted_io::LineEnding;
use texted_kernel::{ChannelEvent, ChannelPair, CH_BUFFER};
use texted_layout::{LayoutEngine, ViewportConfig};
use winit::event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::WindowBuilder;

/// Messages sent to the Buffer Server
#[derive(Debug)]
pub enum BufferRequest {
    Shutdown,
    BreakUndoGroup,
    LoadDocument(ropey::Rope),
    Insert {
        offset: usize,
        text: String,
    },
    DeleteRange {
        start: usize,
        end: usize,
    },
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
    BlockInsert {
        start_line: usize,
        end_line: usize,
        col: usize,
        text: String,
    },
    BlockDelete {
        start_line: usize,
        end_line: usize,
        start_col: usize,
        end_col: usize,
    },
    BlockExtract {
        start_line: usize,
        end_line: usize,
        start_col: usize,
        end_col: usize,
    },
    CommitReplacement {
        revision: u64,
        cursor: usize,
        edited: BufferSnapshot,
    },
}

/// Responses from the Buffer Server
#[derive(Debug)]
pub enum BufferResponse {
    Snapshot(BufferSnapshot),
    Text(String),
    Committed(bool),
    Success,
}

// Dark Mode Palette (VS Code / Zed Dark Charcoal aesthetic)
const COLOR_BG: u32 = 0x001E1E1E; // Editor canvas dark charcoal
const COLOR_GUTTER_BG: u32 = 0x00181818; // Slightly darker gutter
const COLOR_GUTTER_BORDER: u32 = 0x002B2B2B; // Subtle separator border
const COLOR_GUTTER_TEXT: u32 = 0x006E6E6E; // Muted line numbers
const COLOR_TEXT: u32 = 0x00D4D4D4; // Crisp off-white text
const COLOR_CARET: u32 = 0x00FFFFFF; // Sharp white caret
const COLOR_SELECTION: u32 = 0x00264F78; // Stream selection dark blue
const COLOR_BLOCK_SELECTION: u32 = 0x001E4068; // Block selection deep blue
const COLOR_SEARCH_MATCH: u32 = 0x0052441C; // Subtle dark amber highlight
const COLOR_SEARCH_ACTIVE: u32 = 0x008F6400; // Bright gold active match
const COLOR_PANEL_BG: u32 = 0x00252526; // Floating find bar dark panel
const COLOR_PANEL_BORDER: u32 = 0x003F3F46; // Floating find bar border
const COLOR_INPUT_BG: u32 = 0x001E1E1E; // Search input background
const COLOR_INPUT_BORDER: u32 = 0x003C3C3C; // Search input border
const COLOR_INPUT_ACTIVE: u32 = 0x000E639C; // Search input active border
const COLOR_BUTTON_BG: u32 = 0x00333333; // Button background
const COLOR_BUTTON_BORDER: u32 = 0x004A4A4A; // Button border
const COLOR_STATUS_BG: u32 = 0x00141414; // Status bar dark
const COLOR_STATUS_BORDER: u32 = 0x00282828; // Status bar border
const COLOR_STATUS_TEXT: u32 = 0x00888888; // Status bar text
const COLOR_STATUS_ACCENT: u32 = 0x00569CD6; // Status bar accent blue

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

type MutationPlan = (u64, usize, Option<BufferSnapshot>);
type SearchResult = (u64, String, Vec<(usize, usize)>);
pub struct SearchState {
    pub mode: SearchBarMode,
    pub find_query: String,
    pub replace_query: String,
    pub active_field: usize, // 0 = Find, 1 = Replace
    pub matches: Vec<(usize, usize)>,
    pub current_match_idx: Option<usize>,
    worker: std::sync::mpsc::Sender<(BufferSnapshot, String)>,
    results: std::sync::mpsc::Receiver<SearchResult>,
    submitted: Option<(u64, String)>,
    pending: bool,
    pending_navigation: Option<bool>,
    mutation: Option<std::sync::mpsc::Receiver<MutationPlan>>,
}

impl Default for SearchState {
    fn default() -> Self {
        let (worker, jobs) = std::sync::mpsc::channel::<(BufferSnapshot, String)>();
        let (out, results) = std::sync::mpsc::channel();
        thread::spawn(move || {
            while let Ok(mut job) = jobs.recv() {
                while let Ok(next) = jobs.try_recv() {
                    job = next;
                }
                let (snap, query) = job;
                let matches = snap.find_matches(&query);
                if out.send((snap.revision, query, matches)).is_err() {
                    break;
                }
            }
        });
        Self {
            mode: SearchBarMode::Closed,
            find_query: String::new(),
            replace_query: String::new(),
            active_field: 0,
            matches: Vec::new(),
            current_match_idx: None,
            worker,
            results,
            submitted: None,
            pending: false,
            pending_navigation: None,
            mutation: None,
        }
    }
}

// System clipboard helpers via macOS pbcopy / pbpaste
fn copy_to_system_clipboard(text: &str) {
    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
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
    if let Ok(BufferResponse::Snapshot(snap)) = client.send(BufferRequest::GetSnapshot) {
        submit_search(snap, search);
    }
}
fn submit_search(snap: BufferSnapshot, search: &mut SearchState) {
    let key = (snap.revision, search.find_query.clone());
    if search.submitted.as_ref() == Some(&key) {
        return;
    }
    if search
        .submitted
        .as_ref()
        .is_none_or(|(_, query)| query != &search.find_query)
    {
        search.pending_navigation = Some(true);
    }
    search.matches.clear();
    search.current_match_idx = None;
    search.submitted = Some(key);
    search.pending = !search.find_query.is_empty();
    if search.pending {
        let _ = search.worker.send((snap, search.find_query.clone()));
    }
}

fn start_replacement(
    client: &texted_kernel::ChannelClient<BufferRequest, BufferResponse>,
    search: &mut SearchState,
    all: bool,
) {
    if search.mutation.is_some() || search.find_query.is_empty() {
        return;
    }
    let Ok(BufferResponse::Snapshot(snap)) = client.send(BufferRequest::GetSnapshot) else {
        return;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    search.mutation = Some(rx);
    let query = search.find_query.clone();
    let replacement = search.replace_query.clone();
    thread::spawn(move || {
        let (revision, cursor) = (snap.revision, snap.cursor_offset);
        let mut edited = TextBuffer::from_rope(snap.content);
        edited.set_cursor_offset(cursor);
        let changed = if all {
            edited.replace_all(&query, &replacement) > 0
        } else {
            edited.replace_next(&query, &replacement, cursor).is_some()
        };
        let _ = tx.send((revision, cursor, changed.then(|| edited.create_snapshot())));
    });
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
            let (s_line, s_col) = snap.offset_to_line_col(start);
            let (e_line, e_col) = snap.offset_to_line_col(end);

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
    update_search_matches(client, search);
    if search.pending {
        search.pending_navigation = Some(true);
        return;
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
    update_search_matches(client, search);
    if search.pending {
        search.pending_navigation = Some(false);
        return;
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

fn compute_gutter_metrics(
    total_lines: usize,
    show_line_numbers: bool,
    char_w: f32,
) -> (f32, usize, usize) {
    if !show_line_numbers {
        return (0.0, 10, 0);
    }
    let digits = total_lines.max(1).to_string().len().max(3);
    // 10px left pad, digits * char_w, 10px right pad before border
    let gutter_w = (10.0 + (digits as f32 * char_w) + 10.0).ceil();
    // 8px margin after the separator line before text begins
    let text_start_x = (gutter_w + 8.0).round() as usize;
    (gutter_w, text_start_x, digits)
}

struct SnapshotLines<'a>(&'a BufferSnapshot);
impl<'a> SnapshotLines<'a> {
    fn len(&self) -> usize {
        self.0.total_lines
    }
    fn get(&self, line: usize) -> Option<ropey::RopeSlice<'a>> {
        (line < self.len()).then(|| self.0.line(line))
    }
    fn last(&self) -> Option<ropey::RopeSlice<'a>> {
        self.get(self.len() - 1)
    }
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("Texted {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("--help" | "-h") => {
            println!("Texted {}\nUsage: texted [FILE]\n       texted --version\n\nOpen FILE as plain UTF-8 text. Without FILE, open an empty document.",env!("CARGO_PKG_VERSION"));
            return;
        }
        _ => {}
    }

    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event()
        .build()
        .expect("Failed to initialize winit EventLoop");
    native_macos::install(event_loop.create_proxy())
        .expect("Could not install native document callbacks");

    let initial_path = std::env::args().nth(1).map(PathBuf::from);
    let window_title = "Untitled - Texted";
    let window = Arc::new(
        WindowBuilder::new()
            .with_title(window_title)
            .with_inner_size(winit::dpi::LogicalSize::new(980.0, 700.0))
            .build(&event_loop)
            .expect("Failed to create application window"),
    );

    let initial_content = ropey::Rope::new();
    let mut documents = Documents::new(initial_content.clone(), None, false, LineEnding::Lf);
    let mut file_actions = std::collections::VecDeque::new();
    if let Some(path) = initial_path {
        file_actions.push_back(DocumentAction::OpenPath(path));
    }

    // 1. Initialize Microkernel Channels (QNX ChannelCreate)
    let buffer_channel: ChannelPair<BufferRequest, BufferResponse> = ChannelPair::new(CH_BUFFER);
    let buffer_client = buffer_channel.connect();

    // 2. Spawn Isolated Buffer Server Thread (handles Piece Table)
    let init_text = initial_content;
    let buffer_thread = thread::Builder::new()
        .name("server-buffer".to_string())
        .spawn(move || {
            let mut buffer = TextBuffer::from_rope(init_text);
            let server = buffer_channel.server;

            while let Ok(event) = server.receive() {
                match event {
                    ChannelEvent::Pulse(_) => {}
                    ChannelEvent::Message(req) => match req.payload {
                        BufferRequest::BreakUndoGroup => {
                            buffer.break_undo_group();
                            req.reply(BufferResponse::Success);
                        }
                        BufferRequest::LoadDocument(ref text) => {
                            buffer.load_document(text.clone());
                            req.reply(BufferResponse::Success);
                        }
                        BufferRequest::Shutdown => {
                            req.reply(BufferResponse::Success);
                            break;
                        }
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
                        BufferRequest::BlockInsert {
                            start_line,
                            end_line,
                            col,
                            ref text,
                        } => {
                            buffer.block_insert(start_line, end_line, col, text);
                            req.reply(BufferResponse::Success);
                        }
                        BufferRequest::BlockDelete {
                            start_line,
                            end_line,
                            start_col,
                            end_col,
                        } => {
                            buffer.block_delete(start_line, end_line, start_col, end_col);
                            req.reply(BufferResponse::Success);
                        }
                        BufferRequest::BlockExtract {
                            start_line,
                            end_line,
                            start_col,
                            end_col,
                        } => {
                            let extracted =
                                buffer.block_extract(start_line, end_line, start_col, end_col);
                            req.reply(BufferResponse::Text(extracted));
                        }
                        BufferRequest::CommitReplacement {
                            revision,
                            cursor,
                            ref edited,
                        } => {
                            let committed =
                                buffer.commit_snapshot(revision, cursor, edited.clone());
                            req.reply(BufferResponse::Committed(committed));
                        }
                    },
                }
            }
        })
        .expect("Failed to spawn buffer server");

    // 3. Initialize Display & Typography
    let mut renderer = match pollster::block_on(gpu::Renderer::new(window.clone())) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("{error}");
            rfd::MessageDialog::new()
                .set_title("Texted GPU initialization failed")
                .set_description(&error)
                .set_level(rfd::MessageLevel::Error)
                .show();
            let _ = buffer_client.send(BufferRequest::Shutdown);
            return;
        }
    };
    let mut draw_list = DrawList::default();
    let mut renderer_failed = false;

    // Typography Engine (18px crisp scalable font)
    let mut font_engine = FontEngine::new(18.0);
    let mut top_line = 0usize;
    let mut top_wrap_row = 0usize;
    let mut current_view: Option<(texted_layout::ViewportLayout, ViewportConfig)> = None;
    let mut show_line_numbers = true;
    let mut word_wrap = true; // Word wrap enabled by default
    let mut current_modifiers = ModifiersState::empty();
    let mut last_caret_blink = Instant::now();
    let mut caret_visible = true;

    // Viewport auto-scroll pacing: decoupled from passive mouse scrolling
    let mut snap_to_cursor_needed = false;
    let scroll_window = window.clone();
    let mut horizontal = HorizontalScroll::new(move || scroll_window.request_redraw());
    let mut current_scrollbar: Option<horizontal_scroll::Scrollbar> = None;
    let mut vertical_remainder = 0.0_f64;

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
    let mut current_text_start_x = 64.0f32;

    println!("[Texted] GUI editor running: Dark Mode, Word Wrap, macOS Cmd/Opt navigation.");

    let shutdown_client = buffer_client.clone();
    let _ = event_loop.run(move |event, elwt| {
        elwt.set_control_flow(ControlFlow::WaitUntil(
            if search_state.pending || search_state.mutation.is_some() {
                Instant::now() + Duration::from_millis(16)
            } else {
                last_caret_blink + Duration::from_millis(500)
            },
        ));

        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => {
                    file_actions.push_back(DocumentAction::Close);
                    window.request_redraw();
                }
                WindowEvent::DroppedFile(path) => {
                    file_actions.push_back(DocumentAction::OpenPath(path));
                    window.request_redraw();
                }
                WindowEvent::ModifiersChanged(mods) => {
                    current_modifiers = mods.state();
                }
                WindowEvent::Resized(_) => {
                    snap_to_cursor_needed = true;
                    window.request_redraw();
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    if let Some((_, config)) = current_view.as_ref() {
                        let (dx, mut dy) = match delta {
                            MouseScrollDelta::LineDelta(x, y) => (
                                f64::from(x) * f64::from(font_engine.char_width_px) * 3.0,
                                f64::from(y) * f64::from(font_engine.line_height_px) * 2.5,
                            ),
                            MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                        };
                        if !word_wrap {
                            let (horizontal_delta, vertical_delta) = horizontal_scroll::wheel_axes(
                                dx,
                                dy,
                                current_modifiers.shift_key(),
                            );
                            dy = vertical_delta;
                            horizontal.wheel(
                                -horizontal_delta,
                                font_engine.char_width_px,
                                config.columns(),
                            );
                        }
                        vertical_remainder -= dy / f64::from(font_engine.line_height_px);
                        let rows = vertical_remainder.trunc() as i32;
                        vertical_remainder -= f64::from(rows);
                        if rows != 0 {
                            if let Ok(BufferResponse::Snapshot(snap)) =
                                buffer_client.send(BufferRequest::GetSnapshot)
                            {
                                let mut config = *config;
                                config.scroll_rows(&snap, rows);
                                top_line = config.top_line;
                                top_wrap_row = config.top_wrap_row;
                            }
                        }
                    }
                    snap_to_cursor_needed = false;
                    window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if documents.locked() || renderer_failed {
                        return;
                    }
                    last_mouse_pos = (position.x as f32, position.y as f32);

                    if let Some(bar) = current_scrollbar {
                        if horizontal.drag(bar, last_mouse_pos.0) {
                            snap_to_cursor_needed = false;
                            window.request_redraw();
                            return;
                        }
                    }
                    if is_mouse_dragging {
                        let rel_y = (last_mouse_pos.1 - 2.0).max(0.0);
                        let rel_x = (last_mouse_pos.0 - current_text_start_x).max(0.0);

                        let (hover_line, hover_col) = current_view
                            .as_ref()
                            .map(|(layout, config)| {
                                layout.screen_to_line_col(config, last_mouse_pos.0, rel_y)
                            })
                            .unwrap_or((
                                top_line,
                                (rel_x / font_engine.char_width_px).round() as usize,
                            ));

                        if let Some((init_line, init_w_start, init_w_end)) = word_drag_anchor {
                            if let Ok(BufferResponse::Snapshot(snap)) =
                                buffer_client.send(BufferRequest::GetSnapshot)
                            {
                                let bounds = texted_buffer::word::bounds(snap.line(hover_line),hover_col);
                                let (curr_w_start,curr_w_end)=(bounds.start,bounds.end);

                                selection.mode = SelectionMode::Stream;
                                if hover_line > init_line
                                    || (hover_line == init_line && hover_col > init_w_end)
                                {
                                    selection.anchor_line = init_line;
                                    selection.anchor_col = init_w_start;
                                    selection.active_line = hover_line;
                                    selection.active_col = curr_w_end;
                                } else if hover_line < init_line
                                    || (hover_line == init_line && hover_col < init_w_start)
                                {
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

                                let offset = snap.line_col_to_offset(
                                    selection.active_line,
                                    selection.active_col,
                                );
                                let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                            }
                        } else if let Some(init_line) = line_drag_anchor {
                            if let Ok(BufferResponse::Snapshot(snap)) =
                                buffer_client.send(BufferRequest::GetSnapshot)
                            {
                                let lines = SnapshotLines(&snap);
                                let total_lines = lines.len();

                                selection.mode = SelectionMode::Stream;
                                if hover_line >= init_line {
                                    let (e_line, e_col) = if hover_line + 1 < total_lines {
                                        (hover_line + 1, 0)
                                    } else {
                                        let last_line_len = lines
                                            .get(hover_line)
                                            .map(|l| l.len_chars())
                                            .unwrap_or(0);
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
                                        let init_len = lines
                                            .get(init_line)
                                            .map(|l| l.len_chars())
                                            .unwrap_or(0);
                                        (init_line, init_len)
                                    };
                                    selection.anchor_line = a_line;
                                    selection.anchor_col = a_col;
                                    selection.active_line = hover_line;
                                    selection.active_col = 0;
                                }

                                let offset = snap.line_col_to_offset(
                                    selection.active_line,
                                    selection.active_col,
                                );
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
                            if let Ok(BufferResponse::Snapshot(snap)) =
                                buffer_client.send(BufferRequest::GetSnapshot)
                            {
                                let offset = snap.line_col_to_offset(hover_line, hover_col);
                                let _ = buffer_client.send(BufferRequest::SetCursorOffset(offset));
                            }
                        }

                        caret_visible = true;
                        last_caret_blink = Instant::now();
                        window.request_redraw();
                    }
                }
                WindowEvent::MouseInput {
                    state,
                    button: MouseButton::Left,
                    ..
                } => {
                    if documents.locked() || renderer_failed {
                        return;
                    }
                    if state == ElementState::Released && horizontal.end_drag() {
                        window.request_redraw();
                        return;
                    }
                    if let Some(bar) = current_scrollbar {
                        if bar.contains(last_mouse_pos.0, last_mouse_pos.1) {
                            if state == ElementState::Pressed {
                                let columns = current_view
                                    .as_ref()
                                    .map_or(1, |(_, config)| config.columns());
                                horizontal.press(bar, last_mouse_pos.0, columns);
                                is_mouse_dragging = false;
                                snap_to_cursor_needed = false;
                                window.request_redraw();
                            }
                            return;
                        }
                    }
                    let rel_y = (last_mouse_pos.1 - 2.0).max(0.0);
                    let rel_x = (last_mouse_pos.0 - current_text_start_x).max(0.0);

                    // Check interaction with Search Bar buttons
                    if search_state.mode != SearchBarMode::Closed {
                        let size = window.inner_size();
                        let panel_w = 480.0f32;
                        let panel_x = (size.width as f32 - panel_w - 12.0).max(0.0);
                        let panel_h = if search_state.mode == SearchBarMode::Replace {
                            68.0
                        } else {
                            36.0
                        };

                        if last_mouse_pos.0 >= panel_x
                            && last_mouse_pos.0 <= panel_x + panel_w
                            && last_mouse_pos.1 >= 8.0
                            && last_mouse_pos.1 <= 8.0 + panel_h
                        {
                            if state == ElementState::Pressed {
                                // 1. Close [X] button
                                if last_mouse_pos.0 >= panel_x + panel_w - 28.0
                                    && last_mouse_pos.1 <= 36.0
                                {
                                    search_state.mode = SearchBarMode::Closed;
                                    window.request_redraw();
                                    return;
                                }

                                // 2. [< Prev] button
                                let prev_btn_x = panel_x + 285.0;
                                if last_mouse_pos.0 >= prev_btn_x
                                    && last_mouse_pos.0 <= prev_btn_x + 55.0
                                    && last_mouse_pos.1 <= 36.0
                                {
                                    find_prev_match(
                                        &buffer_client,
                                        &mut search_state,
                                        &mut selection,
                                    );
                                    snap_to_cursor_needed = true;
                                    window.request_redraw();
                                    return;
                                }

                                // 3. [Next >] button
                                let next_btn_x = panel_x + 345.0;
                                if last_mouse_pos.0 >= next_btn_x
                                    && last_mouse_pos.0 <= next_btn_x + 55.0
                                    && last_mouse_pos.1 <= 36.0
                                {
                                    find_next_match(
                                        &buffer_client,
                                        &mut search_state,
                                        &mut selection,
                                    );
                                    snap_to_cursor_needed = true;
                                    window.request_redraw();
                                    return;
                                }

                                // 4. Switch between Find and Replace fields
                                if search_state.mode == SearchBarMode::Replace
                                    && last_mouse_pos.1 > 38.0
                                {
                                    // Replace or All buttons
                                    let repl_btn_x = panel_x + 245.0;
                                    if last_mouse_pos.0 >= repl_btn_x
                                        && last_mouse_pos.0 <= repl_btn_x + 70.0
                                    {
                                        start_replacement(&buffer_client, &mut search_state, false);
                                        window.request_redraw();
                                        return;
                                    }

                                    let all_btn_x = panel_x + 325.0;
                                    if last_mouse_pos.0 >= all_btn_x
                                        && last_mouse_pos.0 <= all_btn_x + 45.0
                                    {
                                        start_replacement(&buffer_client, &mut search_state, true);
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

                    let (click_line, click_col) = current_view
                        .as_ref()
                        .map(|(layout, config)| {
                            layout.screen_to_line_col(config, last_mouse_pos.0, rel_y)
                        })
                        .unwrap_or((
                            top_line,
                            (rel_x / font_engine.char_width_px).round() as usize,
                        ));

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

                            if let Ok(BufferResponse::Snapshot(snap)) =
                                buffer_client.send(BufferRequest::GetSnapshot)
                            {
                                let (_, t_start, _) = compute_gutter_metrics(
                                    snap.total_lines,
                                    show_line_numbers,
                                    font_engine.char_width_px,
                                );
                                current_text_start_x = t_start as f32;

                                let lines = SnapshotLines(&snap);
                                let line_len =
                                    lines.get(click_line).map(|l| l.len_chars()).unwrap_or(0);

                                if click_count == 2 {
                                    // Double click: Select Word
                                    let bounds=texted_buffer::word::bounds(snap.line(click_line),click_col);
                                    let (w_start,w_end)=(bounds.start,bounds.end);
                                    selection = SelectionState {
                                        mode: SelectionMode::Stream,
                                        anchor_line: click_line,
                                        anchor_col: w_start,
                                        active_line: click_line,
                                        active_col: w_end,
                                    };
                                    word_drag_anchor = Some((click_line, w_start, w_end));
                                    line_drag_anchor = None;
                                    let offset = snap.line_col_to_offset(click_line, w_end);
                                    let _ =
                                        buffer_client.send(BufferRequest::SetCursorOffset(offset));
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
                                    let offset = snap.line_col_to_offset(e_line, e_col);
                                    let _ =
                                        buffer_client.send(BufferRequest::SetCursorOffset(offset));
                                } else {
                                    // Single click
                                    word_drag_anchor = None;
                                    line_drag_anchor = None;
                                    let is_block = current_modifiers.alt_key();
                                    selection = SelectionState {
                                        mode: if is_block {
                                            SelectionMode::Block
                                        } else {
                                            SelectionMode::None
                                        },
                                        anchor_line: click_line,
                                        anchor_col: click_col,
                                        active_line: click_line,
                                        active_col: click_col,
                                    };
                                    let offset = snap.line_col_to_offset(click_line, click_col);
                                    let _ =
                                        buffer_client.send(BufferRequest::SetCursorOffset(offset));
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
                    event:
                        KeyEvent {
                            state: ElementState::Pressed,
                            logical_key,
                            physical_key,
                            text,
                            ..
                        },
                    ..
                } => {
                    if documents.locked() || renderer_failed {
                        return;
                    }
                    caret_visible = true;
                    last_caret_blink = Instant::now();
                    snap_to_cursor_needed = true; // Key input centers/snaps viewport to caret

                    let is_ctrl_or_cmd =
                        current_modifiers.control_key() || current_modifiers.super_key();
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
                            if let Ok(BufferResponse::Snapshot(snap)) =
                                buffer_client.send(BufferRequest::GetSnapshot)
                            {
                                let lines = SnapshotLines(&snap);
                                let total_lines = lines.len();
                                let last_line_len =
                                    lines.last().map(|l| l.len_chars()).unwrap_or(0);
                                selection = SelectionState {
                                    mode: SelectionMode::Stream,
                                    anchor_line: 0,
                                    anchor_col: 0,
                                    active_line: total_lines.saturating_sub(1),
                                    active_col: last_line_len,
                                };
                                let _ = buffer_client
                                    .send(BufferRequest::SetCursorOffset(snap.total_bytes));
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
                                if let Ok(BufferResponse::Text(extracted)) =
                                    buffer_client.send(BufferRequest::BlockExtract {
                                        start_line: min_l,
                                        end_line: max_l,
                                        start_col: min_c,
                                        end_col: max_c,
                                    })
                                {
                                    copy_to_system_clipboard(&extracted);
                                }
                            } else if selection.mode == SelectionMode::Stream {
                                if let Ok(BufferResponse::Snapshot(snap)) =
                                    buffer_client.send(BufferRequest::GetSnapshot)
                                {
                                    let off_a = snap.line_col_to_offset(
                                        selection.anchor_line,
                                        selection.anchor_col,
                                    );
                                    let off_b = snap.line_col_to_offset(
                                        selection.active_line,
                                        selection.active_col,
                                    );
                                    let min_off = off_a.min(off_b);
                                    let max_off = off_a.max(off_b);
                                    if min_off < max_off && max_off <= snap.total_bytes {
                                        copy_to_system_clipboard(
                                            &snap.content.byte_slice(min_off..max_off).to_string(),
                                        );
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
                                        find_next_match(
                                            &buffer_client,
                                            &mut search_state,
                                            &mut selection,
                                        );
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
                                    let max_clip_line_len = clip_lines
                                        .iter()
                                        .map(|l| l.chars().count())
                                        .max()
                                        .unwrap_or(0);
                                    selection.anchor_line = min_l;
                                    selection.active_line =
                                        (min_l + clip_lines.len().saturating_sub(1)).max(max_l);
                                    selection.anchor_col = min_c + max_clip_line_len;
                                    selection.active_col = min_c + max_clip_line_len;
                                } else {
                                    if selection.mode == SelectionMode::Stream {
                                        if let Ok(BufferResponse::Snapshot(snap)) =
                                            buffer_client.send(BufferRequest::GetSnapshot)
                                        {
                                            let off_a = snap.line_col_to_offset(
                                                selection.anchor_line,
                                                selection.anchor_col,
                                            );
                                            let off_b = snap.line_col_to_offset(
                                                selection.active_line,
                                                selection.active_col,
                                            );
                                            let _ =
                                                buffer_client.send(BufferRequest::DeleteRange {
                                                    start: off_a.min(off_b),
                                                    end: off_a.max(off_b),
                                                });
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
                                if let Ok(BufferResponse::Text(extracted)) =
                                    buffer_client.send(BufferRequest::BlockExtract {
                                        start_line: min_l,
                                        end_line: max_l,
                                        start_col: min_c,
                                        end_col: max_c,
                                    })
                                {
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
                                if let Ok(BufferResponse::Snapshot(snap)) =
                                    buffer_client.send(BufferRequest::GetSnapshot)
                                {
                                    let off_a = snap.line_col_to_offset(
                                        selection.anchor_line,
                                        selection.anchor_col,
                                    );
                                    let off_b = snap.line_col_to_offset(
                                        selection.active_line,
                                        selection.active_col,
                                    );
                                    let min_off = off_a.min(off_b);
                                    let max_off = off_a.max(off_b);
                                    if min_off < max_off && max_off <= snap.total_bytes {
                                        copy_to_system_clipboard(
                                            &snap.content.byte_slice(min_off..max_off).to_string(),
                                        );
                                        let _ = buffer_client.send(BufferRequest::DeleteRange {
                                            start: min_off,
                                            end: max_off,
                                        });
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
                                if let Ok(BufferResponse::Snapshot(snap)) =
                                    buffer_client.send(BufferRequest::GetSnapshot)
                                {
                                    let off_a = snap.line_col_to_offset(
                                        selection.anchor_line,
                                        selection.anchor_col,
                                    );
                                    let off_b = snap.line_col_to_offset(
                                        selection.active_line,
                                        selection.active_col,
                                    );
                                    let min_off = off_a.min(off_b);
                                    let max_off = off_a.max(off_b);
                                    if min_off < max_off && max_off - min_off < 100 {
                                        search_state.find_query =
                                            snap.content.byte_slice(min_off..max_off).to_string();
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
                        } else if matches_key(KeyCode::KeyO, 'o') {
                            file_actions.push_back(DocumentAction::OpenDialog);
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyN, 'n') {
                            file_actions.push_back(DocumentAction::New);
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyQ, 'q') || matches_key(KeyCode::KeyW, 'w')
                        {
                            file_actions.push_back(DocumentAction::Close);
                            window.request_redraw();
                            return;
                        } else if matches_key(KeyCode::KeyS, 's') {
                            file_actions.push_back(DocumentAction::Save {
                                save_as: current_modifiers.shift_key(),
                            });
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
                                if search_state.mode == SearchBarMode::Replace
                                    && search_state.active_field == 1
                                {
                                    start_replacement(&buffer_client, &mut search_state, false);
                                } else if current_modifiers.shift_key() {
                                    find_prev_match(
                                        &buffer_client,
                                        &mut search_state,
                                        &mut selection,
                                    );
                                } else {
                                    find_next_match(
                                        &buffer_client,
                                        &mut search_state,
                                        &mut selection,
                                    );
                                }
                                window.request_redraw();
                                return;
                            }
                            Key::Named(NamedKey::Backspace) => {
                                if search_state.active_field == 0 {
                                    search_state.find_query.pop();
                                    update_search_matches(&buffer_client, &mut search_state);
                                    if !search_state.find_query.is_empty() {
                                        find_next_match(
                                            &buffer_client,
                                            &mut search_state,
                                            &mut selection,
                                        );
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
                                                update_search_matches(
                                                    &buffer_client,
                                                    &mut search_state,
                                                );
                                                find_next_match(
                                                    &buffer_client,
                                                    &mut search_state,
                                                    &mut selection,
                                                );
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
                            snap_to_cursor_needed = false;
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
                            if is_shift
                                && is_alt
                                && (matches!(logical_key, Key::Named(NamedKey::ArrowUp))
                                    || matches!(logical_key, Key::Named(NamedKey::ArrowDown)))
                            {
                                if selection.mode != SelectionMode::Block {
                                    if let Ok(BufferResponse::Snapshot(snap)) =
                                        buffer_client.send(BufferRequest::GetSnapshot)
                                    {
                                        let (cur_l, cur_c) =
                                            snap.offset_to_line_col(snap.cursor_offset);
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
                                    if let Ok(BufferResponse::Snapshot(snap)) =
                                        buffer_client.send(BufferRequest::GetSnapshot)
                                    {
                                        let (cur_l, cur_c) =
                                            snap.offset_to_line_col(snap.cursor_offset);
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
                                    if matches!(logical_key, Key::Named(NamedKey::ArrowLeft))
                                        && !is_ctrl_or_cmd
                                        && !is_alt
                                    {
                                        if let Ok(BufferResponse::Snapshot(snap)) =
                                            buffer_client.send(BufferRequest::GetSnapshot)
                                        {
                                            let off_a = snap.line_col_to_offset(
                                                selection.anchor_line,
                                                selection.anchor_col,
                                            );
                                            let off_b = snap.line_col_to_offset(
                                                selection.active_line,
                                                selection.active_col,
                                            );
                                            let _ = buffer_client.send(
                                                BufferRequest::SetCursorOffset(off_a.min(off_b)),
                                            );
                                        }
                                        selection.mode = SelectionMode::None;
                                        window.request_redraw();
                                        return;
                                    } else if matches!(
                                        logical_key,
                                        Key::Named(NamedKey::ArrowRight)
                                    ) && !is_ctrl_or_cmd
                                        && !is_alt
                                    {
                                        if let Ok(BufferResponse::Snapshot(snap)) =
                                            buffer_client.send(BufferRequest::GetSnapshot)
                                        {
                                            let off_a = snap.line_col_to_offset(
                                                selection.anchor_line,
                                                selection.anchor_col,
                                            );
                                            let off_b = snap.line_col_to_offset(
                                                selection.active_line,
                                                selection.active_col,
                                            );
                                            let _ = buffer_client.send(
                                                BufferRequest::SetCursorOffset(off_a.max(off_b)),
                                            );
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
                                        let _ = buffer_client.send(BufferRequest::MoveHome);
                                    // Cmd+Left: Line start
                                    } else if is_alt {
                                        let _ = buffer_client.send(BufferRequest::MoveWordLeft);
                                    // Opt+Left: Word left
                                    } else {
                                        let _ = buffer_client.send(BufferRequest::MoveLeft);
                                    }
                                }
                                Key::Named(NamedKey::ArrowRight) => {
                                    if is_ctrl_or_cmd {
                                        let _ = buffer_client.send(BufferRequest::MoveEnd);
                                    // Cmd+Right: Line end
                                    } else if is_alt {
                                        let _ = buffer_client.send(BufferRequest::MoveWordRight);
                                    // Opt+Right: Word right
                                    } else {
                                        let _ = buffer_client.send(BufferRequest::MoveRight);
                                    }
                                }
                                Key::Named(NamedKey::ArrowUp) => {
                                    if is_ctrl_or_cmd {
                                        let _ =
                                            buffer_client.send(BufferRequest::SetCursorOffset(0));
                                    // Cmd+Up: Doc start
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
                                        if let Ok(BufferResponse::Snapshot(snap)) =
                                            buffer_client.send(BufferRequest::GetSnapshot)
                                        {
                                            let _ = buffer_client.send(
                                                BufferRequest::SetCursorOffset(snap.total_bytes),
                                            ); // Cmd+Down: Doc end
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
                                if let Ok(BufferResponse::Snapshot(snap)) =
                                    buffer_client.send(BufferRequest::GetSnapshot)
                                {
                                    let (cur_l, cur_c) =
                                        snap.offset_to_line_col(snap.cursor_offset);
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
                                if let Ok(BufferResponse::Snapshot(snap)) =
                                    buffer_client.send(BufferRequest::GetSnapshot)
                                {
                                    let off_a = snap.line_col_to_offset(
                                        selection.anchor_line,
                                        selection.anchor_col,
                                    );
                                    let off_b = snap.line_col_to_offset(
                                        selection.active_line,
                                        selection.active_col,
                                    );
                                    let min_off = off_a.min(off_b);
                                    let max_off = off_a.max(off_b);
                                    let _ = buffer_client.send(BufferRequest::DeleteRange {
                                        start: min_off,
                                        end: max_off,
                                    });
                                }
                                selection.mode = SelectionMode::None;
                            } else if is_ctrl_or_cmd {
                                let _ = buffer_client.send(BufferRequest::DeleteToLineStart);
                            // Cmd+Backspace
                            } else if is_alt {
                                let _ = buffer_client.send(BufferRequest::DeleteWordLeft);
                            // Opt+Backspace
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
                                } else {
                                    let _ = buffer_client.send(BufferRequest::BlockDelete {
                                        start_line: min_l,
                                        end_line: max_l,
                                        start_col: min_c,
                                        end_col: min_c + 1,
                                    });
                                }
                            } else if selection.mode == SelectionMode::Stream {
                                if let Ok(BufferResponse::Snapshot(snap)) =
                                    buffer_client.send(BufferRequest::GetSnapshot)
                                {
                                    let off_a = snap.line_col_to_offset(
                                        selection.anchor_line,
                                        selection.anchor_col,
                                    );
                                    let off_b = snap.line_col_to_offset(
                                        selection.active_line,
                                        selection.active_col,
                                    );
                                    let _ = buffer_client.send(BufferRequest::DeleteRange {
                                        start: off_a.min(off_b),
                                        end: off_a.max(off_b),
                                    });
                                }
                                selection.mode = SelectionMode::None;
                            } else if is_ctrl_or_cmd {
                                let _ = buffer_client.send(BufferRequest::DeleteToLineEnd);
                            // Cmd+Delete
                            } else if is_alt {
                                let _ = buffer_client.send(BufferRequest::DeleteWordRight);
                            // Opt+Delete
                            } else {
                                let _ = buffer_client.send(BufferRequest::DeleteForward);
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
                                            let min_l =
                                                selection.anchor_line.min(selection.active_line);
                                            let max_l =
                                                selection.anchor_line.max(selection.active_line);
                                            let min_c =
                                                selection.anchor_col.min(selection.active_col);
                                            let max_c =
                                                selection.anchor_col.max(selection.active_col);

                                            if min_c < max_c {
                                                let _ = buffer_client.send(
                                                    BufferRequest::BlockDelete {
                                                        start_line: min_l,
                                                        end_line: max_l,
                                                        start_col: min_c,
                                                        end_col: max_c,
                                                    },
                                                );
                                                selection.anchor_col = min_c;
                                                selection.active_col = min_c;
                                            }

                                            let _ =
                                                buffer_client.send(BufferRequest::BlockInsert {
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
                                                if let Ok(BufferResponse::Snapshot(snap)) =
                                                    buffer_client.send(BufferRequest::GetSnapshot)
                                                {
                                                    let off_a = snap.line_col_to_offset(
                                                        selection.anchor_line,
                                                        selection.anchor_col,
                                                    );
                                                    let off_b = snap.line_col_to_offset(
                                                        selection.active_line,
                                                        selection.active_col,
                                                    );
                                                    let min_off = off_a.min(off_b);
                                                    let max_off = off_a.max(off_b);
                                                    let _ = buffer_client.send(
                                                        BufferRequest::DeleteRange {
                                                            start: min_off,
                                                            end: max_off,
                                                        },
                                                    );
                                                }
                                                selection.mode = SelectionMode::None;
                                            }

                                            let _ = buffer_client.send(BufferRequest::Insert {
                                                offset: usize::MAX,
                                                text: ch.to_string(),
                                            });
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
                    let replacement = search_state
                        .mutation
                        .as_ref()
                        .map(|rx| rx.try_recv());
                    if matches!(replacement.as_ref(), Some(Err(std::sync::mpsc::TryRecvError::Disconnected))) {
                        search_state.mutation = None;
                        file_actions.push_back(DocumentAction::ReportError(
                            "The replacement worker stopped before completing.".into()
                        ));
                    }
                    if let Some(Ok(plan)) = replacement {
                        // Commit on the UI event boundary so an input handler cannot race a background mutation.
                        let (revision, cursor, edited) = plan;
                        let committed = edited.is_none_or(|edited| {
                            matches!(
                                buffer_client.send(BufferRequest::CommitReplacement {
                                    revision,
                                    cursor,
                                    edited
                                }),
                                Ok(BufferResponse::Committed(true))
                            )
                        });
                        if committed {
                            selection.mode = SelectionMode::None;
                            snap_to_cursor_needed = true;
                        } else {
                            window.set_title("Replacement skipped: document changed - Texted");
                        }
                        search_state.mutation = None;
                    }
                    if renderer_failed { return; }
                    let size = window.inner_size();
                    if size.width == 0 || size.height == 0 {
                        return;
                    }

                    let width = size.width as usize;
                    let height = size.height as usize;
                    let text_height =
                        height.saturating_sub(28 + if word_wrap { 0 } else { BAR_HEIGHT });

                    let buffer = &mut draw_list;

                    // Blinking cursor pacing
                    if last_caret_blink.elapsed() >= Duration::from_millis(500) {
                        caret_visible = !caret_visible;
                        last_caret_blink = Instant::now();
                    }

                    // 1. Fetch Snapshot from Buffer Server via Microkernel
                    let mut snap = match buffer_client.send(BufferRequest::GetSnapshot) {
                        Ok(BufferResponse::Snapshot(s)) => s,
                        _ => return,
                    };

                    if search_state.mode != SearchBarMode::Closed {
                        submit_search(snap.clone(), &mut search_state);
                    }
                    while let Ok((revision, query, matches)) = search_state.results.try_recv() {
                        if search_state.submitted.as_ref() == Some(&(revision, query)) {
                            search_state.matches = matches;
                            search_state.current_match_idx = None;
                            search_state.pending = false;
                        }
                    }
                    if !search_state.pending && search_state.mode != SearchBarMode::Closed {
                        if let Some(forward) = search_state.pending_navigation.take() {
                            if forward {
                                find_next_match(&buffer_client, &mut search_state, &mut selection);
                            } else {
                                find_prev_match(&buffer_client, &mut search_state, &mut selection);
                            }
                            if let Ok(BufferResponse::Snapshot(updated)) =
                                buffer_client.send(BufferRequest::GetSnapshot)
                            {
                                snap = updated;
                            }
                            snap_to_cursor_needed = true;
                        }
                    }
                    window.set_title(&documents.title(&snap));
                    let (cur_line, cur_col) = snap.offset_to_line_col(snap.cursor_offset);

                    // 2. Viewport auto-scrolling: only triggered on keyboard interaction, decoupled from mouse scrolling
                    let visible_rows =
                        (text_height as f32 / font_engine.line_height_px).floor() as usize;
                    let snap_horiz = snap_to_cursor_needed;
                    if snap_to_cursor_needed {
                        if cur_line < top_line {
                            top_line = cur_line;
                        } else if cur_line >= top_line + visible_rows.saturating_sub(1) {
                            top_line = cur_line.saturating_sub(visible_rows.saturating_sub(2));
                        }
                        snap_to_cursor_needed = false;
                    }

                    let (gutter_w, text_start_x, line_digits) = compute_gutter_metrics(
                        snap.total_lines,
                        show_line_numbers,
                        font_engine.char_width_px,
                    );
                    current_text_start_x = text_start_x as f32;

                    let mut config = ViewportConfig {
                        width_px: width as f32,
                        height_px: text_height as f32,
                        line_height_px: font_engine.line_height_px,
                        char_width_px: font_engine.char_width_px,
                        top_line,
                        top_wrap_row,
                        left_col: 0,
                        word_wrap,
                        show_line_numbers,
                        line_number_gutter_width_px: gutter_w,
                    };

                    config.top_line = config.top_line.min(snap.total_lines - 1);
                    if !word_wrap {
                        config.top_wrap_row = 0;
                    }
                    if snap_horiz && word_wrap {
                        let caret_row = (cur_col / config.columns())
                            .min(config.rows_for_line(&snap, cur_line) - 1);
                        if cur_line == config.top_line && caret_row < config.top_wrap_row {
                            config.top_wrap_row = caret_row;
                        }
                        let probe = LayoutEngine::compute_layout(&snap, &config, None);
                        if probe.caret_rect.y < 0.0 {
                            config.top_line = cur_line;
                            config.top_wrap_row =
                                caret_row.saturating_sub(visible_rows.saturating_sub(2));
                        }
                    }
                    if word_wrap {
                        horizontal.reset();
                        current_scrollbar = None;
                    } else {
                        horizontal.refresh(
                            &snap,
                            config.top_line,
                            visible_rows + 1,
                            config.columns(),
                        );
                        if snap_horiz {
                            horizontal.ensure_caret(cur_col, config.columns());
                        }
                        config.left_col = horizontal.first_col;
                        current_scrollbar = horizontal.scrollbar(
                            config.text_x(),
                            text_height as f32,
                            (width as f32 - config.text_x()).max(0.0),
                            config.columns(),
                        );
                    }
                    top_line = config.top_line;
                    top_wrap_row = config.top_wrap_row;
                    let t_layout = Instant::now();
                    let layout = LayoutEngine::compute_layout(&snap, &config, None);
                    let layout_time = t_layout.elapsed();

                    // 3. Clear Screen to Dark Charcoal
                    buffer.reset(COLOR_BG);

                    // 4. Render Gutter (Line numbers)
                    if show_line_numbers && gutter_w > 0.0 {
                        let gutter_width_int = gutter_w as usize;
                        draw_rect(
                            buffer,
                            width,
                            height,
                            0,
                            0,
                            gutter_width_int,
                            height.saturating_sub(28),
                            COLOR_GUTTER_BG,
                        );
                        draw_rect(
                            buffer,
                            width,
                            height,
                            gutter_width_int.saturating_sub(1),
                            0,
                            1,
                            height.saturating_sub(28),
                            COLOR_GUTTER_BORDER,
                        );
                    }

                    // 5. Render only revision-valid matches intersecting visible rows.
                    let match_revision = search_state.submitted.as_ref()
                        .filter(|(_, query)| query == &search_state.find_query)
                        .map(|(revision, _)| *revision)
                        .unwrap_or(u64::MAX);
                    let active = search_state.current_match_idx.and_then(|i| search_state.matches.get(i).copied());
                    for highlight in layout.search_highlights(&snap, &config, match_revision, &search_state.matches, active).highlights {
                        let rect = highlight.rect;
                        draw_rect(buffer, width, height, rect.x.round() as usize,
                            rect.y.round() as usize + 2, rect.width.round() as usize,
                            rect.height as usize,
                            if highlight.active { COLOR_SEARCH_ACTIVE } else { COLOR_SEARCH_MATCH });
                    }

                    // 6. Render Selection Highlights
                    if selection.mode == SelectionMode::Block {
                        let min_l = selection.anchor_line.min(selection.active_line);
                        let max_l = selection.anchor_line.max(selection.active_line);
                        let min_c = selection.anchor_col.min(selection.active_col);
                        let max_c = selection.anchor_col.max(selection.active_col);

                        for v_line in &layout.visible_lines {
                            let l_idx = v_line.line_index;
                            {
                                if l_idx >= min_l && l_idx <= max_l {
                                    let row_y = v_line.y_px as usize;
                                    let Some(rect) =
                                        layout.highlight(&config, v_line, min_c, max_c)
                                    else {
                                        continue;
                                    };
                                    let sel_x = rect.x.round() as usize;
                                    let sel_w = rect.width.round() as usize;

                                    draw_rect(
                                        buffer,
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
                        let (s_line, s_col) = if (selection.anchor_line, selection.anchor_col)
                            <= (selection.active_line, selection.active_col)
                        {
                            (selection.anchor_line, selection.anchor_col)
                        } else {
                            (selection.active_line, selection.active_col)
                        };
                        let (e_line, e_col) = if (selection.anchor_line, selection.anchor_col)
                            <= (selection.active_line, selection.active_col)
                        {
                            (selection.active_line, selection.active_col)
                        } else {
                            (selection.anchor_line, selection.anchor_col)
                        };

                        let lines = SnapshotLines(&snap);

                        if (s_line, s_col) != (e_line, e_col) {
                            for v_line in &layout.visible_lines {
                                let l_idx = v_line.line_index;
                                {
                                    if l_idx >= s_line && l_idx <= e_line {
                                        let row_y = v_line.y_px as usize;
                                        let line_len =
                                            lines.get(l_idx).map(|l| l.len_chars()).unwrap_or(0);

                                        let from_c = if l_idx == s_line { s_col } else { 0 };
                                        let to_c = if l_idx == e_line { e_col } else { line_len };

                                        if l_idx == e_line && from_c >= to_c {
                                            continue;
                                        }

                                        let Some(rect) =
                                            layout.highlight(&config, v_line, from_c, to_c)
                                        else {
                                            continue;
                                        };
                                        let sel_x = rect.x.round() as usize;
                                        let sel_w = rect.width.round() as usize;

                                        draw_rect(
                                            buffer,
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
                        if y_int + (font_engine.line_height_px as usize) > text_height {
                            break;
                        }

                        // Gutter line number (only on non-continuation rows)
                        if show_line_numbers && !line.is_continuation {
                            let num_str =
                                format!("{:>width$}", line.line_index + 1, width = line_digits);
                            font_engine.draw_string(
                                buffer,
                                width,
                                height,
                                10,
                                y_int + 2,
                                &num_str,
                                COLOR_GUTTER_TEXT,
                            );
                        }

                        // Text content
                        font_engine.draw_string(
                            buffer,
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
                        let cx = layout.caret_rect.x.round() as usize;
                        let cy = layout.caret_rect.y as usize + 2;

                        if cy + (font_engine.line_height_px as usize) <= text_height {
                            draw_rect(
                                buffer,
                                width,
                                height,
                                cx,
                                cy,
                                2,
                                font_engine.line_height_px as usize,
                                COLOR_CARET,
                            );
                        }
                    }

                    if let Some(bar) = current_scrollbar {
                        draw_rect(
                            buffer,
                            width,
                            height,
                            bar.track.x as usize,
                            bar.track.y as usize,
                            bar.track.width as usize,
                            BAR_HEIGHT,
                            COLOR_GUTTER_BG,
                        );
                        draw_rect(
                            buffer,
                            width,
                            height,
                            bar.thumb.x.round() as usize,
                            bar.thumb.y as usize,
                            bar.thumb.width.round() as usize,
                            bar.thumb.height as usize,
                            COLOR_GUTTER_TEXT,
                        );
                    }
                    // 9. Render Floating Find & Replace Bar with Interactive [< Prev] and [Next >] Buttons
                    if search_state.mode != SearchBarMode::Closed {
                        let panel_w = 480usize;
                        let panel_h = if search_state.mode == SearchBarMode::Replace {
                            68usize
                        } else {
                            36usize
                        };
                        let panel_x = width.saturating_sub(panel_w + 16);
                        let panel_y = 10usize;

                        draw_box(
                            buffer,
                            width,
                            height,
                            panel_x,
                            panel_y,
                            panel_w,
                            panel_h,
                            COLOR_PANEL_BG,
                            COLOR_PANEL_BORDER,
                        );

                        // Find Input Box
                        let input_x = panel_x + 50;
                        let input_y = panel_y + 6;
                        let input_w = 160usize;
                        let input_h = 24usize;
                        let border_col = if search_state.active_field == 0 {
                            COLOR_INPUT_ACTIVE
                        } else {
                            COLOR_INPUT_BORDER
                        };
                        draw_box(
                            buffer,
                            width,
                            height,
                            input_x,
                            input_y,
                            input_w,
                            input_h,
                            COLOR_INPUT_BG,
                            border_col,
                        );

                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            panel_x + 8,
                            input_y + 3,
                            "Find:",
                            COLOR_STATUS_TEXT,
                        );

                        let query_display = &search_state.find_query;
                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            input_x + 5,
                            input_y + 3,
                            query_display,
                            COLOR_TEXT,
                        );

                        // Cursor in Find box
                        if search_state.active_field == 0 && caret_visible {
                            let cur_x = input_x
                                + 5
                                + (query_display.chars().count() as f32 * font_engine.char_width_px)
                                    .round() as usize;
                            if cur_x < input_x + input_w - 4 {
                                draw_rect(
                                    buffer,
                                    width,
                                    height,
                                    cur_x,
                                    input_y + 4,
                                    2,
                                    input_h - 8,
                                    COLOR_CARET,
                                );
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
                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            input_x + input_w + 8,
                            input_y + 3,
                            &match_info,
                            COLOR_STATUS_TEXT,
                        );

                        // Interactive [< Prev] button
                        let prev_btn_x = panel_x + 285;
                        draw_box(
                            buffer,
                            width,
                            height,
                            prev_btn_x,
                            input_y,
                            52,
                            input_h,
                            COLOR_BUTTON_BG,
                            COLOR_BUTTON_BORDER,
                        );
                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            prev_btn_x + 6,
                            input_y + 3,
                            "< Prev",
                            COLOR_TEXT,
                        );

                        // Interactive [Next >] button
                        let next_btn_x = panel_x + 343;
                        draw_box(
                            buffer,
                            width,
                            height,
                            next_btn_x,
                            input_y,
                            52,
                            input_h,
                            COLOR_BUTTON_BG,
                            COLOR_BUTTON_BORDER,
                        );
                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            next_btn_x + 6,
                            input_y + 3,
                            "Next >",
                            COLOR_TEXT,
                        );

                        // Close button [x]
                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            panel_x + panel_w - 22,
                            input_y + 3,
                            "X",
                            COLOR_GUTTER_TEXT,
                        );

                        // Replace Row (if in Replace mode)
                        if search_state.mode == SearchBarMode::Replace {
                            let repl_y = panel_y + 36;
                            let repl_border = if search_state.active_field == 1 {
                                COLOR_INPUT_ACTIVE
                            } else {
                                COLOR_INPUT_BORDER
                            };
                            draw_box(
                                buffer,
                                width,
                                height,
                                input_x,
                                repl_y,
                                input_w,
                                input_h,
                                COLOR_INPUT_BG,
                                repl_border,
                            );

                            font_engine.draw_string(
                                buffer,
                                width,
                                height,
                                panel_x + 8,
                                repl_y + 3,
                                "Repl:",
                                COLOR_STATUS_TEXT,
                            );
                            font_engine.draw_string(
                                buffer,
                                width,
                                height,
                                input_x + 5,
                                repl_y + 3,
                                &search_state.replace_query,
                                COLOR_TEXT,
                            );

                            if search_state.active_field == 1 && caret_visible {
                                let cur_x = input_x
                                    + 5
                                    + (search_state.replace_query.chars().count() as f32
                                        * font_engine.char_width_px)
                                        .round() as usize;
                                if cur_x < input_x + input_w - 4 {
                                    draw_rect(
                                        buffer,
                                        width,
                                        height,
                                        cur_x,
                                        repl_y + 4,
                                        2,
                                        input_h - 8,
                                        COLOR_CARET,
                                    );
                                }
                            }

                            // Interactive [Replace] button
                            let repl_btn_x = panel_x + 245;
                            draw_box(
                                buffer,
                                width,
                                height,
                                repl_btn_x,
                                repl_y,
                                65,
                                input_h,
                                COLOR_BUTTON_BG,
                                COLOR_BUTTON_BORDER,
                            );
                            font_engine.draw_string(
                                buffer,
                                width,
                                height,
                                repl_btn_x + 6,
                                repl_y + 3,
                                "Replace",
                                COLOR_TEXT,
                            );

                            // Interactive [All] button
                            let all_btn_x = panel_x + 320;
                            draw_box(
                                buffer,
                                width,
                                height,
                                all_btn_x,
                                repl_y,
                                42,
                                input_h,
                                COLOR_BUTTON_BG,
                                COLOR_BUTTON_BORDER,
                            );
                            font_engine.draw_string(
                                buffer,
                                width,
                                height,
                                all_btn_x + 8,
                                repl_y + 3,
                                "All",
                                COLOR_TEXT,
                            );
                        }
                    }

                    // 10. Render Status Bar
                    let status_y = height.saturating_sub(28);
                    draw_rect(
                        buffer,
                        width,
                        height,
                        0,
                        status_y,
                        width,
                        1,
                        COLOR_STATUS_BORDER,
                    );
                    draw_rect(
                        buffer,
                        width,
                        height,
                        0,
                        status_y + 1,
                        width,
                        27,
                        COLOR_STATUS_BG,
                    );

                    let status_mode = match selection.mode {
                        SelectionMode::Block => {
                            let min_l = selection.anchor_line.min(selection.active_line) + 1;
                            let max_l = selection.anchor_line.max(selection.active_line) + 1;
                            let min_c = selection.anchor_col.min(selection.active_col) + 1;
                            let max_c = selection.anchor_col.max(selection.active_col) + 1;
                            format!("[BLOCK SELECT: R{min_l}-R{max_l}, C{min_c}-C{max_c}]")
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
                        "  Ln {}, Col {}    {}    {}    {} chars / {} B    {}    {}",
                        cur_line + 1,
                        cur_col + 1,
                        status_mode,
                        wrap_text,
                        snap.content.len_chars(),
                        snap.total_bytes,
                        if documents.bom { "UTF-8 BOM" } else { "UTF-8" },
                        if documents.ending == LineEnding::Crlf {
                            "CRLF"
                        } else {
                            "LF"
                        }
                    );

                    let status_color = if selection.mode == SelectionMode::Block {
                        COLOR_STATUS_ACCENT
                    } else {
                        COLOR_STATUS_TEXT
                    };

                    font_engine.draw_string(
                        buffer,
                        width,
                        height,
                        8,
                        status_y + 5,
                        &status_text,
                        status_color,
                    );

                    if show_perf_hud {
                        let hud_w = 360usize;
                        let hud_h = 114usize;
                        let hud_x = width.saturating_sub(hud_w + 16);
                        let hud_y = 12usize;

                        draw_rect(
                            buffer,
                            width,
                            height,
                            hud_x,
                            hud_y,
                            hud_w,
                            hud_h,
                            0xEE1A1A1A,
                        );
                        draw_rect(
                            buffer,
                            width,
                            height,
                            hud_x,
                            hud_y,
                            hud_w,
                            1,
                            0xFF3E3E3E,
                        );
                        draw_rect(
                            buffer,
                            width,
                            height,
                            hud_x,
                            hud_y + hud_h - 1,
                            hud_w,
                            1,
                            0xFF3E3E3E,
                        );
                        draw_rect(
                            buffer,
                            width,
                            height,
                            hud_x,
                            hud_y,
                            1,
                            hud_h,
                            0xFF3E3E3E,
                        );
                        draw_rect(
                            buffer,
                            width,
                            height,
                            hud_x + hud_w - 1,
                            hud_y,
                            1,
                            hud_h,
                            0xFF3E3E3E,
                        );

                        let cur_frame_time = frame_start.elapsed();
                        let l1 = "PERF HUD [F3]";
                        let l2 = format!(
                            "UI prepare  : {:.2} ms",
                            cur_frame_time.as_secs_f64() * 1000.0
                        );
                        let l3 = format!(
                            "Layout time : {:.1} µs",
                            layout_time.as_secs_f64() * 1_000_000.0
                        );
                        let l4 = format!("Doc bytes   : {} B", snap.total_bytes);
                        let l5 = "Snapshot: shared rope".to_string();

                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            hud_x + 10,
                            hud_y + 6,
                            l1,
                            0xFF569CD6,
                        );
                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            hud_x + 10,
                            hud_y + 24,
                            &l2,
                            0xFF4EC9B0,
                        );
                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            hud_x + 10,
                            hud_y + 42,
                            &l3,
                            0xFFDCDCAA,
                        );
                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            hud_x + 10,
                            hud_y + 60,
                            &l4,
                            0xFFCCCCCC,
                        );
                        font_engine.draw_string(
                            buffer,
                            width,
                            height,
                            hud_x + 10,
                            hud_y + 78,
                            &l5,
                            0xFF9CDCFE,
                        );
                        font_engine.draw_string(buffer,width,height,hud_x+10,hud_y+96,&renderer.description,0xFF9CDCFE);
                    }

                    current_view = Some((layout, config));
                    if let Err(error) = renderer.render(buffer, size.width, size.height) {
                        // Preserve in-flight file work; report the failure only after it finishes.
                        renderer_failed = true;
                        file_actions.push_back(DocumentAction::ReportError(format!(
                            "GPU rendering failed: {error}. Texted will close; you can save unsaved changes before closing."
                        )));
                        file_actions.push_back(DocumentAction::Close);
                    }
                }
                _ => {}
            },
            Event::UserEvent(AppEvent(action)) => {
                file_actions.push_back(action);
                window.request_redraw();
            }
            Event::AboutToWait => {
                // Finish an already requested replacement before inspecting dirty state or
                // replacing/closing its document. RedrawRequested drains its result.
                if let (None, Ok(BufferResponse::Snapshot(snap))) = (
                    &search_state.mutation,
                    buffer_client.send(BufferRequest::GetSnapshot),
                ) {
                    let mut effect = documents.poll(&snap, &buffer_client, &window);
                    if matches!(effect, DocumentEffect::None) && documents.is_idle() {
                        if let Some(action) = file_actions.pop_front() {
                            effect = documents.handle(action, &snap, &buffer_client, &window);
                        }
                    }
                    match effect {
                        DocumentEffect::Close => {
                            let _ = buffer_client.send(BufferRequest::Shutdown);
                            elwt.exit();
                            return;
                        }
                        DocumentEffect::Install(document) => {
                            documents.installed(&document);
                            let _ =
                                buffer_client.send(BufferRequest::LoadDocument(document.content));
                            top_line = 0;
                            top_wrap_row = 0;
                            current_view = None;
                            horizontal.reset();
                            current_scrollbar = None;
                            selection = SelectionState::default();
                            search_state = SearchState::default();
                            is_mouse_dragging = false;
                            click_count = 0;
                            word_drag_anchor = None;
                            line_drag_anchor = None;
                            snap_to_cursor_needed = true;
                            caret_visible = true;
                            last_caret_blink = Instant::now();
                            window.request_redraw();
                        }
                        DocumentEffect::None => {}
                    }
                }
                if search_state.pending
                    || search_state.mutation.is_some()
                    || last_caret_blink.elapsed() >= Duration::from_millis(500)
                {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    });

    let _ = shutdown_client.send(BufferRequest::Shutdown);
    buffer_thread.join().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn find_word_boundaries(text: &str, col: usize) -> (usize, usize) {
        let rope = ropey::Rope::from_str(text);
        let bounds = texted_buffer::word::bounds(rope.slice(..), col);
        (bounds.start, bounds.end)
    }

    #[test]
    fn background_search_is_keyed_by_revision_and_query() {
        let mut b = TextBuffer::from_string("їжак їжак".into());
        let mut search = SearchState {
            find_query: "їжак".into(),
            ..Default::default()
        };
        submit_search(b.create_snapshot(), &mut search);
        let (revision, query, matches) =
            search.results.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(search.submitted, Some((revision, query)));
        assert_eq!(matches, vec![(0, 8), (9, 17)]);
        b.insert(0, "!");
        submit_search(b.create_snapshot(), &mut search);
        assert!(search.matches.is_empty());
        let (new_revision, _, matches) =
            search.results.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_ne!(new_revision, revision);
        assert_eq!(matches, vec![(1, 9), (10, 18)]);
    }
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

    #[test]
    fn test_compute_gutter_metrics() {
        // Small file (10 lines): minimum 3 digits, compact gutter
        let (g_w_small, t_x_small, digits_small) = compute_gutter_metrics(10, true, 8.0);
        assert_eq!(digits_small, 3);
        assert_eq!(g_w_small, 44.0); // 10 + 3*8 + 10
        assert_eq!(t_x_small, 52); // 44 + 8

        // Huge file (3.12M lines): 7 digits, expands dynamically without text overlap
        let (g_w_3m, t_x_3m, digits_3m) = compute_gutter_metrics(3_125_761, true, 8.0);
        assert_eq!(digits_3m, 7);
        assert_eq!(g_w_3m, 76.0); // 10 + 7*8 + 10
        assert_eq!(t_x_3m, 84); // 76 + 8

        // 100M lines: 9 digits
        let (g_w_100m, t_x_100m, digits_100m) = compute_gutter_metrics(100_000_000, true, 8.0);
        assert_eq!(digits_100m, 9);
        assert_eq!(g_w_100m, 92.0); // 10 + 9*8 + 10
        assert_eq!(t_x_100m, 100);

        // Line numbers disabled
        let (g_w_off, t_x_off, digits_off) = compute_gutter_metrics(3_125_761, false, 8.0);
        assert_eq!(digits_off, 0);
        assert_eq!(g_w_off, 0.0);
        assert_eq!(t_x_off, 10);
    }
}
