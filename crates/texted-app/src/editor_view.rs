//! Immutable view models and focused drawing-command preparation.
use crate::{
    graphics::{draw_box, draw_rect, DrawList, FontEngine},
    horizontal_scroll::{Scrollbar, BAR_HEIGHT},
    search_ui::{CLOSE_X, NEXT_X, PANEL_WIDTH, PREVIOUS_X},
};
use std::time::{Duration, Instant};
use texted_app::{
    search::{SearchBarMode, SearchState},
    selection::{SelectionMode, SelectionState},
};
use texted_buffer::BufferSnapshot;
use texted_io::LineEnding;
use texted_layout::{ViewportConfig, ViewportLayout};
pub(crate) struct ViewModel<'a> {
    pub palette: &'a texted_app::appearance::Palette,
    pub snap: &'a BufferSnapshot,
    pub layout: &'a ViewportLayout,
    pub config: &'a ViewportConfig,
    pub selection: &'a SelectionState,
    pub search_state: &'a SearchState,
    pub width: usize,
    pub height: usize,
    pub text_height: usize,
    pub gutter_w: f32,
    pub line_digits: usize,
    pub text_start_x: usize,
    pub cur_line: usize,
    pub cur_col: usize,
    pub word_wrap: bool,
    pub show_line_numbers: bool,
    pub caret_visible: bool,
    pub current_scrollbar: Option<Scrollbar>,
    pub show_perf_hud: bool,
    pub bom: bool,
    pub ending: LineEnding,
    pub gpu_name: &'a str,
    pub frame_start: Instant,
    pub layout_time: Duration,
}
pub(crate) fn paint(
    buffer: &mut DrawList,
    font_engine: &mut FontEngine,
    ui_font: &mut FontEngine,
    model: &ViewModel<'_>,
) {
    buffer.reset(model.palette.bg);
    paint_gutter(buffer, model);
    paint_search(buffer, model);
    paint_selection(buffer, font_engine, model);
    paint_text(buffer, font_engine, model);
    paint_caret(buffer, font_engine, model);
    paint_search_bar(buffer, ui_font, model);
    paint_status(buffer, ui_font, model);
    if model.show_perf_hud {
        paint_hud(buffer, ui_font, model);
    }
}
fn paint_gutter(buffer: &mut DrawList, model: &ViewModel<'_>) {
    let ViewModel {
        width,
        height,
        gutter_w,
        show_line_numbers,
        ..
    } = *model;
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
            model.palette.gutter_bg,
        );
        draw_rect(
            buffer,
            width,
            height,
            gutter_width_int.saturating_sub(1),
            0,
            1,
            height.saturating_sub(28),
            model.palette.gutter_border,
        );
    }
}

fn paint_search(buffer: &mut DrawList, model: &ViewModel<'_>) {
    let ViewModel {
        snap,
        layout,
        config,
        search_state,
        width,
        height,
        ..
    } = *model;
    for highlight in search_state.highlights(snap, layout, config).highlights {
        let rect = highlight.rect;
        draw_rect(
            buffer,
            width,
            height,
            rect.x.round() as usize,
            rect.y.round() as usize + 2,
            rect.width.round() as usize,
            rect.height as usize,
            if highlight.active {
                model.palette.search_active
            } else {
                model.palette.search_match
            },
        );
    }
}

fn paint_selection(buffer: &mut DrawList, font_engine: &mut FontEngine, model: &ViewModel<'_>) {
    let ViewModel {
        snap,
        layout,
        config,
        selection,
        width,
        height,
        ..
    } = *model;
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
                    let Some(rect) = layout.highlight(config, v_line, min_c, max_c) else {
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
                        model.palette.block_selection,
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

        let lines = SnapshotLines(snap);

        if (s_line, s_col) != (e_line, e_col) {
            for v_line in &layout.visible_lines {
                let l_idx = v_line.line_index;
                {
                    if l_idx >= s_line && l_idx <= e_line {
                        let row_y = v_line.y_px as usize;
                        let line_len = lines.get(l_idx).map(|l| l.len_chars()).unwrap_or(0);

                        let from_c = if l_idx == s_line { s_col } else { 0 };
                        let to_c = if l_idx == e_line { e_col } else { line_len };

                        if l_idx == e_line && from_c >= to_c {
                            continue;
                        }

                        let Some(rect) = layout.highlight(config, v_line, from_c, to_c) else {
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
                            model.palette.selection,
                        );
                    }
                }
            }
        }
    }
}

