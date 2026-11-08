//! Viewport geometry, scrolling and caret pacing; no document mutation or IPC.
use crate::{
    editor_view::compute_gutter_metrics,
    graphics::FontEngine,
    horizontal_scroll::{HorizontalScroll, Scrollbar, BAR_HEIGHT},
};
use std::time::{Duration, Instant};
use texted_buffer::BufferSnapshot;
use texted_layout::{LayoutEngine, ViewportConfig, ViewportLayout};
pub(crate) struct Frame {
    pub layout: ViewportLayout,
    pub config: ViewportConfig,
    pub text_height: usize,
    pub gutter_w: f32,
    pub text_start_x: usize,
    pub line_digits: usize,
    pub cur_line: usize,
    pub cur_col: usize,
    pub layout_time: Duration,
}
pub(crate) struct Viewport {
    pub top_line: usize,
    pub top_wrap_row: usize,
    pub wrap: bool,
    pub line_numbers: bool,
    pub current: Option<Frame>,
    pub bar: Option<Scrollbar>,
    pub caret: bool,
    pub blink: Instant,
    pub snap: bool,
    pub horizontal: HorizontalScroll,
    vertical_remainder: f64,
}
impl Viewport {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        Self {
            top_line: 0,
            top_wrap_row: 0,
            wrap: true,
            line_numbers: true,
            current: None,
            bar: None,
            caret: true,
            blink: Instant::now(),
            snap: false,
            horizontal: HorizontalScroll::new(wake),
            vertical_remainder: 0.0,
        }
    }
    pub fn input(&mut self, snap: bool) {
        self.caret = true;
        self.blink = Instant::now();
        self.snap |= snap;
    }
    pub fn reset(&mut self) {
        self.top_line = 0;
        self.top_wrap_row = 0;
        self.current = None;
        self.horizontal.reset();
        self.bar = None;
        self.input(true);
    }
    pub fn point(&self, x: f32, y: f32) -> (usize, usize) {
        self.current.as_ref().map_or((self.top_line, 0), |f| {
            f.layout
                .screen_to_line_col(&f.config, x, (y - 2.0).max(0.0))
        })
    }
    pub fn wheel(
        &mut self,
        dx: f64,
        dy: f64,
        shift: bool,
        snapshot: &BufferSnapshot,
        font: &FontEngine,
    ) {
        let Some(frame) = self.current.as_ref() else {
            return;
        };
        let mut dy = dy;
        if !self.wrap {
            let (h, v) = crate::horizontal_scroll::wheel_axes(dx, dy, shift);
            dy = v;
            self.horizontal
                .wheel(-h, font.char_width_px, frame.config.columns());
        }
        self.vertical_remainder -= dy / f64::from(font.line_height_px);
        let rows = self.vertical_remainder.trunc() as i32;
        self.vertical_remainder -= f64::from(rows);
        if rows != 0 {
            let mut config = frame.config;
            config.scroll_rows(snapshot, rows);
            self.top_line = config.top_line;
            self.top_wrap_row = config.top_wrap_row;
        }
        self.snap = false;
    }
    pub fn scroll_press(&mut self, x: f32, y: f32) -> bool {
        if let (Some(bar), Some(frame)) = (self.bar, self.current.as_ref()) {
            if bar.contains(x, y) {
                self.horizontal.press(bar, x, frame.config.columns());
                self.snap = false;
                return true;
            }
        }
        false
    }
    pub fn scroll_drag(&mut self, x: f32) -> bool {
        self.bar.is_some_and(|bar| self.horizontal.drag(bar, x))
    }
    pub fn prepare(
        &mut self,
        snap: &BufferSnapshot,
        font: &FontEngine,
        width: usize,
        height: usize,
    ) -> Frame {
        if self.blink.elapsed() >= Duration::from_millis(500) {
            self.caret = !self.caret;
            self.blink = Instant::now();
        }
        let text_height = height.saturating_sub(if self.wrap { 0 } else { BAR_HEIGHT });
        let (cur_line, cur_col) = snap.offset_to_line_col(snap.cursor_offset);
        let visible_rows = (text_height as f32 / font.line_height_px).floor() as usize;
        let snap_horizontal = self.snap;
        if self.snap {
            if cur_line < self.top_line {
                self.top_line = cur_line;
            } else if cur_line >= self.top_line + visible_rows.saturating_sub(1) {
                self.top_line = cur_line.saturating_sub(visible_rows.saturating_sub(2));
            }
        }
        let (gutter_w, text_start_x, line_digits) =
            compute_gutter_metrics(snap.total_lines, self.line_numbers, font.char_width_px);
        let mut config = ViewportConfig {
            width_px: width as f32,
            height_px: text_height as f32,
            line_height_px: font.line_height_px,
            char_width_px: font.char_width_px,
            top_line: self.top_line,
            top_wrap_row: self.top_wrap_row,
            left_col: if self.wrap {
                0
            } else {
                self.horizontal.first_col
            },
            word_wrap: self.wrap,
            show_line_numbers: self.line_numbers,
            line_number_gutter_width_px: gutter_w,
        };
        config.top_line = config.top_line.min(snap.total_lines - 1);
        if !self.wrap {
            config.top_wrap_row = 0;
        }
        if self.wrap {
            if self.snap {
                let caret_row =
                    (cur_col / config.columns()).min(config.rows_for_line(snap, cur_line) - 1);
                if cur_line == config.top_line && caret_row < config.top_wrap_row {
                    config.top_wrap_row = caret_row;
                }
                let probe = LayoutEngine::compute_layout(snap, &config, None);
                if probe.caret_rect.y < 0.0 {
                    config.top_line = cur_line;
                    config.top_wrap_row = caret_row.saturating_sub(visible_rows.saturating_sub(2));
                }
            }
            self.horizontal.reset();
            self.bar = None;
        } else {
            self.horizontal
                .refresh(snap, config.top_line, visible_rows + 1, config.columns());
            if snap_horizontal {
                self.horizontal.ensure_caret(cur_col, config.columns());
            }
            config.left_col = self.horizontal.first_col;
            self.bar = self.horizontal.scrollbar(
                config.text_x(),
                text_height as f32,
                (width as f32 - config.text_x()).max(0.0),
                config.columns(),
            );
        }
        self.top_line = config.top_line;
        self.top_wrap_row = config.top_wrap_row;
        self.snap = false;
        let now = Instant::now();
        let layout = LayoutEngine::compute_layout(snap, &config, None);
        Frame {
            layout,
            config,
            text_height,
            gutter_w,
            text_start_x,
            line_digits,
            cur_line,
            cur_col,
            layout_time: now.elapsed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_uses_all_space_above_the_one_command_panel() {
        let snapshot =
            texted_buffer::TextBuffer::from_string("one\ntwo\nthree".into()).create_snapshot();
        let font = FontEngine::new(16.0);
        for (width, height, compact) in [(256, 192, true), (980, 700, false)] {
            let g = crate::chrome::Geometry::new(width, height, compact, true);
            let mut viewport = Viewport::new(|| {});
            let frame = viewport.prepare(&snapshot, &font, width, g.document_height());
            assert_eq!(frame.text_height, height - g.top - g.bottom);
            viewport.wrap = false;
            assert_eq!(
                viewport
                    .prepare(&snapshot, &font, width, g.document_height())
                    .text_height,
                height - g.top - g.bottom - BAR_HEIGHT
            );
        }
    }
}
