//! Typed editing intents, independent of native events, transport and rendering.
use crate::selection::SelectionState;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
    WordLeft,
    WordRight,
    Rows(i32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteUnit {
    Character,
    Word,
    Line,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Character,
    Word,
    Line,
}
#[derive(Debug, Clone)]
pub enum EditorCommand {
    Insert(String),
    Delete {
        backward: bool,
        unit: DeleteUnit,
    },
    Move {
        motion: Motion,
        extend: bool,
        block: bool,
    },
    SelectAll,
    ClearSelection,
    ToggleBlock,
    Undo,
    Redo,
    Pointer {
        line: usize,
        col: usize,
        kind: PointerKind,
        extend: bool,
        block: bool,
    },
    Drag {
        line: usize,
        col: usize,
    },
    SelectOffsets {
        revision: u64,
        start: usize,
        end: usize,
    },
    SetSelection(SelectionState),
}
