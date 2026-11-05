//! Persistent UTF-8 rope buffer with shared snapshots and bounded transaction history.
use ropey::{Rope, RopeSlice};
use std::{
    collections::VecDeque,
    fmt,
    time::{Duration, Instant},
};

const HISTORY_LIMIT: usize = 1000;
#[derive(Clone, Debug)]
struct Revision {
    text: Rope,
    cursor: usize,
}
#[derive(Clone, Debug)]
pub struct TextBuffer {
    text: Rope,
    cursor_offset: usize,
    undo_stack: VecDeque<Revision>,
    redo_stack: Vec<Revision>,
    typing: Option<(usize, Instant)>,
    revision: u64,
}
impl Default for TextBuffer {
    fn default() -> Self {
        Self::new()
    }
}
impl fmt::Display for TextBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.text.fmt(f)
    }
}
impl TextBuffer {
    pub fn new() -> Self {
        Self::from_string(String::new())
    }
    pub fn from_string(content: String) -> Self {
        Self::from_rope(Rope::from_str(&content))
    }
    pub fn from_rope(text: Rope) -> Self {
        Self {
            text,
            cursor_offset: 0,
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            typing: None,
            revision: 0,
        }
    }
    pub fn cursor_offset(&self) -> usize {
        self.cursor_offset
    }
    pub fn set_cursor_offset(&mut self, offset: usize) {
        self.cursor_offset = self.clamp_byte(offset);
        self.typing = None;
    }
    fn clamp_byte(&self, offset: usize) -> usize {
        self.text
            .char_to_byte(self.text.byte_to_char(offset.min(self.total_bytes())))
    }
    pub fn total_bytes(&self) -> usize {
        self.text.len_bytes()
    }
    pub fn total_lines(&self) -> usize {
        self.text.len_lines()
    }
    fn checkpoint(&mut self) {
        if self.undo_stack.len() == HISTORY_LIMIT {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(Revision {
            text: self.text.clone(),
            cursor: self.cursor_offset,
        });
        self.redo_stack.clear();
        self.typing = None;
    }
    fn replace_range_raw(&mut self, start: usize, end: usize, text: &str) {
        let a = self.text.byte_to_char(self.clamp_byte(start));
        let b = self.text.byte_to_char(self.clamp_byte(end));
        if a < b {
            self.text.remove(a..b);
        }
        if !text.is_empty() {
            self.text.insert(a, text);
        }
        self.cursor_offset = self.text.char_to_byte(a + text.chars().count());
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn insert(&mut self, offset: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let offset = self.clamp_byte(offset);
        let single = text.chars().count() == 1 && !text.contains(['\n', '\r']);
        let group = single
            && self.typing.is_some_and(|(end, at)| {
                end == offset && at.elapsed() < Duration::from_millis(750)
            });
        if !group {
            self.checkpoint();
        }
        self.replace_range_raw(offset, offset, text);
        self.typing = single.then(|| (self.cursor_offset, Instant::now()));
    }
    pub fn delete(&mut self, start: usize, end: usize) {
        let (start, end) = (self.clamp_byte(start), self.clamp_byte(end));
        if start >= end {
            return;
        }
        self.checkpoint();
        self.replace_range_raw(start, end, "");
    }
    pub fn undo(&mut self) -> bool {
        self.typing = None;
        if let Some(r) = self.undo_stack.pop_back() {
            self.redo_stack.push(Revision {
                text: self.text.clone(),
                cursor: self.cursor_offset,
            });
            self.text = r.text;
            self.cursor_offset = r.cursor;
            self.revision = self.revision.wrapping_add(1);
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self) -> bool {
        self.typing = None;
        if let Some(r) = self.redo_stack.pop() {
            self.undo_stack.push_back(Revision {
                text: self.text.clone(),
                cursor: self.cursor_offset,
            });
            self.text = r.text;
            self.cursor_offset = r.cursor;
            self.revision = self.revision.wrapping_add(1);
            true
        } else {
            false
        }
    }
    pub fn offset_to_line_col(&self, offset: usize) -> (usize, usize) {
        self.create_snapshot().offset_to_line_col(offset)
    }
    pub fn offset_to_line_col_str(text: &str, offset: usize) -> (usize, usize) {
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let prefix = &text[..offset];
        (
            prefix.bytes().filter(|&b| b == b'\n').count(),
            prefix.rsplit('\n').next().unwrap_or("").chars().count(),
        )
    }
    pub fn line_col_to_offset_str(text: &str, line: usize, col: usize) -> usize {
        let mut start = 0;
        for _ in 0..line {
            match text[start..].find('\n') {
                Some(i) => start += i + 1,
                None => return text.len(),
            }
        }
        let s = text[start..].split('\n').next().unwrap_or("");
        start + s.char_indices().nth(col).map_or(s.len(), |(b, _)| b)
    }
    pub fn move_cursor_left(&mut self) {
        let c = self.text.byte_to_char(self.cursor_offset);
        self.set_cursor_offset(self.text.char_to_byte(c.saturating_sub(1)));
    }
    pub fn move_cursor_right(&mut self) {
        let c = self.text.byte_to_char(self.cursor_offset);
        self.set_cursor_offset(self.text.char_to_byte((c + 1).min(self.text.len_chars())));
    }
    pub fn move_cursor_up(&mut self) {
        let (l, c) = self.offset_to_line_col(self.cursor_offset);
        self.set_cursor_offset(
            self.create_snapshot()
                .line_col_to_offset(l.saturating_sub(1), c),
        );
    }
    pub fn move_cursor_down(&mut self) {
        let (l, c) = self.offset_to_line_col(self.cursor_offset);
        self.set_cursor_offset(
            self.create_snapshot()
                .line_col_to_offset((l + 1).min(self.total_lines() - 1), c),
        );
    }
    pub fn move_cursor_home(&mut self) {
        let (l, _) = self.offset_to_line_col(self.cursor_offset);
        self.set_cursor_offset(self.create_snapshot().line_col_to_offset(l, 0));
    }
    pub fn move_cursor_end(&mut self) {
        let (l, _) = self.offset_to_line_col(self.cursor_offset);
        self.set_cursor_offset(self.create_snapshot().line_col_to_offset(l, usize::MAX));
    }
    pub fn move_word_left(&mut self) {
        let mut c = self.text.byte_to_char(self.cursor_offset);
        c = c.saturating_sub(1);
        while c > 0 && !is_word(self.text.char(c)) {
            c -= 1;
        }
        while c > 0 && is_word(self.text.char(c - 1)) {
            c -= 1;
        }
        self.set_cursor_offset(self.text.char_to_byte(c));
    }
    pub fn move_word_right(&mut self) {
        let mut c = self.text.byte_to_char(self.cursor_offset);
        while c < self.text.len_chars() && is_word(self.text.char(c)) {
            c += 1;
        }
        while c < self.text.len_chars() && !is_word(self.text.char(c)) {
            c += 1;
        }
        self.set_cursor_offset(self.text.char_to_byte(c));
    }
    pub fn backspace(&mut self) {
        let end = self.cursor_offset;
        if end > 0 {
            self.move_cursor_left();
            let start = self.cursor_offset;
            self.cursor_offset = end;
            self.delete(start, end);
        }
    }
    pub fn delete_forward(&mut self) {
        let start = self.cursor_offset;
        self.move_cursor_right();
        let end = self.cursor_offset;
        self.cursor_offset = start;
        self.delete(start, end);
    }
    pub fn delete_word_left(&mut self) {
        let end = self.cursor_offset;
        self.move_word_left();
        let start = self.cursor_offset;
        self.cursor_offset = end;
        self.delete(start, end);
    }
    pub fn delete_word_right(&mut self) {
        let start = self.cursor_offset;
        self.move_word_right();
        let end = self.cursor_offset;
        self.cursor_offset = start;
        self.delete(start, end);
    }
    pub fn delete_to_line_start(&mut self) {
        let end = self.cursor_offset;
        self.move_cursor_home();
        let start = self.cursor_offset;
        self.cursor_offset = end;
        self.delete(start, end);
    }
    pub fn delete_to_line_end(&mut self) {
        let start = self.cursor_offset;
        self.move_cursor_end();
        let end = self.cursor_offset;
        self.cursor_offset = start;
        self.delete(start, end);
    }
    pub fn block_extract(&self, a: usize, b: usize, c: usize, d: usize) -> String {
        let snap = self.create_snapshot();
        let mut out = Vec::new();
        for l in a.min(b)..=a.max(b).min(self.total_lines() - 1) {
            let s = snap.line(l);
            let mut row: String = s.chars().skip(c.min(d)).take(c.abs_diff(d)).collect();
            row.push_str(&" ".repeat(c.abs_diff(d).saturating_sub(row.chars().count())));
            out.push(row);
        }
        out.join("\n")
    }
    pub fn block_delete(&mut self, a: usize, b: usize, c: usize, d: usize) {
        if c == d || a.min(b) >= self.total_lines() {
            return;
        }
        let snap = self.create_snapshot();
        self.checkpoint();
        for l in (a.min(b)..=a.max(b).min(snap.total_lines - 1)).rev() {
            let start = snap.line_col_to_offset(l, c.min(d));
            let end = snap.line_col_to_offset(l, c.max(d));
            if start < end {
                self.replace_range_raw(start, end, "");
            }
        }
        self.cursor_offset = self
            .create_snapshot()
            .line_col_to_offset(a.min(b), c.min(d));
    }
    pub fn block_insert(&mut self, a: usize, b: usize, col: usize, text: &str) {
        if text.is_empty() || a.min(b) >= self.total_lines() {
            return;
        }
        let snap = self.create_snapshot();
        self.checkpoint();
        for l in (a.min(b)..=a.max(b).min(snap.total_lines - 1)).rev() {
            let len = snap.line(l).chars().count();
            let at = snap.line_col_to_offset(l, col);
            let insertion = format!("{}{}", " ".repeat(col.saturating_sub(len)), text);
            self.replace_range_raw(at, at, &insertion);
        }
        self.cursor_offset = self
            .create_snapshot()
            .line_col_to_offset(a.min(b), col + text.chars().count());
    }
    pub fn find_matches(&self, query: &str) -> Vec<(usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        // Full-document search is explicitly linear; editing/layout never flatten the rope.
        self.create_snapshot().find_matches(query)
    }
    pub fn find_next(&self, query: &str, start: usize) -> Option<(usize, usize)> {
        let matches = self.find_matches(query);
        let start = self.clamp_byte(start);
        matches
            .iter()
            .copied()
            .find(|&(a, _)| a >= start)
            .or_else(|| matches.first().copied())
    }
    pub fn find_prev(&self, query: &str, start: usize) -> Option<(usize, usize)> {
        let matches = self.find_matches(query);
        let start = self.clamp_byte(start);
        matches
            .iter()
            .rev()
            .copied()
            .find(|&(a, _)| a < start)
            .or_else(|| matches.last().copied())
    }
    pub fn replace_next(
        &mut self,
        query: &str,
        replacement: &str,
        start: usize,
    ) -> Option<(usize, usize)> {
        let (a, b) = self.find_next(query, start)?;
        self.checkpoint();
        self.replace_range_raw(a, b, replacement);
        Some((a, a + replacement.len()))
    }
    pub fn replace_all(&mut self, query: &str, replacement: &str) -> usize {
        let matches = self.find_matches(query);
        if matches.is_empty() {
            return 0;
        }
        let old_cursor = self.cursor_offset;
        let mut cursor = old_cursor as i128;
        self.checkpoint();
        for &(a, b) in matches.iter().rev() {
            self.replace_range_raw(a, b, replacement);
            if old_cursor >= b {
                cursor += replacement.len() as i128 - (b - a) as i128;
            } else if old_cursor > a {
                cursor += (a + replacement.len()) as i128 - old_cursor as i128;
            }
        }
        self.cursor_offset = self.clamp_byte(cursor.max(0) as usize);
        matches.len()
    }
    /// Commit background work only while its base document and cursor are current.
    pub fn commit_snapshot(
        &mut self,
        base_revision: u64,
        base_cursor: usize,
        edited: BufferSnapshot,
    ) -> bool {
        if self.revision != base_revision || self.cursor_offset != base_cursor {
            return false;
        }
        self.checkpoint();
        self.text = edited.content;
        self.cursor_offset = self.clamp_byte(edited.cursor_offset);
        self.revision = self.revision.wrapping_add(1);
        true
    }
    pub fn create_snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            content: self.text.clone(),
            total_bytes: self.total_bytes(),
            total_lines: self.total_lines(),
            cursor_offset: self.cursor_offset,
            revision: self.revision,
        }
    }
}
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
#[derive(Clone, Debug)]
pub struct BufferSnapshot {
    pub content: Rope,
    pub total_bytes: usize,
    pub total_lines: usize,
    pub cursor_offset: usize,
    pub revision: u64,
}
impl BufferSnapshot {
    /// KMP search across rope chunks without flattening; matches do not overlap.
    pub fn find_matches(&self, query: &str) -> Vec<(usize, usize)> {
        let pattern = query.as_bytes();
        if pattern.is_empty() {
            return Vec::new();
        }
        let mut failure = vec![0; pattern.len()];
        let mut j = 0;
        for i in 1..pattern.len() {
            while j > 0 && pattern[i] != pattern[j] {
                j = failure[j - 1];
            }
            if pattern[i] == pattern[j] {
                j += 1;
            }
            failure[i] = j;
        }
        let mut result = Vec::new();
        j = 0;
        for (i, b) in self.content.bytes().enumerate() {
            while j > 0 && b != pattern[j] {
                j = failure[j - 1];
            }
            if b == pattern[j] {
                j += 1;
            }
            if j == pattern.len() {
                result.push((i + 1 - j, i + 1));
                j = 0;
            }
        }
        result
    }
    pub fn line(&self, line: usize) -> RopeSlice<'_> {
        let s = self.content.line(line.min(self.total_lines - 1));
        let end =
            s.len_chars() - usize::from(s.len_chars() > 0 && s.char(s.len_chars() - 1) == '\n');
        s.slice(..end)
    }
    pub fn line_col_to_offset(&self, line: usize, col: usize) -> usize {
        if line >= self.total_lines {
            return self.total_bytes;
        }
        let c = self.content.line_to_char(line) + col.min(self.line(line).len_chars());
        self.content.char_to_byte(c)
    }
    pub fn offset_to_line_col(&self, offset: usize) -> (usize, usize) {
        let c = self.content.byte_to_char(offset.min(self.total_bytes));
        let l = self.content.char_to_line(c);
        (l, c - self.content.line_to_char(l))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_commit_rejects_stale_edits_and_is_undoable() {
        let mut b = TextBuffer::from_string("aб".into());
        let base = b.create_snapshot();
        let mut work = TextBuffer::from_rope(base.content.clone());
        work.replace_all("a", "яя");
        b.insert(0, "!");
        assert!(!b.commit_snapshot(base.revision, base.cursor_offset, work.create_snapshot()));
        assert_eq!(b.to_string(), "!aб");
        let base = b.create_snapshot();
        let mut work = TextBuffer::from_rope(base.content.clone());
        work.replace_all("a", "яя");
        assert!(b.commit_snapshot(base.revision, base.cursor_offset, work.create_snapshot()));
        assert_eq!(b.to_string(), "!яяб");
        assert!(b.undo());
        assert_eq!(b.to_string(), "!aб");
    }
    #[test]
    fn history_is_bounded_and_new_edits_clear_redo() {
        let mut b = TextBuffer::new();
        for _ in 0..1500 {
            b.insert(usize::MAX, "line\n");
        }
        assert_eq!(b.undo_stack.len(), HISTORY_LIMIT);
        assert!(b.undo());
        b.insert(0, "new");
        assert!(!b.redo());
    }
    #[test]
    fn replace_all_preserves_history_and_unicode_cursor() {
        let mut b = TextBuffer::from_string("aб aб".into());
        b.set_cursor_offset(3);
        b.insert(3, "!");
        let before = b.to_string();
        let cursor = b.cursor_offset();
        assert_eq!(b.replace_all("a", "яя"), 2);
        assert!(b.to_string().is_char_boundary(b.cursor_offset()));
        assert_eq!(b.to_string(), "яяб! яяб");
        assert!(b.undo());
        assert_eq!(b.to_string(), before);
        assert_eq!(b.cursor_offset(), cursor);
        assert!(b.undo());
        assert_eq!(b.to_string(), "aб aб");
        assert!(b.redo());
        assert_eq!(b.to_string(), before);
        assert!(b.redo());
        assert_eq!(b.to_string(), "яяб! яяб");
    }
    #[test]
    fn snapshots_remain_immutable_and_typing_is_grouped() {
        let mut b = TextBuffer::new();
        let old = b.create_snapshot();
        for _ in 0..10_000 {
            b.insert(usize::MAX, "ї");
        }
        assert_eq!(b.undo_stack.len(), 1);
        assert_eq!(old.total_bytes, 0);
        assert_eq!(old.content.len_bytes(), 0);
        let typed = b.create_snapshot();
        assert!(b.undo());
        assert_eq!(b.total_bytes(), 0);
        assert_eq!(typed.content.len_chars(), 10_000);
        assert!(b.redo());
        assert_eq!(b.total_bytes(), 20_000);
    }
    #[test]
    fn block_and_replace_next_are_single_transactions() {
        let mut b = TextBuffer::from_string("abc\nabc\nabc".into());
        b.block_delete(0, 2, 1, 2);
        assert_eq!(b.to_string(), "ac\nac\nac");
        assert!(b.undo());
        assert_eq!(b.to_string(), "abc\nabc\nabc");
        b.replace_next("abc", "ї", 0);
        assert!(b.undo());
        assert_eq!(b.to_string(), "abc\nabc\nabc");
    }
    #[test]
    fn byte_offsets_are_clamped_to_unicode_boundaries() {
        let mut b = TextBuffer::from_string("їжак".into());
        b.set_cursor_offset(1);
        assert_eq!(b.cursor_offset(), 0);
        b.insert(1, "!");
        assert_eq!(b.to_string(), "!їжак");
        b.delete(2, 4);
        assert_eq!(b.to_string(), "!жак");
        assert_eq!(
            TextBuffer::line_col_to_offset_str("їжак\nabc", 0, usize::MAX),
            8
        );
        assert_eq!(TextBuffer::line_col_to_offset_str("їжак\nabc", 1, 1), 10);
    }
    #[test]
    fn search_matches_across_rope_chunks_and_does_not_overlap() {
        let text = format!("{}їжак{}", "x".repeat(1023), "ї".repeat(3000));
        let b = TextBuffer::from_string(text.clone());
        for query in ["xї", "їжак", "її", "missing", ""] {
            let expected: Vec<_> = if query.is_empty() {
                Vec::new()
            } else {
                text.match_indices(query)
                    .map(|(i, s)| (i, i + s.len()))
                    .collect()
            };
            assert_eq!(b.find_matches(query), expected);
        }
    }
    #[test]
    fn randomized_edits_and_history_match_string_model() {
        let mut b = TextBuffer::new();
        let mut model = String::new();
        let mut seed = 42u64;
        for i in 0..2000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let boundaries: Vec<_> = model
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(model.len()))
                .collect();
            let a = boundaries[seed as usize % boundaries.len()];
            b.set_cursor_offset(a);
            let before = model.clone();
            if i % 3 == 0 && !model.is_empty() {
                let z = boundaries[(seed >> 32) as usize % boundaries.len()];
                let (start, end) = (a.min(z), a.max(z));
                b.delete(start, end);
                model.replace_range(start..end, "");
            } else {
                let text = if i % 2 == 0 { "ї\n" } else { "ab" };
                b.insert(a, text);
                model.insert_str(a, text);
            }
            assert_eq!(b.to_string(), model);
            if before != model && i % 7 == 0 {
                assert!(b.undo());
                assert_eq!(b.to_string(), before);
                assert!(b.redo());
                assert_eq!(b.to_string(), model);
            }
        }
    }

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
        assert_eq!(
            buf.to_string(),
            "Привіт, чудовий світе!\nЦе українська мова: Їжак, Євген, Ґрунт, Ігор."
        );
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

        // Test inserting at exact end of line does NOT prepend
        let mut buf2 = TextBuffer::from_string("Apple\nPeach".to_string());
        buf2.block_insert(0, 0, 5, "!"); // col 5 is exact end of "Apple"
        assert_eq!(buf2.to_string(), "Apple!\nPeach");

        // Test block replacement (delete selected block, then insert)
        let mut buf3 = TextBuffer::from_string("Hello World\nHello Rust\nHello Block".to_string());
        buf3.block_delete(0, 2, 6, 11);
        buf3.block_insert(0, 2, 6, "Zed");
        assert_eq!(buf3.to_string(), "Hello Zed\nHello Zed\nHello Zed");

        // Test block replacement with Ukrainian Cyrillic
        let mut buf_ua =
            TextBuffer::from_string("Привіт Світ\nПривіт Руст\nПривіт Блок".to_string());
        buf_ua.block_delete(0, 2, 7, 11);
        buf_ua.block_insert(0, 2, 7, "Друг");
        assert_eq!(buf_ua.to_string(), "Привіт Друг\nПривіт Друг\nПривіт Друг");
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
