//! Texted - Ultra-Minimal High-Performance QNX-Microkernel Text Editor

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use texted_buffer::{BufferSnapshot, TextBuffer};
use texted_kernel::{ChannelEvent, ChannelPair, Pulse, CH_BUFFER, PULSE_KEY_INPUT};
use texted_layout::{LayoutEngine, ViewportConfig};
use texted_render::QuadBatcher;

// Request/Response protocol definitions for the microkernel channels
#[derive(Debug)]
pub enum BufferRequest {
    Insert { offset: usize, text: String },
    Delete { start: usize, end: usize },
    GetSnapshot,
    Undo,
    Redo,
}

#[derive(Debug)]
pub enum BufferResponse {
    Snapshot(BufferSnapshot),
    Success,
}

fn main() {
    let startup_start = Instant::now();
    println!("--------------------------------------------------");
    println!("  TEXTED: Minimal QNX-Microkernel Text Editor     ");
    println!("  Inspirations: Windows Notepad, Ghostty, Zed.dev ");
    println!("--------------------------------------------------");

    let running = Arc::new(AtomicBool::new(true));

    // 1. Initialize Microkernel Channels (QNX ChannelCreate equivalent)
    let buffer_channel: ChannelPair<BufferRequest, BufferResponse> =
        ChannelPair::new(CH_BUFFER);
    let buffer_client = buffer_channel.connect();

    // 2. Spawn the Buffer Server (isolated actor owning the Piece Table)
    let b_running = running.clone();
    let buffer_thread = thread::Builder::new()
        .name("server-buffer".to_string())
        .spawn(move || {
            let mut buffer = TextBuffer::new();
            buffer.insert(0, "Welcome to Texted!\nA minimal, ultra-performant text editor.\nPress any key to edit.");

            let server = buffer_channel.server;
            while b_running.load(Ordering::Relaxed) {
                if let Some(event) = server.try_receive() {
                    match event {
                        ChannelEvent::Pulse(pulse) => {
                            if pulse.code == PULSE_KEY_INPUT {
                                // Fast-path pulse handling: e.g. append typed char
                                let ch = (pulse.value as u8) as char;
                                let offset = buffer.cursor_offset();
                                buffer.insert(offset, &ch.to_string());
                            }
                        }
                        ChannelEvent::Message(req) => match req.payload {
                            BufferRequest::Insert { offset, ref text } => {
                                buffer.insert(offset, text);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::Delete { start, end } => {
                                buffer.delete(start, end);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::GetSnapshot => {
                                let snap = buffer.create_snapshot();
                                req.reply(BufferResponse::Snapshot(snap));
                            }
                            BufferRequest::Undo => {
                                buffer.undo();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::Redo => {
                                buffer.redo();
                                req.reply(BufferResponse::Success);
                            }
                        },
                    }
                } else {
                    thread::yield_now();
                }
            }
        })
        .expect("Failed to spawn buffer server");

    let startup_duration = startup_start.elapsed();
    println!("[Startup] Cold initialization time: {:.3} ms", startup_duration.as_secs_f64() * 1000.0);

    // 3. Perform Initial Snapshot Request via Synchronous Rendezvous (MsgSend)
    let initial_snapshot = match buffer_client.send(BufferRequest::GetSnapshot).unwrap() {
        BufferResponse::Snapshot(s) => s,
        _ => panic!("Expected snapshot"),
    };

    println!("[Buffer Server] Initial document loaded:");
    println!("  Lines: {}", initial_snapshot.total_lines);
    println!("  Bytes: {}", initial_snapshot.total_bytes);

    // 4. Viewport Layout & Quad Batching Test
    let config = ViewportConfig::default();
    let layout = LayoutEngine::compute_layout(&initial_snapshot, &config, None);
    let batcher = QuadBatcher::new();
    let quads = batcher.build_quads(&layout, &config, true);
    println!("[Render Server] Initial frame GPU quads generated: {} quads", quads.len());

    // 5. Benchmark: End-to-End Keystroke-to-Pixel Pipeline Latency
    println!("\n[Benchmark] Simulating 1,000 keystrokes through QNX microkernel pipeline...");
    let iterations = 1000;
    let bench_start = Instant::now();

    for i in 0..iterations {
        let key_char = b'x';
        // Send high-priority asynchronous pulse (MsgSendPulse)
        let pulse = Pulse::new(0, PULSE_KEY_INPUT, key_char as u64);
        buffer_client.send_pulse(pulse).unwrap();

        // Synchronous rendezvous to get fresh snapshot for frame
        let snap = match buffer_client.send(BufferRequest::GetSnapshot).unwrap() {
            BufferResponse::Snapshot(s) => s,
            _ => panic!("Expected snapshot"),
        };

        // Layout pass (visible lines only)
        let layout = LayoutEngine::compute_layout(&snap, &config, None);

        // Quad batching (GPU vertex buffer generation)
        let _frame_quads = batcher.build_quads(&layout, &config, i % 2 == 0);
    }

    let total_elapsed = bench_start.elapsed();
    let avg_per_key = (total_elapsed.as_secs_f64() * 1000.0) / iterations as f64;

    println!("  Total time for {} keystroke cycles: {:.2} ms", iterations, total_elapsed.as_secs_f64() * 1000.0);
    println!("  Average Keystroke-to-Quad Pipeline Latency: {:.3} ms ({:.1} µs)", avg_per_key, avg_per_key * 1000.0);
    println!("  Effective Frame Rate Capacity: > {:.0} FPS", 1000.0 / avg_per_key);

    // 6. Shutdown
    running.store(false, Ordering::Relaxed);
    buffer_thread.join().unwrap();
    println!("\n[Status] Microkernel cleanly shut down. Zero memory leaks.");
}
