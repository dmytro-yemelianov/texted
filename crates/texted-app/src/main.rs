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
mod editor_view;
mod gpu;
mod graphics;
mod palette;
use editor_view::{compute_gutter_metrics, SnapshotLines};
use texted_app::search::{SearchBarMode, SearchState};
mod native_macos;
use documents::{Action as DocumentAction, Documents, Effect as DocumentEffect};
#[derive(Debug, Clone)]
pub struct AppEvent(DocumentAction);
mod horizontal_scroll;
use graphics::{DrawList, FontEngine};
use horizontal_scroll::{HorizontalScroll, BAR_HEIGHT};
use std::time::{Duration, Instant};

use texted_buffer::BufferSnapshot;
use texted_io::LineEnding;
use texted_layout::{LayoutEngine, ViewportConfig};
use winit::event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::WindowBuilder;

use texted_app::buffer_host::BufferHost;
pub use texted_app::buffer_host::{BufferRequest, BufferResponse};

pub use texted_app::selection::{SelectionMode, SelectionState};

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
    search.submit(snap);
}
fn start_replacement(
    client: &texted_kernel::ChannelClient<BufferRequest, BufferResponse>,
    search: &mut SearchState,
    all: bool,
) {
    if let Ok(BufferResponse::Snapshot(snap)) = client.send(BufferRequest::GetSnapshot) {
        search.start_replacement(snap, all);
    }
}
fn find_next_match(
    client: &texted_kernel::ChannelClient<BufferRequest, BufferResponse>,
    search: &mut SearchState,
    _selection: &mut SelectionState,
) {
    if let Ok(BufferResponse::Snapshot(snap)) = client.send(BufferRequest::GetSnapshot) {
        search.navigate(snap, true);
    }
}
fn find_prev_match(
    client: &texted_kernel::ChannelClient<BufferRequest, BufferResponse>,
    search: &mut SearchState,
    _selection: &mut SelectionState,
) {
    if let Ok(BufferResponse::Snapshot(snap)) = client.send(BufferRequest::GetSnapshot) {
        search.navigate(snap, false);
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

    let mut buffer_host = BufferHost::spawn(initial_content);
    let buffer_client = buffer_host.client.clone();

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
    let search_wake = window.clone();
    let mut search_state = SearchState::new(move || search_wake.request_redraw());
    let mut is_mouse_dragging = false;
    let mut last_click_time = Instant::now();
    let mut click_count = 0usize;
    let mut last_mouse_pos = (0.0f32, 0.0f32);
    let mut show_perf_hud = false;
    let mut word_drag_anchor: Option<(usize, usize, usize)> = None; // (line, w_start, w_end)
    let mut line_drag_anchor: Option<usize> = None; // line
    let mut current_text_start_x = 64.0f32;

    println!("[Texted] GUI editor running: Dark Mode, Word Wrap, macOS Cmd/Opt navigation.");

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
                                    search_state.close();
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

                                search_state.cancel_navigation();
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
                                search_state.close();
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
                            search_state.cancel_mutation();
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
                            search_state.cancel_navigation();
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
                    if let Some(Ok(Err(error)))=&replacement {
                        file_actions.push_back(DocumentAction::ReportError(error.clone()));
                        search_state.mutation=None;
                    }
                    if let Some(Ok(Ok(plan))) = replacement {
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
                    if let Some((revision,(start,end)))=search_state.poll() {
                        if revision==snap.revision {
                            let (sl,sc)=snap.offset_to_line_col(start);let (el,ec)=snap.offset_to_line_col(end);
                            selection=SelectionState {mode:SelectionMode::Stream,anchor_line:sl,anchor_col:sc,active_line:el,active_col:ec};
                            let _=buffer_client.send(BufferRequest::SetCursorOffset(end));
                            if let Ok(BufferResponse::Snapshot(updated))=buffer_client.send(BufferRequest::GetSnapshot){snap=updated;}
                            snap_to_cursor_needed=true;
                        }
                    }
                    if let Some(error)=search_state.error.take(){file_actions.push_back(DocumentAction::ReportError(error));}
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

                    search_state.set_view(&snap,&layout);
                    editor_view::paint(buffer,&mut font_engine,&editor_view::ViewModel {
                        snap:&snap,layout:&layout,config:&config,selection:&selection,search_state:&search_state,
                        width,height,text_height,gutter_w,line_digits,text_start_x,cur_line,cur_col,
                        word_wrap,show_line_numbers,caret_visible,current_scrollbar,show_perf_hud,
                        bom:documents.bom,ending:documents.ending,gpu_name:&renderer.description,frame_start,layout_time,
                    });

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
                            search_state.reset_document();
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

    buffer_host.shutdown();
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
