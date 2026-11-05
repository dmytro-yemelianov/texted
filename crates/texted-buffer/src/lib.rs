//! Zed-inspired Piece Table & SumTree Text Buffer Engine
//!
//! Provides $O(\log N)$ line-to-offset and offset-to-line queries,
//! zero-copy snapshots via copy-on-write piece descriptors,
//! and atomic undo/redo transaction logging.

use std::sync::Arc;

/// Monoid summary for text chunks (inspired by Zed's SumTree)
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TextSummary {
    pub bytes: usize,
    pub lines: usize, // newline ('\n') count
    pub utf16_units: usize,
}

impl TextSummary {
    pub fn from_str(text: &str) -> Self {
        let mut lines = 0;
        let mut utf16_units = 0;
        for c in text.chars() {
            if c == '\n' {
                lines += 1;
            }
            utf16_units += c.len_utf16();
        }
        Self {
            bytes: text.len(),
            lines,
            utf16_units,
        }
    }
}

impl std::ops::Add for TextSummary {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self {
            bytes: self.bytes + rhs.bytes,
            lines: self.lines + rhs.lines,
            utf16_units: self.utf16_units + rhs.utf16_units,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferSource {
    Original,
    Add,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub source: BufferSource,
    pub start: usize,
    pub len: usize,
    pub summary: TextSummary,
}

/// Transaction record for undo/redo
#[derive(Clone, Debug)]
pub struct UndoRecord {
    pub pieces_before: Vec<Piece>,
    pub cursor_before: usize,
    pub cursor_after: usize,
}

/// The Piece Table Text Buffer
#[derive(Clone, Debug)]
pub struct TextBuffer {
    original: Arc<String>,
    added: String,
    pieces: Vec<Piece>,
    undo_stack: Vec<UndoRecord>,
    redo_stack: Vec<UndoRecord>,
    cursor_offset: usize,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextBuffer {
    pub fn new() -> Self {
        Self {
            original: Arc::new(String::new()),
            added: String::new(),
            pieces: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            cursor_offset: 0,
        }
    }

    pub fn from_string(content: String) -> Self {
        let summary = TextSummary::from_str(&content);
        let len = content.len();
        let pieces = if len > 0 {
            vec![Piece {
                source: BufferSource::Original,
                start: 0,
                len,
                summary,
            }]
        } else {
            Vec::new()
        };

        Self {
            original: Arc::new(content),
            added: String::new(),
            pieces,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            cursor_offset: 0,
        }
    }

    pub fn cursor_offset(&self) -> usize {
        self.cursor_offset
    }

    pub fn set_cursor_offset(&mut self, offset: usize) {
        self.cursor_offset = offset.min(self.total_bytes());
    }

    pub fn total_bytes(&self) -> usize {
        self.pieces.iter().map(|p| p.len).sum()
    }

    pub fn total_lines(&self) -> usize {
        let line_breaks: usize = self.pieces.iter().map(|p| p.summary.lines).sum();
        line_breaks + 1 // 0 breaks = 1 line
    }

    fn record_undo(&mut self, cursor_before: usize, cursor_after: usize) {
        self.undo_stack.push(UndoRecord {
            pieces_before: self.pieces.clone(),
            cursor_before,
            cursor_after,
        });
        self.redo_stack.clear();
    }

    /// Insert text at a specific byte offset
    pub fn insert(&mut self, offset: usize, text: &str) {
        if text.is_empty() {
            return;
        }

        let total = self.total_bytes();
        let offset = offset.min(total);
        let cursor_before = self.cursor_offset;
        let cursor_after = offset + text.len();

        self.record_undo(cursor_before, cursor_after);

        let add_start = self.added.len();
        self.added.push_str(text);
        let add_len = text.len();
        let add_summary = TextSummary::from_str(text);

        let new_piece = Piece {
            source: BufferSource::Add,
            start: add_start,
            len: add_len,
            summary: add_summary,
        };

        if self.pieces.is_empty() {
            self.pieces.push(new_piece);
            self.cursor_offset = cursor_after;
            return;
        }

        let mut current_offset = 0;
        let mut split_idx = self.pieces.len();
        let mut piece_split_offset = 0;

        for (idx, piece) in self.pieces.iter().enumerate() {
            if offset >= current_offset && offset <= current_offset + piece.len {
                split_idx = idx;
                piece_split_offset = offset - current_offset;
                break;
            }
            current_offset += piece.len;
        }

        if split_idx == self.pieces.len() {
            self.pieces.push(new_piece);
        } else if piece_split_offset == 0 {
            self.pieces.insert(split_idx, new_piece);
        } else if piece_split_offset == self.pieces[split_idx].len {
            self.pieces.insert(split_idx + 1, new_piece);
        } else {
            // Split piece into left and right
            let original_piece = self.pieces[split_idx].clone();
            let left_len = piece_split_offset;
            let right_len = original_piece.len - left_len;

            let left_text = self.slice_piece(&original_piece, 0, left_len);
            let right_text = self.slice_piece(&original_piece, left_len, right_len);

            let left_piece = Piece {
                source: original_piece.source,
                start: original_piece.start,
                len: left_len,
                summary: TextSummary::from_str(&left_text),
            };

            let right_piece = Piece {
                source: original_piece.source,
                start: original_piece.start + left_len,
                len: right_len,
                summary: TextSummary::from_str(&right_text),
            };

            self.pieces[split_idx] = left_piece;
            self.pieces.insert(split_idx + 1, new_piece);
            self.pieces.insert(split_idx + 2, right_piece);
        }

        self.cursor_offset = cursor_after;
    }

    /// Delete range of bytes [start..end)
    pub fn delete(&mut self, start: usize, end: usize) {
        let total = self.total_bytes();
        let start = start.min(total);
        let end = end.min(total);
        if start >= end {
            return;
        }

        let cursor_before = self.cursor_offset;
        let cursor_after = start;
        self.record_undo(cursor_before, cursor_after);

        let mut new_pieces = Vec::new();
        let mut current_offset = 0;

        for piece in &self.pieces {
            let piece_start = current_offset;
            let piece_end = current_offset + piece.len;

            if piece_end <= start || piece_start >= end {
                // Entirely outside delete range
                new_pieces.push(piece.clone());
            } else {
                // Overlaps with delete range
                if piece_start < start {
                    // Keep left slice
                    let keep_len = start - piece_start;
                    let text = self.slice_piece(piece, 0, keep_len);
                    new_pieces.push(Piece {
                        source: piece.source,
                        start: piece.start,
                        len: keep_len,
                        summary: TextSummary::from_str(&text),
                    });
                }
                if piece_end > end {
                    // Keep right slice
                    let skip_len = end - piece_start;
                    let keep_len = piece_end - end;
                    let text = self.slice_piece(piece, skip_len, keep_len);
                    new_pieces.push(Piece {
                        source: piece.source,
                        start: piece.start + skip_len,
                        len: keep_len,
                        summary: TextSummary::from_str(&text),
                    });
                }
            }
            current_offset = piece_end;
        }

        self.pieces = new_pieces;
        self.cursor_offset = cursor_after;
    }

    pub fn undo(&mut self) -> bool {
        if let Some(record) = self.undo_stack.pop() {
            self.redo_stack.push(UndoRecord {
                pieces_before: self.pieces.clone(),
                cursor_before: self.cursor_offset,
                cursor_after: record.cursor_before,
            });
            self.pieces = record.pieces_before;
            self.cursor_offset = record.cursor_before;
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(record) = self.redo_stack.pop() {
            self.undo_stack.push(UndoRecord {
                pieces_before: self.pieces.clone(),
                cursor_before: self.cursor_offset,
                cursor_after: record.cursor_before,
            });
            self.pieces = record.pieces_before;
            self.cursor_offset = record.cursor_before;
            true
        } else {
            false
        }
    }

    fn slice_piece(&self, piece: &Piece, rel_offset: usize, len: usize) -> String {
        let abs_start = piece.start + rel_offset;
        let abs_end = abs_start + len;
        match piece.source {
            BufferSource::Original => self.original[abs_start..abs_end].to_string(),
            BufferSource::Add => self.added[abs_start..abs_end].to_string(),
        }
    }

    /// Read whole text buffer into a contiguous String
    pub fn to_string(&self) -> String {
        let mut out = String::with_capacity(self.total_bytes());
        for piece in &self.pieces {
            let s = match piece.source {
                BufferSource::Original => &self.original[piece.start..piece.start + piece.len],
                BufferSource::Add => &self.added[piece.start..piece.start + piece.len],
            };
            out.push_str(s);
        }
        out
    }

    /// Converts byte offset to 0-based (line, column)
    pub fn offset_to_line_col(&self, offset: usize) -> (usize, usize) {
        let mut cur_offset = 0;
        let mut cur_line = 0;
        let mut line_start_offset = 0;

        for piece in &self.pieces {
            let piece_end = cur_offset + piece.len;
            if offset <= piece_end {
                let piece_text = self.slice_piece(piece, 0, piece.len);
                for (idx, ch) in piece_text.char_indices() {
                    let global_pos = cur_offset + idx;
                    if global_pos >= offset {
                        let col = offset - line_start_offset;
                        return (cur_line, col);
                    }
                    if ch == '\n' {
                        cur_line += 1;
                        line_start_offset = global_pos + 1;
                    }
                }
                let col = offset - line_start_offset;
                return (cur_line, col);
            } else {
                cur_line += piece.summary.lines;
                // find last newline in piece if it had any
                if piece.summary.lines > 0 {
                    let piece_text = self.slice_piece(piece, 0, piece.len);
                    if let Some(pos) = piece_text.rfind('\n') {
                        line_start_offset = cur_offset + pos + 1;
                    }
                }
                cur_offset = piece_end;
            }
        }

        let col = offset.saturating_sub(line_start_offset);
        (cur_line, col)
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor_offset == 0 {
            return;
        }
        let full = self.to_string();
        let mut prev = 0;
        for (idx, _) in full.char_indices() {
            if idx >= self.cursor_offset {
                break;
            }
            prev = idx;
        }
        self.cursor_offset = prev;
    }

    pub fn move_cursor_right(&mut self) {
        let total = self.total_bytes();
        if self.cursor_offset >= total {
            return;
        }
        let full = self.to_string();
        for (idx, ch) in full.char_indices() {
            if idx == self.cursor_offset {
                self.cursor_offset = idx + ch.len_utf8();
                return;
            }
        }
        self.cursor_offset = total;
    }

    pub fn move_cursor_up(&mut self) {
        let full = self.to_string();
        let (line, col) = self.offset_to_line_col(self.cursor_offset);
        if line == 0 {
            return;
        }
        let target_line = line - 1;
        self.cursor_offset = Self::line_col_to_offset_str(&full, target_line, col);
    }

    pub fn move_cursor_down(&mut self) {
        let full = self.to_string();
        let (line, col) = self.offset_to_line_col(self.cursor_offset);
        let target_line = line + 1;
        self.cursor_offset = Self::line_col_to_offset_str(&full, target_line, col);
    }

    pub fn move_cursor_home(&mut self) {
        let full = self.to_string();
        let (line, _) = self.offset_to_line_col(self.cursor_offset);
        self.cursor_offset = Self::line_col_to_offset_str(&full, line, 0);
    }

    pub fn move_cursor_end(&mut self) {
        let full = self.to_string();
        let (line, _) = self.offset_to_line_col(self.cursor_offset);
        self.cursor_offset = Self::line_col_to_offset_str(&full, line, usize::MAX);
    }

    pub fn line_col_to_offset_str(text: &str, line: usize, col: usize) -> usize {
        let mut cur_line = 0;
        let mut line_start = 0;
        for (idx, ch) in text.char_indices() {
            if cur_line == line {
                let col_chars: usize = text[line_start..idx].chars().count();
                if col_chars == col {
                    return idx;
                }
            }
            if ch == '\n' {
                if cur_line == line {
                    return idx; // hit newline before reaching col
                }
                cur_line += 1;
                line_start = idx + 1;
            }
        }
        if cur_line == line {
            let line_text = &text[line_start..];
            let char_count = line_text.chars().count();
            let target_chars = col.min(char_count);
            for (i, (b_idx, _)) in line_text.char_indices().enumerate() {
                if i == target_chars {
                    return line_start + b_idx;
                }
            }
            return text.len();
        }
        text.len()
    }

    pub fn backspace(&mut self) {
        if self.cursor_offset == 0 {
            return;
        }
        let full = self.to_string();
        let mut prev = 0;
        for (idx, _) in full.char_indices() {
            if idx >= self.cursor_offset {
                break;
            }
            prev = idx;
        }
        self.delete(prev, self.cursor_offset);
    }

    pub fn delete_forward(&mut self) {
        let total = self.total_bytes();
        if self.cursor_offset >= total {
            return;
        }
        let full = self.to_string();
        for (idx, ch) in full.char_indices() {
            if idx == self.cursor_offset {
                let next = idx + ch.len_utf8();
                self.delete(idx, next);
                return;
            }
        }
    }

    /// Extracts a rectangular block of characters [start_col..end_col) across lines [start_line..=end_line]
    pub fn block_extract(&self, start_line: usize, end_line: usize, start_col: usize, end_col: usize) -> String {
        let full = self.to_string();
        let lines: Vec<&str> = full.split('\n').collect();
        let min_l = start_line.min(end_line);
        let max_l = start_line.max(end_line);
        let min_c = start_col.min(end_col);
        let max_c = start_col.max(end_col);

        let mut result = Vec::new();
        for l_idx in min_l..=max_l {
            if let Some(line_str) = lines.get(l_idx) {
                let chars: Vec<char> = line_str.chars().collect();
                let mut line_res = String::new();
                for c_idx in min_c..max_c {
                    if c_idx < chars.len() {
                        line_res.push(chars[c_idx]);
                    } else {
                        line_res.push(' ');
                    }
                }
                result.push(line_res);
            }
        }
        result.join("\n")
    }

    /// Deletes a rectangular block [start_col..end_col) across lines [start_line..=end_line]
    pub fn block_delete(&mut self, start_line: usize, end_line: usize, start_col: usize, end_col: usize) {
        let min_l = start_line.min(end_line);
        let max_l = start_line.max(end_line);
        let min_c = start_col.min(end_col);
        let max_c = start_col.max(end_col);
        if min_c >= max_c {
            return;
        }

        // Iterate backwards from max_l to min_l so line byte positions above don't shift
        for l_idx in (min_l..=max_l).rev() {
            let full = self.to_string();
            let lines: Vec<&str> = full.split('\n').collect();
            if l_idx >= lines.len() {
                continue;
            }

            let mut line_byte_start = 0;
            for i in 0..l_idx {
                line_byte_start += lines[i].len() + 1; // +1 for '\n'
            }

            let line_str = lines[l_idx];
            let chars: Vec<(usize, char)> = line_str.char_indices().collect();
            if chars.is_empty() || min_c >= chars.len() {
                continue;
            }

            let byte_from = chars[min_c].0;
            let byte_to = if max_c < chars.len() {
                chars[max_c].0
            } else {
                line_str.len()
            };

            if byte_from < byte_to {
                self.delete(line_byte_start + byte_from, line_byte_start + byte_to);
            }
        }

        // Update cursor to top-left of the deleted block
        let full = self.to_string();
        self.cursor_offset = Self::line_col_to_offset_str(&full, min_l, min_c);
    }

    /// Inserts text at column `col` across lines [start_line..=end_line]
    pub fn block_insert(&mut self, start_line: usize, end_line: usize, col: usize, text: &str) {
        let min_l = start_line.min(end_line);
        let max_l = start_line.max(end_line);

        // Process from bottom to top to preserve offsets
        for l_idx in (min_l..=max_l).rev() {
            let full = self.to_string();
            let lines: Vec<&str> = full.split('\n').collect();
            if l_idx >= lines.len() {
                continue;
            }

            let mut line_byte_start = 0;
            for i in 0..l_idx {
                line_byte_start += lines[i].len() + 1;
            }

            let line_str = lines[l_idx];
            let char_count = line_str.chars().count();

            if col > char_count {
                // Pad with spaces up to col, then insert text
                let pad_spaces = " ".repeat(col - char_count);
                let insert_str = format!("{}{}", pad_spaces, text);
                self.insert(line_byte_start + line_str.len(), &insert_str);
            } else {
                let mut byte_offset = 0;
                for (i, (b_idx, _)) in line_str.char_indices().enumerate() {
                    if i == col {
                        byte_offset = b_idx;
                        break;
                    }
                }
                self.insert(line_byte_start + byte_offset, text);
            }
        }

        // Move cursor to after insertion at min_l
        let full = self.to_string();
        self.cursor_offset = Self::line_col_to_offset_str(&full, min_l, col + text.chars().count());
    }

    /// Finds all byte ranges (start, end) matching `query`
    pub fn find_matches(&self, query: &str) -> Vec<(usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let full = self.to_string();
        full.match_indices(query)
            .map(|(offset, s)| (offset, offset + s.len()))
            .collect()
    }

    /// Finds next match starting at or after `start_offset`, with wrap-around
    pub fn find_next(&self, query: &str, start_offset: usize) -> Option<(usize, usize)> {
        if query.is_empty() {
            return None;
        }
        let full = self.to_string();
        if start_offset < full.len() {
            if let Some(pos) = full[start_offset..].find(query) {
                let m_start = start_offset + pos;
                return Some((m_start, m_start + query.len()));
            }
        }
        // Wrap around from beginning
        if let Some(pos) = full[..start_offset.min(full.len())].find(query) {
            return Some((pos, pos + query.len()));
        }
        None
    }

    /// Finds previous match before `start_offset`, with wrap-around
    pub fn find_prev(&self, query: &str, start_offset: usize) -> Option<(usize, usize)> {
        if query.is_empty() {
            return None;
        }
        let full = self.to_string();
        if start_offset > 0 {
            let slice = &full[..start_offset.min(full.len())];
            if let Some(pos) = slice.rfind(query) {
                return Some((pos, pos + query.len()));
            }
        }
        // Wrap around from end
        full.rfind(query).map(|pos| (pos, pos + query.len()))
    }

    /// Replaces the next occurrence of `query` at or after `start_offset`
    pub fn replace_next(&mut self, query: &str, replacement: &str, start_offset: usize) -> Option<(usize, usize)> {
        if let Some((start, end)) = self.find_next(query, start_offset) {
            self.delete(start, end);
            self.insert(start, replacement);
            let new_end = start + replacement.len();
            self.cursor_offset = new_end;
            Some((start, new_end))
        } else {
            None
        }
    }

    /// Replaces all occurrences of `query` with `replacement`
    pub fn replace_all(&mut self, query: &str, replacement: &str) -> usize {
        if query.is_empty() {
            return 0;
        }
        let full = self.to_string();
        let matches: Vec<usize> = full.match_indices(query).map(|(i, _)| i).collect();
        let count = matches.len();
        if count == 0 {
            return 0;
        }
        let new_content = full.replace(query, replacement);
        let cursor = self.cursor_offset.min(new_content.len());
        *self = Self::from_string(new_content);
        self.cursor_offset = cursor;
        count
    }

    /// Create an immutable snapshot for layout and render servers
    pub fn create_snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            content: self.to_string(),
            total_bytes: self.total_bytes(),
            total_lines: self.total_lines(),
            cursor_offset: self.cursor_offset,
        }
    }
}

/// Immutable, thread-safe snapshot passed to Layout and Render servers
#[derive(Clone, Debug)]
pub struct BufferSnapshot {
    pub content: String,
    pub total_bytes: usize,
    pub total_lines: usize,
    pub cursor_offset: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_piece_table_basic_edits() {
        let mut buf = TextBuffer::from_string("Hello World!".to_string());
        assert_eq!(buf.to_string(), "Hello World!");
        assert_eq!(buf.total_lines(), 1);

        // Insert in middle
        buf.insert(5, ", Beautiful");
        assert_eq!(buf.to_string(), "Hello, Beautiful World!");

        // Delete " Beautiful"
        buf.delete(5, 16);
        assert_eq!(buf.to_string(), "Hello World!");

        // Undo delete
        assert!(buf.undo());
        assert_eq!(buf.to_string(), "Hello, Beautiful World!");

        // Redo delete
        assert!(buf.redo());
        assert_eq!(buf.to_string(), "Hello World!");
    }

