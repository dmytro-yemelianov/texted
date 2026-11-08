//! Atomic replacement planning over immutable snapshots, with bounded generated growth.
use crate::{
    search::{ScanState, SearchError, SearchOptions, SearchSession},
    BufferSnapshot,
};
use ropey::{Rope, RopeBuilder, RopeSlice};
pub const MAX_GENERATED_GROWTH: usize = 128 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    Query(SearchError),
    Cancelled,
    Capacity,
}
pub struct ReplacementPlan {
    pub content: Rope,
    pub cursor: usize,
    pub count: usize,
}
fn append(
    builder: &mut RopeBuilder,
    text: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<(), PlanError> {
    let mut rest = text;
    while !rest.is_empty() {
        if cancelled() {
            return Err(PlanError::Cancelled);
        }
        let mut end = rest.len().min(4096);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        builder.append(&rest[..end]);
        rest = &rest[end..];
    }
    Ok(())
}
fn append_slice(
    builder: &mut RopeBuilder,
    slice: RopeSlice<'_>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), PlanError> {
    for chunk in slice.chunks() {
        append(builder, chunk, cancelled)?;
    }
    Ok(())
}
pub fn plan(
    snapshot: &BufferSnapshot,
    query: &str,
    replacement: &str,
    all: bool,
    start: usize,
    cancelled: impl Fn() -> bool,
) -> Result<Option<ReplacementPlan>, PlanError> {
    plan_with(
        snapshot,
        query,
        replacement,
        all,
        start,
        SearchOptions::default(),
        cancelled,
    )
}
/// Plans with the same matching rules the search bar shows.
pub fn plan_with(
    snapshot: &BufferSnapshot,
    query: &str,
    replacement: &str,
    all: bool,
    start: usize,
    options: SearchOptions,
    cancelled: impl Fn() -> bool,
) -> Result<Option<ReplacementPlan>, PlanError> {
    let session = match SearchSession::with_options(snapshot.clone(), query, options, &cancelled) {
        Ok(s) => s,
        Err(SearchError::Empty) => return Ok(None),
        Err(e) => return Err(PlanError::Query(e)),
    };
    let next = if all {
        None
    } else {
        session
            .seek(start, true, &cancelled)
            .map_err(PlanError::Query)?
    };
    let count = if all {
        let r = session.scan(session.start(), &cancelled, |_| true, |_| {});
        if r.state != ScanState::Complete {
            return Err(PlanError::Cancelled);
        }
        r.checkpoint.count
    } else {
        usize::from(next.is_some())
    };
    if count == 0 {
        return Ok(None);
    }
    if replacement
        .len()
        .saturating_sub(query.len())
        .checked_mul(count)
        .is_none_or(|n| n > MAX_GENERATED_GROWTH)
    {
        return Err(PlanError::Capacity);
    }
    let mut builder = RopeBuilder::new();
    let mut previous = 0;
    let original_cursor = snapshot.cursor_offset;
    let mut cursor = original_cursor as i128;
    let mut failure = None;
    let mut apply = |a, b| -> Result<(), PlanError> {
        append_slice(
            &mut builder,
            snapshot.content.byte_slice(previous..a),
            &cancelled,
        )?;
        append(&mut builder, replacement, &cancelled)?;
        previous = b;
        if all {
            if original_cursor >= b {
                cursor += replacement.len() as i128 - (b - a) as i128;
            } else if original_cursor > a {
                cursor += (a + replacement.len()) as i128 - original_cursor as i128;
            }
        } else {
            cursor = (a + replacement.len()) as i128;
        }
        Ok(())
    };
    if let Some(m) = next {
        apply(m.start, m.end)?;
    } else {
        let r = session.scan(
            session.start(),
            &cancelled,
            |m| match apply(m.start, m.end) {
                Ok(()) => true,
                Err(e) => {
                    failure = Some(e);
                    false
                }
            },
            |_| {},
        );
        if let Some(e) = failure {
            return Err(e);
        }
        if r.state != ScanState::Complete {
            return Err(PlanError::Cancelled);
        }
    }
    append_slice(
        &mut builder,
        snapshot.content.byte_slice(previous..),
        &cancelled,
    )?;
    if cancelled() {
        return Err(PlanError::Cancelled);
    }
    Ok(Some(ReplacementPlan {
        content: builder.finish(),
        cursor: cursor.max(0) as usize,
        count,
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextBuffer;
    use std::cell::Cell;
    #[test]
    fn randomized_replacement_matches_string_model_without_match_storage() {
        let mut seed = 71u64;
        for _ in 0..100 {
            let mut text = String::new();
            for _ in 0..200 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                text.push(['a', 'b', 'ї', '\n'][(seed >> 32) as usize % 4]);
            }
            let snap = TextBuffer::from_string(text.clone()).create_snapshot();
            for q in ["a", "aa", "ї\n", "b"] {
                for replacement in ["", "X", "ї\n"] {
                    let p = plan(&snap, q, replacement, true, 0, || false).unwrap();
                    let actual = p.map_or(text.clone(), |p| p.content.to_string());
                    assert_eq!(actual, text.replace(q, replacement));
                }
            }
        }
    }
    #[test]
    fn replace_all_uses_the_search_bar_matching_rules() {
        let snap = TextBuffer::from_string("Кіт кіт кітки КІТ".into()).create_snapshot();
        let options = SearchOptions {
            ignore_case: true,
            whole_word: true,
        };
        let p = plan_with(&snap, "кіт", "пес", true, 0, options, || false).unwrap();
        assert_eq!(p.unwrap().content.to_string(), "пес пес кітки пес");
    }
    #[test]
    fn cancellation_and_preflight_leave_original_snapshot_unchanged() {
        let snap = TextBuffer::from_string("a".repeat(2_000_000)).create_snapshot();
        let checks = Cell::new(0);
        let p = plan(&snap, "a", "b", true, 0, || {
            checks.set(checks.get() + 1);
            checks.get() > 10
        });
        assert!(matches!(
            p,
            Err(PlanError::Cancelled) | Err(PlanError::Query(SearchError::Cancelled))
        ));
        assert!(matches!(
            plan(&snap, "a", &"b".repeat(1000), true, 0, || false),
            Err(PlanError::Capacity)
        ));
        assert_eq!(snap.content.byte(0), b'a');
        assert_eq!(snap.content.len_bytes(), 2_000_000);
    }
    #[test]
    fn cursor_inside_match_and_after_matches_keeps_existing_rules() {
        for (cursor, expected) in [(0, 0), (1, 2), (3, 2), (7, 5)] {
            let mut b = TextBuffer::from_string("abc abc".into());
            b.set_cursor_offset(cursor);
            let p = plan(&b.create_snapshot(), "abc", "ї", true, 0, || false)
                .unwrap()
                .unwrap();
            assert_eq!(p.cursor, expected);
            assert_eq!(p.content.to_string(), "ї ї");
        }
    }
}
