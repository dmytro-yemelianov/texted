//! Window-independent selection model and transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    None,
    Stream, // Standard stream selection
    Block,  // Rectangular / column selection
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionState {
    pub mode: SelectionMode,
    pub anchor_line: usize,
    pub anchor_col: usize,
    pub active_line: usize,
    pub active_col: usize,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            mode: SelectionMode::None,
            anchor_line: 0,
            anchor_col: 0,
            active_line: 0,
            active_col: 0,
        }
    }
}
impl SelectionState {
    pub fn caret(line: usize, col: usize) -> Self {
        Self {
            mode: SelectionMode::None,
            anchor_line: line,
            anchor_col: col,
            active_line: line,
            active_col: col,
        }
    }
    pub fn offsets(&self, snapshot: &texted_buffer::BufferSnapshot) -> (usize, usize) {
        let a = snapshot.line_col_to_offset(self.anchor_line, self.anchor_col);
        let b = snapshot.line_col_to_offset(self.active_line, self.active_col);
        (a.min(b), a.max(b))
    }
    pub fn block(&self) -> (usize, usize, usize, usize) {
        (
            self.anchor_line.min(self.active_line),
            self.anchor_line.max(self.active_line),
            self.anchor_col.min(self.active_col),
            self.anchor_col.max(self.active_col),
        )
    }
    pub fn extend(&mut self, line: usize, col: usize) {
        self.active_line = line;
        self.active_col = col;
    }
}
