//! Canonical editing and selection transitions. The buffer owner alone mutates the live model.
use crate::{
    commands::{DeleteUnit, EditorCommand, Motion, PointerKind},
    selection::{SelectionMode, SelectionState},
};
use texted_buffer::{word, BufferSnapshot, TextBuffer};
#[derive(Clone, Copy)]
enum DragAnchor {
    Character,
    Word {
        line: usize,
        start: usize,
        end: usize,
    },
    Line(usize),
}
pub struct EditorModel {
    pub buffer: TextBuffer,
    pub selection: SelectionState,
    drag: DragAnchor,
}
impl EditorModel {
    pub fn new(content: ropey::Rope) -> Self {
        Self {
            buffer: TextBuffer::from_rope(content),
            selection: SelectionState::default(),
            drag: DragAnchor::Character,
        }
    }
    pub fn from_snapshot(snapshot: &BufferSnapshot, selection: SelectionState) -> Self {
        let mut result = Self::new(snapshot.content.clone());
        result.buffer.set_cursor_offset(snapshot.cursor_offset);
        result.selection = selection;
        result
    }
    pub fn estimated_work(&self, command: &EditorCommand) -> usize {
        if !matches!(
            command,
            EditorCommand::Insert(_) | EditorCommand::Delete { .. }
        ) {
            return 0;
        }
        let snap = self.buffer.create_snapshot();
        let selected = match self.selection.mode {
            SelectionMode::Block => {
                let (a, b, c, d) = self.selection.block();
                (b - a + 1).saturating_mul(d.saturating_sub(c).max(256))
            }
            SelectionMode::Stream => {
                let (a, b) = self.selection.offsets(&snap);
                b - a
            }
            _ => 0,
        };
        selected.saturating_add(match command {
            EditorCommand::Insert(text) => {
                text.len()
                    .saturating_mul(if self.selection.mode == SelectionMode::Block {
                        self.selection.block().1 - self.selection.block().0 + 1
                    } else {
                        1
                    })
            }
            _ => 0,
        })
    }
    pub fn apply(&mut self, command: EditorCommand) -> Result<(), String> {
        self.apply_cancellable(command, &|| false, &|_, _| {})
    }
    pub fn apply_cancellable(
        &mut self,
        command: EditorCommand,
        cancelled: &impl Fn() -> bool,
        progress: &impl Fn(usize, usize),
    ) -> Result<(), String> {
        if cancelled() {
            return Err("Operation cancelled".into());
        }
        match command {
            EditorCommand::Insert(text) => self.insert(&text, cancelled, progress)?,
            EditorCommand::Delete { backward, unit } => {
                self.delete(backward, unit, cancelled, progress)?
            }
            EditorCommand::Move {
                motion,
                extend,
                block,
            } => self.move_cursor(motion, extend, block),
            EditorCommand::SelectAll => {
                let snap = self.buffer.create_snapshot();
                let (l, c) = snap.offset_to_line_col(snap.total_bytes);
                self.selection = SelectionState {
                    mode: SelectionMode::Stream,
                    anchor_line: 0,
                    anchor_col: 0,
                    active_line: l,
                    active_col: c,
                };
                self.buffer.set_cursor_offset(snap.total_bytes);
            }
            EditorCommand::ClearSelection => self.selection.mode = SelectionMode::None,
            EditorCommand::ToggleBlock => {
                if self.selection.mode == SelectionMode::Block {
                    self.selection.mode = SelectionMode::None;
                } else {
                    self.selection = self.current_caret();
                    self.selection.mode = SelectionMode::Block;
                }
            }
            EditorCommand::Undo => {
                self.buffer.undo();
                self.selection.mode = SelectionMode::None;
            }
            EditorCommand::Redo => {
                self.buffer.redo();
                self.selection.mode = SelectionMode::None;
            }
            EditorCommand::Pointer {
                line,
                col,
                kind,
                extend,
                block,
            } => self.pointer(line, col, kind, extend, block),
            EditorCommand::Drag { line, col } => self.drag_to(line, col),
            EditorCommand::SelectOffsets {
                revision,
                start,
                end,
            } => {
                if revision != self.buffer.create_snapshot().revision {
                    return Err("Selection skipped: document changed".into());
                }
                self.select_offsets(start, end);
            }
            EditorCommand::SetSelection(selection) => {
                self.selection = selection;
                let snap = self.buffer.create_snapshot();
                self.buffer.set_cursor_offset(
                    snap.line_col_to_offset(selection.active_line, selection.active_col),
                );
            }
        }
        if cancelled() {
            return Err("Operation cancelled".into());
        }
        Ok(())
    }
    fn current_caret(&self) -> SelectionState {
        let s = self.buffer.create_snapshot();
        let (l, c) = s.offset_to_line_col(s.cursor_offset);
        SelectionState::caret(l, c)
    }
    pub fn select_offsets(&mut self, start: usize, end: usize) {
        let snap = self.buffer.create_snapshot();
        let (sl, sc) = snap.offset_to_line_col(start);
        let (el, ec) = snap.offset_to_line_col(end);
        self.selection = SelectionState {
            mode: SelectionMode::Stream,
            anchor_line: sl,
            anchor_col: sc,
            active_line: el,
            active_col: ec,
        };
        self.buffer.set_cursor_offset(end);
    }
    fn insert(
        &mut self,
        text: &str,
        cancelled: &impl Fn() -> bool,
        progress: &impl Fn(usize, usize),
    ) -> Result<(), String> {
        let snap = self.buffer.create_snapshot();
        if text.len() > texted_buffer::replacement::MAX_GENERATED_GROWTH {
            return Err("Insertion exceeds 128 MiB capacity".into());
        }
        if self.selection.mode == SelectionMode::Block {
            let mut edited = Self::from_snapshot(&snap, self.selection);
            edited.block_edit(
                if text == "\n" { "" } else { text },
                false,
                false,
                cancelled,
                progress,
            )?;
            if text == "\n" {
                edited.buffer.insert(edited.buffer.cursor_offset(), text);
                edited.selection.mode = SelectionMode::None;
            }
            let result = edited.buffer.create_snapshot();
            if !result.content.is_instance(&snap.content) {
                self.buffer
                    .commit_snapshot(snap.revision, snap.cursor_offset, result);
            } else {
                self.buffer.set_cursor_offset(result.cursor_offset);
            }
            self.selection = edited.selection;
            return Ok(());
        }
        let (a, b) = self.selection.offsets(&snap);
        if self.selection.mode == SelectionMode::Stream && a < b {
            self.buffer.transaction(|buffer| {
                buffer.delete(a, b);
                buffer.insert(a, text);
            });
        } else {
            self.buffer.insert(self.buffer.cursor_offset(), text);
        }
        self.selection.mode = SelectionMode::None;
        Ok(())
    }
    fn delete(
        &mut self,
        backward: bool,
        unit: DeleteUnit,
        cancelled: &impl Fn() -> bool,
        progress: &impl Fn(usize, usize),
    ) -> Result<(), String> {
        if self.selection.mode == SelectionMode::Block {
            return self.block_edit("", true, backward, cancelled, progress);
        }
        if self.selection.mode == SelectionMode::Stream {
            let s = self.buffer.create_snapshot();
            let (a, b) = self.selection.offsets(&s);
            if a < b {
                self.buffer.delete(a, b);
                self.selection.mode = SelectionMode::None;
                return Ok(());
            }
        }
        match (backward, unit) {
            (true, DeleteUnit::Character) => self.buffer.backspace(),
            (false, DeleteUnit::Character) => self.buffer.delete_forward(),
            (true, DeleteUnit::Word) => self.buffer.delete_word_left(),
            (false, DeleteUnit::Word) => self.buffer.delete_word_right(),
            (true, DeleteUnit::Line) => self.buffer.delete_to_line_start(),
            (false, DeleteUnit::Line) => self.buffer.delete_to_line_end(),
        }
        self.selection.mode = SelectionMode::None;
        Ok(())
    }
    fn block_edit(
        &mut self,
        text: &str,
        deleting: bool,
        backward: bool,
        cancelled: &impl Fn() -> bool,
        progress: &impl Fn(usize, usize),
    ) -> Result<(), String> {
        let (a, b, c, d) = self.selection.block();
        let capacity = self
            .buffer
            .total_bytes()
            .saturating_add(texted_buffer::replacement::MAX_GENERATED_GROWTH);
        if c > capacity
            || text
                .len()
                .checked_mul(b.saturating_sub(a).saturating_add(1))
                .is_none_or(|n| n > capacity)
        {
            return Err("Block edit exceeds generated-text capacity".into());
        }
        let max = b.min(self.buffer.total_lines() - 1);
        if a > max {
            return Ok(());
        }
        let (from, to) = if deleting && c == d {
            if backward {
                (c.saturating_sub(1), c)
            } else {
                (c, c.saturating_add(1))
            }
        } else {
            (c, d)
        };
        let snapshot = self.buffer.create_snapshot();
        let mut predicted = snapshot.total_bytes;
        for line in a..=max {
            if cancelled() {
                return Err("Operation cancelled".into());
            }
            let start = snapshot.line_col_to_offset(line, from);
            let end = snapshot.line_col_to_offset(line, to);
            let padding = if text.is_empty() {
                0
            } else {
                from.saturating_sub(snapshot.line(line).len_chars())
            };
            predicted = predicted
                .saturating_sub(end - start)
                .checked_add(padding)
                .and_then(|n| n.checked_add(text.len()))
                .ok_or("Block edit exceeds generated-text capacity")?;
            if predicted > capacity {
                return Err("Block edit exceeds generated-text capacity".into());
            }
        }
        self.buffer.try_transaction(|buffer| {
            for (index, line) in (a..=max).rev().enumerate() {
                if cancelled() {
                    return Err("Operation cancelled".to_owned());
                }
                buffer.block_delete(line, line, from, to);
                if !text.is_empty() {
                    buffer.block_insert(line, line, from, text);
                }
                if index % 64 == 0 {
                    progress(index + 1, max - a + 1);
                }
            }
            Ok(())
        })?;
        let col = from + text.chars().count();
        self.selection = SelectionState {
            mode: SelectionMode::Block,
            anchor_line: a,
            active_line: max,
            anchor_col: col,
            active_col: col,
        };
        let snap = self.buffer.create_snapshot();
        self.buffer
            .set_cursor_offset(snap.line_col_to_offset(a, col));
        Ok(())
    }
    fn move_cursor(&mut self, motion: Motion, extend: bool, block: bool) {
        let before = self.current_caret();
        let virtual_col = if block && self.selection.mode == SelectionMode::Block {
            self.selection.active_col
        } else {
            before.active_col
        };
        if extend {
            let mode = if block {
                SelectionMode::Block
            } else {
                SelectionMode::Stream
            };
            if self.selection.mode != mode {
                self.selection = before;
                self.selection.mode = mode;
            }
        } else if self.selection.mode == SelectionMode::Stream
            && matches!(motion, Motion::Left | Motion::Right)
        {
            let snap = self.buffer.create_snapshot();
            let (a, b) = self.selection.offsets(&snap);
            self.buffer
                .set_cursor_offset(if motion == Motion::Left { a } else { b });
            self.selection.mode = SelectionMode::None;
            return;
        } else {
            self.selection.mode = SelectionMode::None;
        }
        match motion {
            Motion::Left => self.buffer.move_cursor_left(),
            Motion::Right => self.buffer.move_cursor_right(),
            Motion::Up => self.buffer.move_cursor_up(),
            Motion::Down => self.buffer.move_cursor_down(),
            Motion::LineStart => self.buffer.move_cursor_home(),
            Motion::LineEnd => self.buffer.move_cursor_end(),
            Motion::DocumentStart => self.buffer.set_cursor_offset(0),
            Motion::DocumentEnd => self.buffer.set_cursor_offset(self.buffer.total_bytes()),
            Motion::WordLeft => self.buffer.move_word_left(),
            Motion::WordRight => self.buffer.move_word_right(),
            Motion::Rows(delta) => {
                for _ in 0..delta.unsigned_abs() {
                    if delta < 0 {
                        self.buffer.move_cursor_up()
                    } else {
                        self.buffer.move_cursor_down()
                    }
                }
            }
        }
        if extend {
            let pos = self.current_caret();
            self.selection.extend(
                pos.active_line,
                if block {
                    match motion {
                        Motion::Left => virtual_col.saturating_sub(1),
                        Motion::Right => virtual_col.saturating_add(1),
                        Motion::Up | Motion::Down | Motion::Rows(_) => virtual_col,
                        _ => pos.active_col,
                    }
                } else {
                    pos.active_col
                },
            );
        }
    }
    fn line_edge(snap: &BufferSnapshot, line: usize) -> (usize, usize) {
        if line + 1 < snap.total_lines {
            (line + 1, 0)
        } else {
            (line, snap.line(line).len_chars())
        }
    }
    fn pointer(&mut self, line: usize, col: usize, kind: PointerKind, extend: bool, block: bool) {
        let snap = self.buffer.create_snapshot();
        let line = line.min(snap.total_lines - 1);
        self.drag = DragAnchor::Character;
        if block {
            self.selection = SelectionState::caret(line, col);
            self.selection.mode = SelectionMode::Block;
        } else {
            match kind {
                PointerKind::Word => {
                    let b = word::bounds(snap.line(line), col);
                    self.selection = SelectionState {
                        mode: SelectionMode::Stream,
                        anchor_line: line,
                        anchor_col: b.start,
                        active_line: line,
                        active_col: b.end,
                    };
                    self.drag = DragAnchor::Word {
                        line,
                        start: b.start,
                        end: b.end,
                    };
                }
                PointerKind::Line => {
                    let (l, c) = Self::line_edge(&snap, line);
                    self.selection = SelectionState {
                        mode: SelectionMode::Stream,
                        anchor_line: line,
                        anchor_col: 0,
                        active_line: l,
                        active_col: c,
                    };
                    self.drag = DragAnchor::Line(line);
                }
                PointerKind::Character => {
                    if extend {
                        if self.selection.mode != SelectionMode::Stream {
                            self.selection = self.current_caret();
                            self.selection.mode = SelectionMode::Stream;
                        }
                        self.selection.extend(line, col);
                    } else {
                        self.selection = SelectionState::caret(line, col);
                    }
                }
            }
        }
        self.buffer.set_cursor_offset(
            snap.line_col_to_offset(self.selection.active_line, self.selection.active_col),
        );
    }
    fn drag_to(&mut self, line: usize, col: usize) {
        let snap = self.buffer.create_snapshot();
        let line = line.min(snap.total_lines - 1);
        match self.drag {
            DragAnchor::Character => {
                if self.selection.mode != SelectionMode::Block {
                    self.selection.mode = SelectionMode::Stream;
                }
                self.selection.extend(line, col);
            }
            DragAnchor::Word {
                line: anchor,
                start,
                end,
            } => {
                let b = word::bounds(snap.line(line), col);
                self.selection.mode = SelectionMode::Stream;
                if (line, col) > (anchor, end) {
                    self.selection.anchor_line = anchor;
                    self.selection.anchor_col = start;
                    self.selection.extend(line, b.end);
                } else if (line, col) < (anchor, start) {
                    self.selection.anchor_line = anchor;
                    self.selection.anchor_col = end;
                    self.selection.extend(line, b.start);
                } else {
                    self.selection.anchor_line = anchor;
                    self.selection.anchor_col = start;
                    self.selection.extend(anchor, end);
                }
            }
            DragAnchor::Line(anchor) => {
                self.selection.mode = SelectionMode::Stream;
                if line >= anchor {
                    self.selection.anchor_line = anchor;
                    self.selection.anchor_col = 0;
                    let (l, c) = Self::line_edge(&snap, line);
                    self.selection.extend(l, c);
                } else {
                    let (l, c) = Self::line_edge(&snap, anchor);
                    self.selection.anchor_line = l;
                    self.selection.anchor_col = c;
                    self.selection.extend(line, 0);
                }
            }
        }
        self.buffer.set_cursor_offset(
            snap.line_col_to_offset(self.selection.active_line, self.selection.active_col),
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_navigation_keeps_virtual_columns_across_short_lines() {
        let mut model = EditorModel::new(ropey::Rope::from_str(
            "123456
x
123456",
        ));
        model
            .apply(EditorCommand::SetSelection(SelectionState {
                mode: SelectionMode::Block,
                anchor_line: 0,
                active_line: 0,
                anchor_col: 5,
                active_col: 5,
            }))
            .unwrap();
        for _ in 0..2 {
            model
                .apply(EditorCommand::Move {
                    motion: Motion::Down,
                    extend: true,
                    block: true,
                })
                .unwrap();
        }
        assert_eq!(
            (model.selection.active_line, model.selection.active_col),
            (2, 5)
        );
        model
            .apply(EditorCommand::Move {
                motion: Motion::Left,
                extend: true,
                block: true,
            })
            .unwrap();
        assert_eq!(model.selection.active_col, 4);
    }
    #[test]
    fn block_enter_is_one_undo_transaction() {
        let mut model = EditorModel::new(ropey::Rope::from_str(
            "one
two",
        ));
        let original = model.buffer.create_snapshot();
        model.selection = SelectionState {
            mode: SelectionMode::Block,
            anchor_line: 0,
            active_line: 1,
            anchor_col: 1,
            active_col: 2,
        };
        model
            .apply(EditorCommand::Insert(
                "
"
                .into(),
            ))
            .unwrap();
        assert!(model.buffer.undo());
        assert!(model
            .buffer
            .create_snapshot()
            .content
            .is_instance(&original.content));
        assert!(!model.buffer.undo());
    }
    #[test]
    fn replacing_stream_selection_is_one_undo_and_subsequent_typing_groups() {
        let mut e = EditorModel::new(ropey::Rope::from_str("їжак"));
        e.select_offsets(0, 8);
        e.apply(EditorCommand::Insert("a".into())).unwrap();
        e.apply(EditorCommand::Insert("b".into())).unwrap();
        assert_eq!(e.buffer.to_string(), "ab");
        e.apply(EditorCommand::Undo).unwrap();
        assert_eq!(e.buffer.to_string(), "їжак");
    }
    #[test]
    fn block_replacement_is_atomic_and_padding_is_preserved() {
        let mut e = EditorModel::new(ropey::Rope::from_str("abc\nx\nabc"));
        e.selection = SelectionState {
            mode: SelectionMode::Block,
            anchor_line: 0,
            active_line: 2,
            anchor_col: 1,
            active_col: 3,
        };
        e.apply(EditorCommand::Insert("ї".into())).unwrap();
        assert_eq!(e.buffer.to_string(), "aї\nxї\naї");
        assert!(e.buffer.undo());
        assert_eq!(e.buffer.to_string(), "abc\nx\nabc");
        assert!(!e.buffer.undo());
    }
    #[test]
    fn cancelled_block_delete_leaves_the_model_unchanged() {
        let mut e = EditorModel::new(ropey::Rope::from_str("abc\nabc\nabc"));
        e.selection = SelectionState {
            mode: SelectionMode::Block,
            anchor_line: 0,
            active_line: 2,
            anchor_col: 1,
            active_col: 2,
        };
        let original = e.buffer.create_snapshot();
        // Progress is reported after the first line changes, so this cancels mid-edit.
        let edited = std::cell::Cell::new(false);
        let result = e.apply_cancellable(
            EditorCommand::Delete {
                backward: true,
                unit: DeleteUnit::Character,
            },
            &|| edited.get(),
            &|_, _| edited.set(true),
        );
        assert!(result.is_err());
        assert_eq!(e.buffer.to_string(), "abc\nabc\nabc");
        assert!(e
            .buffer
            .create_snapshot()
            .content
            .is_instance(&original.content));
        assert!(!e.buffer.undo());
    }
    #[test]
    fn word_drag_keeps_anchor_word_when_pointer_jitters() {
        let mut e = EditorModel::new(ropey::Rope::from_str("one two three"));
        e.apply(EditorCommand::Pointer {
            line: 0,
            col: 5,
            kind: PointerKind::Word,
            extend: false,
            block: false,
        })
        .unwrap();
        e.apply(EditorCommand::Drag { line: 0, col: 5 }).unwrap();
        assert_eq!((e.selection.anchor_col, e.selection.active_col), (4, 7));
        e.apply(EditorCommand::Drag { line: 0, col: 10 }).unwrap();
        assert_eq!((e.selection.anchor_col, e.selection.active_col), (4, 13));
        e.apply(EditorCommand::Drag { line: 0, col: 1 }).unwrap();
        assert_eq!((e.selection.anchor_col, e.selection.active_col), (7, 0));
    }
    #[test]
    fn shift_navigation_keeps_its_first_anchor_and_arrow_collapses_selection() {
        let mut e = EditorModel::new(ropey::Rope::from_str("abc"));
        for _ in 0..2 {
            e.apply(EditorCommand::Move {
                motion: Motion::Right,
                extend: true,
                block: false,
            })
            .unwrap();
        }
        assert_eq!((e.selection.anchor_col, e.selection.active_col), (0, 2));
        e.apply(EditorCommand::Move {
            motion: Motion::Left,
            extend: false,
            block: false,
        })
        .unwrap();
        assert_eq!(e.buffer.cursor_offset(), 0);
    }
}
