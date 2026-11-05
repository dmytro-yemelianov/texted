//! Window-independent selection model and transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    None,
    Stream, // Standard stream selection
    Block,  // Rectangular / column selection
}

#[derive(Clone, Copy, Debug)]
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
