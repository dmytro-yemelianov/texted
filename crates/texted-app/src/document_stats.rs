//! Cancellable word counting over immutable rope chunks. One active job and one latest request.
use ropey::Rope;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
};
use texted_buffer::BufferSnapshot;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Count {
    pub document: u128,
    pub revision: u64,
    pub words: usize,
}
struct Request {
    document: u128,
    revision: u64,
    sequence: u64,
    text: Rope,
}
#[derive(Default)]
struct Slots {
    pending: Option<Request>,
    result: Option<Count>,
    stop: bool,
}
struct Shared {
    slots: Mutex<Slots>,
    wake: Condvar,
    sequence: AtomicU64,
}
pub struct DocumentStats {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    requested: Option<(u128, u64)>,
    latest: Option<Count>,
}
fn count_words(text: &Rope, mut cancelled: impl FnMut() -> bool) -> Option<usize> {
    let mut words = 0;
    let mut inside = false;
    let mut bytes = 0;
    for chunk in text.chunks() {
        for c in chunk.chars() {
            if c.is_alphanumeric() || c == '_' {
                if !inside {
                    words += 1;
                }
                inside = true;
            } else if !(inside && matches!(c, '\'' | '’' | 'ʼ' | '-')) {
                inside = false;
            }
            bytes += c.len_utf8();
            if bytes >= 4096 {
                if cancelled() {
                    return None;
                }
                bytes = 0;
            }
        }
    }
    if cancelled() {
        None
    } else {
        Some(words)
    }
}
impl DocumentStats {
    pub fn new(changed: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            slots: Mutex::new(Slots::default()),
            wake: Condvar::new(),
            sequence: AtomicU64::new(0),
        });
        let work = shared.clone();
        let worker = thread::spawn(move || loop {
            let request = {
                let mut slots = work.slots.lock().unwrap();
                while slots.pending.is_none() && !slots.stop {
                    slots = work.wake.wait(slots).unwrap();
                }
                if slots.stop {
                    return;
                }
                slots.pending.take().unwrap()
            };
            if let Some(words) = count_words(&request.text, || {
                work.sequence.load(Ordering::Relaxed) != request.sequence
            }) {
                let mut slots = work.slots.lock().unwrap();
                if !slots.stop && work.sequence.load(Ordering::Relaxed) == request.sequence {
                    slots.result = Some(Count {
                        document: request.document,
                        revision: request.revision,
                        words,
                    });
                    drop(slots);
                    changed();
                }
            }
        });
        Self {
            shared,
            worker: Some(worker),
            requested: None,
            latest: None,
        }
    }
    pub fn update(&mut self, document: u128, snapshot: &BufferSnapshot) -> Option<usize> {
        if self.requested != Some((document, snapshot.revision)) {
            let sequence = self.shared.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let mut slots = self.shared.slots.lock().unwrap();
            slots.pending = Some(Request {
                document,
                revision: snapshot.revision,
                sequence,
                text: snapshot.content.clone(),
            });
            self.requested = Some((document, snapshot.revision));
            self.shared.wake.notify_one();
        }
        if let Some(count) = self.shared.slots.lock().unwrap().result.take() {
            if count.document == document {
                self.latest = Some(count);
            }
        }
        self.result(document, snapshot.revision)
    }
    pub fn result(&self, document: u128, revision: u64) -> Option<usize> {
        self.latest
            .filter(|c| c.document == document && c.revision == revision)
            .map(|c| c.words)
    }
}
impl Drop for DocumentStats {
    fn drop(&mut self) {
        self.shared.sequence.fetch_add(1, Ordering::Relaxed);
        {
            let mut s = self.shared.slots.lock().unwrap();
            s.stop = true;
            s.pending = None;
        }
        self.shared.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latest_document_wins_and_an_old_result_never_labels_a_new_document() {
        let (send, recv) = std::sync::mpsc::channel();
        let mut stats = DocumentStats::new(move || {
            let _ = send.send(());
        });
        let snapshot = |text: &str| {
            let content = Rope::from_str(text);
            BufferSnapshot {
                total_bytes: content.len_bytes(),
                total_lines: content.len_lines(),
                content,
                cursor_offset: 0,
                revision: 0,
            }
        };
        let old = snapshot(&"old ".repeat(100000));
        let current = snapshot("Нове українське слово");
        stats.update(1, &old);
        assert_eq!(stats.update(2, &current), None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Some(words) = stats.update(2, &current) {
                assert_eq!(words, 3);
                break;
            }
            recv.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap();
        }
    }
    #[test]
    fn unicode_apostrophes_and_rope_boundaries_do_not_split_words() {
        let text = Rope::from_str("Ґава пʼять м’ята hello-world 123 _code\n");
        assert_eq!(count_words(&text, || false), Some(6));
        let text = Rope::from_str(&"слово".repeat(10000));
        assert_eq!(count_words(&text, || false), Some(1));
    }
    #[test]
    fn stale_work_is_cancelled_in_a_bounded_quantum() {
        let text = Rope::from_str(&"hello ".repeat(1_000_000));
        let mut polls = 0;
        assert_eq!(
            count_words(&text, || {
                polls += 1;
                true
            }),
            None
        );
        assert_eq!(polls, 1);
    }
    #[test]
    fn empty_and_punctuation_only_documents_have_no_words() {
        assert_eq!(
            count_words(&Rope::from_str(" … — !!! \n"), || false),
            Some(0)
        );
    }
}
