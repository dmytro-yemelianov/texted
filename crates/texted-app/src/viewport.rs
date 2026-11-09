//! Viewport geometry, scrolling and caret pacing; no document mutation or IPC.
use crate::{
    editor_view::compute_gutter_metrics,
    graphics::FontEngine,
    horizontal_scroll::{HorizontalScroll, Scrollbar, BAR_HEIGHT},
};
use std::time::{Duration, Instant};
use texted_app::lace::layer::Row;
use texted_buffer::BufferSnapshot;
use texted_layout::{LayoutEngine, ViewportConfig, ViewportLayout, VisibleLine, WrapRows};
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
    /// Room before the text in pixels, from the Visuals panel inset.
    pub text_inset: f32,
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
            text_inset: 0.0,
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
            text_inset_px: self.text_inset,
        };
        config.top_line = config.top_line.min(snap.total_lines - 1);
        if !self.wrap {
            config.top_wrap_row = 0;
        }
        if self.wrap {
            if self.snap {
                let caret_row = config.wrap_rows(snap, cur_line).row_of(cur_col);
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

/// The rows just above and just below `drawn`, laid out as `compute_layout` lays out
/// visible rows. Lace selects over them too, so connections across the window's top
/// and bottom edges stay put while rows scroll in and out.
pub(crate) fn edge_rows(
    snap: &BufferSnapshot,
    config: &ViewportConfig,
    drawn: &[VisibleLine],
) -> [Option<VisibleLine>; 2] {
    // Wrap row `wrap` of `line`, whose row breaks are `rows`, as `compute_layout` builds it.
    let row = |line: usize, wrap: usize, rows: &WrapRows, y_px: f32| {
        let text = snap.line(line);
        let len = text.len_chars();
        let (start, end) = if config.word_wrap {
            (rows.start(wrap), rows.end(wrap))
        } else {
            let end = config.left_col.saturating_add(config.columns()).min(len);
            (config.left_col, end)
        };
        VisibleLine {
            line_index: line,
            is_continuation: wrap > 0,
            continues: wrap + 1 < rows.count(),
            start_col: start,
            end_col: end,
            text: text.slice(start.min(len)..end).to_string(),
            y_px,
        }
    };
    // Which wrap row of its line a drawn row is; without wrapping every line is one row.
    let wrap_of = |l: &VisibleLine, rows: &WrapRows| {
        if config.word_wrap {
            rows.row_of(l.start_col)
        } else {
            0
        }
    };
    let step = config.line_height_px;
    let above = drawn.first().and_then(|first| {
        let rows = config.wrap_rows(snap, first.line_index);
        let y = first.y_px - step;
        match wrap_of(first, &rows) {
            0 if first.line_index == 0 => None,
            0 => {
                let line = first.line_index - 1;
                let rows = config.wrap_rows(snap, line);
                Some(row(line, rows.count() - 1, &rows, y))
            }
            wrap => Some(row(first.line_index, wrap - 1, &rows, y)),
        }
    });
    let below = drawn.last().and_then(|last| {
        let y = last.y_px + step;
        if last.continues {
            let rows = config.wrap_rows(snap, last.line_index);
            Some(row(last.line_index, wrap_of(last, &rows) + 1, &rows, y))
        } else if last.line_index + 1 < snap.total_lines {
            let line = last.line_index + 1;
            Some(row(line, 0, &config.wrap_rows(snap, line), y))
        } else {
            None
        }
    });
    [above, below]
}

/// The rows lace selects over, top to bottom: the hidden row above, the drawn rows and
/// the hidden row below, where they exist.
pub(crate) fn lace_rows<'a>(
    edges: &'a [Option<VisibleLine>; 2],
    drawn: &'a [VisibleLine],
) -> Vec<Row<'a>> {
    edges[0]
        .iter()
        .chain(drawn)
        .chain(&edges[1])
        .map(|l| Row {
            line: l.line_index,
            start_col: l.start_col,
            text: &l.text,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lace_at_the_window_edges_does_not_depend_on_the_rows_beyond_them() {
        use crate::lace::{
            layer::{select, Placed, Settings},
            table::{Relation, Tables},
            vectorize::FaceMetrics,
        };
        use texted_app::{appearance::FontFamily, bitmap_fonts::face};
        const WORDS: [&str; 12] = [
            "листопад",
            "щастя",
            "ступінь",
            "пташка",
            "плетиво",
            "мереживо",
            "office",
            "fifth",
            "staff",
            "lattice",
            "thrifty",
            "first",
        ];
        let text = (0..14)
            .map(|i| {
                (0..9)
                    .map(|k| WORDS[(i * 5 + k * 7) % WORDS.len()])
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let snapshot = texted_buffer::TextBuffer::from_string(text).create_snapshot();
        let font = FontEngine::new(16.0);
        let vga = face(FontFamily::Vga);
        let glyphs = |c| vga.glyph(c).map(<[u8]>::to_vec);
        let metrics = FaceMetrics::measure(glyphs, vga.width, vga.height).unwrap();
        let mut tables = Tables::new(metrics, false, [8, 16]);
        let settings = Settings {
            density: 1.0,
            spacing: 1,
            seed: 3,
        };
        // A frame `rows` rows tall whose top is (line, wrap row).
        let frame = |wrap: bool, top: (usize, usize), rows: usize| {
            let mut viewport = Viewport::new(|| {});
            (viewport.wrap, viewport.top_line, viewport.top_wrap_row) = (wrap, top.0, top.1);
            let bar = if wrap { 0 } else { BAR_HEIGHT };
            viewport.prepare(&snapshot, &font, 400, rows * 16 + bar)
        };
        type Joined = ((usize, usize), (usize, usize), usize, Relation, usize);
        let id = |l: &VisibleLine| (l.line_index, l.start_col);
        // Every connection as (upper row, lower row, column, relation, entry), filling
        // the tables until nothing is missing.
        let joined = |f: &Frame, tables: &mut Tables| -> Vec<Joined> {
            let edges = edge_rows(&snapshot, &f.config, &f.layout.visible_lines);
            let rows = lace_rows(&edges, &f.layout.visible_lines);
            let row_id = |r: &Row| (r.line, r.start_col);
            loop {
                let selection = select(&rows, tables, &settings);
                if selection.missing.is_empty() {
                    let below = |p: &Placed| match p.rel {
                        Relation::H | Relation::S(_) => p.row,
                        _ => p.row + 1,
                    };
                    let mut out: Vec<Joined> = selection
                        .placed
                        .iter()
                        .map(|p| {
                            (
                                row_id(&rows[p.row]),
                                row_id(&rows[below(p)]),
                                p.col,
                                p.rel,
                                std::sync::Arc::as_ptr(&p.entry) as usize,
                            )
                        })
                        .collect();
                    out.sort();
                    return out;
                }
                for need in selection.missing {
                    tables.supply(need, &glyphs);
                }
            }
        };
        let (mut across_top, mut across_bottom) = (0, 0);
        for (wrap, top) in [(true, (1, 0)), (true, (1, 1)), (false, (1, 0))] {
            let n = 6;
            // `wide` shows one row more above and below than `narrow`.
            let wide = frame(wrap, top, n + 2);
            let second = &wide.layout.visible_lines[1];
            let wrap_row = if wrap {
                wide.config
                    .wrap_rows(&snapshot, second.line_index)
                    .row_of(second.start_col)
            } else {
                0
            };
            let narrow = frame(wrap, (second.line_index, wrap_row), n);
            let rows = &narrow.layout.visible_lines;
            assert_eq!(rows.len(), n);
            assert_eq!(id(&rows[0]), id(second));
            // The hidden rows are the rows `wide` shows there, one line height away.
            let edges = edge_rows(&snapshot, &narrow.config, rows);
            let shown = &wide.layout.visible_lines;
            for (edge, at, y) in [(&edges[0], 0, -16.0), (&edges[1], n + 1, n as f32 * 16.0)] {
                let mut edge = edge.clone().expect("a row beyond the edge");
                assert_eq!(edge.y_px, y);
                edge.y_px = shown[at].y_px;
                assert_eq!(format!("{edge:?}"), format!("{:?}", shown[at]));
            }
            let (first, last) = (id(&rows[0]), id(&rows[n - 1]));
            let touching = |j: &Joined| [j.0, j.1].iter().any(|r| *r == first || *r == last);
            let a: Vec<Joined> = joined(&narrow, &mut tables)
                .into_iter()
                .filter(touching)
                .collect();
            let b: Vec<Joined> = joined(&wide, &mut tables)
                .into_iter()
                .filter(touching)
                .collect();
            assert_eq!(a, b, "wrap {wrap}, top {top:?}");
            across_top += a.iter().filter(|j| j.1 == first && j.0 != first).count();
            across_bottom += a.iter().filter(|j| j.0 == last && j.1 != last).count();
        }
        assert!(
            across_top > 0 && across_bottom > 0,
            "{across_top} {across_bottom}"
        );
    }
    #[test]
    fn caret_at_the_end_of_a_long_word_wrapped_line_scrolls_into_view() {
        // Rows end at spaces, so this line takes more rows than characters / columns.
        let text = format!("{} ", "x".repeat(51)).repeat(400);
        let mut buffer = texted_buffer::TextBuffer::from_string(text);
        let snapshot = buffer.create_snapshot();
        buffer.set_cursor_offset(snapshot.total_bytes);
        let snapshot = buffer.create_snapshot();
        let font = FontEngine::new(16.0);
        let mut viewport = Viewport::new(|| {});
        viewport.input(true);
        let frame = viewport.prepare(&snapshot, &font, 980, 600);
        assert!(frame.layout.caret_rect.y >= 0.0, "caret off screen");
    }
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
