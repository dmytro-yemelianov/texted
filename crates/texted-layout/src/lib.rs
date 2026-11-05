//! Viewport-bounded layout and a single coordinate mapping for drawing and input.
use texted_buffer::BufferSnapshot;

#[derive(Clone, Copy, Debug)]
pub struct ViewportConfig {
    pub width_px: f32,
    pub height_px: f32,
    pub line_height_px: f32,
    pub char_width_px: f32,
    /// Physical document line at the top of the viewport.
    pub top_line: usize,
    /// Wrapped row within top_line.
    pub top_wrap_row: usize,
    /// First visible character column in no-wrap mode. Ignored when wrapping.
    pub left_col: usize,
    pub word_wrap: bool,
    pub show_line_numbers: bool,
    pub line_number_gutter_width_px: f32,
}
impl Default for ViewportConfig {
    fn default() -> Self {
        Self {
            width_px: 800.0,
            height_px: 600.0,
            line_height_px: 20.0,
            char_width_px: 9.0,
            top_line: 0,
            top_wrap_row: 0,
            left_col: 0,
            word_wrap: false,
            show_line_numbers: true,
            line_number_gutter_width_px: 50.0,
        }
    }
}
impl ViewportConfig {
    pub fn text_x(&self) -> f32 {
        if self.show_line_numbers {
            self.line_number_gutter_width_px + 8.0
        } else {
            10.0
        }
    }
    pub fn columns(&self) -> usize {
        ((self.width_px - self.text_x() - 8.0).max(self.char_width_px) / self.char_width_px)
            .floor()
            .max(1.0) as usize
    }
    pub fn rows_for_line(&self, snapshot: &BufferSnapshot, line: usize) -> usize {
        if self.word_wrap {
            snapshot
                .line(line)
                .len_chars()
                .div_ceil(self.columns())
                .max(1)
        } else {
            1
        }
    }
    pub fn scroll_rows(&mut self, snapshot: &BufferSnapshot, delta: i32) {
        self.top_line = self.top_line.min(snapshot.total_lines - 1);
        self.top_wrap_row = self
            .top_wrap_row
            .min(self.rows_for_line(snapshot, self.top_line) - 1);
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                if self.top_wrap_row + 1 < self.rows_for_line(snapshot, self.top_line) {
                    self.top_wrap_row += 1;
                } else if self.top_line + 1 < snapshot.total_lines {
                    self.top_line += 1;
                    self.top_wrap_row = 0;
                }
            } else if self.top_wrap_row > 0 {
                self.top_wrap_row -= 1;
            } else if self.top_line > 0 {
                self.top_line -= 1;
                self.top_wrap_row = self.rows_for_line(snapshot, self.top_line) - 1;
            }
        }
    }
}
#[derive(Clone, Debug)]
pub struct VisibleLine {
    pub line_index: usize,
    pub is_continuation: bool,
    pub start_col: usize,
    pub end_col: usize,
    pub text: String,
    pub y_px: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
#[derive(Clone, Debug)]
pub struct ViewportLayout {
    pub visible_lines: Vec<VisibleLine>,
    pub caret_rect: Rect,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub selection_rects: Vec<Rect>,
}
#[derive(Debug)]
pub struct MatchHighlight {
    pub rect: Rect,
    pub active: bool,
}
#[derive(Debug, Default)]
pub struct HighlightPlan {
    pub highlights: Vec<MatchHighlight>,
    pub candidates: usize,
    pub row_visits: usize,
}
impl ViewportLayout {
    /// Clips sorted, non-overlapping matches against visible byte windows.
    /// The slice may be a bounded viewport page; a full-document index is not required.
    pub fn search_highlights(
        &self,
        snapshot: &BufferSnapshot,
        config: &ViewportConfig,
        revision: u64,
        matches: &[(usize, usize)],
        active: Option<(usize, usize)>,
    ) -> HighlightPlan {
        let mut plan = HighlightPlan::default();
        if revision != snapshot.revision {
            return plan;
        }
        let rows: Vec<_> = self
            .visible_lines
            .iter()
            .filter_map(|row| {
                let start = snapshot.line_col_to_offset(row.line_index, row.start_col);
                let end = snapshot.line_col_to_offset(row.line_index, row.end_col);
                (start < end).then_some((row, start, end))
            })
            .collect();
        let mut windows: Vec<(usize, usize)> = Vec::new();
        for &(_, start, end) in &rows {
            if let Some(window) = windows.last_mut().filter(|w| start <= w.1) {
                window.1 = window.1.max(end);
            } else {
                windows.push((start, end));
            }
        }
        let mut previous = None;
        for (start, end) in windows {
            let first = matches.partition_point(|&(_, b)| b <= start);
            for (index, &(a, b)) in matches
                .iter()
                .enumerate()
                .skip(first)
                .take_while(|(_, &(a, _))| a < end)
            {
                if previous == Some(index) {
                    continue;
                }
                previous = Some(index);
                plan.candidates += 1;
                let first_row = rows.partition_point(|&(_, _, end)| end <= a);
                let (sl, sc) = snapshot.offset_to_line_col(a);
                let (el, ec) = snapshot.offset_to_line_col(b);
                for &(row, _, _) in rows
                    .iter()
                    .skip(first_row)
                    .take_while(|&&(_, start, _)| start < b)
                {
                    plan.row_visits += 1;
                    let from = if row.line_index == sl {
                        sc
                    } else {
                        row.start_col
                    };
                    let to = if row.line_index == el {
                        ec
                    } else {
                        row.end_col
                    };
                    if let Some(rect) = self.highlight(config, row, from, to) {
                        plan.highlights.push(MatchHighlight {
                            rect,
                            active: active == Some((a, b)),
                        });
                    }
                }
            }
        }
        plan
    }
    pub fn screen_to_line_col(&self, config: &ViewportConfig, x: f32, y: f32) -> (usize, usize) {
        let row = (y.max(0.0) / config.line_height_px).floor() as usize;
        if let Some(line) = self
            .visible_lines
            .get(row)
            .or_else(|| self.visible_lines.last())
        {
            let col = ((x - config.text_x()).max(0.0) / config.char_width_px).round() as usize;
            (line.line_index, line.start_col + col)
        } else {
            (config.top_line, 0)
        }
    }
    pub fn highlight(
        &self,
        config: &ViewportConfig,
        row: &VisibleLine,
        from: usize,
        to: usize,
    ) -> Option<Rect> {
        let a = from.max(row.start_col);
        let b = to.min(row.start_col + config.columns());
        if b <= a {
            return None;
        }
        Some(Rect {
            x: config.text_x() + (a - row.start_col) as f32 * config.char_width_px,
            y: row.y_px,
            width: (b - a) as f32 * config.char_width_px,
            height: config.line_height_px,
        })
    }
}
pub struct LayoutEngine;
impl LayoutEngine {
    pub fn compute_layout(
        snapshot: &BufferSnapshot,
        config: &ViewportConfig,
        selection: Option<(usize, usize)>,
    ) -> ViewportLayout {
        let capacity = (config.height_px / config.line_height_px).floor().max(0.0) as usize;
        let columns = config.columns();
        let mut visible_lines = Vec::with_capacity(capacity);
        let mut line = config.top_line.min(snapshot.total_lines - 1);
        let mut wrap = if config.word_wrap {
            config
                .top_wrap_row
                .min(config.rows_for_line(snapshot, line) - 1)
        } else {
            0
        };
        while line < snapshot.total_lines && visible_lines.len() < capacity {
            let text = snapshot.line(line);
            let len = text.len_chars();
            let start = if config.word_wrap {
                wrap * columns
            } else {
                config.left_col
            };
            let end = start.saturating_add(columns).min(len);
            visible_lines.push(VisibleLine {
                line_index: line,
                is_continuation: wrap > 0,
                start_col: start,
                end_col: end,
                text: text.slice(start.min(len)..end).to_string(),
                y_px: visible_lines.len() as f32 * config.line_height_px,
            });
            if config.word_wrap && wrap + 1 < config.rows_for_line(snapshot, line) {
                wrap += 1;
            } else {
                line += 1;
                wrap = 0;
            }
        }
        let (cursor_line, cursor_col) = snapshot.offset_to_line_col(snapshot.cursor_offset);
        let mut result = ViewportLayout {
            visible_lines,
            caret_rect: Rect {
                x: -100.0,
                y: -100.0,
                width: 2.0,
                height: config.line_height_px,
            },
            cursor_line,
            cursor_col,
            selection_rects: Vec::new(),
        };
        for row in &result.visible_lines {
            if row.line_index == cursor_line
                && cursor_col >= row.start_col
                && cursor_col <= row.end_col
                && (!config.word_wrap
                    || cursor_col < row.start_col + columns
                    || cursor_col == snapshot.line(cursor_line).len_chars())
            {
                result.caret_rect.x =
                    config.text_x() + (cursor_col - row.start_col) as f32 * config.char_width_px;
                result.caret_rect.y = row.y_px;
                break;
            }
        }
        if let Some((a, b)) = selection {
            let (sl, sc) = snapshot.offset_to_line_col(a.min(b));
            let (el, ec) = snapshot.offset_to_line_col(a.max(b));
            for row in &result.visible_lines {
                if row.line_index >= sl && row.line_index <= el {
                    let from = if row.line_index == sl { sc } else { 0 };
                    let to = if row.line_index == el {
                        ec
                    } else {
                        snapshot.line(row.line_index).len_chars()
                    };
                    if let Some(rect) = result.highlight(config, row, from, to) {
                        result.selection_rects.push(rect);
                    }
                }
            }
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use texted_buffer::TextBuffer;
    #[test]
    fn search_work_is_bounded_at_a_million_column_offset_and_deep_wrap() {
        let snap = TextBuffer::from_string("a".repeat(2_000_000)).create_snapshot();
        let matches = snap.find_matches("a");
        for wrap in [false, true] {
            let mut config = ViewportConfig {
                left_col: 1_000_000,
                word_wrap: wrap,
                ..Default::default()
            };
            config.top_wrap_row = 1_000_000 / config.columns();
            let layout = LayoutEngine::compute_layout(&snap, &config, None);
            let plan = layout.search_highlights(
                &snap,
                &config,
                snap.revision,
                &matches,
                Some((1_000_000, 1_000_001)),
            );
            let visible = layout.visible_lines.len() * config.columns();
            assert_eq!(plan.highlights.len(), visible);
            assert!(plan.candidates <= visible + 2);
            assert_eq!(plan.row_visits, visible);
            assert_eq!(plan.highlights.iter().filter(|h| h.active).count(), 1);
        }
    }
    #[test]
    fn multiline_match_visits_visible_rows_instead_of_its_document_span() {
        let snap = TextBuffer::from_string("ї\n".repeat(100_000)).create_snapshot();
        let config = ViewportConfig {
            top_line: 50_000,
            height_px: 60.0,
            ..Default::default()
        };
        let layout = LayoutEngine::compute_layout(&snap, &config, None);
        let plan = layout.search_highlights(
            &snap,
            &config,
            snap.revision,
            &[(0, snap.total_bytes)],
            None,
        );
        assert_eq!(plan.candidates, 1);
        assert_eq!(plan.row_visits, 3);
        assert_eq!(plan.highlights.len(), 3);
        assert!(plan
            .highlights
            .iter()
            .all(|h| h.rect.width == config.char_width_px));
    }
    #[test]
    fn highlight_boundaries_empty_view_and_stale_revision() {
        let mut buffer = TextBuffer::from_string("їїї\nabc".into());
        let snap = buffer.create_snapshot();
        let config = ViewportConfig {
            left_col: 1,
            width_px: 78.0,
            show_line_numbers: false,
            char_width_px: 10.0,
            ..Default::default()
        };
        let layout = LayoutEngine::compute_layout(&snap, &config, None);
        let plan = layout.search_highlights(
            &snap,
            &config,
            snap.revision,
            &[(0, 2), (2, 4), (4, 6)],
            Some((2, 4)),
        );
        assert_eq!(plan.candidates, 2);
        assert_eq!(plan.highlights.len(), 2);
        assert!(plan.highlights[0].active);
        assert_eq!(plan.highlights[0].rect.x, config.text_x());
        buffer.insert(0, "x");
        assert!(layout
            .search_highlights(
                &buffer.create_snapshot(),
                &config,
                snap.revision,
                &[(2, 4)],
                None
            )
            .highlights
            .is_empty());
        let empty = LayoutEngine::compute_layout(
            &snap,
            &ViewportConfig {
                height_px: 0.0,
                ..config
            },
            None,
        );
        assert_eq!(
            empty
                .search_highlights(&snap, &config, snap.revision, &[(0, 6)], None)
                .candidates,
            0
        );
    }
    #[test]
    fn horizontal_offset_aligns_unicode_text_caret_mouse_and_highlights() {
        let mut b = TextBuffer::from_string("абвгґдеєжзиіїйклмн".into());
        b.set_cursor_offset(24);
        let config = ViewportConfig {
            left_col: 9,
            width_px: 108.0,
            char_width_px: 10.0,
            show_line_numbers: false,
            ..Default::default()
        };
        let snap = b.create_snapshot();
        let layout = LayoutEngine::compute_layout(&snap, &config, Some((16, 26)));
        assert_eq!(
            layout.visible_lines[0].text,
            snap.line(0).slice(9..18).to_string()
        );
        assert_eq!(layout.caret_rect.x, 40.0);
        assert_eq!(layout.screen_to_line_col(&config, 40.0, 0.0), (0, 12));
        assert_eq!(layout.selection_rects[0].x, 10.0);
        assert_eq!(layout.selection_rects[0].width, 40.0);
        let wrapped = ViewportConfig {
            word_wrap: true,
            ..config
        };
        let layout = LayoutEngine::compute_layout(&snap, &wrapped, None);
        assert_eq!(layout.visible_lines[0].start_col, 0);
    }
    #[test]
    fn horizontally_scrolled_short_lines_are_empty_and_caret_is_hidden() {
        let b = TextBuffer::from_string("hi\n".into());
        let config = ViewportConfig {
            left_col: 200,
            ..Default::default()
        };
        let layout = LayoutEngine::compute_layout(&b.create_snapshot(), &config, None);
        assert!(layout.visible_lines.iter().all(|row| row.text.is_empty()));
        assert!(layout.caret_rect.x < 0.0);
    }
    #[test]
    fn exact_width_line_does_not_add_a_blank_row_and_caret_remains_visible() {
        let mut b = TextBuffer::from_string("123456789\nx".into());
        b.set_cursor_offset(9);
        let config = ViewportConfig {
            width_px: 108.0,
            char_width_px: 10.0,
            show_line_numbers: false,
            word_wrap: true,
            ..Default::default()
        };
        let layout = LayoutEngine::compute_layout(&b.create_snapshot(), &config, None);
        assert_eq!(layout.visible_lines.len(), 2);
        assert_eq!(layout.caret_rect.y, 0.0);
        assert_eq!(layout.caret_rect.x, 100.0);
    }
    #[test]
    fn wrapped_caret_and_click_share_coordinates() {
        let mut b = TextBuffer::from_string("ї".repeat(30));
        b.set_cursor_offset(24);
        let snap = b.create_snapshot();
        let config = ViewportConfig {
            width_px: 108.0,
            char_width_px: 10.0,
            show_line_numbers: false,
            word_wrap: true,
            ..Default::default()
        };
        let layout = LayoutEngine::compute_layout(&snap, &config, Some((4, 24)));
        assert_eq!(layout.visible_lines[1].start_col, 9);
        assert_eq!(layout.caret_rect.y, 20.0);
        assert_eq!(
            layout.screen_to_line_col(&config, layout.caret_rect.x, layout.caret_rect.y),
            (0, 12)
        );
        assert_eq!(layout.selection_rects.len(), 2);
    }
    #[test]
    fn deep_viewport_and_long_line_are_bounded() {
        let b =
            TextBuffer::from_string(format!("{}{}", "x\n".repeat(100_000), "a".repeat(100_000)));
        let config = ViewportConfig {
            top_line: 100_000,
            ..Default::default()
        };
        let layout = LayoutEngine::compute_layout(&b.create_snapshot(), &config, None);
        assert_eq!(layout.visible_lines.len(), 1);
        assert!(layout.visible_lines[0].text.len() <= config.columns());
    }
    #[test]
    fn scroll_inside_one_wrapped_line() {
        let snap = TextBuffer::from_string("x".repeat(1000)).create_snapshot();
        let mut config = ViewportConfig {
            word_wrap: true,
            ..Default::default()
        };
        config.scroll_rows(&snap, 3);
        assert_eq!(config.top_line, 0);
        assert_eq!(config.top_wrap_row, 3);
        config.scroll_rows(&snap, -2);
        assert_eq!(config.top_wrap_row, 1);
    }
}
