//! Bounded search coordination. Workers publish data; only the UI applies cursor effects.
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
};
use texted_buffer::{
    search::{Checkpoint, ScanState, SearchMatch, SearchSession, MAX_QUERY_BYTES},
    BufferSnapshot, TextBuffer,
};
use texted_layout::{
    search::{Coverage, SearchViewport},
    HighlightPlan, ViewportConfig, ViewportLayout,
};
pub const SEARCH_BUDGET: usize = 8 * 1024 * 1024;
const MAX_VIEW_CELLS: usize = 1_048_576;
const CHECKPOINTS: usize = 2048;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchBarMode {
    Closed,
    Find,
    Replace,
}
pub type MutationPlan = Result<(u64, usize, Option<BufferSnapshot>), String>;
#[derive(Clone, Copy)]
struct Seek {
    serial: u64,
    target: i128,
}
struct Job {
    generation: u64,
    view_id: u64,
    snapshot: BufferSnapshot,
    query: Arc<str>,
    viewport: SearchViewport,
    seek: Option<Seek>,
}
struct ResultPage {
    generation: u64,
    view_id: u64,
    count: usize,
    complete: bool,
    coverage: Option<Coverage>,
    seek: Option<Seek>,
    found: Option<SearchMatch>,
    error: Option<String>,
}
#[derive(Default)]
struct Slots {
    pending: Option<Job>,
    result: Option<ResultPage>,
}
struct Shared {
    slots: Mutex<Slots>,
    ready: Condvar,
    generation: AtomicU64,
    stop: AtomicBool,
    active: AtomicBool,
    worker_bytes: AtomicUsize,
    started: AtomicUsize,
    #[cfg(test)]
    test_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl Shared {
    fn publish(&self, result: ResultPage) {
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        if self.generation.load(Ordering::Acquire) == result.generation
            && !self.stop.load(Ordering::Acquire)
        {
            slots.result = Some(result);
            self.ready.notify_all();
            drop(slots);
            (self.wake)();
        }
    }
}
struct Cache {
    generation: u64,
    session: SearchSession,
    checkpoints: Vec<Checkpoint>,
    count: Option<usize>,
}
impl Cache {
    fn before_offset(&self, offset: usize) -> Checkpoint {
        self.checkpoints
            .iter()
            .rev()
            .find(|c| c.offset <= offset)
            .copied()
            .unwrap_or_else(|| self.session.start())
    }
    fn before_ordinal(&self, target: usize) -> Checkpoint {
        self.checkpoints
            .iter()
            .rev()
            .find(|c| c.count <= target)
            .copied()
            .unwrap_or_else(|| self.session.start())
    }
}
fn run_job(shared: &Shared, job: &Job, cache: &mut Option<Cache>) -> Result<(), String> {
    let cancelled = || {
        shared.stop.load(Ordering::Acquire)
            || shared.generation.load(Ordering::Acquire) != job.generation
    };
    if cancelled() {
        return Ok(());
    }
    if cache
        .as_ref()
        .is_none_or(|c| c.generation != job.generation)
    {
        *cache = None;
        let session = match SearchSession::new(job.snapshot.clone(), &job.query, cancelled) {
            Ok(s) => s,
            Err(texted_buffer::search::SearchError::Cancelled) => return Ok(()),
            Err(e) => return Err(format!("Search could not start: {e:?}")),
        };
        *cache = Some(Cache {
            generation: job.generation,
            checkpoints: vec![session.start()],
            session,
            count: None,
        });
    }
    let cache = cache.as_mut().unwrap();
    let mut coverage = Coverage::new(job.viewport.clone());
    let mut bytes = cache.session.auxiliary_bytes()
        + coverage.allocated_bytes()
        + cache.checkpoints.capacity() * std::mem::size_of::<Checkpoint>();
    shared.worker_bytes.store(bytes, Ordering::Relaxed);
    let session = &cache.session;
    if cache.count.is_none() {
        let stride = session
            .snapshot
            .total_bytes
            .div_ceil(CHECKPOINTS)
            .max(4096)
            .next_multiple_of(4096);
        let mut next = stride;
        let mut progress_at = 1024 * 1024;
        let report = session.scan(
            session.start(),
            cancelled,
            |m| {
                coverage.mark(&session.snapshot, m);
                true
            },
            |cp| {
                if cp.offset >= next && cache.checkpoints.len() <= CHECKPOINTS {
                    cache.checkpoints.push(cp);
                    next = cp.offset.saturating_add(stride);
                }
                if cp.offset >= progress_at {
                    progress_at = cp.offset.saturating_add(1024 * 1024);
                    shared.publish(ResultPage {
                        generation: job.generation,
                        view_id: job.view_id,
                        count: cp.count,
                        complete: false,
                        coverage: None,
                        seek: None,
                        found: None,
                        error: None,
                    });
                }
            },
        );
        if report.state == ScanState::Cancelled {
            return Ok(());
        }
        cache.count = Some(report.checkpoint.count);
    } else if job.viewport.start() < job.viewport.end() {
        let end = job
            .viewport
            .end()
            .saturating_add(job.query.len().saturating_sub(1))
            .min(session.snapshot.total_bytes);
        let report = session.scan_until(
            cache.before_offset(job.viewport.start()),
            end,
            cancelled,
            |m| {
                coverage.mark(&session.snapshot, m);
                true
            },
            |_| {},
        );
        if report.state == ScanState::Cancelled {
            return Ok(());
        }
    }
    let count = cache.count.unwrap_or(0);
    let mut found = None;
    if let Some(seek) = job.seek.filter(|_| count > 0) {
        let target = seek.target.rem_euclid(count as i128) as usize;
        let report = session.scan(
            cache.before_ordinal(target),
            cancelled,
            |m| {
                if m.ordinal == target {
                    found = Some(m);
                    false
                } else {
                    true
                }
            },
            |_| {},
        );
        if report.state == ScanState::Cancelled {
            return Ok(());
        }
    }
    bytes += cache.checkpoints.capacity() * std::mem::size_of::<Checkpoint>();
    shared.worker_bytes.store(bytes, Ordering::Relaxed);
    shared.publish(ResultPage {
        generation: job.generation,
        view_id: job.view_id,
        count,
        complete: true,
        coverage: Some(coverage),
        seek: job.seek,
        found,
        error: None,
    });
    Ok(())
}
fn worker(shared: Arc<Shared>) {
    let mut cache = None;
    loop {
        let job = {
            let mut slots = shared.slots.lock().unwrap_or_else(|p| p.into_inner());
            while slots.pending.is_none() && !shared.stop.load(Ordering::Acquire) {
                if cache.as_ref().is_some_and(|c: &Cache| {
                    c.generation != shared.generation.load(Ordering::Acquire)
                }) {
                    cache = None;
                    shared.worker_bytes.store(0, Ordering::Relaxed);
                    shared.ready.notify_all();
                }
                slots = shared.ready.wait(slots).unwrap_or_else(|p| p.into_inner());
            }
            if shared.stop.load(Ordering::Acquire) {
                break;
            }
            slots.pending.take().unwrap()
        };
        shared.active.store(true, Ordering::Release);
        shared.started.fetch_add(1, Ordering::Relaxed);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(test)]
            {
                let hook = shared
                    .test_hook
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take();
                if let Some(hook) = hook {
                    hook();
                }
            }
            run_job(&shared, &job, &mut cache)
        }));
        if let Some(error) = match result {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(_) => Some("Search worker failed before completing.".into()),
        } {
            cache = None;
            shared.publish(ResultPage {
                generation: job.generation,
                view_id: job.view_id,
                count: 0,
                complete: true,
                coverage: None,
                seek: job.seek,
                found: None,
                error: Some(error),
            });
        }
        shared.active.store(false, Ordering::Release);
    }
    shared.worker_bytes.store(0, Ordering::Relaxed);
}
pub struct SearchState {
    pub mode: SearchBarMode,
    pub find_query: String,
    pub replace_query: String,
    pub active_field: usize,
    pub pending: bool,
    pub mutation: Option<std::sync::mpsc::Receiver<MutationPlan>>,
    pub submitted: Option<(u64, String)>,
    pub active: Option<SearchMatch>,
    pub count: Option<usize>,
    pub counted: usize,
    pub error: Option<String>,
    shared: Arc<Shared>,
    generation: u64,
    view_id: u64,
    viewport: SearchViewport,
    coverage: Option<Coverage>,
    serial: u64,
    applied_serial: u64,
    target: Option<i128>,
    latest: Option<BufferSnapshot>,
    mutation_cancel: Arc<AtomicBool>,
    mutation_revision: Option<u64>,
}
impl Default for SearchState {
    fn default() -> Self {
        Self::new(|| {})
    }
}
impl Drop for SearchState {
    fn drop(&mut self) {
        self.cancel_mutation();
        self.shared.stop.store(true, Ordering::Release);
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.ready.notify_all();
    }
}
impl SearchState {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            slots: Mutex::new(Slots::default()),
            ready: Condvar::new(),
            generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            active: AtomicBool::new(false),
            worker_bytes: AtomicUsize::new(0),
            started: AtomicUsize::new(0),
            #[cfg(test)]
            test_hook: Mutex::new(None),
            wake: Arc::new(wake),
        });
        let input = shared.clone();
        thread::spawn(move || worker(input));
        Self {
            mode: SearchBarMode::Closed,
            find_query: String::new(),
            replace_query: String::new(),
            active_field: 0,
            pending: false,
            mutation: None,
            submitted: None,
            active: None,
            count: None,
            counted: 0,
            error: None,
            shared,
            generation: 0,
            view_id: 0,
            viewport: SearchViewport::default(),
            coverage: None,
            serial: 0,
            applied_serial: 0,
            target: None,
            latest: None,
            mutation_cancel: Arc::new(AtomicBool::new(false)),
            mutation_revision: None,
        }
    }
    fn invalidate(&mut self) {
        self.generation = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let mut slots = self.shared.slots.lock().unwrap_or_else(|p| p.into_inner());
        slots.pending = None;
        slots.result = None;
        self.pending = false;
        self.count = None;
        self.counted = 0;
        self.active = None;
        self.coverage = None;
        self.target = None;
        self.latest = None;
        self.shared.ready.notify_all();
    }
    pub fn close(&mut self) {
        self.cancel_mutation();
        self.mode = SearchBarMode::Closed;
        self.invalidate();
        self.submitted = None;
    }
    pub fn reset_document(&mut self) {
        self.close();
        self.find_query = String::new();
        self.replace_query = String::new();
        self.viewport = SearchViewport::default();
        self.view_id += 1;
    }
    pub fn submit(&mut self, snap: BufferSnapshot) {
        if self
            .mutation_revision
            .is_some_and(|rev| rev != snap.revision)
        {
            self.cancel_mutation();
        }
        if self.find_query.len() > MAX_QUERY_BYTES {
            self.find_query = self
                .submitted
                .as_ref()
                .map_or_else(String::new, |(_, q)| q.clone());
            self.error = Some("Find queries are limited to 64 KiB of UTF-8 text.".into());
            return;
        }
        if self
            .submitted
            .as_ref()
            .is_some_and(|(rev, q)| *rev == snap.revision && q == &self.find_query)
        {
            self.latest = Some(snap);
            return;
        }
        let new_query = self
            .submitted
            .as_ref()
            .is_none_or(|(_, q)| q != &self.find_query);
        self.invalidate();
        self.submitted = Some((snap.revision, self.find_query.clone()));
        self.latest = Some(snap);
        if self.find_query.is_empty() {
            return;
        }
        if new_query {
            self.target = Some(0);
            self.serial += 1;
        }
        self.enqueue();
    }
    fn enqueue(&mut self) {
        let Some(snap) = self.latest.as_ref() else {
            return;
        };
        if self.find_query.is_empty() {
            return;
        }
        let job = Job {
            generation: self.generation,
            view_id: self.view_id,
            snapshot: snap.clone(),
            query: Arc::from(self.find_query.as_str()),
            viewport: self.viewport.clone(),
            seek: self.target.map(|target| Seek {
                serial: self.serial,
                target,
            }),
        };
        let mut slots = self.shared.slots.lock().unwrap_or_else(|p| p.into_inner());
        slots.pending = Some(job);
        self.pending = true;
        self.shared.ready.notify_one();
    }
    pub fn set_view(&mut self, snap: &BufferSnapshot, layout: &ViewportLayout) {
        let view = SearchViewport::new(snap, layout);
        if view
            .rows
            .iter()
            .map(|r| r.end_col.saturating_sub(r.start_col))
            .sum::<usize>()
            > MAX_VIEW_CELLS
        {
            self.error = Some("Search viewport exceeds the supported cell budget.".into());
            return;
        }
        if self.viewport != view {
            self.viewport = view;
            self.view_id += 1;
            self.coverage = None;
            self.enqueue();
        }
    }
    pub fn navigate(&mut self, snap: BufferSnapshot, forward: bool) {
        let had_query = self.submitted.is_some();
        self.submit(snap);
        if self.find_query.is_empty() {
            return;
        }
        self.target = Some(if had_query {
            self.target.map_or(0, |n| n + if forward { 1 } else { -1 })
        } else {
            0
        });
        self.serial += 1;
        self.enqueue();
    }
    pub fn cancel_navigation(&mut self) {
        self.target = None;
        self.serial += 1;
        self.active = None;
        if self.pending {
            self.enqueue();
        }
    }
    pub fn poll(&mut self) -> Option<(u64, (usize, usize))> {
        let result = self
            .shared
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .result
            .take()?;
        if result.generation != self.generation {
            return None;
        }
        self.counted = result.count;
        self.count = result.complete.then_some(result.count);
        if let Some(error) = result.error {
            self.error = Some(error);
            self.pending = false;
            return None;
        }
        if result.view_id == self.view_id {
            if let Some(c) = result.coverage {
                self.coverage = Some(c);
            }
        }
        let current = result.seek.is_none_or(|s| s.serial == self.serial);
        if result.complete && current && result.view_id == self.view_id {
            self.pending = false;
        }
        if let Some(seek) = result
            .seek
            .filter(|s| s.serial == self.serial && s.serial > self.applied_serial)
        {
            self.applied_serial = seek.serial;
            self.active = result.found;
            if let Some(m) = result.found {
                return Some((self.submitted.as_ref()?.0, (m.start, m.end)));
            }
        }
        None
    }
    pub fn highlights(
        &self,
        snapshot: &BufferSnapshot,
        layout: &ViewportLayout,
        config: &ViewportConfig,
    ) -> HighlightPlan {
        debug_assert!(self.auxiliary_bytes() <= SEARCH_BUDGET);
        if self
            .submitted
            .as_ref()
            .is_none_or(|(rev, q)| *rev != snapshot.revision || q != &self.find_query)
        {
            return HighlightPlan::default();
        }
        let mut plan = self
            .coverage
            .as_ref()
            .map_or_else(HighlightPlan::default, |c| c.highlights(layout, config));
        if let Some(m) = self.active {
            let mut active = layout.search_highlights(
                snapshot,
                config,
                snapshot.revision,
                &[(m.start, m.end)],
                Some((m.start, m.end)),
            );
            plan.highlights.append(&mut active.highlights);
        }
        plan
    }
    pub fn label(&self) -> String {
        match self.count {
            None if self.pending => format!("Searching… {}+", self.counted),
            Some(0) => "No matches".into(),
            Some(n) => format!("{}/{}", self.active.map_or(0, |m| m.ordinal + 1), n),
            None => String::new(),
        }
    }
    pub fn auxiliary_bytes(&self) -> usize {
        let slots = self.shared.slots.lock().unwrap_or_else(|p| p.into_inner());
        self.find_query.capacity()
            + self.submitted.as_ref().map_or(0, |(_, q)| q.capacity())
            + self.viewport.allocated_bytes()
            + self.coverage.as_ref().map_or(0, Coverage::allocated_bytes)
            + self.shared.worker_bytes.load(Ordering::Relaxed)
            + slots
                .pending
                .as_ref()
                .map_or(0, |j| j.query.len() + j.viewport.allocated_bytes())
            + slots
                .result
                .as_ref()
                .and_then(|r| r.coverage.as_ref())
                .map_or(0, Coverage::allocated_bytes)
    }
    pub fn queue_occupancy(&self) -> (usize, usize, usize) {
        let slots = self.shared.slots.lock().unwrap_or_else(|p| p.into_inner());
        (
            usize::from(self.shared.active.load(Ordering::Acquire)),
            usize::from(slots.pending.is_some()),
            usize::from(slots.result.is_some()),
        )
    }
    pub fn start_replacement(&mut self, snap: BufferSnapshot, all: bool) {
        if self.mutation.is_some() || self.find_query.is_empty() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.mutation = Some(rx);
        self.mutation_cancel = Arc::new(AtomicBool::new(false));
        self.mutation_revision = Some(snap.revision);
        let cancel = self.mutation_cancel.clone();
        let query = self.find_query.clone();
        let replacement = self.replace_query.clone();
        thread::spawn(move || {
            let result = texted_buffer::replacement::plan(
                &snap,
                &query,
                &replacement,
                all,
                snap.cursor_offset,
                || cancel.load(Ordering::Acquire),
            )
            .map(|plan| {
                let edited = plan.map(|p| {
                    let mut b = TextBuffer::from_rope(p.content);
                    b.set_cursor_offset(p.cursor);
                    b.create_snapshot()
                });
                (snap.revision, snap.cursor_offset, edited)
            })
            .map_err(|e| format!("Replacement failed: {e:?}"));
            let _ = tx.send(result);
        });
    }
    pub fn cancel_mutation(&self) {
        self.mutation_cancel.store(true, Ordering::Release);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use texted_layout::LayoutEngine;
    fn finish(search: &mut SearchState) -> Option<(u64, (usize, usize))> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut navigation = None;
        while search.pending {
            if let Some(n) = search.poll() {
                navigation = Some(n);
            }
            if !search.pending {
                break;
            }
            let guard = search.shared.slots.lock().unwrap();
            let (_guard, _) = search
                .shared
                .ready
                .wait_timeout_while(
                    guard,
                    deadline.saturating_duration_since(Instant::now()),
                    |s| s.result.is_none(),
                )
                .unwrap();
            assert!(Instant::now() < deadline, "search deadline");
        }
        navigation
    }
    #[test]
    fn one_active_job_and_one_latest_pending_job_survive_query_churn() {
        let mut s = SearchState::default();
        let (started, ready) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        *s.shared.test_hook.lock().unwrap() = Some(Box::new(move || {
            started.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(30)).unwrap();
        }));
        let snap = TextBuffer::from_string("abc ї abc".into()).create_snapshot();
        s.find_query = "a".into();
        s.submit(snap.clone());
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        for query in ["b", "c", "ї", "abc"] {
            s.find_query = query.into();
            s.submit(snap.clone());
            assert_eq!(s.queue_occupancy(), (1, 1, 0));
        }
        release.send(()).unwrap();
        finish(&mut s);
        assert_eq!(s.count, Some(2));
        assert_eq!(s.shared.started.load(Ordering::Relaxed), 2);
        s.close();
        let guard = s.shared.slots.lock().unwrap();
        let (_guard, timeout) = s
            .shared
            .ready
            .wait_timeout_while(guard, Duration::from_secs(5), |_| {
                s.shared.worker_bytes.load(Ordering::Relaxed) != 0
            })
            .unwrap();
        assert!(!timeout.timed_out());
        assert_eq!(s.shared.worker_bytes.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn worker_failure_clears_pending_and_a_new_generation_can_recover() {
        let mut s = SearchState::default();
        *s.shared.test_hook.lock().unwrap() = Some(Box::new(|| panic!("injected search failure")));
        let snap = TextBuffer::from_string("abc".into()).create_snapshot();
        s.find_query = "a".into();
        s.submit(snap.clone());
        finish(&mut s);
        assert!(s.error.take().unwrap().contains("failed"));
        assert!(!s.pending);
        s.find_query = "b".into();
        s.submit(snap);
        finish(&mut s);
        assert_eq!(s.count, Some(1));
    }
    #[test]
    fn dense_search_counts_with_bounded_memory_and_viewport_coverage() {
        let snap = TextBuffer::from_string("a".repeat(64 * 1024 * 1024)).create_snapshot();
        let config = ViewportConfig {
            left_col: 1_000_000,
            ..Default::default()
        };
        let layout = LayoutEngine::compute_layout(&snap, &config, None);
        let mut search = SearchState::default();
        search.find_query = "a".into();
        search.submit(snap.clone());
        search.set_view(&snap, &layout);
        finish(&mut search);
        assert_eq!(search.count, Some(64 * 1024 * 1024));
        assert!(search.auxiliary_bytes() < SEARCH_BUDGET);
        assert_eq!(
            search.highlights(&snap, &layout, &config).highlights.len(),
            1
        );
    }
    #[test]
    fn rapid_generations_only_apply_current_results_and_navigation_steps() {
        let snap = TextBuffer::from_string("aa ї aa ї aa".into()).create_snapshot();
        let mut search = SearchState::default();
        for q in ["a", "aa", "ї", "aa"] {
            search.find_query = q.into();
            search.submit(snap.clone());
        }
        search.navigate(snap.clone(), true);
        search.navigate(snap.clone(), true);
        let (_, range) = finish(&mut search).unwrap();
        assert_eq!(range, (12, 14));
        assert_eq!(search.count, Some(3));
        search.navigate(snap.clone(), true);
        assert_eq!(finish(&mut search).unwrap().1, (0, 2));
        search.close();
        assert!(!search.pending);
        assert!(search.coverage.is_none());
        assert!(search.shared.slots.lock().unwrap().pending.is_none());
    }
    #[test]
    fn query_limit_is_visible_and_does_not_retain_the_oversized_input() {
        let mut s = SearchState::default();
        s.find_query = "x".repeat(MAX_QUERY_BYTES + 1);
        s.submit(TextBuffer::new().create_snapshot());
        assert!(s.error.is_some());
        assert!(s.find_query.is_empty());
        assert!(!s.pending);
        assert!(s.auxiliary_bytes() < SEARCH_BUDGET);
    }
}
