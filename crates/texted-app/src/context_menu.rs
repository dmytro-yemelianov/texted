//! A right-click waits for earlier accepted edits, then preserves a selection or positions the caret.
use crate::transfers::submit;
use texted_app::{
    buffer_service::{BufferClient, Command, Published},
    commands::{EditorCommand, PointerKind},
    selection::SelectionMode,
};
#[derive(Clone, Copy)]
pub(crate) struct ContextRequest {
    pub position: (f32, f32),
    pub point: (usize, usize),
    pub document: u64,
    pub after: u64,
    pub positioned: bool,
}
pub(crate) enum ContextStep {
    Wait,
    Cancel,
    Show,
}
impl ContextRequest {
    pub fn advance(&mut self, client: &BufferClient) -> Result<ContextStep, String> {
        let p = client.latest();
        if p.document != self.document || client.barrier() != self.after {
            return Ok(ContextStep::Cancel);
        }
        if p.completed_through < self.after || p.busy.is_some() {
            return Ok(ContextStep::Wait);
        }
        if !self.positioned && !contains_selection(&p, self.point) {
            let (line, col) = self.point;
            self.after = submit(
                client,
                Command::Edit(EditorCommand::Pointer {
                    line,
                    col,
                    kind: PointerKind::Character,
                    extend: false,
                    block: false,
                }),
            )?;
            self.positioned = true;
            return Ok(ContextStep::Wait);
        }
        Ok(ContextStep::Show)
    }
}
pub(crate) fn has_selection(p: &Published) -> bool {
    match p.selection.mode {
        SelectionMode::None => false,
        SelectionMode::Stream => {
            let (a, b) = p.selection.offsets(&p.snapshot);
            a != b
        }
        SelectionMode::Block => {
            let (_, _, a, b) = p.selection.block();
            a != b
        }
    }
}
pub(crate) fn contains_selection(p: &Published, (line, col): (usize, usize)) -> bool {
    match p.selection.mode {
        SelectionMode::None => false,
        SelectionMode::Stream => {
            let (start, end) = p.selection.offsets(&p.snapshot);
            let offset = p.snapshot.line_col_to_offset(line, col);
            start <= offset && offset < end
        }
        SelectionMode::Block => {
            let (top, bottom, left, right) = p.selection.block();
            (top..=bottom).contains(&line) && (left..right).contains(&col)
        }
    }
}
/// The same context menu is painted and hit-tested in both presentation backends.
pub(crate) struct Popup {
    at: (f32, f32),
    selected: usize,
    first: usize,
}
const ITEMS: &[(crate::menus::MenuCommand, &str)] = &[
    (crate::menus::MenuCommand::Undo, "Undo"),
    (crate::menus::MenuCommand::Redo, "Redo"),
    (crate::menus::MenuCommand::Cut, "Cut"),
    (crate::menus::MenuCommand::Copy, "Copy"),
    (crate::menus::MenuCommand::Paste, "Paste"),
    (crate::menus::MenuCommand::Delete, "Delete selection"),
    (crate::menus::MenuCommand::SelectAll, "Select all"),
    (crate::menus::MenuCommand::Block, "Column selection"),
    (crate::menus::MenuCommand::Find, "Find..."),
    (crate::menus::MenuCommand::Replace, "Replace..."),
    (crate::menus::MenuCommand::Style, "Appearance..."),
    (crate::menus::MenuCommand::DocumentInfo, "Document details"),
];
impl Popup {
    pub fn new(at: (f32, f32), state: crate::menus::MenuState) -> Self {
        Self {
            at,
            selected: ITEMS
                .iter()
                .position(|(c, _)| state.enabled(*c))
                .unwrap_or(0),
            first: 0,
        }
    }
    fn layout(
        &self,
        size: (usize, usize),
        compact: bool,
    ) -> (crate::appearance_ui::Rect, usize, usize) {
        let ch = if compact { 8 } else { 16 };
        let visible = (size.1 / ch).saturating_sub(2).clamp(1, ITEMS.len());
        let w = 184.min(size.0);
        let h = ((visible + 2) * ch).min(size.1);
        (
            crate::appearance_ui::Rect {
                x: (self.at.0.max(0.) as usize).min(size.0.saturating_sub(w)) / 8 * 8,
                y: (self.at.1.max(0.) as usize).min(size.1.saturating_sub(h)) / ch * ch,
                w,
                h,
            },
            ch,
            visible,
        )
    }
    pub fn navigate(&mut self, forward: bool, state: crate::menus::MenuState) {
        for _ in 0..ITEMS.len() {
            self.selected = if forward {
                (self.selected + 1) % ITEMS.len()
            } else {
                (self.selected + ITEMS.len() - 1) % ITEMS.len()
            };
            if state.enabled(ITEMS[self.selected].0) {
                break;
            }
        }
    }
    pub fn selected(&self, state: crate::menus::MenuState) -> Option<crate::menus::MenuCommand> {
        let c = ITEMS[self.selected].0;
        state.enabled(c).then_some(c)
    }
    pub fn hover(
        &mut self,
        size: (usize, usize),
        compact: bool,
        x: f32,
        y: f32,
        state: crate::menus::MenuState,
    ) {
        let (r, ch, visible) = self.layout(size, compact);
        if r.contains(x, y) && y >= (r.y + ch) as f32 {
            let i = self.first + (y as usize - r.y - ch) / ch;
            if i < ITEMS.len() && i < self.first + visible && state.enabled(ITEMS[i].0) {
                self.selected = i;
            }
        }
    }
    pub fn click(
        &mut self,
        size: (usize, usize),
        compact: bool,
        x: f32,
        y: f32,
        state: crate::menus::MenuState,
    ) -> Option<crate::menus::MenuCommand> {
        self.hover(size, compact, x, y, state);
        let (r, ch, visible) = self.layout(size, compact);
        if r.contains(x, y) && y >= (r.y + ch) as f32 && y < (r.y + (visible + 1) * ch) as f32 {
            let i = self.first + (y as usize - r.y - ch) / ch;
            ITEMS
                .get(i)
                .and_then(|(c, _)| state.enabled(*c).then_some(*c))
        } else {
            None
        }
    }
    pub fn paint(
        &mut self,
        draws: &mut crate::graphics::DrawList,
        font: &mut crate::graphics::FontEngine,
        size: (usize, usize),
        compact: bool,
        state: crate::menus::MenuState,
        p: crate::appearance::Palette,
    ) {
        draws.cells.push(crate::cell_grid::CellCommand::HideCursor);
        let (r, ch, visible) = self.layout(size, compact);
        if self.selected < self.first {
            self.first = self.selected;
        }
        if self.selected >= self.first + visible {
            self.first = self.selected + 1 - visible;
        }
        crate::graphics::draw_box(
            draws,
            size.0,
            size.1,
            r.x,
            r.y,
            r.w,
            r.h,
            p.panel_bg,
            p.panel_border,
        );
        for (row, (command, label)) in ITEMS.iter().skip(self.first).take(visible).enumerate() {
            let y = r.y + (row + 1) * ch;
            if self.first + row == self.selected {
                crate::graphics::draw_rect(
                    draws,
                    size.0,
                    size.1,
                    r.x + 8,
                    y,
                    r.w.saturating_sub(16),
                    ch,
                    p.selection,
                );
            }
            font.draw_string(
                draws,
                (r.x + r.w).saturating_sub(8),
                size.1,
                r.x + 8,
                y,
                label,
                if state.enabled(*command) {
                    p.text
                } else {
                    p.gutter_text
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use texted_app::{buffer_service::BufferService, selection::SelectionState};
    #[test]
    fn popup_waits_for_selection_and_caret_commands_and_rejects_newer_input() {
        use std::{sync::mpsc, time::Duration};
        let (wake, ready) = mpsc::channel();
        let service = BufferService::new(move || {
            let _ = wake.send(());
        });
        let client = &service.client;
        let loaded = submit(client, Command::Load(ropey::Rope::from_str("one\ntwo\n"))).unwrap();
        while client.latest().completed_through < loaded {
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let selected = submit(client, Command::Edit(EditorCommand::SelectAll)).unwrap();
        let mut request = ContextRequest {
            position: (100.0, 100.0),
            point: (1, 1),
            document: client.latest().document,
            after: selected,
            positioned: false,
        };
        loop {
            match request.advance(client).unwrap() {
                ContextStep::Wait => {
                    ready.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                ContextStep::Show => break,
                ContextStep::Cancel => panic!("selection should be preserved"),
            }
        }
        assert_eq!(client.latest().selection.mode, SelectionMode::Stream);
        assert_eq!(client.barrier(), selected); // No caret command inside the selection.
        request.point = (2, 0); // The selection's exclusive end is outside it.
        assert!(matches!(
            request.advance(client).unwrap(),
            ContextStep::Wait
        ));
        while client.latest().completed_through < request.after {
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert!(matches!(
            request.advance(client).unwrap(),
            ContextStep::Show
        ));
        assert_eq!(client.latest().selection.mode, SelectionMode::None);
        assert_eq!(client.latest().snapshot.cursor_offset, 8);
        let newer = submit(client, Command::Edit(EditorCommand::Insert("x".into()))).unwrap();
        assert!(matches!(
            request.advance(client).unwrap(),
            ContextStep::Cancel
        ));
        assert_eq!(client.barrier(), newer); // A stale request cannot enqueue another caret move.
    }
    #[test]
    fn right_click_preserves_only_the_selected_stream_or_rectangle() {
        let service = BufferService::new(|| {});
        let mut p = (*service.client.latest()).clone();
        p.snapshot =
            texted_buffer::TextBuffer::from_string("абв\nghi\njkl".into()).create_snapshot();
        p.selection = SelectionState {
            mode: SelectionMode::Stream,
            anchor_line: 1,
            anchor_col: 2,
            active_line: 0,
            active_col: 1,
        };
        assert!(has_selection(&p));
        assert!(contains_selection(&p, (0, 1)));
        assert!(contains_selection(&p, (1, 1)));
        assert!(!contains_selection(&p, (0, 0)));
        assert!(!contains_selection(&p, (1, 2)));
        p.selection.mode = SelectionMode::Block;
        assert!(contains_selection(&p, (0, 1)));
        assert!(!contains_selection(&p, (1, 2)));
        assert!(!contains_selection(&p, (2, 1)));
        p.selection.active_col = p.selection.anchor_col;
        assert!(!has_selection(&p));
        assert!(!contains_selection(&p, (0, 2)));
    }
}
