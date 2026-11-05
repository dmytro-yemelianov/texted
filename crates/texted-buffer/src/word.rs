//! Word boundaries around a rope position without copying the physical line.
use ropey::RopeSlice;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordBounds {
    pub start: usize,
    pub end: usize,
    pub inspected: usize,
}
fn class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}
pub fn bounds(line: RopeSlice<'_>, click_col: usize) -> WordBounds {
    if line.len_chars() == 0 {
        return WordBounds {
            start: 0,
            end: 0,
            inspected: 0,
        };
    }
    let col = click_col.min(line.len_chars() - 1);
    let target = line.char(col);
    let mut result = WordBounds {
        start: col,
        end: col + 1,
        inspected: 1,
    };
    if target == '\n' {
        return result;
    }
    let category = class(target);
    let mut left = line.chars_at(col);
    while let Some(c) = left.prev() {
        result.inspected += 1;
        if c == '\n' || class(c) != category {
            break;
        }
        result.start -= 1;
    }
    let right = line.chars_at(col + 1);
    for c in right {
        result.inspected += 1;
        if c == '\n' || class(c) != category {
            break;
        }
        result.end += 1;
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;
    #[test]
    fn short_words_do_not_scan_a_long_line() {
        let text = Rope::from_str(&"x ".repeat(5_000_000));
        for col in [0, 5_000_000, 9_999_998] {
            let b = bounds(text.slice(..), col);
            assert_eq!((b.start, b.end), (col, col + 1));
            assert!(b.inspected <= 3);
        }
    }
    #[test]
    fn classification_and_chunk_boundaries_are_preserved() {
        let s = Rope::from_str(&format!("{}Їжак_2  !!!\nnext", "x ".repeat(1023)));
        for col in [2046, 2047, 2051] {
            let b = bounds(s.slice(..), col);
            assert_eq!((b.start, b.end), (2046, 2052));
        }
        assert_eq!(
            (
                bounds(s.slice(..), 2052).start,
                bounds(s.slice(..), 2052).end
            ),
            (2052, 2054)
        );
        assert_eq!(
            (
                bounds(s.slice(..), 2054).start,
                bounds(s.slice(..), 2054).end
            ),
            (2054, 2057)
        );
        assert_eq!(
            (
                bounds(s.slice(..), 2057).start,
                bounds(s.slice(..), 2057).end
            ),
            (2057, 2058)
        );
        assert_eq!(bounds(Rope::new().slice(..), 5).inspected, 0);
        assert_eq!(bounds(Rope::from_str("abc").slice(..), usize::MAX).end, 3);
    }
}
