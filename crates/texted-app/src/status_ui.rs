//! On-demand details and delayed help for the single command panel; no reserved footer row.
use crate::{
    appearance_ui::Rect,
    chrome::Geometry,
    graphics::{draw_box, DrawList, FontEngine},
};
use std::time::{Duration, Instant};
use texted_app::{
    appearance::Palette,
    selection::{SelectionMode, SelectionState},
};
use texted_io::LineEnding;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Wrap,
    /// Cycle the encoding the next save writes.
    Encoding,
    /// Reread the file in the encoding the details card offers.
    Reread,
    Close,
    Consumed,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Tip {
    pub rect: Rect,
    pub text: String,
}
pub(crate) struct Model<'a> {
    pub geometry: Geometry,
    pub palette: &'a Palette,
    pub snap: &'a texted_buffer::BufferSnapshot,
    pub selection: &'a SelectionState,
    pub words: Option<usize>,
    pub cur_line: usize,
    pub cur_col: usize,
    pub word_wrap: bool,
    pub format: String,
    /// The encoding a reread would use; `None` until the document has a file.
    pub reread: Option<&'static str>,
    pub ending: LineEnding,
}
pub(crate) struct Layout {
    pub card: Rect,
    pub reread: Rect,
    pub encoding: Rect,
    pub wrap: Rect,
    pub close: Rect,
}
impl Layout {
    pub fn new(g: Geometry) -> Self {
        let pad = if g.compact { 2 } else { 12 };
        let y = g.height.saturating_sub(g.bottom);
        let card_h = if g.compact { 126 } else { 236 };
        let card_w = if g.compact { 250 } else { 380 };
        let card = Rect {
            x: g.width.saturating_sub(card_w + pad).max(pad).min(g.width),
            y: y.saturating_sub(card_h + 8),
            w: card_w.min(g.width.saturating_sub(pad * 2)),
            h: card_h.min(y.saturating_sub(8)),
        };
        let wrap = Rect {
            x: card.x + 8,
            y: card.y + card.h.saturating_sub(if g.compact { 18 } else { 34 }),
            w: card.w.saturating_sub(16),
            h: if g.compact { 14 } else { 24 },
        };
        let close = Rect {
            x: card.x + card.w.saturating_sub(24),
            y: card.y + 4,
            w: 20,
            h: 20,
        };
        let row = if g.compact { 14 } else { 26 };
        let encoding = Rect {
            y: wrap.y.saturating_sub(row),
            ..wrap
        };
        let reread = Rect {
            y: encoding.y.saturating_sub(row),
            ..wrap
        };
        Self {
            card,
            reread,
            encoding,
            wrap,
            close,
        }
    }
}
pub(crate) fn selection_label(
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
pub(crate) struct PanelHelp {
    pub details: bool,
    hover: Option<Tip>,
    since: Instant,
    tooltip: bool,
}
impl PanelHelp {
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
    pub fn hover(&mut self, g: Geometry, x: f32, y: f32, command: Option<Tip>) -> bool {
        let l = Layout::new(g);
        let next = if self.details && l.close.contains(x, y) {
            Some(Tip {
                rect: l.close,
                text: "Close document details (Esc)".into(),
            })
        } else if self.details && l.wrap.contains(x, y) {
            Some(Tip {
                rect: l.wrap,
                text: "Toggle line wrapping".into(),
            })
        } else if self.details && l.encoding.contains(x, y) {
            Some(Tip {
                rect: l.encoding,
                text: "Change the encoding the next save writes".into(),
            })
        } else if self.details && l.reread.contains(x, y) {
            Some(Tip {
                rect: l.reread,
                text: "Read the file again in another encoding".into(),
            })
        } else {
            command
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
    pub fn cursor(&self, g: Geometry, x: f32, y: f32) -> Option<crate::cursor_ui::Shape> {
        let l = Layout::new(g);
        if self.details
            && [l.close, l.wrap, l.encoding, l.reread]
                .iter()
                .any(|r| r.contains(x, y))
        {
            Some(crate::cursor_ui::Shape::Pointer)
        } else if self.details && l.card.contains(x, y) {
            Some(crate::cursor_ui::Shape::Arrow)
        } else {
            None
        }
    }
    pub fn click(&mut self, g: Geometry, x: f32, y: f32) -> Option<Action> {
        if !self.details {
            return None;
        }
        let l = Layout::new(g);
        if l.close.contains(x, y) {
            return Some(Action::Close);
        }
        if l.wrap.contains(x, y) {
            return Some(Action::Wrap);
        }
        if l.encoding.contains(x, y) {
            return Some(Action::Encoding);
        }
        if l.reread.contains(x, y) {
            return Some(Action::Reread);
        }
        if l.card.contains(x, y) {
            return Some(Action::Consumed);
        }
        self.dismiss();
        Some(Action::Consumed)
    }
    pub fn paint(&self, draws: &mut DrawList, font: &mut FontEngine, m: &Model<'_>) {
        let g = m.geometry;
        let l = Layout::new(g);
        let c = m.palette;
        if self.details {
            let r = l.card;
            draw_box(
                draws,
                g.width,
                g.height,
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
                format!(
                    "{word} · {} chars{}",
                    m.snap.content.len_chars(),
                    selection_label(m.snap, m.selection)
                        .map_or(String::new(), |s| format!(" · {s}"))
                ),
                format!("Line {}  ·  Column {}", m.cur_line + 1, m.cur_col + 1),
                format!("Text: {} bytes", m.snap.total_bytes),
                format!(
                    "Line endings: {}",
                    if m.ending == LineEnding::Crlf {
                        "Windows (CRLF)"
                    } else {
                        "Unix (LF)"
                    }
                ),
            ];
            let step = if g.compact { 13 } else { 26 };
            for (i, line) in lines.iter().enumerate() {
                font.draw_string(
                    draws,
                    r.x + r.w.saturating_sub(12),
                    g.height,
                    r.x + 10,
                    r.y + 8 + i * step,
                    line,
                    if i == 0 { c.text } else { c.status_text },
                );
            }
            font.draw_string(
                draws,
                g.width,
                g.height,
                l.close.x + 4,
                l.close.y + 2,
                "x",
                c.gutter_text,
            );
            let reread = m.reread.map_or_else(
                || "Reread: save the file first".to_owned(),
                |name| format!("Reread as {name} (click)"),
            );
            for (rect, text, color) in [
                (
                    l.encoding,
                    format!("Encoding: {} (change)", m.format),
                    c.status_accent,
                ),
                (
                    l.reread,
                    reread,
                    if m.reread.is_some() {
                        c.status_accent
                    } else {
                        c.gutter_text
                    },
                ),
            ] {
                font.draw_string(
                    draws,
                    rect.x + rect.w,
                    g.height,
                    rect.x + 2,
                    rect.y + 3,
                    &text,
                    color,
                );
            }
            font.draw_string(
                draws,
                l.wrap.x + l.wrap.w,
                g.height,
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
            if let Some(tip) = &self.hover {
                let w = (tip.text.chars().count() * font.char_width_px as usize + 16)
                    .min(g.width.saturating_sub(8));
                let lines = help_lines(
                    &tip.text,
                    w.saturating_sub(16) / (font.char_width_px as usize).max(1),
                );
                let step = if g.compact { 10 } else { 18 };
                let h = lines.len() * step + 10;
                let x = tip.rect.x.min(g.width.saturating_sub(w + 4));
                let y = if self.details {
                    l.card.y.saturating_sub(h + 4)
                } else {
                    tip.rect.y.saturating_sub(h + 6)
                };
                draw_box(
                    draws,
                    g.width,
                    g.height,
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
                        g.height,
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
    fn details_fit_above_the_command_panel_at_small_resolutions() {
        for (w, h, compact) in [(256, 192, true), (320, 200, true), (980, 700, false)] {
            for shown in [true, false] {
                let g = Geometry::new(w, h, compact, shown);
                let l = Layout::new(g);
                assert!(l.card.x + l.card.w <= w);
                assert!(l.card.y + l.card.h <= h - g.bottom);
                assert!(l.wrap.y + l.wrap.h <= l.card.y + l.card.h);
            }
        }
    }
    #[test]
    fn encoding_rows_sit_between_the_text_and_the_wrap_switch() {
        for (w, h, compact) in [(256, 192, true), (320, 200, true), (980, 700, false)] {
            let g = Geometry::new(w, h, compact, true);
            let l = Layout::new(g);
            let step = if compact { 13 } else { 26 };
            assert!(l.reread.y >= l.card.y + 8 + 5 * step, "{w}x{h}");
            assert!(l.reread.y + l.reread.h <= l.encoding.y, "{w}x{h}");
            assert!(l.encoding.y + l.encoding.h <= l.wrap.y, "{w}x{h}");
        }
    }
    #[test]
    fn encoding_rows_report_their_own_actions() {
        let mut f = PanelHelp::new();
        f.details = true;
        let g = Geometry::new(980, 700, false, true);
        let l = Layout::new(g);
        let center = |r: Rect| ((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32);
        for (rect, action) in [
            (l.encoding, Action::Encoding),
            (l.reread, Action::Reread),
            (l.wrap, Action::Wrap),
        ] {
            let (x, y) = center(rect);
            assert_eq!(f.click(g, x, y), Some(action));
        }
    }
    #[test]
    fn details_close_on_outside_click_and_do_not_edit_the_document() {
        let mut f = PanelHelp::new();
        f.details = true;
        assert_eq!(
            f.click(Geometry::new(980, 700, false, true), 400., 100.),
            Some(Action::Consumed)
        );
        assert!(!f.details);
    }
    #[test]
    fn command_hover_opens_delayed_help_without_a_footer_hit_region() {
        let mut f = PanelHelp::new();
        let g = Geometry::new(980, 700, false, true);
        let tip = Tip {
            rect: Rect {
                x: 600,
                y: 672,
                w: 80,
                h: 28,
            },
            text: "Reset text scale to 100%".into(),
        };
        assert!(f.hover(g, 601., 680., Some(tip.clone())));
        assert!(!f.tick());
        f.since = Instant::now() - Duration::from_millis(500);
        assert!(f.tick());
        assert!(f.tooltip);
        assert!(!f.hover(g, 601., 680., Some(tip)));
        assert!(f.hover(g, 0., 0., None));
        assert!(!f.tooltip);
    }
}
