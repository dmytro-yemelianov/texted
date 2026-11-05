//! Generated audit workloads. The legacy loop is retained only as a benchmark comparator.
use super::measure;
use std::{hint::black_box, time::Instant};
use texted_buffer::{BufferSnapshot, TextBuffer};
use texted_layout::{LayoutEngine, ViewportConfig};
fn legacy_highlights(
    snap: &BufferSnapshot,
    config: &ViewportConfig,
    matches: &[(usize, usize)],
) -> usize {
    let layout = LayoutEngine::compute_layout(snap, config, None);
    let first = snap.line_col_to_offset(config.top_line, 0);
    let last = layout
        .visible_lines
        .last()
        .map_or(first, |r| snap.line_col_to_offset(r.line_index, r.end_col));
    let start = matches.partition_point(|&(_, b)| b < first);
    let mut visited = 0;
    for &(a, b) in matches.iter().skip(start).take_while(|&&(a, _)| a <= last) {
        let (sl, sc) = snap.offset_to_line_col(a);
        let (el, ec) = snap.offset_to_line_col(b);
        for line in sl..=el {
            for row in &layout.visible_lines {
                if row.line_index == line {
                    black_box(layout.highlight(
                        config,
                        row,
                        if line == sl { sc } else { 0 },
                        if line == el {
                            ec
                        } else {
                            snap.line(line).len_chars()
                        },
                    ));
                }
            }
        }
        visited += 1;
    }
    visited
}
pub fn run(legacy: bool) {
    println!(
        "Hardening workloads: release CPU timings, 3 warmups/30 samples; no display latency claims"
    );
    println!("rustc/OS/hardware/commit should accompany saved output");
    let snap = TextBuffer::from_string("a".repeat(2_000_000)).create_snapshot();
    let matches = snap.find_matches("a");
    println!(
        "dense search baseline: bytes={} matches={} offset capacity={} B",
        snap.total_bytes,
        matches.len(),
        matches.capacity() * std::mem::size_of::<(usize, usize)>()
    );
    for left in [0, 100_000, 1_000_000] {
        let config = ViewportConfig {
            left_col: left,
            ..Default::default()
        };
        let layout = LayoutEngine::compute_layout(&snap, &config, None);
        let plan = layout.search_highlights(&snap, &config, snap.revision, &matches, None);
        println!(
            "left={left}: candidates={} row_visits={} highlights={}",
            plan.candidates,
            plan.row_visits,
            plan.highlights.len()
        );
        measure(&format!("visible highlights left={left}"), 30, || {
            black_box(layout.search_highlights(&snap, &config, snap.revision, &matches, None));
        });
        if legacy {
            measure(&format!("legacy highlights left={left}"), 30, || {
                black_box(legacy_highlights(&snap, &config, &matches));
            });
        }
    }
    let config = ViewportConfig {
        word_wrap: true,
        top_wrap_row: 12_345,
        ..Default::default()
    };
    let layout = LayoutEngine::compute_layout(&snap, &config, None);
    measure("deep wrapped highlights", 30, || {
        black_box(layout.search_highlights(&snap, &config, snap.revision, &matches, None));
    });
    let mut source = TextBuffer::from_string(format!("{}\n", "x".repeat(2048)).repeat(10_000));
    let base = source.create_snapshot();
    measure("block insert 10k rows / 2048 chars", 30, || {
        source.load_document(base.content.clone());
        source.block_insert(0, 9999, 2048, "a");
        assert!(source.undo());
        assert_eq!(source.total_bytes(), base.total_bytes);
    });
    measure("block extraction 10k rows / 32 chars", 30, || {
        black_box(source.block_extract(0, 9999, 0, 32));
    });
    let paste = "ї".repeat(1_000_000);
    measure("paste 2 MB into isolated buffer", 30, || {
        let mut b = TextBuffer::new();
        b.insert(0, &paste);
        black_box(b.create_snapshot());
    });
    let words = TextBuffer::from_string("x ".repeat(5_000_000)).create_snapshot();
    for col in [0, 9_999_998] {
        let b = texted_buffer::word::bounds(words.line(0), col);
        println!("short word col={col}: inspected={}", b.inspected);
        measure(&format!("rope-native word col={col}"), 30, || {
            black_box(texted_buffer::word::bounds(words.line(0), col));
        });
    }
    let line = "x ".repeat(5_000_000);
    if legacy {
        measure("legacy short-word whole-line copies", 30, || {
            let copied = line.clone();
            let chars: Vec<_> = copied.chars().collect();
            black_box(chars[0]);
        });
    }
    let now = Instant::now();
    black_box(line);
    println!(
        "Fixture generation excluded from the measured operations; final cleanup {:?}",
        now.elapsed()
    );
}
