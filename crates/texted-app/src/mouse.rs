//! Pointer gesture state. The buffer model owns all selection semantics.
use std::time::{Duration, Instant};
use texted_app::commands::PointerKind;
pub(crate) struct Mouse {
    pub position: (f32, f32),
    pub dragging: bool,
    last: Option<(Instant, (f32, f32))>,
    clicks: usize,
}
impl Mouse {
    pub fn new() -> Self {
        Self {
            position: (0.0, 0.0),
            dragging: false,
            last: None,
            clicks: 0,
        }
    }
    pub fn press(&mut self, gutter: bool) -> PointerKind {
        let now = Instant::now();
        self.clicks = if self.last.is_some_and(|(t, p)| {
            now.duration_since(t) < Duration::from_millis(500)
                && (p.0 - self.position.0).abs() < 5.0
                && (p.1 - self.position.1).abs() < 5.0
        }) {
            self.clicks % 3 + 1
        } else {
            1
        };
        self.last = Some((now, self.position));
        self.dragging = true;
        if gutter || self.clicks == 3 {
            PointerKind::Line
        } else if self.clicks == 2 {
            PointerKind::Word
        } else {
            PointerKind::Character
        }
    }
}
