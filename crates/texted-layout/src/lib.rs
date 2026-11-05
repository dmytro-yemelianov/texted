//! Viewport Layout & Typography Engine
//!
//! Computes visible viewport lines, caret bounding boxes,
//! selection rectangles, and screen-to-buffer coordinate mappings.

use texted_buffer::BufferSnapshot;

#[derive(Clone, Copy, Debug)]
pub struct ViewportConfig {
    pub width_px: f32,
    pub height_px: f32,
    pub line_height_px: f32,
    pub char_width_px: f32,
    pub top_line: usize,
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
            word_wrap: false,
            show_line_numbers: true,
            line_number_gutter_width_px: 50.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VisibleLine {
    pub line_index: usize,
    pub is_continuation: bool,
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

pub struct LayoutEngine;

impl LayoutEngine {
    /// Computes layout geometry strictly for visible viewport rows
    pub fn compute_layout(
        snapshot: &BufferSnapshot,
        config: &ViewportConfig,
        selection: Option<(usize, usize)>, // (start_offset, end_offset)
    ) -> ViewportLayout {
        let lines: Vec<&str> = snapshot.content.split('\n').collect();
        let total_lines = lines.len();

        let gutter_offset = if config.show_line_numbers {
            config.line_number_gutter_width_px
        } else {
            10.0
        };

        let mut visible_lines = Vec::new();
        let mut cur_y = 0.0f32;

        if config.word_wrap {
            let max_chars = ((config.width_px - gutter_offset - 20.0).max(100.0) / config.char_width_px).floor().max(10.0) as usize;
            let mut skipped_visual_rows = 0usize;

            for (idx, line_str) in lines.iter().enumerate() {
                let char_vec: Vec<char> = line_str.chars().collect();
                if char_vec.is_empty() {
                    if skipped_visual_rows >= config.top_line {
                        if cur_y + config.line_height_px <= config.height_px + config.line_height_px {
                            visible_lines.push(VisibleLine {
                                line_index: idx,
                                is_continuation: false,
                                text: String::new(),
                                y_px: cur_y,
                            });
                            cur_y += config.line_height_px;
                        }
                    } else {
                        skipped_visual_rows += 1;
                    }
                } else {
                    let mut chunk_start = 0;
                    let mut is_first = true;

                    while chunk_start < char_vec.len() {
                        let chunk_end = (chunk_start + max_chars).min(char_vec.len());
                        let chunk_text: String = char_vec[chunk_start..chunk_end].iter().collect();

                        if skipped_visual_rows >= config.top_line {
                            if cur_y + config.line_height_px <= config.height_px + config.line_height_px {
                                visible_lines.push(VisibleLine {
                                    line_index: idx,
                                    is_continuation: !is_first,
                                    text: chunk_text,
                                    y_px: cur_y,
                                });
                                cur_y += config.line_height_px;
                            }
                        } else {
                            skipped_visual_rows += 1;
                        }

                        chunk_start = chunk_end;
                        is_first = false;
                    }
                }

                if cur_y > config.height_px {
                    break;
                }
            }
        } else {
            let visible_row_capacity = (config.height_px / config.line_height_px).ceil() as usize;
            let start_line = config.top_line.min(total_lines.saturating_sub(1));
            let end_line = (start_line + visible_row_capacity + 2).min(total_lines);

            for (idx, line_str) in lines.iter().enumerate().take(end_line).skip(start_line) {
                let row_y = (idx - start_line) as f32 * config.line_height_px;
                visible_lines.push(VisibleLine {
                    line_index: idx,
                    is_continuation: false,
                    text: line_str.to_string(),
                    y_px: row_y,
                });
            }
        }

        // Calculate cursor position (line, col)
        let (cursor_line, cursor_col) = Self::offset_to_line_col(&snapshot.content, snapshot.cursor_offset);

        // Find visual position of cursor
        let mut caret_x = -100.0f32;
        let mut caret_y = -100.0f32;

        for v_line in &visible_lines {
            if v_line.line_index == cursor_line {
                // If not word wrap or first line with col
                caret_y = v_line.y_px;
                caret_x = gutter_offset + (cursor_col as f32 * config.char_width_px);
                break;
            }
        }

        let caret_rect = Rect {
            x: caret_x,
            y: caret_y,
            width: 2.0,
            height: config.line_height_px,
        };

        let mut selection_rects = Vec::new();
        if let Some((sel_start, sel_end)) = selection {
            let (s_line, s_col) = Self::offset_to_line_col(&snapshot.content, sel_start.min(sel_end));
            let (e_line, e_col) = Self::offset_to_line_col(&snapshot.content, sel_start.max(sel_end));

            for line_idx in s_line..=e_line {
                for v_line in &visible_lines {
                    if v_line.line_index == line_idx && !v_line.is_continuation {
                        let y = v_line.y_px;
                        let line_len = lines.get(line_idx).map(|s| s.chars().count()).unwrap_or(0);

                        let col_from = if line_idx == s_line { s_col } else { 0 };
                        let col_to = if line_idx == e_line { e_col } else { line_len };

                        let x = gutter_offset + (col_from as f32 * config.char_width_px);
                        let w = ((col_to - col_from) as f32 * config.char_width_px).max(config.char_width_px / 2.0);

                        selection_rects.push(Rect {
                            x,
                            y,
                            width: w,
                            height: config.line_height_px,
                        });
                    }
                }
            }
        }

        ViewportLayout {
            visible_lines,
            caret_rect,
            cursor_line,
            cursor_col,
            selection_rects,
        }
    }

    pub fn offset_to_line_col(text: &str, target_offset: usize) -> (usize, usize) {
        let mut cur_line = 0;
        let mut line_start_offset = 0;

        for (offset, ch) in text.char_indices() {
            if offset >= target_offset {
                let col = text[line_start_offset..offset].chars().count();
                return (cur_line, col);
            }
            if ch == '\n' {
                cur_line += 1;
                line_start_offset = offset + 1;
            }
        }

        let clamped = target_offset.min(text.len());
        let col = text[line_start_offset..clamped].chars().count();
        (cur_line, col)
    }

    /// Maps a screen click coordinate (x, y) to a text buffer byte offset
    pub fn screen_to_offset(
        snapshot: &BufferSnapshot,
        config: &ViewportConfig,
        click_x: f32,
        click_y: f32,
    ) -> usize {
        let gutter_offset = if config.show_line_numbers {
            config.line_number_gutter_width_px
        } else {
            10.0
        };

        let row_idx = (click_y / config.line_height_px).floor() as usize + config.top_line;
        let col_idx = ((click_x - gutter_offset).max(0.0) / config.char_width_px).round() as usize;

        let lines: Vec<&str> = snapshot.content.split('\n').collect();
        if row_idx >= lines.len() {
            return snapshot.total_bytes;
        }

        let mut offset = 0;
        for line in lines.iter().take(row_idx) {
            offset += line.len() + 1; // +1 for '\n'
        }

        let target_line = lines[row_idx];
        let char_count = target_line.chars().count();
        let target_col = col_idx.min(char_count);

        let mut col_bytes = 0;
        for (i, ch) in target_line.chars().enumerate() {
            if i >= target_col {
                break;
            }
            col_bytes += ch.len_utf8();
        }

        offset + col_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viewport_layout_calculation() {
        let snapshot = BufferSnapshot {
            content: "First Line\nSecond Line\nThird Line".to_string(),
            total_bytes: 33,
            total_lines: 3,
            cursor_offset: 14, // 'e' in "Second Line"
        };

        let config = ViewportConfig::default();
        let layout = LayoutEngine::compute_layout(&snapshot, &config, None);

        assert_eq!(layout.visible_lines.len(), 3);
        assert_eq!(layout.cursor_line, 1);
        assert_eq!(layout.cursor_col, 3);
        assert!(layout.caret_rect.x > 0.0);
    }
}
