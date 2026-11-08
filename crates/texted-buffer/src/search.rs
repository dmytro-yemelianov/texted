//! Resumable, cancellable KMP scanning with constant match-storage cost.
use crate::BufferSnapshot;
use std::sync::atomic::{AtomicU64, Ordering};
pub const MAX_QUERY_BYTES: usize = 64 * 1024;
pub const SCAN_QUANTUM: usize = 4096;
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchError {
    Empty,
    TooLong,
    Cancelled,
}
/// How a query matches: the default is exact, case-sensitive and anywhere in a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOptions {
    pub ignore_case: bool,
    pub whole_word: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub start: usize,
    pub end: usize,
    pub ordinal: usize,
}
#[derive(Debug, Clone, Copy)]
pub struct Checkpoint {
    session: u64,
    pub offset: usize,
    prefix: usize,
    pub count: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Complete,
    Cancelled,
    Stopped,
    InvalidCheckpoint,
}
#[derive(Debug, Clone, Copy)]
pub struct ScanReport {
    pub state: ScanState,
    pub checkpoint: Checkpoint,
    pub bytes_scanned: usize,
}
pub struct SearchSession {
    pub snapshot: BufferSnapshot,
    query: Box<str>,
    /// Case-folded query characters when case is ignored; empty for the byte matcher.
    folded: Vec<char>,
    failure: Vec<usize>,
    options: SearchOptions,
    id: u64,
}
/// Simple case folding that keeps one character per character, so matches stay on
/// character boundaries and the text and query fold the same way.
fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn failure_of<T: PartialEq>(
    pattern: &[T],
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<usize>, SearchError> {
    let mut failure = vec![0; pattern.len()];
    let mut prefix = 0;
    for i in 0..pattern.len() {
        if i % SCAN_QUANTUM == 0 && cancelled() {
            return Err(SearchError::Cancelled);
        }
        if i == 0 {
            continue;
        }
        while prefix > 0 && pattern[i] != pattern[prefix] {
            prefix = failure[prefix - 1];
        }
        if pattern[i] == pattern[prefix] {
            prefix += 1;
        }
        failure[i] = prefix;
    }
    Ok(failure)
}
impl SearchSession {
    pub fn new(
        snapshot: BufferSnapshot,
        query: &str,
        cancelled: impl FnMut() -> bool,
    ) -> Result<Self, SearchError> {
        Self::with_options(snapshot, query, SearchOptions::default(), cancelled)
    }
    pub fn with_options(
        snapshot: BufferSnapshot,
        query: &str,
        options: SearchOptions,
        cancelled: impl FnMut() -> bool,
    ) -> Result<Self, SearchError> {
        if query.is_empty() {
            return Err(SearchError::Empty);
        }
        if query.len() > MAX_QUERY_BYTES {
            return Err(SearchError::TooLong);
        }
        let (folded, failure) = if options.ignore_case {
            let folded: Vec<char> = query.chars().map(fold).collect();
            let failure = failure_of(&folded, cancelled)?;
            (folded, failure)
        } else {
            (Vec::new(), failure_of(query.as_bytes(), cancelled)?)
        };
        Ok(Self {
            snapshot,
            query: query.into(),
            folded,
            failure,
            options,
            id: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
        })
    }
    pub fn auxiliary_bytes(&self) -> usize {
        self.query.len()
            + self.folded.capacity() * std::mem::size_of::<char>()
            + self.failure.capacity() * std::mem::size_of::<usize>()
    }
    /// The most bytes one match can span; folding may change a character's UTF-8 length.
    pub fn match_reach(&self) -> usize {
        if self.options.ignore_case {
            self.folded.len() * 4
        } else {
            self.query.len()
        }
    }
    fn pattern_len(&self) -> usize {
        if self.options.ignore_case {
            self.folded.len()
        } else {
            self.query.len()
        }
    }
    /// Whole-word matches need a non-word character, or the text edge, on both sides.
    fn bounded(&self, start: usize, end: usize) -> bool {
        if !self.options.whole_word {
            return true;
        }
        let text = &self.snapshot.content;
        let first = text.byte_to_char(start);
        let last = text.byte_to_char(end);
        let open_before = first == 0 || !is_word(text.char(first - 1));
        let open_after = last >= text.len_chars() || !is_word(text.char(last));
        open_before && open_after
    }
    pub fn start(&self) -> Checkpoint {
        Checkpoint {
            session: self.id,
            offset: 0,
            prefix: 0,
            count: 0,
        }
    }
    /// Checkpoints retain the global non-overlap phase, including partial matches across chunks.
    pub fn scan(
        &self,
        at: Checkpoint,
        cancelled: impl FnMut() -> bool,
        visit: impl FnMut(SearchMatch) -> bool,
        checkpoint: impl FnMut(Checkpoint),
    ) -> ScanReport {
        self.scan_until(at, self.snapshot.total_bytes, cancelled, visit, checkpoint)
    }
    pub fn scan_until(
        &self,
        mut at: Checkpoint,
        stop_at: usize,
        mut cancelled: impl FnMut() -> bool,
        mut visit: impl FnMut(SearchMatch) -> bool,
        mut checkpoint: impl FnMut(Checkpoint),
    ) -> ScanReport {
        let begin = at.offset;
        let report = |state, at: Checkpoint| ScanReport {
            state,
            bytes_scanned: at.offset.saturating_sub(begin),
            checkpoint: at,
        };
        if at.session != self.id
            || at.offset > self.snapshot.total_bytes
            || at.prefix >= self.pattern_len()
        {
            return report(ScanState::InvalidCheckpoint, at);
        }
        if cancelled() {
            return report(ScanState::Cancelled, at);
        }
        if self.options.ignore_case {
            at = match self.scan_folded(
                at,
                stop_at,
                begin,
                &mut cancelled,
                &mut visit,
                &mut checkpoint,
            ) {
                Ok(at) => at,
                Err((state, at)) => return report(state, at),
            };
            checkpoint(at);
            return report(
                if cancelled() {
                    ScanState::Cancelled
                } else if at.offset == self.snapshot.total_bytes {
                    ScanState::Complete
                } else {
                    ScanState::Stopped
                },
                at,
            );
        }
        let pattern = self.query.as_bytes();
        for byte in self
            .snapshot
            .content
            .bytes_at(at.offset)
            .take(stop_at.saturating_sub(at.offset))
        {
            while at.prefix > 0 && byte != pattern[at.prefix] {
                at.prefix = self.failure[at.prefix - 1];
            }
            if byte == pattern[at.prefix] {
                at.prefix += 1;
            }
            at.offset += 1;
            if at.prefix == pattern.len() {
                let start = at.offset - pattern.len();
                if !self.bounded(start, at.offset) {
                    // Keep the partial match so a word starting inside this one is found.
                    at.prefix = self.failure[pattern.len() - 1];
                    continue;
                }
                let found = SearchMatch {
                    start,
                    end: at.offset,
                    ordinal: at.count,
                };
                at.count += 1;
                at.prefix = 0;
                if !visit(found) {
                    return report(ScanState::Stopped, at);
                }
            }
            if (at.offset - begin) % SCAN_QUANTUM == 0 {
                checkpoint(at);
                if cancelled() {
                    return report(ScanState::Cancelled, at);
                }
            }
        }
        checkpoint(at);
        report(
            if cancelled() {
                ScanState::Cancelled
            } else if at.offset == self.snapshot.total_bytes {
                ScanState::Complete
            } else {
                ScanState::Stopped
            },
            at,
        )
    }
    /// Character-level KMP over case-folded text; checkpoints stay on character boundaries.
    fn scan_folded(
        &self,
        mut at: Checkpoint,
        stop_at: usize,
        begin: usize,
        cancelled: &mut impl FnMut() -> bool,
        visit: &mut impl FnMut(SearchMatch) -> bool,
        checkpoint: &mut impl FnMut(Checkpoint),
    ) -> Result<Checkpoint, (ScanState, Checkpoint)> {
        let text = &self.snapshot.content;
        let pattern = &self.folded;
        let mut index = text.byte_to_char(at.offset);
        let mut next_checkpoint = begin + SCAN_QUANTUM;
        for original in text.chars_at(index) {
            if at.offset >= stop_at {
                break;
            }
            let c = fold(original);
            while at.prefix > 0 && c != pattern[at.prefix] {
                at.prefix = self.failure[at.prefix - 1];
            }
            if c == pattern[at.prefix] {
                at.prefix += 1;
            }
            at.offset += original.len_utf8();
            index += 1;
            if at.prefix == pattern.len() {
                let start = text.char_to_byte(index - pattern.len());
                if self.bounded(start, at.offset) {
                    let found = SearchMatch {
                        start,
                        end: at.offset,
                        ordinal: at.count,
                    };
                    at.count += 1;
                    at.prefix = 0;
                    if !visit(found) {
                        return Err((ScanState::Stopped, at));
                    }
                } else {
                    at.prefix = self.failure[pattern.len() - 1];
                }
            }
            if at.offset >= next_checkpoint {
                next_checkpoint = at.offset + SCAN_QUANTUM;
                checkpoint(at);
                if cancelled() {
                    return Err((ScanState::Cancelled, at));
                }
            }
        }
        Ok(at)
    }
    pub fn seek(
        &self,
        start: usize,
        forward: bool,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<SearchMatch>, SearchError> {
        let mut first = None;
        let mut last = None;
        let mut found = None;
        let report = self.scan(
            self.start(),
            &mut cancelled,
            |m| {
                first.get_or_insert(m);
                last = Some(m);
                if forward && m.start >= start {
                    found = Some(m);
                    return false;
                }
                if !forward && m.start < start {
                    found = Some(m);
                }
                if !forward && m.start >= start && found.is_some() {
                    return false;
                }
                true
            },
            |_| {},
        );
        if report.state == ScanState::Cancelled {
            return Err(SearchError::Cancelled);
        }
        Ok(found.or(if forward { first } else { last }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextBuffer;
    use std::cell::Cell;
    #[test]
    fn streaming_matches_agree_with_reference_and_checkpoints_preserve_non_overlap() {
        let mut seed = 7u64;
        for _ in 0..100 {
            let mut text = String::new();
            for _ in 0..300 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                text.push(['a', 'b', 'ї', '\n'][(seed >> 32) as usize % 4]);
            }
            let snap = TextBuffer::from_string(text.clone()).create_snapshot();
            for query in ["a", "aa", "ї", "ї\nb", "\n", "ba"] {
                let expected: Vec<_> = text
                    .match_indices(query)
                    .map(|(a, s)| (a, a + s.len()))
                    .collect();
                let session = SearchSession::new(snap.clone(), query, || false).unwrap();
                let mut at = session.start();
                let mut actual = Vec::new();
                loop {
                    let r = session.scan(
                        at,
                        || false,
                        |m| {
                            actual.push((m.start, m.end));
                            false
                        },
                        |_| {},
                    );
                    at = r.checkpoint;
                    if r.state == ScanState::Complete {
                        break;
                    }
                }
                assert_eq!(actual, expected);
            }
        }
    }
    #[test]
    fn cancellation_bounds_work_and_can_resume_inside_utf8_and_a_pattern() {
        let snap = TextBuffer::from_string("їa\n".repeat(20_000)).create_snapshot();
        let session = SearchSession::new(snap, "їa\nї", || false).unwrap();
        let cancel = Cell::new(false);
        let mut matches = Vec::new();
        let r = session.scan(
            session.start(),
            || cancel.get(),
            |m| {
                matches.push((m.start, m.end));
                cancel.set(true);
                true
            },
            |_| {},
        );
        assert_eq!(r.state, ScanState::Cancelled);
        assert!(r.bytes_scanned <= SCAN_QUANTUM);
        let r = session.scan(
            r.checkpoint,
            || false,
            |m| {
                matches.push((m.start, m.end));
                true
            },
            |_| {},
        );
        assert_eq!(r.state, ScanState::Complete);
        assert_eq!(matches, session.snapshot.find_matches("їa\nї"));
    }
    #[test]
    fn dense_count_is_bounded_and_foreign_checkpoints_are_rejected() {
        let snap = TextBuffer::from_string("a".repeat(2_000_000)).create_snapshot();
        let a = SearchSession::new(snap.clone(), "a", || false).unwrap();
        let b = SearchSession::new(snap, "aa", || false).unwrap();
        let r = a.scan(a.start(), || false, |_| true, |_| {});
        assert_eq!(r.checkpoint.count, 2_000_000);
        assert!(a.auxiliary_bytes() < 64);
        assert_eq!(
            b.scan(r.checkpoint, || false, |_| true, |_| {}).state,
            ScanState::InvalidCheckpoint
        );
        assert!(matches!(
            SearchSession::new(a.snapshot.clone(), &"x".repeat(MAX_QUERY_BYTES + 1), || {
                false
            }),
            Err(SearchError::TooLong)
        ));
        assert!(matches!(
            SearchSession::new(a.snapshot.clone(), "x", || true),
            Err(SearchError::Cancelled)
        ));
    }
    fn all_matches(text: &str, query: &str, options: SearchOptions) -> Vec<(usize, usize)> {
        let snap = TextBuffer::from_string(text.into()).create_snapshot();
        let session = SearchSession::with_options(snap, query, options, || false).unwrap();
        let mut found = vec![];
        session.scan(
            session.start(),
            || false,
            |m| {
                found.push((m.start, m.end));
                true
            },
            |_| {},
        );
        found
    }
    const IGNORE_CASE: SearchOptions = SearchOptions {
        ignore_case: true,
        whole_word: false,
    };
    const WHOLE_WORD: SearchOptions = SearchOptions {
        ignore_case: false,
        whole_word: true,
    };
    #[test]
    fn ignoring_case_matches_cyrillic_and_latin_capitals() {
        let text = "Їжак, їжак, ЇЖАК; Hello hello";
        assert_eq!(
            all_matches(text, "їжак", IGNORE_CASE),
            [(0, 8), (10, 18), (20, 28)]
        );
        assert_eq!(
            all_matches(text, "HELLO", IGNORE_CASE),
            [(30, 35), (36, 41)]
        );
        assert_eq!(
            all_matches(text, "їжак", SearchOptions::default()),
            [(10, 18)]
        );
    }
    #[test]
    fn whole_words_skip_matches_inside_longer_words() {
        assert_eq!(
            all_matches("cat concat cat_x cats cat.", "cat", WHOLE_WORD),
            [(0, 3), (22, 25)]
        );
        assert_eq!(
            all_matches("кіт кітки кіт", "кіт", WHOLE_WORD),
            [(0, 6), (18, 24)]
        );
        // A rejected candidate must not swallow the next real match.
        assert_eq!(all_matches("aa a", "a", WHOLE_WORD), [(3, 4)]);
        let both = SearchOptions {
            ignore_case: true,
            whole_word: true,
        };
        assert_eq!(
            all_matches("Кіт кітки КІТ", "кіт", both),
            [(0, 6), (18, 24)]
        );
    }
    #[test]
    fn ignore_case_scans_resume_from_checkpoints() {
        let text = "Їa\n".repeat(20_000);
        let snap = TextBuffer::from_string(text.clone()).create_snapshot();
        let session = SearchSession::with_options(snap, "ЇA\nї", IGNORE_CASE, || false).unwrap();
        let cancel = Cell::new(false);
        let mut matches = Vec::new();
        let r = session.scan(
            session.start(),
            || cancel.get(),
            |m| {
                matches.push((m.start, m.end));
                cancel.set(true);
                true
            },
            |_| {},
        );
        assert_eq!(r.state, ScanState::Cancelled);
        assert!(r.bytes_scanned <= SCAN_QUANTUM + 4);
        let r = session.scan(
            r.checkpoint,
            || false,
            |m| {
                matches.push((m.start, m.end));
                true
            },
            |_| {},
        );
        assert_eq!(r.state, ScanState::Complete);
        assert_eq!(matches, session.snapshot.find_matches("Їa\nЇ"));
    }
    #[test]
    fn seeks_wrap_without_a_complete_match_vector() {
        let s = SearchSession::new(
            TextBuffer::from_string("aaaaїaa".into()).create_snapshot(),
            "aa",
            || false,
        )
        .unwrap();
        assert_eq!(s.seek(1, true, || false).unwrap().unwrap().start, 2);
        assert_eq!(s.seek(20, true, || false).unwrap().unwrap().start, 0);
        assert_eq!(s.seek(0, false, || false).unwrap().unwrap().start, 6);
        assert_eq!(s.seek(2, false, || false).unwrap().unwrap().start, 0);
    }
}
