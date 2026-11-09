//! Immutable view models and focused drawing-command preparation.
use crate::{
    graphics::{draw_box, draw_rect, DrawList, FontEngine},
    horizontal_scroll::{Scrollbar, BAR_HEIGHT},
    lace::layer::Placed,
    search_ui::{CLOSE_X, NEXT_X, PANEL_WIDTH, PREVIOUS_X},
};
use std::time::{Duration, Instant};
use texted_app::{
    search::{SearchBarMode, SearchState},
    selection::{SelectionMode, SelectionState},
};
use texted_buffer::BufferSnapshot;
use texted_layout::{ViewportConfig, ViewportLayout};
pub(crate) struct ViewModel<'a> {
    pub palette: &'a texted_app::appearance::Palette,
    pub snap: &'a BufferSnapshot,
    pub layout: &'a ViewportLayout,
    pub config: &'a ViewportConfig,
    pub selection: &'a SelectionState,
    pub search_state: &'a SearchState,
    pub compact: bool,
    pub width: usize,
    pub height: usize,
    pub text_height: usize,
    pub gutter_w: f32,
    pub line_digits: usize,
    pub text_start_x: usize,
    pub show_line_numbers: bool,
    pub caret_visible: bool,
    pub current_scrollbar: Option<Scrollbar>,
    pub show_perf_hud: bool,
    pub gpu_name: &'a str,
    pub frame_start: Instant,
    pub layout_time: Duration,
    pub lace: Option<LaceLayer<'a>>,
}
/// The lace to draw under the text.
pub(crate) struct LaceLayer<'a> {
    /// Connections; `Placed::row` counts the selected rows, which start at `first_row`.
    pub placed: &'a [Placed],
    /// The first selected row's place in `layout.visible_lines`: −1 when the hidden row
    /// above leads. Rows outside the visible lines sit one line height beyond them.
    pub first_row: isize,
    /// How far the colour goes from the background towards the text, 0 to 1.
    pub strength: f32,
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
    paint_lace(buffer, font_engine, model);
    paint_text(buffer, font_engine, model);
    paint_caret(buffer, font_engine, model);
    if model.search_state.mode != SearchBarMode::Closed {
        buffer
            .cells
            .push(texted_app::cell_grid::CellCommand::HideCursor);
    }
    paint_search_bar(buffer, ui_font, model);
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
            model.text_height,
            model.palette.gutter_bg,
        );
        draw_rect(
            buffer,
            width,
            height,
            gutter_width_int.saturating_sub(1),
            0,
            1,
            model.text_height,
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

/// The lace under the text: each placed sprite as a glyph quad on the text's pixel
/// grid, clipped to the text area, in the background colour moved `strength` of the
/// way to the text colour.
fn paint_lace(buffer: &mut DrawList, font: &mut FontEngine, model: &ViewModel<'_>) {
    let Some(lace) = &model.lace else {
        return;
    };
    let lines = &model.layout.visible_lines;
    let (placed, Some(first)) = (lace.placed, lines.first()) else {
        return;
    };
    // A selected row's top: a visible line's, or one line height beyond them.
    let step = font.line_height_px;
    let row_top = |row: usize| {
        let at = lace.first_row + row as isize;
        match usize::try_from(at).ok().and_then(|at| lines.get(at)) {
            Some(line) => (line.y_px as usize) as f32,
            None => first.y_px + at as f32 * step,
        }
    };
    let color = mix(model.palette.bg, model.palette.text, lace.strength);
    let pixel = font.pixel_px();
    // Column x accumulated and rounded as `draw_string` places glyphs, so faces whose
    // width lands on half pixels round the same way as the text.
    let columns = placed.iter().map(|p| p.col).max().unwrap_or(0) + 1;
    let mut xs = Vec::with_capacity(columns);
    let mut x = model.text_start_x as f32;
    for _ in 0..columns {
        xs.push(x.round());
        x += font.char_width_px;
    }
    let area = [
        model.text_start_x as f32,
        0.0,
        model.width as f32,
        model.text_height as f32,
    ];
    for p in placed {
        let sprite = &p.entry.sprite;
        if sprite.width == 0 || sprite.height == 0 {
            continue;
        }
        let glyph = font.sprite_glyph(sprite);
        let x = xs[p.col] + sprite.origin[0] as f32 * pixel;
        // Text glyphs sit two pixels below the row's top, as in `paint_text`.
        let y = row_top(p.row) + 2.0 + sprite.origin[1] as f32 * pixel;
        let size = [sprite.width as f32 * pixel, sprite.height as f32 * pixel];
        buffer.push_within([x, y, size[0], size[1]], area, color, Some(glyph));
    }
}

/// Each channel of `from` moved `t` of the way to `to`.
fn mix(from: u32, to: u32, t: f32) -> u32 {
    [16, 8, 0].iter().fold(0, |out, &shift| {
        let (a, b) = (((from >> shift) & 255) as f32, ((to >> shift) & 255) as f32);
        out | (((a + (b - a) * t).round().clamp(0.0, 255.0) as u32) << shift)
    })
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
    // 7. Render visible lines using the selected native bitmap strike.
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
            buffer
                .cells
                .push(texted_app::cell_grid::CellCommand::Cursor {
                    at: [cx as f32, cy as f32],
                });
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
        model.width.min(x + w.saturating_sub(3)),
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
            buffer
                .cells
                .push(texted_app::cell_grid::CellCommand::Cursor {
                    at: [caret as f32, (y + 4) as f32],
                });
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
    if model.compact || model.width < PANEL_WIDTH + 16 {
        paint_compact_search(buffer, font_engine, model);
        return;
    }
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

fn paint_compact_search(buffer: &mut DrawList, font: &mut FontEngine, model: &ViewModel<'_>) {
    use crate::search_ui::CompactBar;
    let mode = model.search_state.mode;
    if mode == SearchBarMode::Closed {
        return;
    }
    let g = CompactBar::new(mode, model.width);
    let c = model.palette;
    draw_box(
        buffer,
        model.width,
        model.height,
        g.panel.x,
        g.panel.y,
        g.panel.w,
        g.panel.h,
        c.panel_bg,
        c.panel_border,
    );
    for (r, text, active) in [
        (
            g.find,
            model.search_state.find_query.as_str(),
            model.search_state.active_field == 0,
        ),
        (
            g.replacement,
            model.search_state.replace_query.as_str(),
            model.search_state.active_field == 1,
        ),
    ] {
        if r.y == 40 && mode != SearchBarMode::Replace {
            continue;
        }
        draw_box(
            buffer,
            model.width,
            model.height,
            r.x,
            r.y,
            r.w,
            r.h,
            c.input_bg,
            if active {
                c.input_active
            } else {
                c.input_border
            },
        );
        font.draw_string(
            buffer,
            (r.x + r.w).saturating_sub(2),
            model.height,
            r.x + 2,
            r.y + 3,
            text,
            c.text,
        );
        if active && model.caret_visible {
            let count = text.chars().take(r.w / 8).count();
            let x = r.x + 2 + count * 8;
            if x + 1 < r.x + r.w {
                buffer
                    .cells
                    .push(texted_app::cell_grid::CellCommand::Cursor {
                        at: [x as f32, (r.y + 2) as f32],
                    });
                draw_rect(
                    buffer,
                    model.width,
                    model.height,
                    x,
                    r.y + 2,
                    1,
                    10,
                    c.caret,
                );
            }
        }
    }
    font.draw_string(
        buffer,
        model.width,
        model.height,
        g.panel.x + 2,
        10,
        "Find",
        c.text,
    );
    font.draw_string(
        buffer,
        g.panel.x + g.panel.w - 4,
        model.height,
        g.panel.x + 4,
        25,
        &model.search_state.label(),
        c.status_text,
    );
    for (r, text) in [
        (g.previous, "<"),
        (g.next, ">"),
        (g.close, "X"),
        (g.one, "One"),
        (g.all, "All"),
    ] {
        if r.y == 40 && mode != SearchBarMode::Replace {
            continue;
        }
        draw_box(
            buffer,
            model.width,
            model.height,
            r.x,
            r.y,
            r.w,
            r.h,
            c.button_bg,
            c.button_border,
        );
        font.draw_string(
            buffer,
            r.x + r.w,
            model.height,
            r.x + 2,
            r.y + 3,
            text,
            c.text,
        );
    }
    if mode == SearchBarMode::Replace {
        font.draw_string(
            buffer,
            model.width,
            model.height,
            g.panel.x + 2,
            43,
            "Repl",
            c.text,
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        graphics::Draw,
        lace::{
            ligatures::Mech,
            sprite::Sprite,
            table::{Entry, Relation},
        },
        viewport::Viewport,
    };
    use std::sync::Arc;
    use texted_app::appearance::{Appearance, FontFamily, Palette, Typography};

    fn placed(row: usize, col: usize, origin: [i32; 2], size: [u32; 2]) -> Placed {
        Placed {
            row,
            col,
            rel: Relation::H,
            entry: Arc::new(Entry {
                mech: Mech::Fuse,
                cost: 0.0,
                keys: Vec::new(),
                sprite: Arc::new(Sprite {
                    origin,
                    width: size[0],
                    height: size[1],
                    coverage: vec![255; (size[0] * size[1]) as usize],
                }),
                trim: None,
                drop: Vec::new(),
                absorb: None,
            }),
        }
    }

    /// Paints `text` in VGA at `zoom` percent with `lace` from `first_row` at strength 0.35.
    fn painted(text: &str, zoom: u16, lace: &[Placed], first_row: isize) -> (DrawList, Palette) {
        let snapshot = texted_buffer::TextBuffer::from_string(text.into()).create_snapshot();
        let mut font = FontEngine::with_typography(Typography {
            family: FontFamily::Vga,
            size_px: 16,
            line_spacing: 100,
            zoom_percent: zoom,
        });
        let mut ui = FontEngine::new(16.0);
        let mut viewport = Viewport::new(|| {});
        let frame = viewport.prepare(&snapshot, &font, 800, 400);
        let palette = Appearance::default().palette().unwrap();
        let (selection, search) = (SelectionState::default(), SearchState::default());
        let mut list = DrawList::default();
        paint(
            &mut list,
            &mut font,
            &mut ui,
            &ViewModel {
                palette: &palette,
                snap: &snapshot,
                layout: &frame.layout,
                config: &frame.config,
                selection: &selection,
                search_state: &search,
                compact: false,
                width: 800,
                height: 400,
                text_height: frame.text_height,
                gutter_w: frame.gutter_w,
                line_digits: frame.line_digits,
                text_start_x: frame.text_start_x,
                show_line_numbers: viewport.line_numbers,
                caret_visible: false,
                current_scrollbar: None,
                show_perf_hud: false,
                gpu_name: "",
                frame_start: Instant::now(),
                layout_time: Duration::ZERO,
                lace: Some(LaceLayer {
                    placed: lace,
                    first_row,
                    strength: 0.35,
                }),
            },
        );
        (list, palette)
    }

    /// Glyph quads whose bitmap has `size` font pixels, with their places in the list.
    fn quads(list: &DrawList, size: [u32; 2]) -> Vec<(usize, &Draw)> {
        list.draws
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                d.glyph
                    .as_ref()
                    .is_some_and(|g| [g.width, g.height] == size)
            })
            .collect()
    }

    fn text_quads<'a>(list: &'a DrawList, palette: &Palette) -> Vec<(usize, &'a Draw)> {
        list.draws
            .iter()
            .enumerate()
            .filter(|(_, d)| d.glyph.is_some() && d.color == palette.text)
            .collect()
    }

    #[test]
    fn lace_draws_before_text() {
        let lace = [
            placed(0, 0, [2, 4], [3, 5]),
            placed(0, 7, [5, 9], [3, 5]),
            placed(1, 1, [-1, 12], [3, 5]),
        ];
        let (list, palette) = painted("office staff\nfifth", 100, &lace, 0);
        let strokes = quads(&list, [3, 5]);
        assert_eq!(strokes.len(), lace.len());
        let first_text = text_quads(&list, &palette)[0].0;
        let at: Vec<usize> = strokes.iter().map(|(i, _)| *i).collect();
        assert!(at.iter().all(|&i| i < first_text), "{at:?} vs {first_text}");
        // Each channel goes 35 % of the way from the background to the text colour.
        let channel = |c: u32, shift: u32| ((c >> shift) & 255) as f32;
        let want = [16, 8, 0].iter().fold(0, |out, &shift| {
            let (bg, text) = (channel(palette.bg, shift), channel(palette.text, shift));
            out | (((bg + (text - bg) * 0.35).round() as u32) << shift)
        });
        assert_ne!(want, palette.bg);
        assert!(strokes.iter().all(|(_, d)| d.color == want), "{want:06x}");
    }

    #[test]
    fn sprites_align_with_text_at_fractional_zoom() {
        let text = "abcdefghijklmnopqrstuvwxyz".repeat(2);
        let lace: Vec<Placed> = (0..text.len())
            .map(|col| placed(0, col, [0, 0], [1, 1]))
            .collect();
        let (list, palette) = painted(&text, 110, &lace, 0);
        let strokes = quads(&list, [1, 1]);
        let glyphs = text_quads(&list, &palette);
        assert_eq!(strokes.len(), text.len());
        for (col, ((_, s), (_, g))) in strokes.iter().zip(&glyphs).enumerate() {
            assert_eq!(s.rect[..2], g.rect[..2], "column {col}");
            assert!((s.rect[2] - 1.1).abs() < 1e-4, "{:?}", s.rect);
        }
    }

    #[test]
    fn hidden_rows_draw_one_line_beyond_the_visible_ones() {
        // Rows: hidden above, the two visible lines, hidden below.
        let lace = [
            placed(0, 0, [0, 13], [1, 4]),
            placed(1, 0, [0, 0], [2, 2]),
            placed(3, 0, [0, 0], [3, 3]),
        ];
        let (list, _) = painted("office\nfifth", 100, &lace, -1);
        let at = |size| quads(&list, size)[0].1.rect;
        // 16 px above the first line, two below its top, 13 down: one pixel above the
        // text area, which clips it.
        let clipped = at([1, 4]);
        assert_eq!([clipped[1], clipped[3]], [0.0, 3.0]);
        assert_eq!(at([2, 2])[1], 2.0);
        assert_eq!(at([3, 3])[1], 2.0 * 16.0 + 2.0);
        assert_eq!(at([1, 4])[0], at([3, 3])[0]);
    }
}
