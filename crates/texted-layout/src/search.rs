//! Compact viewport coverage. Persistent storage scales with visible cells, not match count.
use crate::{HighlightPlan, MatchHighlight, ViewportConfig, ViewportLayout};
use texted_buffer::{search::SearchMatch, BufferSnapshot};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRow {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchViewport {
    pub rows: Vec<SearchRow>,
}
impl SearchViewport {
    pub fn new(snapshot: &BufferSnapshot, layout: &ViewportLayout) -> Self {
        Self {
            rows: layout
                .visible_lines
                .iter()
                .map(|r| SearchRow {
                    line: r.line_index,
                    start_col: r.start_col,
                    end_col: r.end_col,
                    start: snapshot.line_col_to_offset(r.line_index, r.start_col),
                    end: snapshot.line_col_to_offset(r.line_index, r.end_col),
                })
                .collect(),
        }
    }
    pub fn start(&self) -> usize {
        self.rows.first().map_or(0, |r| r.start)
    }
    pub fn end(&self) -> usize {
        self.rows.last().map_or(0, |r| r.end)
    }
    pub fn allocated_bytes(&self) -> usize {
        self.rows.capacity() * std::mem::size_of::<SearchRow>()
    }
}
#[derive(Debug, Clone)]
pub struct Coverage {
    pub viewport: SearchViewport,
    bits: Vec<Vec<u64>>,
}
impl Coverage {
    pub fn new(viewport: SearchViewport) -> Self {
        let bits = viewport
            .rows
            .iter()
            .map(|r| vec![0; r.end_col.saturating_sub(r.start_col).div_ceil(64)])
            .collect();
        Self { viewport, bits }
    }
    pub fn allocated_bytes(&self) -> usize {
        self.viewport.allocated_bytes()
            + self.bits.capacity() * std::mem::size_of::<Vec<u64>>()
            + self.bits.iter().map(|v| v.capacity() * 8).sum::<usize>()
    }
    pub fn mark(&mut self, snapshot: &BufferSnapshot, found: SearchMatch) {
        if found.end <= self.viewport.start() || found.start >= self.viewport.end() {
            return;
        }
        let first = self.viewport.rows.partition_point(|r| r.end <= found.start);
        let (sl, sc) = snapshot.offset_to_line_col(found.start);
        let (el, ec) = snapshot.offset_to_line_col(found.end);
        for (r, bits) in self
            .viewport
            .rows
            .iter()
            .zip(&mut self.bits)
            .skip(first)
            .take_while(|(r, _)| r.start < found.end)
        {
            let from = if r.line == sl {
                sc.max(r.start_col)
            } else {
                r.start_col
            };
            let end = if r.line == el {
                ec.min(r.end_col)
            } else {
                r.end_col
            };
            let mut col = from.saturating_sub(r.start_col);
            let end = end.saturating_sub(r.start_col);
            while col < end {
                let count = (64 - col % 64).min(end - col);
                let mask = if count == 64 {
                    u64::MAX
                } else {
                    ((1u64 << count) - 1) << (col % 64)
                };
                bits[col / 64] |= mask;
                col += count;
            }
        }
    }
    pub fn highlights(&self, layout: &ViewportLayout, config: &ViewportConfig) -> HighlightPlan {
        let mut plan = HighlightPlan::default();
        if self.viewport.rows.len() != layout.visible_lines.len() {
            return plan;
        }
        for ((r, bits), row) in self
            .viewport
            .rows
            .iter()
            .zip(&self.bits)
            .zip(&layout.visible_lines)
        {
            if (r.line, r.start_col, r.end_col) != (row.line_index, row.start_col, row.end_col) {
                return HighlightPlan::default();
            }
            let len = r.end_col.saturating_sub(r.start_col);
            let mut col = 0;
            while col < len {
                if bits[col / 64] & (1 << (col % 64)) == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < len && bits[col / 64] & (1 << (col % 64)) != 0 {
                    col += 1;
                }
                if let Some(rect) =
                    layout.highlight(config, row, r.start_col + start, r.start_col + col)
                {
                    plan.highlights.push(MatchHighlight {
                        rect,
                        active: false,
                    });
                }
            }
            plan.row_visits += 1;
        }
        plan
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::LayoutEngine;
    use texted_buffer::TextBuffer;
    #[test]
    fn dense_matches_collapse_into_bounded_coverage_and_keep_gaps() {
        let snap = TextBuffer::from_string("a".repeat(2_000_000)).create_snapshot();
        let config = ViewportConfig {
            left_col: 1_000_000,
            ..Default::default()
        };
        let layout = LayoutEngine::compute_layout(&snap, &config, None);
        let mut coverage = Coverage::new(SearchViewport::new(&snap, &layout));
        for i in 1_000_000..1_000_081 {
            coverage.mark(
                &snap,
                SearchMatch {
                    start: i,
                    end: i + 1,
                    ordinal: i,
                },
            );
        }
        assert!(coverage.allocated_bytes() < 1024);
        let plan = coverage.highlights(&layout, &config);
        assert_eq!(plan.highlights.len(), 1);
        assert_eq!(plan.highlights[0].rect.width, config.char_width_px * 81.0);
        let snap = TextBuffer::from_string("a a".into()).create_snapshot();
        let layout = LayoutEngine::compute_layout(&snap, &ViewportConfig::default(), None);
        let mut c = Coverage::new(SearchViewport::new(&snap, &layout));
        c.mark(
            &snap,
            SearchMatch {
                start: 0,
                end: 1,
                ordinal: 0,
            },
        );
        c.mark(
            &snap,
            SearchMatch {
                start: 2,
                end: 3,
                ordinal: 1,
            },
        );
        assert_eq!(
            c.highlights(&layout, &ViewportConfig::default())
                .highlights
                .len(),
            2
        );
    }
}