fn paint_text(buffer: &mut DrawList, font_engine: &mut FontEngine, model: &ViewModel<'_>) {
    let ViewModel {
        layout,
        width,
        height,
        text_height,
        line_digits,
        text_start_x,
        show_line_numbers,
        ..
    } = *model;
    // 7. Render Visible Lines (with antialiased Ukrainian font support)
    for line in &layout.visible_lines {
        let y_int = line.y_px as usize;
        if y_int + (font_engine.line_height_px as usize) > text_height {
            break;
        }

        // Gutter line number (only on non-continuation rows)
        if show_line_numbers && !line.is_continuation {
            let num_str = format!("{:>width$}", line.line_index + 1, width = line_digits);
            font_engine.draw_string(
                buffer,
                width,
                height,
                10,
                y_int + 2,
                &num_str,
                model.palette.gutter_text,
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
            model.palette.text,
        );
    }
}

fn paint_caret(buffer: &mut DrawList, font_engine: &mut FontEngine, model: &ViewModel<'_>) {
    let ViewModel {
        layout,
        width,
        height,
        text_height,
        caret_visible,
        current_scrollbar,
        ..
    } = *model;
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
                model.palette.caret,
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
            model.palette.gutter_bg,
        );
        draw_rect(
            buffer,
            width,
            height,
            bar.thumb.x.round() as usize,
            bar.thumb.y as usize,
            bar.thumb.width.round() as usize,
            bar.thumb.height as usize,
            model.palette.gutter_text,
        );
    }
}

fn input_box(
    buffer: &mut DrawList,
    font_engine: &mut FontEngine,
    model: &ViewModel<'_>,
    rect: (usize, usize, usize, usize),
    text: &str,
    active: bool,
) {
    let (x, y, w, h) = rect;
    draw_box(
        buffer,
        model.width,
        model.height,
        x,
        y,
        w,
        h,
        model.palette.input_bg,
        if active {
            model.palette.input_active
        } else {
            model.palette.input_border
        },
    );
    font_engine.draw_string(
        buffer,
        model.width,
        model.height,
        x + 5,
        y + 3,
        text,
        model.palette.text,
    );
    if active && model.caret_visible {
        let max_chars = (w.saturating_sub(9) as f32 / font_engine.char_width_px).ceil() as usize;
        let chars = text.chars().take(max_chars + 1).count();
        let caret = x + 5 + (chars as f32 * font_engine.char_width_px).round() as usize;
        if caret < x + w - 4 {
            draw_rect(
                buffer,
                model.width,
                model.height,
                caret,
                y + 4,
                2,
                h - 8,
                model.palette.caret,
            );
        }
    }
}
fn button(
    buffer: &mut DrawList,
    font_engine: &mut FontEngine,
    model: &ViewModel<'_>,
    rect: (usize, usize, usize),
    label: &str,
    padding: usize,
) {
    let (x, y, w) = rect;
    draw_box(
        buffer,
        model.width,
        model.height,
        x,
        y,
        w,
        24,
        model.palette.button_bg,
        model.palette.button_border,
    );
    font_engine.draw_string(
        buffer,
        model.width,
        model.height,
        x + padding,
        y + 3,
        label,
        model.palette.text,
    );
}
fn paint_search_bar(buffer: &mut DrawList, font_engine: &mut FontEngine, model: &ViewModel<'_>) {
    let ViewModel {
        width,
        height,
        search_state,
        ..
    } = *model;
    if search_state.mode == SearchBarMode::Closed {
        return;
    }
    let panel_x = width.saturating_sub(PANEL_WIDTH + 16);
    let panel_y = 10;
    let panel_h = if search_state.mode == SearchBarMode::Replace {
        68
    } else {
        36
    };
    draw_box(
        buffer,
        width,
        height,
        panel_x,
        panel_y,
        PANEL_WIDTH,
        panel_h,
        model.palette.panel_bg,
        model.palette.panel_border,
    );
    let input_x = panel_x + 50;
    let input_y = panel_y + 6;
    font_engine.draw_string(
        buffer,
        width,
        height,
        panel_x + 8,
        input_y + 3,
        "Find:",
        model.palette.status_text,
    );
    input_box(
        buffer,
        font_engine,
        model,
        (input_x, input_y, 160, 24),
        &search_state.find_query,
        search_state.active_field == 0,
    );
    font_engine.draw_string(
        buffer,
        width,
        height,
        input_x + 168,
        input_y + 3,
        &search_state.label(),
        model.palette.status_text,
    );
    button(
        buffer,
        font_engine,
        model,
        (panel_x + PREVIOUS_X, input_y, 52),
        "< Prev",
        6,
    );
    button(
        buffer,
        font_engine,
        model,
        (panel_x + NEXT_X, input_y, 52),
        "Next >",
        6,
    );
    font_engine.draw_string(
        buffer,
        width,
        height,
        panel_x + CLOSE_X,
        input_y + 3,
        "X",
        model.palette.gutter_text,
    );
    if search_state.mode == SearchBarMode::Replace {
        let y = panel_y + 36;
        font_engine.draw_string(
            buffer,
            width,
            height,
            panel_x + 8,
            y + 3,
            "Repl:",
            model.palette.status_text,
        );
        input_box(
            buffer,
            font_engine,
            model,
            (input_x, y, 160, 24),
            &search_state.replace_query,
            search_state.active_field == 1,
        );
        button(
            buffer,
            font_engine,
            model,
            (panel_x + 245, y, 65),
            "Replace",
            6,
        );
        button(buffer, font_engine, model, (panel_x + 320, y, 42), "All", 8);
    }
}

