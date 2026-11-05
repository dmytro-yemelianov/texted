//! CPU-side measurements. These do not measure keyboard-to-display latency.
use std::{
    hint::black_box,
    thread,
    time::{Duration, Instant},
};
use texted_buffer::TextBuffer;
use texted_kernel::{ChannelEvent, ChannelPair};
use texted_layout::{LayoutEngine, ViewportConfig};
fn measure(label: &str, iterations: usize, mut operation: impl FnMut()) {
    for _ in 0..3 {
        operation();
    }
    let mut samples: Vec<_> = (0..iterations)
        .map(|_| {
            let start = Instant::now();
            operation();
            start.elapsed()
        })
        .collect();
    samples.sort_unstable();
    let mean = samples.iter().sum::<Duration>() / iterations as u32;
    println!(
        "{label:<44} mean={mean:?} p50={:?} p99={:?}",
        samples[iterations / 2],
        samples[(iterations * 99 / 100).min(iterations - 1)]
    );
}
fn main() {
    println!("Texted CPU benchmarks (release build recommended; no display-latency claims)");
    let pair: ChannelPair<u64, u64> = ChannelPair::new(1);
    let client = pair.connect();
    let server = pair.server;
    let worker = thread::spawn(move || {
        while let Ok(ChannelEvent::Message(req)) = server.receive() {
            let value = req.payload;
            req.reply(value);
            if value == u64::MAX {
                break;
            }
        }
    });
    measure("Blocking server IPC roundtrip", 1000, || {
        assert_eq!(client.send(42).unwrap(), 42)
    });
    client.send(u64::MAX).unwrap();
    worker.join().unwrap();
    let mut typing = TextBuffer::new();
    measure("Grouped single-character insertion", 10_000, || {
        typing.insert(usize::MAX, black_box("ї"))
    });
    for mib in [1, 16, 64] {
        let b = TextBuffer::from_string("short line of text\n".repeat(mib * 1024 * 1024 / 19));
        measure(&format!("{mib} MiB shared snapshot"), 100, || {
            black_box(b.create_snapshot());
        });
        let snap = b.create_snapshot();
        for wrap in [false, true] {
            for top in [0, snap.total_lines.saturating_sub(100)] {
                let config = ViewportConfig {
                    word_wrap: wrap,
                    top_line: top,
                    ..Default::default()
                };
                measure(
                    &format!("{mib} MiB layout wrap={wrap} top={top}"),
                    100,
                    || {
                        black_box(LayoutEngine::compute_layout(&snap, &config, None));
                    },
                );
            }
        }
    }
    for chars in [10_000, 40_000, 160_000] {
        let b = TextBuffer::from_string("ї".repeat(chars));
        let snap = b.create_snapshot();
        let text = b.to_string();
        measure(
            &format!("{chars} chars indexed coordinate lookup"),
            100,
            || {
                black_box(snap.line_col_to_offset(0, chars - 1));
            },
        );
        measure(
            &format!("{chars} chars string helper (linear)"),
            100,
            || {
                black_box(TextBuffer::line_col_to_offset_str(
                    black_box(&text),
                    0,
                    chars - 1,
                ));
            },
        );
    }
    let mut deletion = TextBuffer::from_string("слово ".repeat(10_000));
    deletion.set_cursor_offset(deletion.total_bytes());
    measure("Delete previous word (nonempty document)", 100, || {
        let before = deletion.total_bytes();
        deletion.delete_word_left();
        assert!(deletion.total_bytes() < before);
    });
    let b = TextBuffer::from_string("Texted український текст\n".repeat(10_000));
    measure("Streaming search (10k matches)", 30, || {
        assert_eq!(black_box(b.find_matches("Texted")).len(), 10_000)
    });
    let mut history = TextBuffer::new();
    for _ in 0..1000 {
        history.insert(usize::MAX, "line\n");
    }
    let start = Instant::now();
    let mut count = 0;
    while history.undo() {
        count += 1;
    }
    assert_eq!(count, 1000);
    println!("Undo {count} actual transactions: {:?}", start.elapsed());
}
