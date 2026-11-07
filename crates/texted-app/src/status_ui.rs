//! Minimal, actionable document footer and one on-demand details card.
use crate::{
    appearance_ui::Rect,
    editor_view::ViewModel,
    graphics::{draw_box, draw_rect, DrawList, FontEngine},
};
use std::time::{Duration, Instant};
use texted_app::selection::SelectionMode;
use texted_io::LineEnding;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Statistics,
    Smaller,
    Reset,
    Larger,
    Appearance,
    Wrap,
    Close,
    Consumed,
}
pub(crate) struct Layout {
    pub stats: Rect,
    pub smaller: Rect,
    pub reset: Rect,
    pub larger: Rect,
    pub appearance: Rect,
    pub card: Rect,
    pub wrap: Rect,
    pub close: Rect,
}
impl Layout {
    pub fn new(width: usize, height: usize, compact: bool) -> Self {
        let (h, pad, gap, button, zoom, appearance) = if compact {
            (14, 2, 2, 12, 34, 24)
        } else {
            (28, 12, 6, 24, 56, 96)
        };
        let y = height.saturating_sub(h);
        let appearance = Rect {
            x: width.saturating_sub(appearance + pad),
            y,
            w: appearance.min(width),
            h,
        };
        let larger = Rect {
            x: appearance.x.saturating_sub(button + gap),
            y,
            w: button.min(width),
            h,
        };
        let reset = Rect {
            x: larger.x.saturating_sub(zoom + gap),
            y,
            w: zoom.min(width),
            h,
        };
        let smaller = Rect {
            x: reset.x.saturating_sub(button + gap),
            y,
            w: button.min(width),
            h,
        };
        let stats = Rect {
            x: pad.min(width),
            y,
            w: smaller.x.saturating_sub(pad * 2),
            h,
        };
        let card_h = if compact { 100 } else { 184 };
        let card_w = if compact { 250 } else { 380 };
        let card = Rect {
            x: pad.min(width),
            y: y.saturating_sub(card_h + 8),
            w: card_w.min(width.saturating_sub(pad * 2)),
            h: card_h.min(y.saturating_sub(8)),
        };
        let wrap = Rect {
            x: card.x + 8,
            y: card.y + card.h.saturating_sub(if compact { 18 } else { 34 }),
            w: card.w.saturating_sub(16),
            h: if compact { 14 } else { 24 },
        };
        let close = Rect {
            x: card.x + card.w.saturating_sub(24),
            y: card.y + 4,
            w: 20,
            h: 20,
        };
        Self {
            stats,
            smaller,
            reset,
            larger,
            appearance,
            card,
            wrap,
            close,
        }
    }
    fn action(&self, x: f32, y: f32) -> Option<Action> {
        [
            (self.stats, Action::Statistics),
            (self.smaller, Action::Smaller),
            (self.reset, Action::Reset),
            (self.larger, Action::Larger),
            (self.appearance, Action::Appearance),
        ]
        .into_iter()
        .find_map(|(r, a)| r.contains(x, y).then_some(a))
    }
}
fn selection_label(
    snapshot: &texted_buffer::BufferSnapshot,
    selection: &texted_app::selection::SelectionState,
) -> Option<String> {
    match selection.mode {
        SelectionMode::Stream => {
            let (a, b) = selection.offsets(snapshot);
            Some(format!(
                "{} selected",
                snapshot.content.byte_to_char(b) - snapshot.content.byte_to_char(a)
            ))
        }
        SelectionMode::Block => {
            let (a, b, c, d) = selection.block();
            Some(format!("{} rows x {} cols", b - a + 1, d - c))
        }
        SelectionMode::None => None,
    }
}
fn help_lines(text: &str, columns: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let last = lines.last_mut().unwrap();
        if !last.is_empty() && last.len() + 1 + word.len() > columns {
            lines.push(word.into());
        } else {
            if !last.is_empty() {
                last.push(' ');
            }
            last.push_str(word);
        }
    }
    lines
}
pub(crate) struct Footer {
    pub details: bool,
    hover: Option<Action>,
    since: Instant,
    tooltip: bool,
}
impl Footer {
    pub fn new() -> Self {
        Self {
            details: false,
            hover: None,
            since: Instant::now(),
            tooltip: false,
        }
    }
    pub fn clear_hover(&mut self) {
        self.hover = None;
        self.tooltip = false;
    }
    pub fn dismiss(&mut self) {
        self.details = false;
        self.clear_hover();
    }
    pub fn hover(&mut self, width: usize, height: usize, compact: bool, x: f32, y: f32) -> bool {
        let l = Layout::new(width, height, compact);
        let next = if self.details && l.close.contains(x, y) {
            Some(Action::Close)
        } else if self.details && l.wrap.contains(x, y) {
            Some(Action::Wrap)
        } else {
            l.action(x, y)
        };
        if next != self.hover {
            self.hover = next;
            self.since = Instant::now();
            self.tooltip = false;
            true
        } else {
            false
        }
    }
    pub fn tick(&mut self) -> bool {
        if self.hover.is_some()
            && !self.tooltip
            && self.since.elapsed() >= Duration::from_millis(400)
        {
            self.tooltip = true;
            true
        } else {
            false
        }
    }
    pub fn pending_tip(&self) -> bool {
        self.hover.is_some() && !self.tooltip
    }
    pub fn cursor(
        &self,
        width: usize,
        height: usize,
        compact: bool,
        x: f32,
        y: f32,
    ) -> Option<crate::cursor_ui::Shape> {
        let l = Layout::new(width, height, compact);
        if l.action(x, y).is_some()
            || (self.details && (l.close.contains(x, y) || l.wrap.contains(x, y)))
        {
            Some(crate::cursor_ui::Shape::Pointer)
        } else if self.details && l.card.contains(x, y) {
            Some(crate::cursor_ui::Shape::Arrow)
        } else {
            None
        }
    }
    pub fn click(
        &mut self,
        width: usize,
        height: usize,
        compact: bool,
        x: f32,
        y: f32,
    ) -> Option<Action> {
        let l = Layout::new(width, height, compact);
        if let Some(a) = l.action(x, y) {
            return Some(a);
        }
        if self.details {
            if l.close.contains(x, y) {
                return Some(Action::Close);
            }
            if l.wrap.contains(x, y) {
                return Some(Action::Wrap);
            }
            if l.card.contains(x, y) {
                return Some(Action::Consumed);
            }
            self.dismiss();
            return Some(Action::Consumed);
        }
        None
    }
    pub fn paint(&self, draws: &mut DrawList, font: &mut FontEngine, m: &ViewModel<'_>) {
        let l = Layout::new(m.width, m.height, m.compact);
        let c = m.palette;
        let y = l.stats.y;
        draw_rect(draws, m.width, m.height, 0, y, m.width, m.height - y, c.bg);
        let stats = selection_label(m.snap, m.selection).unwrap_or_else(|| {
            m.words
                .map_or_else(|| "... words".into(), |n| format!("{n} words"))
        });
        for (r, text, action) in [
            (l.stats, stats, Action::Statistics),
            (l.smaller, "-".into(), Action::Smaller),
            (l.reset, format!("{}%", m.zoom_percent), Action::Reset),
            (l.larger, "+".into(), Action::Larger),
            (
                l.appearance,
                if m.compact { "Aa" } else { "Appearance" }.into(),
                Action::Appearance,
            ),
        ] {
            let active =
                self.hover == Some(action) || (action == Action::Statistics && self.details);
            if active {
                draw_rect(draws, m.width, m.height, r.x, r.y, r.w, r.h, c.panel_bg);
            }
            let text_width = text.chars().count() * font.char_width_px as usize;
            let x = if action == Action::Statistics {
                r.x + 2
            } else {
                r.x + r.w.saturating_sub(text_width) / 2
            };
            font.draw_string(
                draws,
                (r.x + r.w).min(m.width),
                m.height,
                x,
                r.y + if m.compact { 3 } else { 6 },
                &text,
                if active { c.text } else { c.gutter_text },
            );
        }
        if self.details {
            let r = l.card;
            draw_box(
                draws,
                m.width,
                m.height,
                r.x,
                r.y,
                r.w,
                r.h,
                c.panel_bg,
                c.panel_border,
            );
            let word = m
                .words
                .map_or_else(|| "Counting words...".into(), |n| format!("{n} words"));
            let lines = [
                "Document details".into(),
                format!("{word}  ·  {} characters", m.snap.content.len_chars()),
                format!("Line {}  ·  Column {}", m.cur_line + 1, m.cur_col + 1),
                format!(
                    "Text: {} bytes  ·  {}",
                    m.snap.total_bytes,
                    if m.bom { "UTF-8 with BOM" } else { "UTF-8" }
                ),
                format!(
                    "Line endings: {}",
                    if m.ending == LineEnding::Crlf {
                        "Windows (CRLF)"
                    } else {
                        "Unix (LF)"
                    }
                ),
            ];
            let step = if m.compact { 13 } else { 26 };
            for (i, line) in lines.iter().enumerate() {
                font.draw_string(
                    draws,
                    r.x + r.w.saturating_sub(12),
                    m.height,
                    r.x + 10,
                    r.y + 8 + i * step,
                    line,
                    if i == 0 { c.text } else { c.status_text },
                );
            }
            font.draw_string(
                draws,
                m.width,
                m.height,
                l.close.x + 4,
                l.close.y + 2,
                "x",
                c.gutter_text,
            );
            font.draw_string(
                draws,
                l.wrap.x + l.wrap.w,
                m.height,
                l.wrap.x + 2,
                l.wrap.y + 3,
                if m.word_wrap {
                    "Wrap lines: On  (click to change)"
                } else {
                    "Wrap lines: Off (click to change)"
                },
                c.status_accent,
            );
        }
        if self.tooltip {
            if let Some(action) = self.hover {
                let text = match action {
                    Action::Statistics => "Document statistics and details (Cmd+Shift+I)",
                    Action::Smaller => "Smaller text (Cmd+-)",
                    Action::Reset => "Reset text to native size (Cmd+0)",
                    Action::Larger => "Larger text (Cmd++)",
                    Action::Appearance => "Fonts, themes and screen settings (Cmd+,)",
                    Action::Wrap => "Toggle line wrapping",
                    Action::Close => "Close document details (Esc)",
                    _ => "",
                };
                let at = match action {
                    Action::Statistics => l.stats,
                    Action::Smaller => l.smaller,
                    Action::Reset => l.reset,
                    Action::Larger => l.larger,
                    Action::Wrap => l.wrap,
                    Action::Close => l.close,
                    _ => l.appearance,
                };
                let w = (text.chars().count() * font.char_width_px as usize + 16)
                    .min(m.width.saturating_sub(8));
                let lines = help_lines(
                    text,
                    w.saturating_sub(16) / (font.char_width_px as usize).max(1),
                );
                let step = if m.compact { 10 } else { 18 };
                let h = lines.len() * step + 10;
                let x = at.x.min(m.width.saturating_sub(w + 4));
                let y = if self.details {
                    l.card.y.saturating_sub(h + 4)
                } else {
                    at.y.saturating_sub(h + 6)
                };
                draw_box(
                    draws,
                    m.width,
                    m.height,
                    x,
                    y,
                    w,
                    h,
                    c.panel_bg,
                    c.panel_border,
                );
                for (i, line) in lines.iter().enumerate() {
                    font.draw_string(
                        draws,
                        x + w.saturating_sub(6),
                        m.height,
                        x + 8,
                        y + 5 + i * step,
                        line,
                        c.text,
                    );
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn help_wraps_for_small_framebuffers_without_losing_words() {
        let text = "Document statistics and details (Cmd+Shift+I)";
        let lines = help_lines(text, 28);
        assert_eq!(lines.join(" "), text);
        assert!(lines.iter().all(|l| l.len() <= 28));
    }
    #[test]
    fn selection_count_is_unicode_characters_not_utf8_bytes() {
        let text = ropey::Rope::from_str("АїB");
        let snapshot = texted_buffer::BufferSnapshot {
            total_bytes: text.len_bytes(),
            total_lines: text.len_lines(),
            content: text,
            cursor_offset: 0,
            revision: 0,
        };
        let selection = texted_app::selection::SelectionState {
            mode: SelectionMode::Stream,
            anchor_line: 0,
            anchor_col: 0,
            active_line: 0,
            active_col: 2,
        };
        assert_eq!(
            selection_label(&snapshot, &selection).as_deref(),
            Some("2 selected")
        );
    }
    #[test]
    fn all_footer_controls_have_matching_nonoverlapping_hit_regions() {
        for compact in [false, true] {
            let l = Layout::new(980, 700, compact);
            for (r, a) in [
                (l.stats, Action::Statistics),
                (l.smaller, Action::Smaller),
                (l.reset, Action::Reset),
                (l.larger, Action::Larger),
                (l.appearance, Action::Appearance),
            ] {
                assert_eq!(l.action((r.x + 1) as f32, (r.y + 1) as f32), Some(a));
            }
        }
    }
    #[test]
    fn details_close_on_outside_click_and_do_not_edit_the_document() {
        let mut f = Footer::new();
        f.details = true;
        assert_eq!(f.click(980, 700, false, 400., 100.), Some(Action::Consumed));
        assert!(!f.details);
    }
    #[test]
    fn hover_opens_only_the_relevant_tooltip_after_a_delay() {
        let mut f = Footer::new();
        let l = Layout::new(980, 700, false);
        assert!(f.hover(
            980,
            700,
            false,
            l.reset.x as f32 + 1.,
            l.reset.y as f32 + 1.
        ));
        assert!(!f.tick());
        f.since = Instant::now() - Duration::from_millis(500);
        assert!(f.tick());
        assert!(f.tooltip);
        assert!(f.hover(980, 700, false, 0., 0.));
        assert!(!f.tooltip);
    }
}