fn paint_status(buffer: &mut DrawList, font_engine: &mut FontEngine, model: &ViewModel<'_>) {
    let ViewModel {
        snap,
        selection,
        search_state,
        width,
        height,
        cur_line,
        cur_col,
        word_wrap,
        bom,
        ending,
        ..
    } = *model;
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
        model.palette.status_border,
    );
    draw_rect(
        buffer,
        width,
        height,
        0,
        status_y + 1,
        width,
        27,
        model.palette.status_bg,
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
        if bom { "UTF-8 BOM" } else { "UTF-8" },
        if ending == LineEnding::Crlf {
            "CRLF"
        } else {
            "LF"
        }
    );

    let status_color = if selection.mode == SelectionMode::Block {
        model.palette.status_accent
    } else {
        model.palette.status_text
    };

    font_engine.draw_string(
        buffer,
        width.saturating_sub(100),
        height,
        8,
        status_y + 5,
        &status_text,
        status_color,
    );
    crate::appearance_ui::paint_status_button(buffer, font_engine, width, height, model.palette);
}

fn paint_hud(buffer: &mut DrawList, font_engine: &mut FontEngine, model: &ViewModel<'_>) {
    let ViewModel {
        snap,
        width,
        height,
        gpu_name,
        frame_start,
        layout_time,
        ..
    } = *model;

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
        model.palette.panel_bg,
    );
    draw_rect(
        buffer,
        width,
        height,
        hud_x,
        hud_y,
        hud_w,
        1,
        model.palette.panel_border,
    );
    draw_rect(
        buffer,
        width,
        height,
        hud_x,
        hud_y + hud_h - 1,
        hud_w,
        1,
        model.palette.panel_border,
    );
    draw_rect(
        buffer,
        width,
        height,
        hud_x,
        hud_y,
        1,
        hud_h,
        model.palette.panel_border,
    );
    draw_rect(
        buffer,
        width,
        height,
        hud_x + hud_w - 1,
        hud_y,
        1,
        hud_h,
        model.palette.panel_border,
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
        model.palette.status_accent,
    );
    font_engine.draw_string(
        buffer,
        width,
        height,
        hud_x + 10,
        hud_y + 24,
        &l2,
        model.palette.text,
    );
    font_engine.draw_string(
        buffer,
        width,
        height,
        hud_x + 10,
        hud_y + 42,
        &l3,
        model.palette.text,
    );
    font_engine.draw_string(
        buffer,
        width,
        height,
        hud_x + 10,
        hud_y + 60,
        &l4,
        model.palette.text,
    );
    font_engine.draw_string(
        buffer,
        width,
        height,
        hud_x + 10,
        hud_y + 78,
        &l5,
        model.palette.text,
    );
    font_engine.draw_string(
        buffer,
        width,
        height,
        hud_x + 10,
        hud_y + 96,
        gpu_name,
        model.palette.text,
    );
}

pub(crate) fn compute_gutter_metrics(
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

pub(crate) struct SnapshotLines<'a>(pub(crate) &'a BufferSnapshot);
impl<'a> SnapshotLines<'a> {
    pub(crate) fn len(&self) -> usize {
        self.0.total_lines
    }
    pub(crate) fn get(&self, line: usize) -> Option<ropey::RopeSlice<'a>> {
        (line < self.len()).then(|| self.0.line(line))
    }
}
