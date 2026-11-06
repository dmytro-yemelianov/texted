//! Bounded dropdown geometry and navigation, shared by drawing and input.
use crate::appearance_ui::Rect;
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dropdown {
    pub field: usize,
    pub highlighted: usize,
    pub first: usize,
    remainder: f64,
}
impl Dropdown {
    pub fn new(field: usize, selected: usize) -> Self {
        Self {
            field,
            highlighted: selected,
            first: 0,
            remainder: 0.0,
        }
    }
    pub fn reveal(&mut self, visible: usize, total: usize) {
        if self.highlighted < self.first {
            self.first = self.highlighted;
        }
        if self.highlighted >= self.first + visible {
            self.first = self.highlighted + 1 - visible;
        }
        self.first = self.first.min(total.saturating_sub(visible));
    }
    pub fn navigate(&mut self, delta: i32, enabled: &[bool]) {
        if enabled.is_empty() {
            return;
        }
        let direction = delta.signum();
        for _ in 0..delta.unsigned_abs().min(enabled.len() as u32) {
            for _ in 0..enabled.len() {
                self.highlighted =
                    (self.highlighted as i32 + direction).rem_euclid(enabled.len() as i32) as usize;
                if enabled[self.highlighted] {
                    break;
                }
            }
        }
    }
    pub fn wheel(&mut self, rows: f64, visible: usize, total: usize) {
        self.remainder += rows;
        let steps = self.remainder.trunc();
        self.remainder -= steps;
        self.first =
            (self.first as f64 + steps).clamp(0.0, total.saturating_sub(visible) as f64) as usize;
    }
}
pub(crate) struct Popup {
    pub rect: Rect,
    pub row_height: usize,
    pub visible: usize,
    pub scrollbar: usize,
}
impl Popup {
    pub fn new(
        anchor: Rect,
        size: (usize, usize),
        compact: bool,
        total: usize,
        label_width: usize,
    ) -> Self {
        let (width, height) = size;
        let margin = if compact { 4 } else { 8 };
        let row_height = if compact { 14 } else { 26 };
        let visible = (height.saturating_sub(2 * margin + 2) / row_height)
            .clamp(1, 12)
            .min(total.max(1));
        let h = (visible * row_height + 2).min(height);
        let scrollbar = if visible < total {
            if compact {
                8
            } else {
                12
            }
        } else {
            0
        };
        let w = anchor
            .w
            .max(label_width + 24 + scrollbar)
            .min(width.saturating_sub(2 * margin).max(1));
        let x = anchor.x.min(width.saturating_sub(margin + w));
        let below = anchor.y + anchor.h;
        let y = if below + h + margin <= height {
            below
        } else if anchor.y >= h + margin {
            anchor.y - h
        } else {
            height.saturating_sub(margin + h)
        };
        Self {
            rect: Rect { x, y, w, h },
            row_height,
            visible,
            scrollbar,
        }
    }
    pub fn row(&self, index: usize, first: usize) -> Option<Rect> {
        (index >= first && index < first + self.visible).then(|| Rect {
            x: self.rect.x + 1,
            y: self.rect.y + 1 + (index - first) * self.row_height,
            w: self.rect.w.saturating_sub(2 + self.scrollbar),
            h: self.row_height,
        })
    }
    pub fn item_at(&self, x: f32, y: f32, first: usize, total: usize) -> Option<usize> {
        if !self.rect.contains(x, y)
            || x >= (self.rect.x + self.rect.w.saturating_sub(self.scrollbar + 1)) as f32
            || y < (self.rect.y + 1) as f32
        {
            return None;
        }
        let row = ((y - self.rect.y as f32 - 1.0) / self.row_height as f32) as usize;
        (row < self.visible && first + row < total).then_some(first + row)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_platform_lists_and_scrollable_size_lists_fit_and_share_hit_geometry() {
        for (width, height, compact) in [(256, 192, true), (320, 200, true), (980, 700, false)] {
            for total in [4, 5, 9, 10, 33] {
                let popup = Popup::new(
                    Rect {
                        x: width - 168,
                        y: height - 60,
                        w: 160,
                        h: 18,
                    },
                    (width, height),
                    compact,
                    total,
                    160,
                );
                assert!(
                    popup.rect.x + popup.rect.w <= width && popup.rect.y + popup.rect.h <= height
                );
                if total <= 10 && compact {
                    assert_eq!(popup.visible, total);
                }
                let first = total.saturating_sub(popup.visible);
                for index in first..total {
                    let row = popup.row(index, first).unwrap();
                    assert_eq!(
                        popup.item_at((row.x + 2) as f32, (row.y + 2) as f32, first, total),
                        Some(index)
                    );
                }
            }
        }
    }
    #[test]
    fn navigation_skips_disabled_choices_and_fractional_wheel_scrolls_stay_bounded() {
        let mut d = Dropdown::new(2, 0);
        d.navigate(1, &[true, false, false, true]);
        assert_eq!(d.highlighted, 3);
        d.navigate(1, &[true, false, false, true]);
        assert_eq!(d.highlighted, 0);
        d.highlighted = 32;
        d.reveal(10, 33);
        assert_eq!(d.first, 23);
        d.wheel(-0.4, 10, 33);
        d.wheel(-0.7, 10, 33);
        assert_eq!(d.first, 22);
        d.wheel(-1000.0, 10, 33);
        assert_eq!(d.first, 0);
        d.wheel(2000.0, 10, 33);
        assert_eq!(d.first, 23);
    }
}