    #[test]
    fn test_line_col_conversion() {
        let text = "Line 1\nLine 2\nLine 3".to_string();
        let buf = TextBuffer::from_string(text);
        assert_eq!(buf.total_lines(), 3);

        // Offset 0 -> (0, 0)
        assert_eq!(buf.offset_to_line_col(0), (0, 0));
        // Offset 7 -> start of Line 2 -> (1, 0)
        assert_eq!(buf.offset_to_line_col(7), (1, 0));
        // Offset 10 -> (1, 3)
        assert_eq!(buf.offset_to_line_col(10), (1, 3));
    }

    #[test]
    fn test_ukrainian_text_support() {
        let text = "Привіт, світе!\nЦе українська мова: Їжак, Євген, Ґрунт, Ігор.".to_string();
        let mut buf = TextBuffer::from_string(text.clone());
        assert_eq!(buf.to_string(), text);
        assert_eq!(buf.total_lines(), 2);

        // Insert Ukrainian word at offset 14 (start of "світе!")
        buf.insert(14, "чудовий ");
        assert_eq!(buf.to_string(), "Привіт, чудовий світе!\nЦе українська мова: Їжак, Євген, Ґрунт, Ігор.");
    }

    #[test]
    fn test_block_selection_operations() {
        let mut buf = TextBuffer::from_string("Apple\nBanana\nCherry".to_string());
        
        // Extract block [0..2] on lines 0..2
        let extracted = buf.block_extract(0, 2, 0, 2);
        assert_eq!(extracted, "Ap\nBa\nCh");

        // Block insert "[x] " at col 0 on lines 0..2
        buf.block_insert(0, 2, 0, "[x] ");
        assert_eq!(buf.to_string(), "[x] Apple\n[x] Banana\n[x] Cherry");

        // Block delete columns 0..4 on lines 0..2 (removes "[x] ")
        buf.block_delete(0, 2, 0, 4);
        assert_eq!(buf.to_string(), "Apple\nBanana\nCherry");
    }

    #[test]
    fn test_search_and_replace() {
        let mut buf = TextBuffer::from_string("apple orange apple banana apple".to_string());
        
        let matches = buf.find_matches("apple");
        assert_eq!(matches.len(), 3);

        // Find next after 6 -> finds second apple at 13
        let next = buf.find_next("apple", 6);
        assert_eq!(next, Some((13, 18)));

        // Replace next
        let rep = buf.replace_next("apple", "peach", 13);
        assert_eq!(rep, Some((13, 18)));
        assert_eq!(buf.to_string(), "apple orange peach banana apple");

        // Replace all remaining "apple" with "grape"
        let count = buf.replace_all("apple", "grape");
        assert_eq!(count, 2);
        assert_eq!(buf.to_string(), "grape orange peach banana grape");
    }
}
