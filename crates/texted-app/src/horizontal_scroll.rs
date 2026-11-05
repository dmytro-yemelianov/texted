//! Character-aligned horizontal viewport, scrollbar geometry and background width measurement.
use std::{sync::mpsc, thread};
use texted_buffer::BufferSnapshot;
use texted_layout::Rect;
pub const BAR_HEIGHT: usize = 14;
/// Shift redirects a vertical wheel into the horizontal axis. Native horizontal events stay single-axis.
pub fn wheel_axes(dx: f64, dy: f64, shift: bool) -> (f64, f64) {
    if shift {
        (if dx != 0.0 { dx } else { dy }, 0.0)
    } else {
        (dx, dy)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Scrollbar {
    pub track: Rect,
    pub thumb: Rect,
    max_offset: usize,
}
impl Scrollbar {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.track.x
            && x < self.track.x + self.track.width
            && y >= self.track.y
            && y < self.track.y + self.track.height
    }
    fn offset_at(&self, x: f32, grab: f32) -> usize {
        let travel = self.track.width - self.thumb.width;
        if travel <= 0.0 {
            return 0;
        }
        (((x - self.track.x - grab) / travel).clamp(0.0, 1.0) * self.max_offset as f32).round()
            as usize
    }
}
pub struct HorizontalScroll {
    pub first_col: usize,
    max_cols: usize,
    revision: Option<u64>,
    jobs: mpsc::Sender<BufferSnapshot>,
    results: mpsc::Receiver<(u64, usize)>,
    remainder: f64,
    drag_grab: Option<f32>,
}
impl HorizontalScroll {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let (jobs, input) = mpsc::channel::<BufferSnapshot>();
        let (output, results) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(mut snapshot) = input.recv() {
                'restart: loop {
                    let mut width = 0;
                    for (i, line) in snapshot.content.lines().enumerate() {
                        let len = line.len_chars();
                        width = width.max(len - usize::from(len > 0 && line.char(len - 1) == '\n'));
                        if i % 256 == 0 {
                            match input.try_recv() {
                                Ok(next) => {
                                    snapshot = next;
                                    continue 'restart;
                                }
                                Err(mpsc::TryRecvError::Disconnected) => return,
                                Err(mpsc::TryRecvError::Empty) => {}
                            }
                        }
                    }
                    if output.send((snapshot.revision, width)).is_err() {
                        return;
                    }
                    wake();
                    break;
                }
            }
        });
        Self {
            first_col: 0,
            max_cols: 0,
            revision: None,
            jobs,
            results,
            remainder: 0.0,
            drag_grab: None,
        }
    }
    pub fn refresh(&mut self, snapshot: &BufferSnapshot, top: usize, rows: usize, columns: usize) {
        if self.revision != Some(snapshot.revision) {
            self.revision = Some(snapshot.revision);
            let _ = self.jobs.send(snapshot.clone());
        }
        // Visible line summaries immediately allow scrolling while the global width is computed.
        for line in top..top.saturating_add(rows).min(snapshot.total_lines) {
            self.max_cols = self.max_cols.max(snapshot.line(line).len_chars());
        }
        while let Ok((revision, width)) = self.results.try_recv() {
            self.accept_measurement(revision, width);
        }
        self.clamp(columns);
    }
    fn accept_measurement(&mut self, revision: u64, width: usize) {
        if self.revision == Some(revision) {
            self.max_cols = width;
        }
    }
    fn max_offset(&self, columns: usize) -> usize {
        self.max_cols
            .saturating_add(1)
            .saturating_sub(columns.max(1))
    }
    fn clamp(&mut self, columns: usize) {
        self.first_col = self.first_col.min(self.max_offset(columns));
    }
    pub fn ensure_caret(&mut self, col: usize, columns: usize) {
        let columns = columns.max(1);
        self.max_cols = self.max_cols.max(col);
        if col < self.first_col {
            self.first_col = col;
        } else if col >= self.first_col.saturating_add(columns) {
            self.first_col = col.saturating_sub(columns - 1);
        }
        self.clamp(columns);
        self.remainder = 0.0;
    }
    pub fn wheel(&mut self, pixels: f64, char_width: f32, columns: usize) {
        self.remainder += pixels / f64::from(char_width.max(1.0));
        let steps = self.remainder.trunc();
        self.remainder -= steps;
        let next = (self.first_col as f64 + steps).clamp(0.0, self.max_offset(columns) as f64);
        self.first_col = next as usize;
        if (next == 0.0 && self.remainder < 0.0)
            || (next == self.max_offset(columns) as f64 && self.remainder > 0.0)
        {
            self.remainder = 0.0;
        }
    }
    pub fn reset(&mut self) {
        self.first_col = 0;
        self.remainder = 0.0;
        self.drag_grab = None;
    }
    pub fn scrollbar(&self, x: f32, y: f32, width: f32, columns: usize) -> Option<Scrollbar> {
        let max_offset = self.max_offset(columns);
        if max_offset == 0 || width <= 0.0 {
            return None;
        }
        let thumb_width = (width * columns as f32 / self.max_cols.saturating_add(1) as f32)
            .clamp(24.0_f32.min(width), width);
        let thumb_x = x + (width - thumb_width) * self.first_col as f32 / max_offset as f32;
        Some(Scrollbar {
            track: Rect {
                x,
                y,
                width,
                height: BAR_HEIGHT as f32,
            },
            thumb: Rect {
                x: thumb_x,
                y: y + 2.0,
                width: thumb_width,
                height: BAR_HEIGHT as f32 - 4.0,
            },
            max_offset,
        })
    }
    pub fn press(&mut self, bar: Scrollbar, x: f32, columns: usize) {
        self.remainder = 0.0;
        if x >= bar.thumb.x && x <= bar.thumb.x + bar.thumb.width {
            self.drag_grab = Some(x - bar.thumb.x);
        } else if x < bar.thumb.x {
            self.first_col = self.first_col.saturating_sub(columns);
        } else {
            self.first_col = self.first_col.saturating_add(columns).min(bar.max_offset);
        }
    }
    pub fn drag(&mut self, bar: Scrollbar, x: f32) -> bool {
        if let Some(grab) = self.drag_grab {
            self.first_col = bar.offset_at(x, grab);
            true
        } else {
            false
        }
    }
    pub fn end_drag(&mut self) -> bool {
        self.drag_grab.take().is_some()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use texted_buffer::TextBuffer;
    fn scroll() -> HorizontalScroll {
        let mut s = HorizontalScroll::new(|| {});
        s.max_cols = 1000;
        s
    }
    #[test]
    fn caret_scroll_and_resize_clamp() {
        let mut s = scroll();
        s.ensure_caret(999, 80);
        assert_eq!(s.first_col, 920);
        s.ensure_caret(5, 80);
        assert_eq!(s.first_col, 5);
        s.clamp(2000);
        assert_eq!(s.first_col, 0);
    }
    #[test]
    fn tiny_trackpad_deltas_accumulate_and_bounds_do_not_store_overscroll() {
        let mut s = scroll();
        for _ in 0..4 {
            s.wheel(2.5, 10.0, 80);
        }
        assert_eq!(s.first_col, 1);
        s.wheel(-10000.0, 10.0, 80);
        assert_eq!(s.first_col, 0);
        s.wheel(10.0, 10.0, 80);
        assert_eq!(s.first_col, 1);
    }
    #[test]
    fn scrollbar_drag_hits_both_ends_and_track_click_pages() {
        let mut s = scroll();
        let bar = s.scrollbar(50.0, 600.0, 800.0, 80).unwrap();
        s.press(bar, bar.thumb.x + 4.0, 80);
        assert!(s.drag(bar, 900.0));
        assert_eq!(s.first_col, 921);
        s.drag(bar, 0.0);
        assert_eq!(s.first_col, 0);
        assert!(s.end_drag());
        s.press(bar, bar.thumb.x + bar.thumb.width + 10.0, 80);
        assert_eq!(s.first_col, 80);
    }
    #[test]
    fn background_measurement_finds_offscreen_longest_line_and_shrinks_after_edit() {
        let (wake, ready) = mpsc::channel();
        let mut s = HorizontalScroll::new(move || {
            let _ = wake.send(());
        });
        let mut b = TextBuffer::from_string(format!("short\n{}", "ї".repeat(2000)));
        let old_revision = b.create_snapshot().revision;
        s.refresh(&b.create_snapshot(), 0, 1, 80);
        ready
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        s.refresh(&b.create_snapshot(), 0, 1, 80);
        assert_eq!(s.max_cols, 2000);
        s.first_col = 500;
        b.delete(0, b.total_bytes());
        s.refresh(&b.create_snapshot(), 0, 1, 80);
        ready
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        s.refresh(&b.create_snapshot(), 0, 1, 80);
        assert_eq!(s.max_cols, 0);
        assert_eq!(s.first_col, 0);
        s.accept_measurement(old_revision, 9000);
        assert_eq!(s.max_cols, 0);
    }
    #[test]
    fn shift_wheel_redirects_once_and_regular_trackpad_keeps_both_axes() {
        assert_eq!(wheel_axes(0.0, -40.0, true), (-40.0, 0.0));
        assert_eq!(wheel_axes(-40.0, -40.0, true), (-40.0, 0.0));
        assert_eq!(wheel_axes(-20.0, 30.0, false), (-20.0, 30.0));
    }
}
